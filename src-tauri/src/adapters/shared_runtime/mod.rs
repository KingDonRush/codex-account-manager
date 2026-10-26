#[cfg(unix)]
mod activity;
#[cfg(unix)]
mod connection;
mod discovery;
mod external_calls;
#[cfg(test)]
mod tests;

use super::process_supervisor::configure_supervised_process;
#[cfg(any(unix, test))]
use crate::domain::RuntimeThread;
use crate::domain::{
    RuntimeConsent, RuntimeRecoveryReport, SharedRuntimeSnapshot, SharedRuntimeState,
};
use crate::error::AppError;
use discovery::{discover, DaemonIdentity};
#[cfg(unix)]
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct SharedRuntime {
    home: PathBuf,
    #[cfg(windows)]
    metadata: super::database::Database,
}

impl SharedRuntime {
    pub fn new(home: PathBuf, metadata: super::database::Database) -> Self {
        #[cfg(not(windows))]
        let _ = metadata;
        Self {
            home,
            #[cfg(windows)]
            metadata,
        }
    }

    fn active_calls(&self) -> Result<Vec<u32>, AppError> {
        #[cfg(windows)]
        {
            external_calls::active_windows_calls(self.metadata.desktop_executable()?)
        }
        #[cfg(not(windows))]
        {
            external_calls::active_calls(&self.home)
        }
    }

    pub fn snapshot(&self, cancel: &AtomicBool) -> SharedRuntimeSnapshot {
        match self.inspect(cancel) {
            Ok((_, mut snapshot)) => match self.active_calls() {
                Ok(calls) => {
                    if !calls.is_empty() {
                        snapshot.state = SharedRuntimeState::Busy;
                        snapshot.explanation = "Há uma CLI ou um App Server externo usando a sessão global. Eles serão preservados; a troca aguardará sua conclusão.".to_owned();
                    }
                    snapshot.external_calls = calls;
                    snapshot
                }
                Err(error) => SharedRuntimeSnapshot::unavailable(super::logging::sanitize_error(
                    &error.to_string(),
                )),
            },
            Err(error) => SharedRuntimeSnapshot::unavailable(super::logging::sanitize_error(
                &error.to_string(),
            )),
        }
    }

