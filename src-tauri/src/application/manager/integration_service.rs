use super::ManagerCore;
use crate::error::AppError;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[cfg(unix)]
struct EndpointCleanup {
    path: std::path::PathBuf,
    inode: u64,
    device: u64,
}
#[cfg(unix)]
impl EndpointCleanup {
    fn new(path: std::path::PathBuf) -> Result<Self, AppError> {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(&path)?;
        Ok(Self {
            path,
            inode: metadata.ino(),
            device: metadata.dev(),
        })
    }
}
#[cfg(unix)]
impl Drop for EndpointCleanup {
    fn drop(&mut self) {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        if std::fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
            metadata.file_type().is_socket()
                && metadata.ino() == self.inode
                && metadata.dev() == self.device
        }) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
struct ClientSlot(Arc<std::sync::atomic::AtomicUsize>);
impl ClientSlot {
    fn acquire(count: &Arc<std::sync::atomic::AtomicUsize>) -> Option<Self> {
        count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < 16).then_some(current + 1)
            })
            .ok()?;
        Some(Self(count.clone()))
    }
}
impl Drop for ClientSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl ManagerCore {
    pub(crate) fn shutdown_token(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.maintenance.cancel.clone()
    }

    #[cfg(unix)]
    pub fn start_integration_service(&self) -> Result<(), AppError> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
        use std::os::unix::net::{UnixListener, UnixStream};
        let path = self.app_data_dir.join("integration-v2.sock");
        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                let metadata = std::fs::symlink_metadata(&path)?;
                // SAFETY: geteuid has no preconditions.
                if !metadata.file_type().is_socket()
                    || metadata.uid() != unsafe { libc::geteuid() }
                    || UnixStream::connect(&path).is_ok()
                {
                    return Err(AppError::Busy(
                        "Outro proprietário já atende a integração.".to_owned(),
                    ));
                }
                let current = std::fs::symlink_metadata(&path)?;
                if current.ino() != metadata.ino() {
                    return Err(AppError::Busy(
                        "Endpoint mudou durante recuperação.".to_owned(),
                    ));
                }
                std::fs::remove_file(&path)?;
                UnixListener::bind(&path)?
            }
            Err(error) => return Err(error.into()),
        };
        let endpoint = EndpointCleanup::new(path.clone())?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let owner = self.tasks.enter()?;
        let core = self.clone();
        std::thread::Builder::new()
            .name("integration-listener".to_owned())
            .spawn(move || {
                let _owner = owner;
                let _endpoint = endpoint;
                let clients = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                while !core.maintenance.cancel.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            core.spawn_integration_client(stream, &clients);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(25))
                        }
                        Err(_) => break,
                    }
                }
                drop(listener);
            })?;
        Ok(())
    }

    #[cfg(windows)]
    pub fn start_integration_service(&self) -> Result<(), AppError> {
        let mut listener = crate::application::local_transport::windows::PipeListener::bind()?;
        let owner = self.tasks.enter()?;
        let core = self.clone();
        std::thread::Builder::new()
            .name("integration-listener".into())
            .spawn(move || {
                let _owner = owner;
                let clients = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                while !core.maintenance.cancel.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok(Some(stream)) => core.spawn_integration_client(stream, &clients),
                        Ok(None) | Err(AppError::AccessDenied(_)) => {}
                        Err(error) => {
                            let _ = crate::adapters::logging::record(
                                &core.database,
                                None,
                                None,
                                "integration_listener_failed",
                                serde_json::json!({"errorCode":error.code()}),
                            );
                            break;
                        }
                    }
                }
                drop(listener);
            })?;
        Ok(())
    }

    fn spawn_integration_client(
        &self,
        stream: impl crate::application::local_transport::LocalStream + Send + 'static,
        clients: &Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let Some(slot) = ClientSlot::acquire(clients) else {
            return;
        };
        let Ok(lease) = self.tasks.enter() else {
            return;
        };
        let worker = self.clone();
        let _ = std::thread::Builder::new()
            .name("integration-client".into())
            .spawn(move || {
                let _lease = lease;
                let _slot = slot;
                let _ = crate::application::integration_protocol::serve_connection(worker, stream);
            });
    }
}
