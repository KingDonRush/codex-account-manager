use super::*;
use crate::adapters::app_server::{AppServerClient, IsolatedCodexHome};
use crate::application::execution_book::GenerationLease;
use crate::domain::execution::{ExecutionState, ExecutionStatus};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, TryRecvError},
    Arc,
};
use std::time::{Duration, Instant};

struct Worker {
    core: ManagerCore,
    status: ApplicationServerStatus,
    cwd: String,
    remote: String,
    session: super::super::application_auth::ApplicationAuth,
    capacity: u32,
    input: Receiver<Value>,
    cancel: Arc<AtomicBool>,
    pending: HashMap<String, (u64, String)>,
    tools: HashSet<String>,
    generation: Option<(GenerationLease, ExecutionStatus, u64)>,
}
impl ManagerCore {
    pub(super) fn run_application_server(
        &self,
        task: ServerTask,
        input: Receiver<Value>,
        cancel: Arc<AtomicBool>,
    ) {
        let ServerTask {
            status,
            cwd,
            remote,
            session,
            capacity,
        } = task;
        let mut worker = Worker {
            core: self.clone(),
            status,
            cwd,
            remote,
            session,
            capacity,
            input,
            cancel,
            pending: HashMap::new(),
            tools: HashSet::new(),
            generation: None,
        };
        let result = worker.run();
        if let Some((_, mut job, _)) = worker.generation.take() {
            job.state = ExecutionState::Interrupted;
            job.finished_at = Some(Utc::now());
            job.error_code = Some("session_closed".to_owned());
            let _ = self.update_execution(job);
        }
        worker.status.state = match result {
            Ok(()) | Err(AppError::Cancelled) => ServerState::Closed,
            Err(_) => ServerState::Failed,
        };
        worker.status.last_activity = Utc::now();
        worker.status.error_code = result.err().map(|error| error.code().to_owned());
        let _ = self.update_application_server(worker.status, &worker.cwd);
    }
}

impl Worker {
    fn run(&mut self) -> Result<(), AppError> {
        let directory = self
            .core
            .app_data_dir
            .join("application-servers")
            .join(&self.status.id);
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        let temp = (!self.status.persist_history)
            .then(IsolatedCodexHome::create)
            .transpose()?;
        let home = if let Some(temp) = &temp {
            temp.directory.path().to_path_buf()
        } else {
            let home = directory.join("runtime");
            std::fs::create_dir_all(&home)?;
            home
        };
        let mut client = AppServerClient::spawn_read_only(&home, self.cancel.clone())?;
        let document = self.core.load_application_auth(
            &self.session,
            &self.remote,
            self.cancel.clone(),
            false,
        )?;
        client.set_read_only_session(&document)?;
        let mut options = std::fs::OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut output = options.open(directory.join("events.jsonl"))?;
        self.status.state = ServerState::Ready;
        self.core
            .update_application_server(self.status.clone(), &self.cwd)?;
        let mut last_authority_check = Instant::now() - Duration::from_secs(1);
        while !self.cancel.load(Ordering::Acquire)
            && !self.core.maintenance.cancel.load(Ordering::Acquire)
        {
            if last_authority_check.elapsed() >= Duration::from_secs(1) {
                let grant = self.core.authorization_grant(&self.status.grant_id)?;
                self.core.validate_server_authority(&grant, &self.status)?;
                last_authority_check = Instant::now();
            }
            match self.input.try_recv() {
                Ok(message) => {
                    if let Err(error) = self.send(&mut client, message.clone()) {
                        if !matches!(
                            error,
                            AppError::Validation(_)
                                | AppError::AccessDenied(_)
                                | AppError::ConsentRequired(_)
                                | AppError::Busy(_)
                        ) {
                            return Err(error);
                        }
                        self.record(&mut output,json!({"id":message.get("id"),"error":{"code":-32000,"message":crate::adapters::logging::sanitize_error(&error.to_string()),"data":{"code":error.code()}}}))?;
                    }
                }
                Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }
            if let Some(message) = client.poll_application_message(Duration::from_millis(20))? {
                self.received(&mut client, &mut output, message)?;
            }
        }
        self.status.state = ServerState::Closing;
        self.core
            .update_application_server(self.status.clone(), &self.cwd)?;
        drop(client);
        drop(temp);
        Ok(())
    }

