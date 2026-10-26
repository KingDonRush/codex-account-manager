mod account_cleanup;
mod account_collection;
mod account_lifecycle;
mod app_servers;
mod application_auth;
mod application_credentials;
mod authorization;
mod background;
mod credential_maintenance;
mod deletion;
mod execution;
mod execution_route;
mod integration_service;
mod integrations;
mod login_flow;
mod operation_control;
mod profile_management;
mod replacement;
mod runtime_control;
mod shutdown;
mod switch_flow;
mod usage_credentials;
mod usage_refresh;

use operation_control::ensure_login_active;
use profile_management::validate_mail_url;

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use parking_lot::{Condvar, Mutex, RwLock};
use serde_json::json;

use crate::adapters::auth_store;
use crate::adapters::browser_runtime::{BrowserHandle, BrowserRuntime};
use crate::adapters::credential_vault::CredentialVault;
use crate::adapters::database::Database;
use crate::adapters::logging;
use crate::adapters::session_switch::SessionSwitcher;
use crate::domain::{
    DashboardSnapshot, LoginOperation, ManagedProfile, ManagerSettings, OperationKind,
    OperationStage, ProfileOptions, RuntimeStatus,
};
use crate::error::AppError;
use crate::ports::{CredentialStore, EventSink, UsageReader};

struct OperationControl {
    profile_id: String,
    kind: OperationKind,
    cancel: Arc<AtomicBool>,
    cancelable: AtomicBool,
    cancellation_gate: Mutex<()>,
    browser: Arc<Mutex<Option<BrowserHandle>>>,
    replacement: Mutex<Option<crate::domain::ReplacementPreview>>,
    confirmed: AtomicBool,
}

#[derive(Default)]
struct OperationRegistry {
    controls: HashMap<String, Arc<OperationControl>>,
    starting: HashMap<String, OperationKind>,
}

struct RetainedLoginBrowser {
    operation_id: String,
    handle: BrowserHandle,
}

struct MaintenanceControl {
    started: AtomicBool,
    stopped: Mutex<bool>,
    wake: Condvar,
    cancel: Arc<AtomicBool>,
    cleanup_requested: AtomicBool,
}

const BROWSER_INITIAL_NAVIGATION_OBSERVATION: Duration = Duration::from_secs(8);
const BROWSER_RETRY_NAVIGATION_OBSERVATION: Duration = Duration::from_secs(37);

#[derive(Clone)]
pub struct ManagerCore {
    #[cfg(test)]
    exec_test_binary: Option<PathBuf>,
    #[cfg(test)]
    login_test_binary: Option<PathBuf>,
    database: Database,
    browser: BrowserRuntime,
    app_data_dir: PathBuf,
    codex_home: Arc<PathBuf>,
    operations: Arc<Mutex<OperationRegistry>>,
    browser_sessions: Arc<Mutex<HashMap<String, BrowserHandle>>>,
    retained_login_browsers: Arc<Mutex<HashMap<String, RetainedLoginBrowser>>>,
    event_sink: Arc<RwLock<Option<Arc<dyn EventSink>>>>,
    active_account_id: Arc<RwLock<Option<String>>>,
    session_switcher: Arc<SessionSwitcher>,
    switch_lock: Arc<Mutex<()>>,
    credentials: Arc<dyn CredentialStore>,
    application_credentials: Arc<dyn crate::ports::ApplicationSessionStore>,
    maintenance: Arc<MaintenanceControl>,
    usage_reader: Arc<dyn UsageReader>,
    usage_lock: Arc<Mutex<Option<usage_refresh::CompletedUsage>>>,
    usage_access: Arc<Mutex<HashMap<String, usage_credentials::CachedAccess>>>,
    shared_runtime: Arc<crate::adapters::shared_runtime::SharedRuntime>,
    tasks: Arc<super::task_tracker::TaskTracker>,
    shutdown_running: Arc<AtomicBool>,
    exit_ready: Arc<AtomicBool>,
    dashboard_revision: Arc<AtomicU64>,
    authorization: Arc<super::authorization::AuthorizationBook>,
    deletions: Arc<super::deletions::DeletionBook>,
    executions: Arc<super::execution_book::ExecutionBook>,
    // Admission and destructive mutation share a boundary, not a check-then-act race.
    account_admission: Arc<Mutex<()>>,
    application_servers: Arc<super::application_servers::ApplicationServers>,
    ui_visible: Arc<AtomicBool>,
}

