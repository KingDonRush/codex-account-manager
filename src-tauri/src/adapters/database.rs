mod account_lifecycle;
mod accounts;
pub use accounts::SwitchCommit;
mod application_servers;
mod application_sessions;
mod credentials;
mod diagnostics;
mod executions;
mod integration_authorization;
mod integrations;
mod operations;
mod profiles;
mod providers;
mod rows;
mod schema;
mod settings;
mod upgrade;
pub use upgrade::CollectedAccount;

use std::path::{Path, PathBuf};
use std::time::Duration;

use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;

use crate::error::AppError;

fn restrict_app_data_directory(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = std::fs::symlink_metadata(path)?;
        // SAFETY: geteuid has no pointers or preconditions.
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(AppError::AccessDenied(
                "Diretório de dados não pertence ao usuário ou é um link.".to_owned(),
            ));
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[derive(Clone)]
pub struct Database {
    pool: Pool<SqliteConnectionManager>,
    profiles_root: PathBuf,
}

impl Database {
    /// Read-only CLI queries never initialize schemas, trim logs or create data.
    #[cfg(test)]
    pub fn open_readonly(app_data_dir: &Path) -> Result<Self, AppError> {
        let manager = SqliteConnectionManager::file(app_data_dir.join("manager.sqlite3"))
            .with_flags(rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_init(|connection| {
                connection.execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 3000;")
            });
        let pool = Pool::builder()
            .max_size(1)
            .connection_timeout(Duration::from_secs(3))
            .build(manager)
            .map_err(|error| AppError::DatabasePool(error.to_string()))?;
        Ok(Self {
            pool,
            profiles_root: app_data_dir.join("profiles"),
        })
    }

    pub fn open(app_data_dir: &Path) -> Result<Self, AppError> {
        std::fs::create_dir_all(app_data_dir)?;
        restrict_app_data_directory(app_data_dir)?;
        let profiles_root = app_data_dir.join("profiles");
        std::fs::create_dir_all(&profiles_root)?;
        let path = app_data_dir.join("manager.sqlite3");
        let manager = SqliteConnectionManager::file(&path).with_init(|connection| {
            connection.execute_batch(
                "PRAGMA foreign_keys = ON;
                 PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = NORMAL;
                 PRAGMA busy_timeout = 3000;",
            )
        });
        let pool = Pool::builder()
            .max_size(4)
            .connection_timeout(Duration::from_secs(3))
            .build(manager)
            .map_err(|error| AppError::DatabasePool(error.to_string()))?;
        let database = Self {
            pool,
            profiles_root,
        };
        database.upgrade_state()?;
        database.seed_providers()?;
        database.trim_diagnostics()?;
        Ok(database)
    }

    fn connection(&self) -> Result<PooledConnection<SqliteConnectionManager>, AppError> {
        self.pool
            .get()
            .map_err(|error| AppError::DatabasePool(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::Database;
    use crate::domain::{AccountMetadata, ProfileStatus, RateLimits, ResetCredits, UsageWindow};
    use crate::ports::StoredCredentialGeneration;
    use chrono::Utc;

    #[test]
    fn metadata_queries_never_cancel_live_operations_or_write_the_database() {
        let fixture = tempfile::tempdir().expect("fixture");
        let writer = Database::open(fixture.path()).expect("writer");
        let profile = writer.create_profile("Live", None, None).expect("profile");
        let operation = writer
            .create_switch_operation(&profile.id)
            .expect("operation");
        let reader =
            crate::application::manager::ManagerCore::for_metadata(fixture.path().to_path_buf())
                .expect("metadata client");
        assert_eq!(
            reader.list_operations().expect("query")[0].state.as_str(),
            "idle"
        );
        let readonly = Database::open_readonly(fixture.path()).expect("readonly");
        assert!(readonly.rename_profile(&profile.id, "Wrong").is_err());
        assert_eq!(
            writer
                .operation(&operation.id)
                .expect("live operation")
                .state
                .as_str(),
            "idle"
        );
        assert_eq!(writer.profile(&profile.id).expect("profile").name, "Live");
    }

    #[test]
    fn switch_commit_marker_is_atomic_with_metadata() {
        let fixture = tempfile::tempdir().expect("fixture");
        let database = Database::open(fixture.path()).expect("database");
        let profile = database
            .create_profile("Account", None, None)
            .expect("profile");
        let account = || AccountMetadata {
            account_id: "target",
            email: None,
            plan_type: None,
            credential_ref: None,
            rate_limits: None,
            observed_at: Utc::now(),
        };
        let commit = || super::SwitchCommit {
            provider_id: None,
            previous_account_id: None,
            transaction_id: Some("transaction"),
        };
        assert!(database
            .persist_switch_commit("absent", account(), commit())
            .is_err());
        assert!(!database
            .switch_was_committed("transaction")
            .expect("not committed"));
        database
            .persist_switch_commit(&profile.id, account(), commit())
            .expect("commit");
        assert!(database
            .switch_was_committed("transaction")
            .expect("committed"));
    }

    #[test]
    fn profile_lifecycle_never_deletes_the_neighboring_profile() {
        let fixture = tempfile::tempdir().expect("fixture");
        let database = Database::open(fixture.path()).expect("database");
        let first = database
            .create_profile("Primeiro", None, None)
            .expect("first");
        let second = database
            .create_profile("Segundo", None, None)
            .expect("second");
        database
            .create_operation(&first.id, false)
            .expect("dependent operation");

        let archived = database
            .set_profile_status(&first.id, ProfileStatus::Archived)
            .expect("archive");
        assert!(matches!(archived.status, ProfileStatus::Archived));
        let restored = database
            .set_profile_status(&first.id, ProfileStatus::Active)
            .expect("restore");
        assert!(matches!(restored.status, ProfileStatus::Active));
        database
            .delete_profile(&first.id, true)
            .expect("delete first");

        assert!(database.profile(&first.id).is_err());
        assert_eq!(
            database.profile(&second.id).expect("second remains").name,
            "Segundo"
        );
        assert!(std::path::Path::new(&second.data_dir).is_dir());
    }

    #[test]
    fn account_plan_and_quota_are_returned_with_the_profile() {
        let fixture = tempfile::tempdir().expect("fixture");
        let database = Database::open(fixture.path()).expect("database");
        let profile = database
            .create_profile("Com quota", None, None)
            .expect("profile");
        let limits = RateLimits {
            primary: Some(UsageWindow {
                used_percent: Some(37.0),
                window_duration_mins: Some(300),
                resets_at: Some(1_800_000_000),
            }),
            secondary: None,
            rate_limit_reached_type: None,
            reset_credits: Some(ResetCredits {
                available_count: Some(2),
            }),
            ..RateLimits::default()
        };
        let observed_at = Utc::now() - chrono::Duration::minutes(30);
        database
            .persist_connected_account(
                &profile.id,
                AccountMetadata {
                    account_id: "account-1",
                    email: Some("person@example.com"),
                    plan_type: Some("plus"),
                    credential_ref: Some("native-keyring"),
                    rate_limits: Some(&limits),
                    observed_at,
                },
                None,
            )
            .expect("profile account");

        let loaded = database.profile(&profile.id).expect("loaded profile");

        assert_eq!(loaded.plan_type.as_deref(), Some("plus"));
        assert_eq!(
            loaded
                .rate_limits
                .expect("quota")
                .primary
                .expect("primary")
                .used_percent,
            Some(37.0)
        );
        assert_eq!(loaded.usage_updated_at, Some(observed_at));

        database
            .set_credential_state("account-1", crate::domain::CredentialState::Ready)
            .expect("credential audit");
        database
            .persist_switched_account(
                &profile.id,
                AccountMetadata {
                    account_id: "account-1",
                    email: None,
                    plan_type: None,
                    credential_ref: None,
                    rate_limits: None,
                    observed_at: Utc::now(),
                },
                None,
                None,
            )
            .expect("switch metadata");
        assert_eq!(
            database
                .profile(&profile.id)
                .expect("profile after unrelated updates")
                .usage_updated_at,
            Some(observed_at),
            "credential and rotation writes must not fake a quota observation"
        );
    }

    #[test]
    fn a_corrupt_directory_association_cannot_delete_another_profile() {
        let directory = tempfile::tempdir().expect("fixture");
        let database = Database::open(directory.path()).expect("database");
        let first = database.create_profile("First", None, None).expect("first");
        let neighbor = database
            .create_profile("Neighbor", None, None)
            .expect("neighbor");
        database
            .connection()
            .expect("fixture connection")
            .execute(
                "UPDATE profiles SET data_dir = ?1 WHERE id = ?2",
                rusqlite::params![neighbor.data_dir, first.id],
            )
            .expect("corrupt association");
        assert!(database.delete_profile(&first.id, true).is_err());
        assert!(std::path::Path::new(&neighbor.data_dir).is_dir());
        assert!(database.profile(&first.id).is_ok());
    }

    #[test]
    fn browser_open_records_last_use_and_browser_open_time_separately_from_authentication() {
        let fixture = tempfile::tempdir().expect("fixture");
        let database = Database::open(fixture.path()).expect("database");
        let profile = database
            .create_profile("Acesso do navegador", None, None)
            .expect("profile");

        database
            .record_browser_opened(&profile.id)
            .expect("browser open");

        let loaded = database.profile(&profile.id).expect("loaded");
        assert!(loaded.last_used_at.is_some());
        assert!(loaded.last_browser_opened_at.is_some());
        assert!(loaded.last_browser_authenticated_at.is_none());
    }

    #[test]
    fn account_identity_is_unique_across_visible_profiles() {
        let fixture = tempfile::tempdir().expect("fixture");
        let database = Database::open(fixture.path()).expect("database");
        let canonical = database
            .create_profile("Canônico", None, None)
            .expect("canonical");
        let duplicate = database
            .create_profile("Duplicado", None, None)
            .expect("duplicate");
        let metadata = |account_id| AccountMetadata {
            account_id,
            email: Some("same@example.com"),
            plan_type: None,
            credential_ref: None,
            rate_limits: None,
            observed_at: Utc::now(),
        };
        database
            .persist_connected_account(&canonical.id, metadata("same-account"), None)
            .expect("first association");

        assert!(database
            .persist_connected_account(&duplicate.id, metadata("same-account"), None)
            .is_err());
        assert_eq!(
            database
                .profile_for_account("same-account")
                .expect("query")
                .expect("canonical profile")
                .id,
            canonical.id
        );
    }

    #[test]
    fn successful_reauthentication_restores_an_archived_canonical_profile() {
        let fixture = tempfile::tempdir().expect("fixture");
        let database = Database::open(fixture.path()).expect("database");
        let profile = database
            .create_profile("Arquivado", None, None)
            .expect("profile");
        database
            .set_profile_status(&profile.id, ProfileStatus::Archived)
            .expect("archive");

        let restored = database
            .persist_connected_account(
                &profile.id,
                AccountMetadata {
                    account_id: "restored-account",
                    email: None,
                    plan_type: None,
                    credential_ref: None,
                    rate_limits: None,
                    observed_at: Utc::now(),
                },
                None,
            )
            .expect("reauthentication");

        assert_eq!(restored.status, ProfileStatus::Active);
    }

    #[test]
    fn startup_marks_unfinished_login_as_cancelled() {
        let fixture = tempfile::tempdir().expect("fixture");
        let database = Database::open(fixture.path()).expect("database");
        let profile = database
            .create_profile("Interrompido", None, None)
            .expect("profile");
        let operation = database
            .create_operation(&profile.id, true)
            .expect("operation");

        let recovered = database.cancel_interrupted_operations().expect("recovery");
        let loaded = database.operation(&operation.id).expect("loaded operation");

        assert_eq!(recovered, vec![(operation.id, profile.id)]);
        assert!(matches!(
            loaded.state,
            crate::domain::OperationState::Cancelled
        ));
        assert_eq!(loaded.error_code.as_deref(), Some("app_closed"));
        assert!(loaded.finished_at.is_some());
    }

    #[test]
    fn clicks_and_cancelled_operations_do_not_change_rotation_timestamps() {
        let fixture = tempfile::tempdir().expect("fixture");
        let database = Database::open(fixture.path()).expect("database");
        let profile = database
            .create_profile("Sem rotação", None, None)
            .expect("profile");
        database
            .persist_connected_account(
                &profile.id,
                AccountMetadata {
                    account_id: "account-click",
                    email: None,
                    plan_type: None,
                    credential_ref: None,
                    rate_limits: None,
                    observed_at: Utc::now(),
                },
                None,
            )
            .expect("account");
        database
            .create_operation(&profile.id, true)
            .expect("operation");
        database
            .cancel_interrupted_operations()
            .expect("cancel operation");

        let loaded = database.profile(&profile.id).expect("loaded");
        assert!(loaded.last_activated_at.is_none());
        assert!(loaded.last_disconnected_at.is_none());
    }

    #[test]
    fn validated_switch_updates_both_rotation_edges() {
        let fixture = tempfile::tempdir().expect("fixture");
        let database = Database::open(fixture.path()).expect("database");
        let first = database
            .create_profile("Primeira", None, None)
            .expect("first");
        let second = database
            .create_profile("Segunda", None, None)
            .expect("second");
        for (profile, account_id) in [(&first, "first"), (&second, "second")] {
            database
                .persist_connected_account(
                    &profile.id,
                    AccountMetadata {
                        account_id,
                        email: None,
                        plan_type: None,
                        credential_ref: None,
                        rate_limits: None,
                        observed_at: Utc::now(),
                    },
                    None,
                )
                .expect("account");
        }
        let switched_at = Utc::now();
        database
            .persist_switched_account(
                &second.id,
                AccountMetadata {
                    account_id: "second",
                    email: None,
                    plan_type: None,
                    credential_ref: None,
                    rate_limits: None,
                    observed_at: switched_at,
                },
                None,
                Some("first"),
            )
            .expect("switch timestamps");

        assert_eq!(
            database
                .profile(&first.id)
                .expect("first loaded")
                .last_disconnected_at,
            Some(switched_at)
        );
        assert_eq!(
            database
                .profile(&second.id)
                .expect("second loaded")
                .last_activated_at,
            Some(switched_at)
        );
    }

    #[test]
    fn quarantine_expiration_is_exactly_queryable_after_seven_days() {
        let fixture = tempfile::tempdir().expect("fixture");
        let database = Database::open(fixture.path()).expect("database");
        let created_at = Utc::now() - chrono::Duration::days(10);
        database
            .upsert_credential_generation(
                &StoredCredentialGeneration {
                    account_id: "account-vault".to_owned(),
                    generation_id: "generation-a".to_owned(),
                    fingerprint: "f".repeat(64),
                    created_at,
                    is_active: true,
                },
                true,
            )
            .expect("generation");
        database
            .quarantine_credential_generation(
                "account-vault",
                "generation-a",
                Utc::now() - chrono::Duration::days(8),
            )
            .expect("quarantine");

        assert_eq!(
            database
                .expired_credential_generations(Utc::now())
                .expect("expired"),
            vec![("account-vault".to_owned(), "generation-a".to_owned())]
        );
    }
}
