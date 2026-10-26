use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use super::rows::profile_from_row;
use super::Database;
use crate::domain::{ManagedProfile, ProfileStatus};
use crate::error::AppError;

impl Database {
    pub fn list_profiles(&self) -> Result<Vec<ManagedProfile>, AppError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT p.id, p.name, p.account_id, p.email, p.mail_provider_id, p.mail_url,
                    p.data_dir, p.status, p.last_used_at, p.last_error, p.created_at,
                    p.updated_at, a.plan_type, a.rate_limits_json, a.quota_observed_at,
                    a.last_activated_at, a.last_disconnected_at,
                    p.last_browser_opened_at, a.last_browser_authenticated_at,
                    COALESCE(a.credential_state, 'not_connected')
             FROM profiles p LEFT JOIN accounts a ON a.id = p.account_id
             WHERE p.status != 'deleted' ORDER BY p.status ASC, p.name COLLATE NOCASE ASC",
        )?;
        let rows = statement.query_map([], profile_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn profile(&self, profile_id: &str) -> Result<ManagedProfile, AppError> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT p.id, p.name, p.account_id, p.email, p.mail_provider_id, p.mail_url,
                        p.data_dir, p.status, p.last_used_at, p.last_error, p.created_at,
                        p.updated_at, a.plan_type, a.rate_limits_json, a.quota_observed_at,
                        a.last_activated_at, a.last_disconnected_at,
                        p.last_browser_opened_at, a.last_browser_authenticated_at,
                        COALESCE(a.credential_state, 'not_connected')
                 FROM profiles p LEFT JOIN accounts a ON a.id = p.account_id WHERE p.id = ?1",
                [profile_id],
                profile_from_row,
            )
            .optional()?
            .ok_or_else(|| AppError::ProfileNotFound(profile_id.to_owned()))
    }

    pub fn profile_for_account(
        &self,
        account_id: &str,
    ) -> Result<Option<ManagedProfile>, AppError> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT p.id, p.name, p.account_id, p.email, p.mail_provider_id, p.mail_url,
                        p.data_dir, p.status, p.last_used_at, p.last_error, p.created_at,
                        p.updated_at, a.plan_type, a.rate_limits_json, a.quota_observed_at,
                        a.last_activated_at, a.last_disconnected_at,
                        p.last_browser_opened_at, a.last_browser_authenticated_at,
                        COALESCE(a.credential_state, 'not_connected')
                 FROM profiles p LEFT JOIN accounts a ON a.id = p.account_id
                 WHERE p.account_id = ?1 AND p.status != 'deleted'",
                [account_id],
                profile_from_row,
            )
            .optional()
            .map_err(AppError::from)
    }

    pub fn create_profile(
        &self,
        name: &str,
        provider_id: Option<&str>,
        mail_url: Option<&str>,
    ) -> Result<ManagedProfile, AppError> {
        let clean_name = clean_name(name)?;
        let id = Uuid::new_v4().to_string();
        let data_dir = self.profiles_root.join(&id);
        std::fs::create_dir_all(&data_dir)?;
        restrict_directory(&data_dir)?;
        let now = Utc::now();
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO profiles
             (id, name, account_id, email, mail_provider_id, mail_url, data_dir, status,
              last_used_at, last_error, created_at, updated_at)
             VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?5, 'active', NULL, NULL, ?6, ?6)",
            params![
                id,
                clean_name,
                provider_id,
                mail_url,
                data_dir.to_string_lossy(),
                now.to_rfc3339()
            ],
        )?;
        drop(connection);
        self.profile(&id)
    }

    pub fn rename_profile(&self, profile_id: &str, name: &str) -> Result<ManagedProfile, AppError> {
        let clean_name = clean_name(name)?;
        self.ensure_profile(profile_id)?;
        let connection = self.connection()?;
        connection.execute(
            "UPDATE profiles SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![clean_name, Utc::now().to_rfc3339(), profile_id],
        )?;
        drop(connection);
        self.profile(profile_id)
    }

    pub fn set_profile_status(
        &self,
        profile_id: &str,
        status: ProfileStatus,
    ) -> Result<ManagedProfile, AppError> {
        self.ensure_profile(profile_id)?;
        let connection = self.connection()?;
        connection.execute(
            "UPDATE profiles SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.as_str(), Utc::now().to_rfc3339(), profile_id],
        )?;
        drop(connection);
        self.profile(profile_id)
    }

    pub fn delete_profile(&self, profile_id: &str, permanently: bool) -> Result<(), AppError> {
        let profile = self.profile(profile_id)?;
        if !permanently {
            self.set_profile_status(profile_id, ProfileStatus::Archived)?;
            return Ok(());
        }
        let quarantined = self.quarantine_profile_directory(&profile)?;
        if let Err(error) = self.delete_profile_rows(profile_id) {
            if let Some(path) = quarantined.as_deref() {
                let _ = std::fs::rename(path, &profile.data_dir);
            }
            return Err(error);
        }
        if let Some(path) = quarantined {
            std::fs::remove_dir_all(path)?;
        }
        Ok(())
    }

    pub fn discard_unconnected_profile(&self, profile_id: &str) -> Result<(), AppError> {
        let profile = self.profile(profile_id)?;
        if profile.account_id.is_some() {
            return Err(AppError::Validation(
                "somente um perfil temporário sem conta pode ser descartado".to_owned(),
            ));
        }
        let quarantined = self.quarantine_profile_directory(&profile)?;
        let connection = self.connection()?;
        connection.execute(
            "UPDATE profiles SET status = 'deleted', updated_at = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), profile_id],
        )?;
        if let Some(path) = quarantined {
            std::fs::remove_dir_all(path)?;
        }
        Ok(())
    }

    fn quarantine_profile_directory(
        &self,
        profile: &ManagedProfile,
    ) -> Result<Option<PathBuf>, AppError> {
        let source = PathBuf::from(&profile.data_dir);
        if !source.exists() {
            return Ok(None);
        }
        if std::fs::symlink_metadata(&source)?.file_type().is_symlink() {
            return Err(AppError::Validation(
                "o diretório do perfil não pode ser um link simbólico".to_owned(),
            ));
        }
        let root = self.profiles_root.canonicalize()?;
        let canonical_source = source.canonicalize()?;
        if !canonical_source.starts_with(&root) || canonical_source == root {
            return Err(AppError::Validation(
                "o diretório do perfil está fora da área gerenciada".to_owned(),
            ));
        }
        if canonical_source != root.join(&profile.id) {
            return Err(AppError::Validation(
                "O diretório não pertence ao UUID deste perfil; a exclusão foi bloqueada."
                    .to_owned(),
            ));
        }
        let quarantine = root.join(format!(".deleting-{}", Uuid::new_v4()));
        std::fs::rename(&canonical_source, &quarantine)?;
        Ok(Some(quarantine))
    }

    fn delete_profile_rows(&self, profile_id: &str) -> Result<(), AppError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM diagnostic_events WHERE profile_id = ?1",
            [profile_id],
        )?;
        transaction.execute("DELETE FROM operations WHERE profile_id = ?1", [profile_id])?;
        transaction.execute("DELETE FROM profiles WHERE id = ?1", [profile_id])?;
        super::settings::remove_profile_options(&transaction, profile_id)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn profile_count_for_account(&self, account_id: &str) -> Result<usize, AppError> {
        let connection = self.connection()?;
        let count = connection.query_row(
            "SELECT COUNT(*) FROM profiles WHERE account_id = ?1 AND status != 'deleted'",
            [account_id],
            |row| row.get::<_, i64>(0),
        )?;
        usize::try_from(count)
            .map_err(|_| AppError::Internal("contagem de perfis inválida".to_owned()))
    }

    pub fn record_browser_opened(&self, profile_id: &str) -> Result<(), AppError> {
        self.ensure_profile(profile_id)?;
        let now = Utc::now().to_rfc3339();
        let connection = self.connection()?;
        connection.execute(
            "UPDATE profiles SET last_used_at = ?1, last_browser_opened_at = ?1,
                    updated_at = ?1 WHERE id = ?2",
            params![now, profile_id],
        )?;
        Ok(())
    }

    pub fn update_profile_mail_url(
        &self,
        profile_id: &str,
        mail_url: Option<&str>,
    ) -> Result<ManagedProfile, AppError> {
        self.ensure_profile(profile_id)?;
        let connection = self.connection()?;
        connection.execute(
            "UPDATE profiles SET mail_url = ?1, updated_at = ?2 WHERE id = ?3",
            params![mail_url, Utc::now().to_rfc3339(), profile_id],
        )?;
        drop(connection);
        self.profile(profile_id)
    }

    pub fn update_profile_error(&self, profile_id: &str, message: &str) -> Result<(), AppError> {
        self.ensure_profile(profile_id)?;
        let connection = self.connection()?;
        connection.execute(
            "UPDATE profiles SET last_error = ?1, updated_at = ?2 WHERE id = ?3",
            params![message, Utc::now().to_rfc3339(), profile_id],
        )?;
        Ok(())
    }

    pub(super) fn ensure_profile(&self, profile_id: &str) -> Result<(), AppError> {
        let connection = self.connection()?;
        let exists = connection.query_row::<i64, _, _>(
            "SELECT COUNT(*) FROM profiles WHERE id = ?1",
            [profile_id],
            |row| row.get(0),
        )? > 0;
        if exists {
            Ok(())
        } else {
            Err(AppError::ProfileNotFound(profile_id.to_owned()))
        }
    }
}

fn clean_name(value: &str) -> Result<String, AppError> {
    let name = value.trim();
    if name.is_empty() {
        return Err(AppError::Validation(
            "o nome do perfil não pode ficar vazio".to_owned(),
        ));
    }
    if name.chars().count() > 80 {
        return Err(AppError::Validation(
            "o nome do perfil pode ter no máximo 80 caracteres".to_owned(),
        ));
    }
    Ok(name.to_owned())
}

fn restrict_directory(_path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(_path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
