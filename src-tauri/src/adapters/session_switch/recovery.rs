use super::{current_account_id, SessionSwitchOutcome, SessionSwitcher, SwitchJournal};
use crate::adapters::auth_store;
use crate::error::AppError;

impl SessionSwitcher {
    pub fn recover(
        &self,
        was_committed: impl FnOnce(&str) -> Result<bool, AppError>,
    ) -> Result<Option<String>, AppError> {
        let journal = match std::fs::read_to_string(&self.journal_path) {
            Ok(content) => serde_json::from_str::<SwitchJournal>(&content)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(AppError::Io(error)),
        };
        if !matches!(journal.version, 1 | 2)
            || journal.version == 2 && journal.transaction_id.is_none()
        {
            return Err(AppError::Validation(
                "Versão ou identidade do diário de troca inválida; sessão preservada.".to_owned(),
            ));
        }
        if let Some(id) = journal.transaction_id.as_deref() {
            uuid::Uuid::parse_str(id).map_err(|_| {
                AppError::Validation("Diário de troca inválido; sessão preservada.".to_owned())
            })?;
            if was_committed(id)? {
                std::fs::remove_file(&self.journal_path)?;
                return current_account_id(&self.auth_path);
            }
            return self.recover_uncommitted(&journal);
        }
        if current_account_id(&self.auth_path)?.as_deref() == Some(&journal.target_account_id) {
            std::fs::remove_file(&self.journal_path)?;
            return Ok(Some(journal.target_account_id));
        }
        let recovery_id = journal
            .previous_account_id
            .as_deref()
            .unwrap_or(&journal.target_account_id);
        let document = self.credentials.load(recovery_id)?;
        auth_store::atomic_write_auth(&self.auth_path, &document)?;
        std::fs::remove_file(&self.journal_path)?;
        Ok(Some(recovery_id.to_owned()))
    }

    fn recover_uncommitted(&self, journal: &SwitchJournal) -> Result<Option<String>, AppError> {
        let current = current_account_id(&self.auth_path)?;
        if current == journal.previous_account_id {
            std::fs::remove_file(&self.journal_path)?;
            return Ok(current);
        }
        if current.as_deref() != Some(&journal.target_account_id) {
            return Err(AppError::Busy(
                "A sessão mudou fora da transação interrompida; recuperação automática bloqueada."
                    .to_owned(),
            ));
        }
        let previous = journal
            .previous_account_id
            .as_deref()
            .map(|id| self.credentials.load(id))
            .transpose()?;
        let desktop = self.desktop.stop()?;
        let result = (|| {
            let latest = auth_store::read_auth(&self.auth_path)?;
            auth_store::validate_auth_document(&latest, Some(&journal.target_account_id))?;
            self.credentials.save_generation(&latest)?;
            self.rollback(&self.auth_path, previous.as_ref())
        })();
        let restart = self.desktop.restart(&desktop);
        result?;
        restart?;
        std::fs::remove_file(&self.journal_path)?;
        Ok(journal.previous_account_id.clone())
    }

    /// Called only after SQLite committed its transaction marker with metadata.
    pub fn acknowledge_metadata(&self, outcome: &SessionSwitchOutcome) -> Result<(), AppError> {
        if outcome.transaction_id.is_none() {
            return Ok(());
        }
        let current: SwitchJournal =
            serde_json::from_reader(std::fs::File::open(&self.journal_path)?)?;
        if current.transaction_id != outcome.transaction_id {
            return Err(AppError::Busy(
                "O diário mudou; limpeza adiada para preservar a outra operação.".to_owned(),
            ));
        }
        std::fs::remove_file(&self.journal_path)?;
        Ok(())
    }

    pub fn rollback_materialized_switch(
        &self,
        outcome: &SessionSwitchOutcome,
    ) -> Result<(), AppError> {
        self.rollback(&self.auth_path, outcome.previous_document.as_ref())?;
        if let Some(desktop) = &outcome.desktop {
            self.desktop.restart(desktop)?;
        }
        match std::fs::remove_file(&self.journal_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}