    fn send(&mut self, client: &mut AppServerClient, mut message: Value) -> Result<(), AppError> {
        crate::application::application_servers::validate_harness_message(&message)?;
        crate::application::application_servers::apply_model_policy(&mut message, &self.status)?;
        let grant = self.core.authorization_grant(&self.status.grant_id)?;
        self.core.validate_server_authority(&grant, &self.status)?;
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(method) = method {
            let id = message.get("id").and_then(Value::as_u64).ok_or_else(|| {
                AppError::Validation("Request de harness precisa de ID numérico.".to_owned())
            })?;
            let wire_id = format!("consumer:{id}");
            if self.pending.len() >= 64 {
                return Err(AppError::Busy(
                    "Há 64 RPCs aguardando resposta nesta sessão.".to_owned(),
                ));
            }
            if self.pending.contains_key(&wire_id) {
                return Err(AppError::Busy("ID RPC já está em voo.".to_owned()));
            }
            if method == "turn/start" || method == "command/exec" {
                self.reserve_generation(id)?;
                let prepared = (|| {
                    let document = self.core.load_application_auth(
                        &self.session,
                        &self.remote,
                        self.cancel.clone(),
                        false,
                    )?;
                    client.set_read_only_session(&document)
                })();
                if let Err(error) = prepared {
                    self.finish_generation(ExecutionState::Failed, Some(error.code()))?;
                    return Err(error);
                }
            }
            if method == "thread/start" && message.pointer("/params/cwd").is_none() {
                message["params"]["cwd"] = Value::String(self.cwd.clone());
            }
            message["id"] = Value::String(wire_id.clone());
            self.pending.insert(wire_id, (id, method));
        } else {
            let id = message
                .get("id")
                .ok_or_else(|| AppError::Validation("Resposta sem ID.".to_owned()))?
                .to_string();
            if !self.tools.remove(&id) {
                return Err(AppError::AccessDenied(
                    "Não há pedido do servidor com esse ID.".to_owned(),
                ));
            }
        }
        let grant = self.core.authorization_grant(&self.status.grant_id)?;
        if let Err(error) = self.core.validate_server_authority(&grant, &self.status) {
            self.finish_generation(ExecutionState::Failed, Some(error.code()))?;
            return Err(error);
        }
        // Keep the lease until the owned client is collected on fatal transport failure.
        client.send_application_message(message)?;
        self.status.last_activity = Utc::now();
        Ok(())
    }

    fn reserve_generation(&mut self, rpc_id: u64) -> Result<(), AppError> {
        if self.generation.is_some() {
            return Err(AppError::Busy(
                "Já há um turno ativo nesta sessão.".to_owned(),
            ));
        }
        let mut job = ExecutionStatus {
            id: uuid::Uuid::new_v4().to_string(),
            application_id: self.status.application_id.clone(),
            grant_id: self.status.grant_id.clone(),
            binding_id: self.status.binding_id.clone(),
            binding_revision: self.status.binding_revision,
            group_id: self.status.group_id.clone(),
            account_id: Some(self.status.account_id.clone()),
            state: ExecutionState::Queued,
            started_at: Utc::now(),
            finished_at: None,
            error_code: None,
            exit_code: None,
            background: self.status.background,
        };
        self.core.executions.add(
            job.clone(),
            self.cancel.clone(),
            std::collections::BTreeSet::from([self.status.account_id.clone()]),
        )?;
        if let Err(error) = self.core.update_execution(job.clone()) {
            self.core.executions.fail_submission(&job.id, &error);
            return Err(error);
        }
        let result = self.wait_generation(&job.id);
        let lease = match result {
            Ok(lease) => lease,
            Err(error) => {
                job.state = if matches!(error, AppError::Cancelled) {
                    ExecutionState::Cancelled
                } else {
                    ExecutionState::Failed
                };
                job.finished_at = Some(Utc::now());
                job.error_code = Some(error.code().to_owned());
                let _ = self.core.update_execution(job);
                return Err(error);
            }
        };
        job.state = ExecutionState::Running;
        self.generation = Some((lease, job.clone(), rpc_id));
        self.core.update_execution(job)?;
        self.status.state = ServerState::Generating;
        self.core
            .update_application_server(self.status.clone(), &self.cwd)
    }

