use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sessionweft_core::SessionId;
use uuid::Uuid;

use super::{
    ClaimRequest, HandoverRequest, RepositoryError, SchedulerError,
    SchedulerPrerequisiteRepository, SchedulerRecoveryRepository,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollingConfig {
    pub batch_limit: usize,
}

impl Default for PollingConfig {
    fn default() -> Self {
        Self { batch_limit: 100 }
    }
}

impl PollingConfig {
    pub fn validate(self) -> Result<Self, SchedulerError> {
        if self.batch_limit == 0 || self.batch_limit > 1_000 {
            return Err(SchedulerError::Validation(
                "scheduler polling batch limit must be between 1 and 1000".into(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadyWorkflowCandidate {
    pub workflow_id: Uuid,
    pub session_id: SessionId,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PollingTickReport {
    pub stale_claims_recovered: usize,
    pub claims_handed_over: usize,
    pub ready_claims_created: usize,
}

impl PollingTickReport {
    #[must_use]
    pub const fn made_progress(self) -> bool {
        self.stale_claims_recovered > 0
            || self.claims_handed_over > 0
            || self.ready_claims_created > 0
    }
}

#[async_trait]
pub trait SchedulerPollingRepository:
    SchedulerRecoveryRepository + SchedulerPrerequisiteRepository
{
    async fn pending_handover_claim_ids(&self, limit: usize) -> Result<Vec<Uuid>, RepositoryError>;

    async fn ready_workflows(
        &self,
        limit: usize,
    ) -> Result<Vec<ReadyWorkflowCandidate>, RepositoryError>;

    async fn available_agent_ids(
        &self,
        session_id: SessionId,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<Uuid>, RepositoryError>;
}

#[derive(Clone)]
pub struct SchedulerPollingService<R>
where
    R: SchedulerPollingRepository,
{
    repository: Arc<R>,
    config: PollingConfig,
}

impl<R> SchedulerPollingService<R>
where
    R: SchedulerPollingRepository,
{
    pub fn new(repository: Arc<R>, config: PollingConfig) -> Result<Self, SchedulerError> {
        Ok(Self {
            repository,
            config: config.validate()?,
        })
    }

    pub async fn tick(
        &self,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<PollingTickReport, SchedulerError> {
        let mut report = PollingTickReport::default();
        let recovered = self
            .repository
            .recover_stale_claims(now, self.config.batch_limit, correlation_id, actor_id)
            .await
            .map_err(SchedulerError::Repository)?;
        report.stale_claims_recovered = recovered.len();

        let released_claims = self
            .repository
            .pending_handover_claim_ids(self.config.batch_limit)
            .await
            .map_err(SchedulerError::Repository)?;
        for previous_claim_id in released_claims {
            let handover = self
                .repository
                .handover_released_claim_guarded(&HandoverRequest {
                    previous_claim_id,
                    now,
                    correlation_id,
                    actor_id: actor_id.map(str::to_owned),
                })
                .await
                .map_err(SchedulerError::Repository)?;
            if handover.is_some() {
                report.claims_handed_over = report.claims_handed_over.saturating_add(1);
            }
        }

        let workflows = self
            .repository
            .ready_workflows(self.config.batch_limit)
            .await
            .map_err(SchedulerError::Repository)?;
        for workflow in workflows {
            let agent_ids = self
                .repository
                .available_agent_ids(workflow.session_id, now, self.config.batch_limit)
                .await
                .map_err(SchedulerError::Repository)?;
            for agent_id in agent_ids {
                let claimed = self
                    .repository
                    .claim_next_guarded(
                        &ClaimRequest {
                            workflow_id: workflow.workflow_id,
                            agent_id,
                            correlation_id,
                            actor_id: actor_id.map(str::to_owned),
                        },
                        now,
                    )
                    .await
                    .map_err(SchedulerError::Repository)?;
                if claimed.is_some() {
                    report.ready_claims_created = report.ready_claims_created.saturating_add(1);
                }
            }
        }
        Ok(report)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExponentialBackoff {
    minimum_millis: u64,
    maximum_millis: u64,
    current_millis: u64,
}

impl ExponentialBackoff {
    pub fn new(minimum_millis: u64, maximum_millis: u64) -> Result<Self, SchedulerError> {
        if minimum_millis == 0 || maximum_millis < minimum_millis {
            return Err(SchedulerError::Validation(
                "scheduler backoff requires 0 < minimum <= maximum".into(),
            ));
        }
        Ok(Self {
            minimum_millis,
            maximum_millis,
            current_millis: minimum_millis,
        })
    }

    #[must_use]
    pub const fn current_millis(self) -> u64 {
        self.current_millis
    }

    pub fn observe(&mut self, made_progress: bool) -> u64 {
        if made_progress {
            self.current_millis = self.minimum_millis;
        } else {
            self.current_millis = self
                .current_millis
                .saturating_mul(2)
                .min(self.maximum_millis);
        }
        self.current_millis
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_resets_on_progress_and_caps_when_idle() {
        let mut backoff = ExponentialBackoff::new(100, 800).expect("backoff");
        assert_eq!(backoff.current_millis(), 100);
        assert_eq!(backoff.observe(false), 200);
        assert_eq!(backoff.observe(false), 400);
        assert_eq!(backoff.observe(false), 800);
        assert_eq!(backoff.observe(false), 800);
        assert_eq!(backoff.observe(true), 100);
    }
}
