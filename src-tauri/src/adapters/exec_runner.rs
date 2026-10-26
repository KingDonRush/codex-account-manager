use super::process_launcher::spawn_supervised;
use super::process_supervisor::configure_supervised_process;
use super::process_supervisor::ProcessSupervisor;
#[cfg(test)]
mod tests;
use crate::domain::execution::ExecRequest;
use crate::error::AppError;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{Builder, Scope, ScopedJoinHandle};
use std::time::Duration;

pub fn validate_request(request: &ExecRequest) -> Result<(), AppError> {
    if request.prompt.len() > 192 * 1024
        || request.arguments.len() > 128
        || !Path::new(&request.cwd).is_absolute()
        || !Path::new(&request.cwd).is_dir()
    {
        return Err(AppError::Validation(
            "Entrada excede limite ou workspace não é um diretório absoluto.".to_owned(),
        ));
    }
    for argument in &request.arguments {
        if argument.len() > 8192
            || argument.contains('\0')
            || argument.contains("CODEX_HOME")
            || argument.contains("cli_auth_credentials_store")
            || argument.contains("forced_login_method")
            || argument.contains("model_provider")
            || argument.contains("api_key")
            || argument.contains("access_token")
            || argument.contains("base_url")
        {
            return Err(AppError::AccessDenied(
                "Opção tenta sobrescrever o contexto de autenticação.".to_owned(),
            ));
        }
    }
    Ok(())
}

pub fn run(
    request: &ExecRequest,
    home: &Path,
    output: &Path,
    cancel: Arc<AtomicBool>,
) -> Result<i32, AppError> {
    validate_request(request)?;
    let binary = super::app_server::resolve_codex_binary()
        .ok_or_else(|| AppError::AppServer("Codex não encontrado.".to_owned()))?;
    execute(
        command_for(request, home, &binary),
        &request.prompt,
        output,
        cancel,
    )
}

fn command_for(request: &ExecRequest, home: &Path, binary: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .arg("exec")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .arg("--ephemeral")
        .arg("--ignore-user-config")
        .arg("--strict-config")
        .args(&request.arguments)
        .args([
            "-c",
            "cli_auth_credentials_store=\"file\"",
            "-c",
            "forced_login_method=\"chatgpt\"",
            "-c",
            "model_provider=\"openai\"",
        ])
        .arg("-")
        .current_dir(&request.cwd)
        .env_clear()
        .envs(super::app_server::inherited_environment())
        .env("CODEX_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[cfg(test)]
pub(crate) struct TestExecutionContext<'a> {
    pub home: &'a Path,
    pub output: &'a Path,
    pub binary: &'a Path,
}

#[cfg(test)]
pub(crate) fn run_test_binary(
    request: &ExecRequest,
    context: TestExecutionContext<'_>,
    cancel: Arc<AtomicBool>,
) -> Result<i32, AppError> {
    validate_request(request)?;
    execute(
        command_for(request, context.home, context.binary),
        &request.prompt,
        context.output,
        cancel,
    )
}

fn execute(
    mut command: Command,
    prompt: &str,
    output: &Path,
    cancel: Arc<AtomicBool>,
) -> Result<i32, AppError> {
    if cancel.load(Ordering::Acquire) {
        return Err(AppError::Cancelled);
    }
    configure_supervised_process(&mut command);
    let io_failed = AtomicBool::new(false);
    std::thread::scope(|scope| {
        // Keep this guard inside the scope closure: kill the child on early
        // return/unwind before scoped threads wait for their pipes to close.
        let mut process = scopeguard::guard(spawn_supervised(command)?, |mut process| {
            let _ = finish_process(&mut process);
        });
        let workers = ExecIo::spawn(
            scope,
            &mut process.0,
            IoContext {
                prompt,
                output,
                failed: &io_failed,
            },
        )?;
        let status = wait_for_exec(&mut process.0, &cancel, &io_failed);
        let cleanup = finish_process(&mut process);
        let result = workers.finish(status);
        if cleanup.is_ok() {
            scopeguard::ScopeGuard::into_inner(process);
        }
        cleanup?;
        result
    })
}

struct IoContext<'a> {
    prompt: &'a str,
    output: &'a Path,
    failed: &'a AtomicBool,
}

struct ExecIo<'scope> {
    input: ScopedJoinHandle<'scope, Result<(), AppError>>,
    output: ScopedJoinHandle<'scope, Result<(), AppError>>,
    stderr: ScopedJoinHandle<'scope, Result<(), AppError>>,
}