    fn wait_generation(&self, job: &str) -> Result<GenerationLease, AppError> {
        loop {
            if self.cancel.load(Ordering::Acquire)
                || self.core.maintenance.cancel.load(Ordering::Acquire)
            {
                return Err(AppError::Cancelled);
            }
            let grant = self.core.authorization_grant(&self.status.grant_id)?;
            self.core.validate_server_authority(&grant, &self.status)?;
            let target = super::super::execution::GenerationTarget {
                account: &self.status.account_id,
                remote: &self.remote,
                session: &self.session,
                capacity: self.capacity,
            };
            if let Some(lease) = self.core.reserve_generation(job, target, &self.cancel)? {
                return Ok(lease);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn received(
        &mut self,
        client: &mut AppServerClient,
        output: &mut std::fs::File,
        mut message: Value,
    ) -> Result<(), AppError> {
        let method_owned = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let method = method_owned.as_deref();
        if method == Some("account/chatgptAuthTokens/refresh") {
            let response = self.refresh_access();
            if matches!(response, Err(AppError::CredentialRecoveryPending(_))) {
                return response.map(|_| ());
            }
            let reply = match response {
                Ok(document) => {
                    json!({"id":message["id"],"result":{"accessToken":document["tokens"]["access_token"],"chatgptAccountId":self.remote}})
                }
                Err(error) => {
                    json!({"id":message["id"],"error":{"code":-32000,"message":crate::adapters::logging::sanitize_error(&error.to_string())}})
                }
            };
            return client.send_application_message(reply);
        }
        if method
            .is_some_and(|method| method.starts_with("account/") || method.starts_with("config/"))
        {
            return Ok(());
        }
        if let Some(id) = message.get("id") {
            if method.is_some() {
                if self.tools.len() >= 64 || self.tools.contains(&id.to_string()) {
                    return Err(AppError::AppServer(
                        "Pedidos de host excederam o limite ou repetiram um ID ativo.".to_owned(),
                    ));
                }
                self.tools.insert(id.to_string());
            } else if let Some(wire_id) = id.as_str() {
                let Some((id, request)) = self.pending.remove(wire_id) else {
                    return Ok(());
                };
                message["id"] = json!(id);
                if message.get("error").is_some()
                    && self
                        .generation
                        .as_ref()
                        .is_some_and(|(_, _, generation)| *generation == id)
                {
                    self.finish_generation(ExecutionState::Failed, Some("rpc_error"))?;
                }
                if request == "command/exec" && message.get("error").is_none() {
                    self.finish_generation(ExecutionState::Completed, None)?;
                }
            } else {
                return Ok(());
            }
        }
        if method == Some("turn/completed") {
            let state = match message
                .pointer("/params/turn/status")
                .and_then(Value::as_str)
            {
                Some("completed") => ExecutionState::Completed,
                Some("interrupted") => ExecutionState::Cancelled,
                _ => ExecutionState::Failed,
            };
            self.finish_generation(
                state,
                (state == ExecutionState::Failed).then_some("turn_failed"),
            )?;
        }
        self.record(output, message)
    }

    fn refresh_access(&self) -> Result<Value, AppError> {
        if self.generation.is_none() {
            return Err(AppError::Busy(
                "Renovação sem lease de geração não é permitida.".to_owned(),
            ));
        }
        let super::super::application_auth::ApplicationAuth::Dedicated(session) = &self.session
        else {
            return self.core.load_application_auth(
                &self.session,
                &self.remote,
                self.cancel.clone(),
                true,
            );
        };
        let document = self
            .core
            .application_credentials
            .load(session, &self.remote)?;
        let home = IsolatedCodexHome::create()?;
        crate::adapters::auth_store::atomic_write_auth(
            &home.directory.path().join("auth.json"),
            &document,
        )?;
        let mut refresher =
            AppServerClient::spawn_cancellable(home.directory.path(), self.cancel.clone())?;
        let validation = refresher.validate_stored_session_with_trace(&mut |_, _| {});
        drop(refresher);
        let refreshed =
            crate::adapters::auth_store::read_auth(&home.directory.path().join("auth.json"))?;
        crate::adapters::auth_store::validate_auth_document(&refreshed, Some(&self.remote))?;
        if refreshed != document {
            self.core
                .save_application_credential(session, &self.remote, &refreshed)?;
        }
        validation?;
        Ok(refreshed)
    }
    fn finish_generation(
        &mut self,
        state: ExecutionState,
        error: Option<&str>,
    ) -> Result<(), AppError> {
        if let Some((_lease, mut job, _)) = self.generation.take() {
            job.state = state;
            job.finished_at = Some(Utc::now());
            job.error_code = error.map(str::to_owned);
            self.core.update_execution(job)?;
        }
        self.status.state = ServerState::Ready;
        self.core
            .update_application_server(self.status.clone(), &self.cwd)
    }
    fn record(&self, output: &mut std::fs::File, message: Value) -> Result<(), AppError> {
        let mut bytes = serde_json::to_vec(&message)?;
        bytes.push(b'\n');
        if output.metadata()?.len() + bytes.len() as u64 > 32 * 1024 * 1024 {
            return Err(AppError::Busy(
                "Saída da sessão atingiu limite; encerre/exporte antes de continuar.".to_owned(),
            ));
        }
        output.write_all(&bytes)?;
        output.flush()?;
        Ok(())
    }
}
