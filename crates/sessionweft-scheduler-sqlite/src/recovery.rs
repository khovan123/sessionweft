use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::json;
use sessionweft_core::EventEnvelope;
use sessionweft_execution::AgentRecord;
use sessionweft_orchestration::WorkflowNodeStatus;
use sessionweft_scheduler::{
    ClaimState, RepositoryError, SchedulerRecoveryRepository, TaskClaimStatus,
};
use sqlx::Row;
use uuid::Uuid;

use super::{SqliteSchedulerRepository, backend, domain};

impl SqliteSchedulerRepository {
    async fn stale_claim_ids(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<Uuid>, RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT claim.claim_id, agent.data_json
            FROM scheduler_claims AS claim
            JOIN agent_records AS agent ON agent.id = claim.agent_id
            WHERE claim.status = 'active' AND agent.status = 'running'
            ORDER BY agent.heartbeat_at ASC
            LIMIT ?
            "#,
        )
        .bind(i64::try_from(limit).map_err(backend)?)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;

        rows.into_iter()
            .filter_map(|row| {
                let agent = serde_json::from_str::<AgentRecord>(row.get::<&str, _>("data_json"));
                match agent {
                    Ok(agent) if agent.is_stale_at(now) => Some(
                        Uuid::parse_str(row.get::<&str, _>("claim_id")).map_err(backend),
                    ),
                    Ok(_) => None,
                    Err(error) => Some(Err(backend(error))),
                }
            })
            .collect()
    }

    async fn recover_stale_claim(
        &self,
        claim_id: Uuid,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Option<ClaimState>, RepositoryError> {
        let _guard = self.claim_guard.lock().await;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut claim = Self::load_claim(&mut transaction, claim_id).await?;
        if claim.status != TaskClaimStatus::Active {
            return Ok(None);
        }

        let mut agent = Self::load_agent(&mut transaction, claim.agent_id).await?;
        if !agent.is_stale_at(now) {
            return Ok(None);
        }
        if agent.current_task_id.as_deref() != Some(claim.task_id.as_str()) {
            return Err(RepositoryError::Conflict(
                "stale agent task ownership does not match scheduler claim".into(),
            ));
        }

        let mut workflow = Self::load_workflow(&mut transaction, claim.workflow_id).await?;
        let workflow_version = workflow.version;
        let agent_version = agent.version;
        let reason = "agent heartbeat expired while owning scheduler claim";
        let mut events = workflow
            .fail_node(
                workflow_version,
                &claim.node_id,
                reason,
                correlation_id,
                actor_id,
            )
            .map_err(domain)?;
        events.push(
            agent
                .fail(agent_version, reason, correlation_id, actor_id)
                .map_err(domain)?,
        );

        claim.status = TaskClaimStatus::Released;
        claim.workflow_version = workflow.version;
        claim.agent_version = agent.version;
        claim.updated_at = now;
        let node_status = workflow
            .nodes
            .get(&claim.node_id)
            .map(|state| state.status)
            .ok_or_else(|| RepositoryError::Conflict("claimed workflow node is missing".into()))?;
        events.push(EventEnvelope::new(
            "scheduler.claim_released",
            Some(claim.session_id),
            correlation_id,
            actor_id,
            json!({
                "claim_id": claim.id,
                "workflow_id": claim.workflow_id,
                "node_id": claim.node_id,
                "stale_agent_id": claim.agent_id,
                "idempotency_key": claim.idempotency_key,
                "reason": "stale_agent",
                "node_status": node_status,
            }),
        ));
        if node_status == WorkflowNodeStatus::Ready {
            events.push(EventEnvelope::new(
                "scheduler.handover_required",
                Some(claim.session_id),
                correlation_id,
                actor_id,
                json!({
                    "workflow_id": claim.workflow_id,
                    "node_id": claim.node_id,
                    "previous_claim_id": claim.id,
                    "previous_agent_id": claim.agent_id,
                    "next_attempt": workflow.nodes[&claim.node_id].attempts.saturating_add(1),
                }),
            ));
        }

        Self::save_workflow(&mut transaction, workflow_version, &workflow).await?;
        Self::save_agent(&mut transaction, agent_version, &agent).await?;
        Self::save_claim(&mut transaction, &claim).await?;
        Self::insert_events(&mut transaction, &events).await?;
        transaction.commit().await.map_err(backend)?;
        Ok(Some(ClaimState {
            claim,
            workflow,
            agent,
        }))
    }
}

