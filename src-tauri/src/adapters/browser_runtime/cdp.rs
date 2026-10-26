use crate::error::AppError;
#[cfg(test)]
mod tests;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::{net::UnixStream, process::CommandExt};
use std::process::Command;
use std::time::{Duration, Instant};

/// Private inherited pipe; no HTTP debugger, DOM, cookies or generic frontend RPC.
pub struct CdpPipe {
    channel: Mutex<(BufReader<UnixStream>, u64)>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetInventory {
    target_infos: Vec<TargetInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetInfo {
    target_id: String,
    #[serde(rename = "type")]
    kind: String,
}
impl CdpPipe {
    pub fn prepare(command: &mut Command) -> Result<Self, AppError> {
        let (parent, child) = UnixStream::pair()?;
        // SAFETY: duplicate a live descriptor above reserved stdio/CDP descriptors.
        let descriptor = unsafe { libc::fcntl(child.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10) };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: fcntl returned a new descriptor owned exclusively by this closure.
        let inherited = unsafe { OwnedFd::from_raw_fd(descriptor) };
        // SAFETY: child hook uses only async-signal-safe dup2; no allocation/locks.
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(inherited.as_raw_fd(), 3) < 0
                    || libc::dup2(inherited.as_raw_fd(), 4) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        drop(child);
        parent.set_read_timeout(Some(Duration::from_secs(3)))?;
        parent.set_write_timeout(Some(Duration::from_secs(3)))?;
        Ok(Self {
            channel: Mutex::new((BufReader::new(parent), 0)),
        })
    }
    fn request(&self, method: &str, params: Value) -> Result<Value, AppError> {
        let timeout = Duration::from_secs(3);
        let mut channel = self.channel.try_lock_for(timeout).ok_or_else(|| {
            AppError::Browser("O controle do navegador não ficou disponível no prazo.".to_owned())
        })?;
        let deadline = Instant::now() + timeout;
        channel.1 += 1;
        let id = channel.1;
        let mut encoded = serde_json::to_vec(&json!({"id":id,"method":method,"params":params}))?;
        encoded.push(0);
        if encoded.len() > 256 * 1024 {
            return Err(AppError::Browser(
                "Comando do navegador excede o limite local.".to_owned(),
            ));
        }
        channel.0.get_mut().write_all(&encoded)?;
        for _ in 0..64 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AppError::Browser(
                    "O navegador não respondeu no prazo.".to_owned(),
                ));
            }
            channel.0.get_mut().set_read_timeout(Some(remaining))?;
            let mut frame = Vec::new();
            let read = std::io::Read::by_ref(&mut channel.0)
                .take(256 * 1024 + 1)
                .read_until(0, &mut frame)?;
            if read == 0 || read > 256 * 1024 || frame.last() != Some(&0) {
                return Err(AppError::Browser(
                    "Canal do navegador encerrado ou mensagem excessiva.".to_owned(),
                ));
            }
            frame.pop();
            let message: Value = serde_json::from_slice(&frame)?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if message.get("error").is_some() {
                return Err(AppError::Browser(
                    "O navegador recusou o controle desta janela.".to_owned(),
                ));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(AppError::Browser(
            "O navegador não confirmou o comando.".to_owned(),
        ))
    }
    pub fn create_target(&self, url: &str) -> Result<String, AppError> {
        let response = self.request("Target.createTarget", json!({"url":url,"newWindow":true}))?;
        response
            .get("targetId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| AppError::Browser("Janela não identificada.".to_owned()))
    }
    pub fn owns_alive_target(&self, targets: &[String]) -> Result<bool, AppError> {
        Ok(self
            .targets()?
            .iter()
            .any(|info| targets.contains(&info.target_id)))
    }
    pub fn close_targets(&self, targets: &[String]) -> Result<bool, AppError> {
        let existing = self.targets()?;
        for target in existing
            .iter()
            .filter(|info| targets.contains(&info.target_id))
        {
            let result = self.request("Target.closeTarget", json!({"targetId":target.target_id}));
            if let Err(error) = result {
                // The user can close a target between inventory and request.
                // A new valid inventory, not error text, proves idempotent closure.
                if self
                    .targets()?
                    .iter()
                    .any(|info| info.target_id == target.target_id)
                {
                    return Err(error);
                }
            }
        }
        let remaining = self.wait_targets_closed(targets)?;
        Ok(!remaining.iter().any(|info| info.kind == "page"))
    }

    fn wait_targets_closed(&self, targets: &[String]) -> Result<Vec<TargetInfo>, AppError> {
        let deadline = Instant::now() + Duration::from_millis(750);
        loop {
            let remaining = self.targets()?;
            if !remaining
                .iter()
                .any(|info| targets.contains(&info.target_id))
            {
                return Ok(remaining);
            }
            if Instant::now() >= deadline {
                return Err(AppError::Browser(
                    "O navegador ainda mantém uma aba da operação; fechamento não confirmado."
                        .to_owned(),
                ));
            }
            // Closing is asynchronous. Observe the acknowledgement's effect,
            // never issue another close or assume a remaining page is manual.
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn targets(&self) -> Result<Vec<TargetInfo>, AppError> {
        let inventory: TargetInventory = serde_json::from_value(
            self.request("Target.getTargets", json!({}))?,
        )
        .map_err(|_| {
            AppError::Browser(
                "O navegador não confirmou o inventário de abas; janelas preservadas.".to_owned(),
            )
        })?;
        if inventory
            .target_infos
            .iter()
            .any(|info| info.target_id.is_empty() || info.kind.is_empty())
        {
            return Err(AppError::Browser(
                "O inventário de abas contém identidade inválida; janelas preservadas.".to_owned(),
            ));
        }
        Ok(inventory.target_infos)
    }

    pub fn create_operation_targets(
        &self,
        url: &str,
        mail: Option<&str>,
    ) -> Result<Vec<String>, AppError> {
        let mut targets = Vec::new();
        if let Err(error) = self.create_targets_into(url, mail, &mut targets) {
            self.close_targets(&targets).map_err(|_| AppError::Browser("Não foi possível abrir todas as abas nem confirmar o fechamento da aba parcial.".to_owned()))?;
            return Err(error);
        }
        Ok(targets)
    }

    pub(super) fn create_targets_into(
        &self,
        url: &str,
        mail: Option<&str>,
        targets: &mut Vec<String>,
    ) -> Result<(), AppError> {
        targets.push(self.create_target(url)?);
        if let Some(mail) = mail {
            targets.push(self.create_target(mail)?);
        }
        Ok(())
    }
}