impl<'scope> ExecIo<'scope> {
    fn spawn(
        scope: &'scope Scope<'scope, '_>,
        child: &mut Child,
        context: IoContext<'scope>,
    ) -> Result<Self, AppError> {
        let mut stdin = take_pipe(&mut child.stdin, "stdin")?;
        let stdout = take_pipe(&mut child.stdout, "stdout")?;
        let stderr = take_pipe(&mut child.stderr, "stderr")?;
        let input = spawn_worker(scope, "exec-stdin", context.failed, move || {
            stdin
                .write_all(context.prompt.as_bytes())
                .map_err(AppError::Io)
        })?;
        let output = spawn_worker(scope, "exec-jsonl", context.failed, move || {
            capture(stdout, context.output)
        })?;
        let stderr = spawn_worker(scope, "exec-stderr", context.failed, move || {
            std::io::copy(&mut BufReader::new(stderr), &mut std::io::sink())
                .map(|_| ())
                .map_err(AppError::Io)
        })?;
        Ok(Self {
            input,
            output,
            stderr,
        })
    }

    fn finish(self, status: Result<Option<i32>, AppError>) -> Result<i32, AppError> {
        // No early `?` between joins; the session lease covers every worker.
        let input = collect_worker(self.input);
        let output = collect_worker(self.output);
        let stderr = collect_worker(self.stderr);
        let status = status?;
        output?;
        if let Some(code) = status.filter(|code| *code != 0) {
            return Ok(code);
        }
        input?;
        stderr?;
        status.ok_or_else(|| {
            AppError::Internal("Auxiliar do exec encerrou sem resultado.".to_owned())
        })
    }
}

fn take_pipe<T>(pipe: &mut Option<T>, name: &str) -> Result<T, AppError> {
    pipe.take()
        .ok_or_else(|| AppError::Internal(format!("{name} indisponível.")))
}

fn finish_process(process: &mut (Child, ProcessSupervisor)) -> Result<(), AppError> {
    process.1.terminate(&mut process.0)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        if process.0.try_wait()?.is_some() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(AppError::Runtime(
                "O filho do exec não concluiu o encerramento no prazo.".to_owned(),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn spawn_worker<'scope, T: Send + 'scope>(
    scope: &'scope Scope<'scope, '_>,
    name: &str,
    failed: &'scope AtomicBool,
    operation: impl FnOnce() -> Result<T, AppError> + Send + 'scope,
) -> Result<ScopedJoinHandle<'scope, Result<T, AppError>>, AppError> {
    Builder::new()
        .name(name.to_owned())
        .spawn_scoped(scope, move || stop_on_error(failed, operation))
        .map_err(AppError::Io)
}

fn wait_for_exec(
    child: &mut std::process::Child,
    cancel: &AtomicBool,
    io_failed: &AtomicBool,
) -> Result<Option<i32>, AppError> {
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(AppError::Cancelled);
        }
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status.code().unwrap_or(1)));
        }
        if io_failed.load(Ordering::Acquire) {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn stop_on_error<T>(
    failed: &AtomicBool,
    operation: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    let guard = scopeguard::guard(failed, |flag| flag.store(true, Ordering::Release));
    let result = operation();
    if result.is_ok() {
        scopeguard::ScopeGuard::into_inner(guard);
    }
    result
}

fn collect_worker<T>(worker: ScopedJoinHandle<'_, Result<T, AppError>>) -> Result<T, AppError> {
    worker
        .join()
        .map_err(|_| AppError::Internal("Auxiliar de I/O do exec interrompido.".to_owned()))?
}

fn capture(reader: impl Read, path: &Path) -> Result<(), AppError> {
    let mut file = private_file(path)?;
    let mut reader = BufReader::new(reader);
    let mut total = 0_u64;
    loop {
        let mut frame = Vec::new();
        let bytes = reader
            .by_ref()
            .take(256 * 1024 + 1)
            .read_until(b'\n', &mut frame)?;
        if bytes == 0 {
            break;
        }
        total += bytes as u64;
        if bytes > 256 * 1024 || total > 32 * 1024 * 1024 {
            return Err(AppError::Validation(
                "Saída excede limite; trabalho cancelado.".to_owned(),
            ));
        }
        serde_json::from_slice::<serde_json::Value>(&frame)?;
        file.write_all(&frame)?;
    }
    file.sync_all()?;
    Ok(())
}

pub fn private_file(path: &Path) -> Result<std::fs::File, AppError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
