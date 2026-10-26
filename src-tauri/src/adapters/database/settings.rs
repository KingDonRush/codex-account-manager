use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};

use super::Database;
use crate::domain::{ManagerSettings, ProfileOptions};
use crate::error::AppError;

impl Database {
    pub(crate) fn desktop_executable(&self) -> Result<Option<std::path::PathBuf>, AppError> {
        let selected: Option<String> = self.read_setting("desktop_executable")?;
        Ok(selected.map(std::path::PathBuf::from))
    }

    #[cfg(any(windows, test))]
    pub(crate) fn save_desktop_executable(&self, path: &std::path::Path) -> Result<(), AppError> {
        self.write_setting(
            "desktop_executable",
            &Some(path.to_string_lossy().into_owned()),
        )
    }

    pub fn settings(&self) -> Result<ManagerSettings, AppError> {
        self.read_setting("manager_settings")
    }

    pub fn save_settings(&self, settings: &ManagerSettings) -> Result<(), AppError> {
        self.write_setting("manager_settings", settings)
    }

    pub fn profile_options(&self) -> Result<HashMap<String, ProfileOptions>, AppError> {
        self.read_setting("profile_options")
    }

    pub fn save_profile_options(&self, id: &str, options: &ProfileOptions) -> Result<(), AppError> {
        self.profile(id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let json: Option<String> = transaction
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'profile_options'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let mut all: HashMap<String, ProfileOptions> = json
            .map(|value| serde_json::from_str(&value))
            .transpose()?
            .unwrap_or_default();
        all.insert(id.to_owned(), options.clone());
        transaction.execute(
            "INSERT INTO schema_meta (key, value) VALUES ('profile_options', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [serde_json::to_string(&all)?],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn read_setting<T: DeserializeOwned + Default>(&self, key: &str) -> Result<T, AppError> {
        let json: Option<String> = self
            .connection()?
            .query_row(
                "SELECT value FROM schema_meta WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(json
            .map(|value| serde_json::from_str(&value))
            .transpose()?
            .unwrap_or_default())
    }

    fn write_setting(&self, key: &str, value: &impl Serialize) -> Result<(), AppError> {
        self.connection()?.execute(
            "INSERT INTO schema_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, serde_json::to_string(value)?],
        )?;
        Ok(())
    }
}

pub(super) fn remove_profile_options(
    connection: &rusqlite::Connection,
    id: &str,
) -> Result<(), AppError> {
    let json: Option<String> = connection
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'profile_options'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(json) = json else {
        return Ok(());
    };
    let mut options: HashMap<String, ProfileOptions> = serde_json::from_str(&json)?;
    options.remove(id);
    if options.is_empty() {
        connection.execute("DELETE FROM schema_meta WHERE key = 'profile_options'", [])?;
    } else {
        connection.execute(
            "UPDATE schema_meta SET value = ?1 WHERE key = 'profile_options'",
            [serde_json::to_string(&options)?],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod desktop_tests {
    use super::*;

    #[test]
    fn selecting_a_desktop_preserves_other_settings_and_survives_reopening() {
        let fixture = tempfile::tempdir().expect("fixture");
        let database = Database::open(fixture.path()).expect("database");
        let settings = ManagerSettings {
            rotation_strategy: crate::domain::RotationStrategy::LargestBalance,
        };
        database.save_settings(&settings).expect("settings");
        let path = fixture.path().join("ChatGPT.exe");
        database.save_desktop_executable(&path).expect("selection");
        drop(database);
        let reopened = Database::open(fixture.path()).expect("reopened");
        assert_eq!(
            reopened.desktop_executable().expect("selection"),
            Some(path)
        );
        assert_eq!(reopened.settings().expect("settings"), settings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::RotationStrategy;

    #[test]
    fn preferences_survive_reopen_without_touching_profiles() {
        let fixture = tempfile::tempdir().expect("fixture");
        let db = Database::open(fixture.path()).expect("database");
        let profile = db.create_profile("B3", None, None).expect("profile");
        let options = ProfileOptions {
            rotation_enabled: false,
            preferred: true,
        };
        db.save_profile_options(&profile.id, &options)
            .expect("options");
        let settings = ManagerSettings {
            rotation_strategy: RotationStrategy::LargestBalance,
        };
        db.save_settings(&settings).expect("settings");
        drop(db);
        let db = Database::open(fixture.path()).expect("reopen");
        assert_eq!(db.settings().expect("settings"), settings);
        assert_eq!(
            db.profile_options().expect("options").get(&profile.id),
            Some(&options)
        );
        assert_eq!(db.profile(&profile.id).expect("profile").name, "B3");
    }
}