impl ManagerCore {
    pub fn with_bundled_runtime(
        app_data_dir: PathBuf,
        bundled_runtime_dir: Option<PathBuf>,
    ) -> Result<Self, AppError> {
        let database = Database::open(&app_data_dir)?;
        let interrupted = database.cancel_interrupted_operations()?;
        for (operation_id, profile_id) in interrupted {
            logging::record(
                &database,
                Some(&operation_id),
                Some(&profile_id),
                "operation_recovered_after_app_exit",
                json!({ "state": "cancelled", "errorCode": "app_closed" }),
            )?;
        }
        let vault: Arc<dyn CredentialStore> = Arc::new(CredentialVault::new(database.clone()));
        let credentials: Arc<dyn CredentialStore> = Arc::new(
            crate::adapters::desktop_credential_recovery::RecoverableDesktopStore::new(
                &app_data_dir,
                vault,
                Arc::new(crate::adapters::application_recovery::NativeRecoveryKeys),
            ),
        );
        Self::from_components(app_data_dir, bundled_runtime_dir, database, credentials)
    }

    /// Metadata clients must not recover/cancel operations belonging to the UI.
    #[cfg(test)]
    pub fn for_metadata(app_data_dir: PathBuf) -> Result<Self, AppError> {
        let database = Database::open_readonly(&app_data_dir)?;
        let credentials = Arc::new(CredentialVault::new(database.clone()));
        Self::from_components(app_data_dir, None, database, credentials)
    }

    fn from_components(
        app_data_dir: PathBuf,
        bundled_runtime_dir: Option<PathBuf>,
        database: Database,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self, AppError> {
        #[cfg(not(test))]
        let codex_home = auth_store::codex_home();
        #[cfg(test)]
        let codex_home = app_data_dir.join("codex-home-fixture");
        #[cfg(test)]
        std::fs::create_dir_all(&codex_home)?;
        let session_switcher = Arc::new(SessionSwitcher::new(
            &app_data_dir,
            &codex_home,
            credentials.clone(),
            database.clone(),
        ));
        let active_account_id = auth_store::active_account_id_at(&codex_home);
        let application_credentials = Arc::new(
            crate::adapters::application_recovery::RecoverableApplicationStore::new(
                &app_data_dir,
                Arc::new(crate::adapters::keyring_store::ApplicationCredentialStore),
                Arc::new(crate::adapters::application_recovery::NativeRecoveryKeys),
            ),
        );
        Ok(Self {
            #[cfg(test)]
            exec_test_binary: None,
            #[cfg(test)]
            login_test_binary: None,
            database: database.clone(),
            browser: BrowserRuntime::with_bundled_runtime(&app_data_dir, bundled_runtime_dir),
            app_data_dir,
            codex_home: Arc::new(codex_home.clone()),
            operations: Arc::new(Mutex::new(OperationRegistry::default())),
            browser_sessions: Arc::new(Mutex::new(HashMap::new())),
            retained_login_browsers: Arc::new(Mutex::new(HashMap::new())),
            event_sink: Arc::new(RwLock::new(None)),
            active_account_id: Arc::new(RwLock::new(active_account_id)),
            session_switcher,
            switch_lock: Arc::new(Mutex::new(())),
            credentials,
            application_credentials,
            maintenance: Arc::new(MaintenanceControl {
                started: AtomicBool::new(false),
                stopped: Mutex::new(false),
                wake: Condvar::new(),
                cancel: Arc::new(AtomicBool::new(false)),
                cleanup_requested: AtomicBool::new(false),
            }),
            usage_reader: Arc::new(crate::adapters::usage_reader::AppServerUsageReader::default()),
            usage_lock: Arc::new(Mutex::new(None)),
            usage_access: Arc::new(Mutex::new(HashMap::new())),
            shared_runtime: Arc::new(crate::adapters::shared_runtime::SharedRuntime::new(
                codex_home, database,
            )),
            tasks: Arc::new(super::task_tracker::TaskTracker::default()),
            shutdown_running: Arc::new(AtomicBool::new(false)),
            exit_ready: Arc::new(AtomicBool::new(false)),
            dashboard_revision: Arc::new(AtomicU64::new(0)),
            authorization: Arc::new(super::authorization::AuthorizationBook::default()),
            deletions: Arc::new(super::deletions::DeletionBook::default()),
            executions: Arc::new(super::execution_book::ExecutionBook::default()),
            account_admission: Arc::new(Mutex::new(())),
            application_servers: Arc::new(super::application_servers::ApplicationServers::default()),
            ui_visible: Arc::new(AtomicBool::new(true)),
        })
    }

    pub fn set_event_sink(&self, sink: Arc<dyn EventSink>) {
        *self.event_sink.write() = Some(sink);
        self.audit_credentials_in_background();
        self.start_credential_maintenance();
    }

    pub fn list_profiles(&self) -> Result<Vec<ManagedProfile>, AppError> {
        let (active_account_id, refreshed_at) = if let Some(_guard) = self.switch_lock.try_lock() {
            let document = auth_store::read_auth(&self.codex_home.join("auth.json")).ok();
            let current = document
                .as_ref()
                .and_then(|document| auth_store::validate_auth_document(document, None).ok());
            let refreshed_at = document
                .as_ref()
                .and_then(|document| document.get("last_refresh"))
                .and_then(serde_json::Value::as_str)
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|date| date.with_timezone(&Utc));
            *self.active_account_id.write() = current.clone();
            (current, refreshed_at)
        } else {
            (self.active_account_id.read().clone(), None)
        };
        let mut profiles = self.database.list_profiles()?;
        let options = self.database.profile_options()?;
        for profile in &mut profiles {
            if let Some(remote) = profile.account_id.as_deref() {
                if let Ok((_, policy, _, mail)) = self.database.account_integration_options(remote)
                {
                    profile.open_mail_with_login = mail;
                    if policy == crate::domain::integrations::AccountPolicy::ApplicationsOnly {
                        profile.rotation_enabled = false;
                    }
                }
            }
            if let Some(options) = options.get(&profile.id) {
                profile.rotation_enabled = options.rotation_enabled;
                profile.preferred = options.preferred;
            }
            profile.is_active_account = profile
                .account_id
                .as_deref()
                .is_some_and(|account_id| Some(account_id) == active_account_id.as_deref());
            profile.needs_activation = needs_activation(profile, refreshed_at);
            if profile
                .account_id
                .as_deref()
                .and_then(|id| self.database.account_integration_options(id).ok())
                .is_some_and(|(_, policy, _, _)| {
                    policy == crate::domain::integrations::AccountPolicy::ApplicationsOnly
                })
            {
                profile.rotation_enabled = false;
            }
        }
        Ok(profiles)
    }