#[async_trait]
impl SchedulerRecoveryRepository for SqliteSchedulerRepository {
    async fn recover_stale_claims(
        &self,
        now: DateTime<Utc>,
        limit: usize,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Vec<ClaimState>, RepositoryError> {
        let claim_ids = self.stale_claim_ids(now, limit).await?;
        let mut recovered = Vec::with_capacity(claim_ids.len());
        for claim_id in claim_ids {
            if let Some(state) = self
                .recover_stale_claim(claim_id, now, correlation_id, actor_id)
                .await?
            {
                recovered.push(state);
            }
        }
        Ok(recovered)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
    };

    use chrono::Duration;
    use sessionweft_core::SessionId;
    use sessionweft_execution::{
        AgentManifest, AgentRepository, AgentRole, AgentService, AgentStatus, Capability,
    };
    use sessionweft_execution_sqlite::SqliteAgentRepository;
    use sessionweft_orchestration::{
        OrchestrationService, WorkflowDefinition, WorkflowNodeDefinition, WorkflowNodeKind,
        WorkflowNodeStatus,
    };
    use sessionweft_orchestration_sqlite::SqliteOrchestrationRepository;
    use sessionweft_scheduler::{
        ClaimRequest, SchedulerPlan, SchedulerRecoveryService, SchedulerService, TaskRequirement,
    };

    use super::*;

    #[tokio::test]
    async fn stale_agent_releases_claim_and_returns_node_to_ready() {
        let path = std::env::temp_dir().join(format!(
            "sessionweft-scheduler-recovery-{}.db",
            Uuid::new_v4()
        ));
        let database_url = format!("sqlite://{}", path.display());
        let workflow_repository = Arc::new(
            SqliteOrchestrationRepository::connect(&database_url)
                .await
                .expect("workflow repository"),
        );
        let agent_repository = Arc::new(
            SqliteAgentRepository::connect(&database_url)
                .await
                .expect("agent repository"),
        );
        let scheduler_repository = Arc::new(
            SqliteSchedulerRepository::connect(&database_url)
                .await
                .expect("scheduler repository"),
        );
        let session_id = SessionId::new();
        let workflow = OrchestrationService::new(workflow_repository)
            .create_workflow(
                session_id,
                WorkflowDefinition {
                    name: "stale recovery".into(),
                    version: 1,
                    nodes: vec![WorkflowNodeDefinition {
                        id: "worker".into(),
                        kind: WorkflowNodeKind::Task,
                        dependencies: Vec::new(),
                        max_attempts: 2,
                        continue_on_failure: false,
                        fallback: None,
                    }],
                },
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("workflow");
        let agent_service = AgentService::new(Arc::clone(&agent_repository));
        let agent = agent_service
            .register(
                session_id,
                AgentManifest {
                    name: "worker".into(),
                    role: AgentRole::Worker,
                    capabilities: BTreeSet::from([Capability::WorkspaceWrite]),
                    heartbeat_timeout_seconds: 5,
                },
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("agent");
        let agent = agent_service
            .mutate(agent.id, agent.version, |agent| {
                Ok(vec![agent.start(
                    agent.version,
                    Uuid::new_v4(),
                    Some("test"),
                )?])
            })
            .await
            .expect("start agent");
        let scheduler = SchedulerService::new(Arc::clone(&scheduler_repository));
        scheduler
            .register_plan(
                &SchedulerPlan::new(
                    &workflow,
                    BTreeMap::from([(
                        "worker".into(),
                        TaskRequirement {
                            role: Some(AgentRole::Worker),
                            capabilities: BTreeSet::from([Capability::WorkspaceWrite]),
                        },
                    )]),
                )
                .expect("plan"),
            )
            .await
            .expect("register plan");
        let claimed = scheduler
            .claim_next(&ClaimRequest {
                workflow_id: workflow.id,
                agent_id: agent.id,
                correlation_id: Uuid::new_v4(),
                actor_id: Some("scheduler".into()),
            })
            .await
            .expect("claim")
            .expect("ready claim");

        let mut stale_agent = agent_repository
            .get(agent.id)
            .await
            .expect("load agent")
            .expect("agent exists");
        stale_agent.heartbeat_at = Utc::now() - Duration::seconds(60);
        stale_agent.updated_at = stale_agent.heartbeat_at;
        agent_repository
            .save(stale_agent.version, &stale_agent, &[])
            .await
            .expect("persist stale heartbeat");

        let recovery = SchedulerRecoveryService::new(Arc::clone(&scheduler_repository));
        let recovered = recovery
            .recover_stale_claims(Utc::now(), 100, Uuid::new_v4(), Some("scheduler"))
            .await
            .expect("recover stale claims");
        assert_eq!(recovered.len(), 1);
        let recovered = &recovered[0];
        assert_eq!(recovered.claim.id, claimed.claim.id);
        assert_eq!(recovered.claim.status, TaskClaimStatus::Released);
        assert_eq!(recovered.agent.status, AgentStatus::Failed);
        assert!(recovered.agent.current_task_id.is_none());
        assert_eq!(
            recovered.workflow.nodes["worker"].status,
            WorkflowNodeStatus::Ready
        );

        assert!(recovery
            .recover_stale_claims(Utc::now(), 100, Uuid::new_v4(), Some("scheduler"))
            .await
            .expect("idempotent recovery")
            .is_empty());
    }
}