    /// Called before a global auth commit. Busy/unknown runtimes remain intact;
    /// idle runtimes are suspended automatically rather than kept with stale auth.
    pub fn prepare_global_change(&self, cancel: &AtomicBool) -> Result<(), AppError> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(AppError::Cancelled);
            }
            if !self.active_calls()?.is_empty() {
                if Instant::now() >= deadline {
                    return Err(AppError::RuntimeBusy("Uma CLI ou um App Server externo ainda usa esta sessão. A conta não foi alterada e o processo não foi interrompido.".to_owned()));
                }
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            let (identity, snapshot) = self.inspect(cancel).map_err(|error| match error {
                AppError::Cancelled => AppError::Cancelled,
                _ => AppError::RuntimeBusy(format!(
                    "Não foi possível confirmar a atividade do servidor; sessão preservada. {}",
                    super::logging::sanitize_error(&error.to_string())
                )),
            })?;
            match snapshot.state {
                SharedRuntimeState::Absent => return Ok(()),
                SharedRuntimeState::Idle => { if let Some(identity) = identity { self.stop_verified(&identity, cancel, None)?; } return Ok(()); },
                SharedRuntimeState::Busy if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(250)),
                SharedRuntimeState::Busy => return Err(AppError::RuntimeBusy("Há chats ou terminais trabalhando no servidor compartilhado. A conta não foi alterada; abra o controle de runtime para acompanhar ou autorizar uma interrupção.".to_owned())),
                SharedRuntimeState::Unavailable => return Err(AppError::RuntimeBusy(snapshot.explanation)),
            }
        }
    }

    /// A recovery always targets the existing thread; no fork, archive or lock
    /// deletion is used. Consent applies only to interrupting a busy server.
    pub fn recover_thread(
        &self,
        id: &str,
        consent: Option<&RuntimeConsent>,
        cancel: &AtomicBool,
    ) -> Result<RuntimeRecoveryReport, AppError> {
        uuid::Uuid::parse_str(id).map_err(|_| {
            AppError::Validation("Informe o UUID ou endereço do chat original.".to_owned())
        })?;
        let (identity, snapshot) = self.inspect(cancel)?;
        let owns = snapshot.threads.iter().any(|thread| thread.id == id);
        if !owns {
            return Ok(RuntimeRecoveryReport {
                recovered: false,
                needs_confirmation: false,
                explanation: "Esse daemon não possui o chat. Nenhum processo foi encerrado."
                    .to_owned(),
                affected_threads: 0,
                confirmation: None,
            });
        }
        let identity = identity.ok_or_else(|| {
            AppError::Runtime("O proprietário não pôde ser verificado.".to_owned())
        })?;
        if snapshot.state != SharedRuntimeState::Idle
            && !consent.is_some_and(|consent| consent_matches(consent, &identity, &snapshot))
        {
            return Ok(RuntimeRecoveryReport { recovered: false, needs_confirmation: true, explanation: "O servidor tem trabalho ativo. Aguarde ou confirme a interrupção desta instância e destes chats.".to_owned(), affected_threads: snapshot.threads.len(), confirmation: Some(RuntimeConsent { pid: identity.pid, started_ticks: identity.started_ticks, thread_ids: snapshot.threads.iter().map(|thread| thread.id.clone()).collect() }) });
        }
        self.stop_verified(&identity, cancel, consent)?;
        Ok(RuntimeRecoveryReport { recovered: true, needs_confirmation: false, explanation: "Proprietário anterior encerrado. O chat original pode ser retomado no Desktop; nenhuma cópia de histórico foi criada.".to_owned(), affected_threads: snapshot.threads.len(), confirmation: None })
    }

    fn inspect(
        &self,
        cancel: &AtomicBool,
    ) -> Result<(Option<DaemonIdentity>, SharedRuntimeSnapshot), AppError> {
        let Some(identity) = discover(&self.home)? else {
            return Ok((None, SharedRuntimeSnapshot::absent()));
        };
        #[cfg(unix)]
        {
            let mut connection = connection::Connection::connect(&identity.socket, identity.pid)?;
            connection.initialize(cancel)?;
            let listed = connection.request("thread/loaded/list", json!({"limit":128}), cancel)?;
            let ids = activity::complete_list(&listed)?;
            let threads = ids
                .iter()
                .map(|id| {
                    inspect_thread(
                        &mut connection,
                        id.as_str().ok_or_else(|| {
                            AppError::Runtime("Identificador de chat inválido.".to_owned())
                        })?,
                        cancel,
                    )
                })
                .collect::<Result<Vec<_>, AppError>>()?;
            let state = if threads.iter().all(RuntimeThread::is_idle) {
                SharedRuntimeState::Idle
            } else {
                SharedRuntimeState::Busy
            };
            let snapshot = SharedRuntimeSnapshot { state, pid: Some(identity.pid), threads, external_calls: Vec::new(), external_inspection_supported: cfg!(target_os = "linux"), explanation: "Atividade confirmada no servidor proprietário; nenhum chat foi retomado durante a consulta.".to_owned() };
            Ok((Some(identity), snapshot))
        }
        #[cfg(not(unix))]
        {
            let _ = (identity, cancel);
            Err(AppError::Runtime(
                "A inspeção do daemon ainda não está disponível nesta plataforma.".to_owned(),
            ))
        }
    }

    fn stop_verified(
        &self,
        identity: &DaemonIdentity,
        cancel: &AtomicBool,
        consent: Option<&RuntimeConsent>,
    ) -> Result<(), AppError> {
        if cancel.load(Ordering::Acquire) {
            return Err(AppError::Cancelled);
        }
        if !identity.is_current() {
            return Err(AppError::Runtime(
                "O proprietário mudou durante a recuperação; tente verificar novamente.".to_owned(),
            ));
        }
        let (current, activity) = self.inspect(cancel)?;
        if current.as_ref().is_none_or(|current| {
            current.pid != identity.pid || current.started_ticks != identity.started_ticks
        }) {
            return Err(AppError::Runtime(
                "A identidade do servidor mudou; nenhum processo foi encerrado.".to_owned(),
            ));
        }
        if activity.state != SharedRuntimeState::Idle
            && !consent.is_some_and(|consent| consent_matches(consent, identity, &activity))
        {
            return Err(AppError::RuntimeBusy(
                "O servidor voltou a trabalhar durante a verificação. A sessão foi preservada."
                    .to_owned(),
            ));
        }
        run_management(&identity.executable, &self.home)?;
        if identity.is_current() {
            return Err(AppError::Runtime(
                "O servidor ainda está ativo; nenhum arquivo de lock foi removido.".to_owned(),
            ));
        }
        Ok(())
    }
}

fn consent_matches(
    consent: &RuntimeConsent,
    identity: &DaemonIdentity,
    snapshot: &SharedRuntimeSnapshot,
) -> bool {
    let mut expected = consent
        .thread_ids
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let mut actual = snapshot
        .threads
        .iter()
        .map(|thread| thread.id.as_str())
        .collect::<Vec<_>>();
    expected.sort_unstable();
    actual.sort_unstable();
    consent.pid == identity.pid
        && consent.started_ticks == identity.started_ticks
        && expected == actual
}

#[cfg(unix)]
fn inspect_thread(
    connection: &mut connection::Connection,
    id: &str,
    cancel: &AtomicBool,
) -> Result<RuntimeThread, AppError> {
    let data = connection.request(
        "thread/read",
        json!({"threadId":id,"includeTurns":false}),
        cancel,
    )?;
    let goal = connection.request("thread/goal/get", json!({"threadId":id}), cancel)?;
    let terminals = connection.request(
        "thread/backgroundTerminals/list",
        json!({"threadId":id,"limit":128}),
        cancel,
    )?;
    activity::parse_thread(id, &data, &goal, &terminals)
}

fn run_management(executable: &Path, home: &Path) -> Result<(), AppError> {
    use std::process::{Command, Stdio};
    let mut command = Command::new(executable);
    command
        .args(["app-server", "daemon", "stop"])
        .env("CODEX_HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_supervised_process(&mut command);
    let (mut child, supervisor) = crate::adapters::process_launcher::spawn_supervised(command)?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(AppError::Runtime(
                    "O comando oficial não conseguiu encerrar o daemon.".to_owned(),
                ))
            };
        }
        if Instant::now() >= deadline {
            supervisor.terminate(&mut child)?;
            return Err(AppError::Runtime(
                "O encerramento do daemon excedeu o prazo.".to_owned(),
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
