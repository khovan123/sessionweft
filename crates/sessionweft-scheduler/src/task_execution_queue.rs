use async_trait::async_trait;
use uuid::Uuid;

use super::{RepositoryError, TaskExecutionRecord};

#[async_trait]
pub trait TaskExecutionQueueRepository: Send + Sync {
    async fn executable_claim_ids(&self, limit: usize) -> Result<Vec<Uuid>, RepositoryError>;

    async fn failed_unfinalized_executions(
        &self,
        limit: usize,
    ) -> Result<Vec<TaskExecutionRecord>, RepositoryError>;
}
