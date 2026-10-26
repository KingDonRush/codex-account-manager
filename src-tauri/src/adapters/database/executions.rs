use super::Database;
use crate::domain::execution::{ExecutionState, ExecutionStatus};
use crate::error::AppError;
use rusqlite::params;
impl Database {
    pub fn persist_execution(&self, status: &ExecutionStatus) -> Result<(), AppError> {
        self.connection()?.execute(
            "INSERT INTO integration_objects(kind,id,revision,body) VALUES('execution',?1,1,?2)
            ON CONFLICT(kind,id) DO UPDATE SET revision=revision+1,body=excluded.body",
            params![status.id, serde_json::to_string(status)?],
        )?;
        Ok(())
    }
    pub fn recover_execution_metadata(&self) -> Result<Vec<ExecutionStatus>, AppError> {
        let mut jobs: Vec<ExecutionStatus> = self.integration_objects("execution")?;
        for job in &mut jobs {
            if !job.state.terminal() {
                job.state = ExecutionState::Interrupted;
                job.error_code = Some("owner_interrupted".to_owned());
                job.finished_at = Some(chrono::Utc::now());
                self.persist_execution(job)?;
            }
        }
        Ok(jobs)
    }
}
