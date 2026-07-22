use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{ClaimState, RepositoryError, SchedulerError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoverRequest {
    pub previous_claim_id: Uuid,
    pub now: DateTime<Utc>,
    pub correlation_id: Uuid,
    pub actor_id: Option<String>,
}

#[async_trait]
pub trait SchedulerHandoverRepository: Send + Sync {
    async fn handover_released_claim(
        &self,
        request: &HandoverRequest,
    ) -> Result<Option<ClaimState>, RepositoryError>;
}

#[derive(Clone)]
pub struct SchedulerHandoverService<R>
where
    R: SchedulerHandoverRepository,
{
    repository: Arc<R>,
}

impl<R> SchedulerHandoverService<R>
where
    R: SchedulerHandoverRepository,
{
    #[must_use]
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    pub async fn handover_released_claim(
        &self,
        request: &HandoverRequest,
    ) -> Result<Option<ClaimState>, SchedulerError> {
        self.repository
            .handover_released_claim(request)
            .await
            .map_err(SchedulerError::Repository)
    }
}
