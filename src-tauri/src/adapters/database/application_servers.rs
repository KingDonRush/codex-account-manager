use super::Database;
use crate::domain::application_server::ApplicationServerStatus;
use crate::error::AppError;
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Saved {
    status: ApplicationServerStatus,
    cwd: String,
}
impl Database {
    pub fn register_history_export(&self, id: &str) -> Result<(), AppError> {
        self.connection()?.execute("INSERT INTO integration_objects(kind,id,revision,body) VALUES('history_export',?1,1,?2)", params![id, serde_json::to_string(&chrono::Utc::now())?])?;
        Ok(())
    }
    pub fn expired_history_exports(&self) -> Result<Vec<String>, AppError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT id,body FROM integration_objects WHERE kind='history_export'")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, body)| {
                serde_json::from_str::<chrono::DateTime<chrono::Utc>>(&body)
                    .ok()
                    .filter(|at| chrono::Utc::now() - at >= chrono::Duration::days(7))
                    .map(|_| id)
            })
            .collect())
    }
    pub fn remove_history_export_metadata(&self, id: &str) -> Result<(), AppError> {
        self.connection()?.execute(
            "DELETE FROM integration_objects WHERE kind='history_export' AND id=?1",
            [id],
        )?;
        Ok(())
    }
    pub fn recover_application_servers(&self) -> Result<Vec<ApplicationServerStatus>, AppError> {
        use crate::domain::application_server::ServerState;
        let mut saved: Vec<Saved> = self.integration_objects("server")?;
        for item in &mut saved {
            if !item.status.state.terminal() {
                item.status.state = ServerState::Interrupted;
                item.status.error_code = Some("owner_interrupted".to_owned());
                self.persist_application_server(&item.status, &item.cwd)?;
            }
        }
        Ok(saved.into_iter().map(|item| item.status).collect())
    }
    pub fn persist_application_server(
        &self,
        status: &ApplicationServerStatus,
        cwd: &str,
    ) -> Result<(), AppError> {
        self.connection()?.execute(
            "INSERT INTO integration_objects(kind,id,revision,body) VALUES('server',?1,1,?2)
            ON CONFLICT(kind,id) DO UPDATE SET revision=revision+1,body=excluded.body",
            params![
                status.id,
                serde_json::to_string(&Saved {
                    status: status.clone(),
                    cwd: cwd.to_owned()
                })?
            ],
        )?;
        Ok(())
    }
    pub fn saved_application_server(
        &self,
        id: &str,
    ) -> Result<(ApplicationServerStatus, String), AppError> {
        let saved: Saved = self
            .integration_object("server", id)?
            .ok_or_else(|| AppError::OperationNotFound(id.to_owned()))?;
        Ok((saved.status, saved.cwd))
    }
}
