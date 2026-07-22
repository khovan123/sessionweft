use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{ClaimState, RepositoryError, SchedulerError};

#[async_trait]
pub trait SchedulerRecoveryRepository: Send + Sync {
    async fn recover_stale_claims(
        &self,
        now: DateTime<Utc>,
        limit: usize,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Vec<ClaimState>, RepositoryError>;
}

#[derive(Clone)]
pub struct SchedulerRecoveryService<R>
where
    R: SchedulerRecoveryRepository,
{
    repository: Arc<R>,
}

impl<R> SchedulerRecoveryService<R>
where
    R: SchedulerRecoveryRepository,
{
    #[must_use]
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    pub async fn recover_stale_claims(
        &self,
        now: DateTime<Utc>,
        limit: usize,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Vec<ClaimState>, SchedulerError> {
        if limit == 0 || limit > 1_000 {
            return Err(SchedulerError::Validation(
                "stale recovery limit must be between 1 and 1000".into(),
            ));
        }
        self.repository
            .recover_stale_claims(now, limit, correlation_id, actor_id)
            .await
            .map_err(SchedulerError::Repository)
    }
}