    pub fn dashboard_snapshot(&self) -> Result<DashboardSnapshot, AppError> {
        let revision = self.dashboard_revision.load(Ordering::Acquire);
        let mut snapshot = crate::application::dashboard::build_snapshot_with_strategy(
            self.list_profiles()?,
            Utc::now(),
            self.database.settings()?.rotation_strategy,
        );
        snapshot.revision = revision;
        Ok(snapshot)
    }

    pub fn list_operations(&self) -> Result<Vec<LoginOperation>, AppError> {
        self.database.list_operations()
    }

    pub fn settings(&self) -> Result<ManagerSettings, AppError> {
        self.database.settings()
    }

    pub fn save_settings(&self, settings: &ManagerSettings) -> Result<(), AppError> {
        self.database.save_settings(settings)?;
        self.emit_dashboard_changed();
        Ok(())
    }

    pub fn save_profile_options(&self, id: &str, options: &ProfileOptions) -> Result<(), AppError> {
        self.database.save_profile_options(id, options)?;
        self.emit_dashboard_changed();
        Ok(())
    }

    pub fn operation_stages(&self, operation_id: &str) -> Result<Vec<OperationStage>, AppError> {
        self.database.operation_stages(operation_id)
    }
    pub fn runtime_status(&self) -> RuntimeStatus {
        self.browser.status()
    }

    pub fn export_diagnostics(&self) -> Result<String, AppError> {
        let directory = self.app_data_dir.join("exports");
        std::fs::create_dir_all(&directory)?;
        let name = format!(
            "diagnostics-{}-{}.jsonl",
            Utc::now().format("%Y%m%d-%H%M%S"),
            &uuid::Uuid::new_v4().to_string()[..8],
        );
        let path = directory.join(&name);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        for mut event in self.database.diagnostic_export()? {
            logging::sanitize_export(&mut event);
            serde_json::to_writer(&mut file, &event)?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
        // Return only the generated name. The absolute application-data path
        // stays inside the native layer instead of becoming frontend state.
        Ok(name)
    }
}

fn needs_activation(profile: &ManagedProfile, refreshed_at: Option<chrono::DateTime<Utc>>) -> bool {
    let Some(login) = profile.last_browser_authenticated_at else {
        return false;
    };
    profile.is_active_account
        && profile
            .last_activated_at
            .is_none_or(|activated| activated < login)
        && refreshed_at.is_none_or(|refreshed| refreshed < login)
}

fn mask_id(value: &str) -> String {
    let characters = value.chars().collect::<Vec<_>>();
    if characters.len() <= 8 {
        return "[id]".to_owned();
    }
    let prefix = characters[..4].iter().collect::<String>();
    let suffix = characters[characters.len() - 4..]
        .iter()
        .collect::<String>();
    format!("{prefix}…{suffix}")
}

#[cfg(test)]
mod tests;
