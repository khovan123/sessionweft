use async_trait::async_trait;
use sessionweft_scheduler::{
    RepositoryError, TaskExecutionQueueRepository, TaskExecutionRecord, TaskExecutionRepository,
};
use sqlx::Row;
use uuid::Uuid;

use super::{SqliteSchedulerRepository, backend};

#[async_trait]
impl TaskExecutionQueueRepository for SqliteSchedulerRepository {
    async fn executable_claim_ids(&self, limit: usize) -> Result<Vec<Uuid>, RepositoryError> {
        let _ = self.prepared_executions(1).await?;
        let rows = sqlx::query(
            r#"
            SELECT claim.claim_id
            FROM scheduler_claims AS claim
            JOIN scheduler_execution_specs AS spec
              ON spec.workflow_id = claim.workflow_id AND spec.node_id = claim.node_id
            LEFT JOIN scheduler_task_executions AS execution
              ON execution.claim_id = claim.claim_id
            WHERE claim.status = 'active' AND execution.execution_id IS NULL
            ORDER BY claim.updated_at ASC, claim.claim_id ASC
            LIMIT ?
            "#,
        )
        .bind(i64::try_from(limit.clamp(1, 1_000)).map_err(backend)?)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.into_iter()
            .map(|row| Uuid::parse_str(row.get::<&str, _>("claim_id")).map_err(backend))
            .collect()
    }

    async fn failed_unfinalized_executions(
        &self,
        limit: usize,
    ) -> Result<Vec<TaskExecutionRecord>, RepositoryError> {
        let _ = self.prepared_executions(1).await?;
        let rows = sqlx::query(
            r#"
            SELECT data_json FROM scheduler_task_executions
            WHERE status = 'failed' AND claim_finalized = 0
            ORDER BY updated_at ASC LIMIT ?
            "#,
        )
        .bind(i64::try_from(limit.clamp(1, 1_000)).map_err(backend)?)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.into_iter()
            .map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .collect()
    }
}
