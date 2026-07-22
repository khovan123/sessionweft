use async_trait::async_trait;
use sessionweft_core::{EventEnvelope, SessionId};
use sessionweft_execution::{AgentRecord, AgentStatus};
use sessionweft_orchestration::WorkflowNodeStatus;
use sessionweft_scheduler::{
    ClaimState, HandoverRequest, RepositoryError, SchedulerHandoverRepository, TaskClaim,
    TaskClaimStatus,
};
use sqlx::{Row, Sqlite, Transaction};
use uuid::Uuid;

use super::{SqliteSchedulerRepository, backend, domain};

impl SqliteSchedulerRepository {
    async fn active_claim_for_node(
        transaction: &mut Transaction<'_, Sqlite>,
        workflow_id: Uuid,
        node_id: &str,
    ) -> Result<Option<TaskClaim>, RepositoryError> {
        let row = sqlx::query(
            "SELECT data_json FROM scheduler_claims WHERE workflow_id = ? AND node_id = ? AND status = 'active'",
        )
        .bind(workflow_id.to_string())
        .bind(node_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }

    async fn replacement_agent(
        transaction: &mut Transaction<'_, Sqlite>,
        session_id: SessionId,
        excluded_agent_id: Uuid,
        now: chrono::DateTime<chrono::Utc>,
        requirement: &sessionweft_scheduler::TaskRequirement,
    ) -> Result<Option<AgentRecord>, RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT data_json
            FROM agent_records
            WHERE session_id = ? AND status = 'running'
              AND current_task_id IS NULL AND id != ?
            ORDER BY updated_at ASC, id ASC
            "#,
        )
        .bind(session_id.to_string())
        .bind(excluded_agent_id.to_string())
        .fetch_all(&mut **transaction)
        .await
        .map_err(backend)?;

        for row in rows {
            let agent = serde_json::from_str::<AgentRecord>(row.get::<&str, _>("data_json"))
                .map_err(backend)?;
            if !agent.is_stale_at(now) && requirement.matches(&agent) {
                return Ok(Some(agent));
            }
        }
        Ok(None)
    }
}

#[async_trait]
impl SchedulerHandoverRepository for SqliteSchedulerRepository {
    async fn handover_released_claim(
        &self,
        request: &HandoverRequest,
    ) -> Result<Option<ClaimState>, RepositoryError> {
        let _guard = self.claim_guard.lock().await;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let previous = Self::load_claim(&mut transaction, request.previous_claim_id).await?;
        if previous.status != TaskClaimStatus::Released {
            return Err(RepositoryError::Conflict(
                "only released claims can be handed over".into(),
            ));
        }

        if let Some(active) = Self::active_claim_for_node(
            &mut transaction,
            previous.workflow_id,
            &previous.node_id,
        )
        .await?
        {
            return Self::current_state(&mut transaction, active).await.map(Some);
        }

        let plan = Self::load_plan(&mut transaction, previous.workflow_id).await?;
        let mut workflow = Self::load_workflow(&mut transaction, previous.workflow_id).await?;
        if workflow.session_id != previous.session_id || plan.session_id != previous.session_id {
            return Err(RepositoryError::SessionMismatch);
        }
        let node_status = workflow
            .nodes
            .get(&previous.node_id)
            .map(|state| state.status)
            .ok_or_else(|| RepositoryError::Conflict("released claim node is missing".into()))?;
        if node_status != WorkflowNodeStatus::Ready {
            return Err(RepositoryError::Conflict(
                "released claim node is not ready for retry".into(),
            ));
        }

        let requirement = plan.requirement_for(&previous.node_id);
        let Some(mut agent) = Self::replacement_agent(
            &mut transaction,
            previous.session_id,
            previous.agent_id,
            request.now,
            &requirement,
        )
        .await?
        else {
            transaction.rollback().await.map_err(backend)?;
            return Ok(None);
        };
        if agent.status != AgentStatus::Running || agent.current_task_id.is_some() {
            return Err(RepositoryError::AgentUnavailable);
        }

        let workflow_version = workflow.version;
        let agent_version = agent.version;
        let workflow_event = workflow
            .start_node(
                workflow_version,
                &previous.node_id,
                agent.id.to_string(),
                request.correlation_id,
                request.actor_id.as_deref(),
            )
            .map_err(domain)?;
        let attempt = workflow.nodes[&previous.node_id].attempts;
        let task_id = format!("{}:{}:{attempt}", workflow.id, previous.node_id);
        let agent_event = agent
            .assign_task(
                agent_version,
                task_id,
                request.correlation_id,
                request.actor_id.as_deref(),
            )
            .map_err(domain)?;
        let claim = TaskClaim::new(&workflow, previous.node_id.clone(), attempt, &agent);
        let handover_event = EventEnvelope::new(
            "scheduler.claim_handed_over",
            Some(claim.session_id),
            request.correlation_id,
            request.actor_id.as_deref(),
            serde_json::json!({
                "previous_claim_id": previous.id,
                "previous_agent_id": previous.agent_id,
                "claim_id": claim.id,
                "agent_id": claim.agent_id,
                "workflow_id": claim.workflow_id,
                "node_id": claim.node_id,
                "attempt": claim.attempt,
                "idempotency_key": claim.idempotency_key,
            }),
        );

        Self::save_workflow(&mut transaction, workflow_version, &workflow).await?;
        Self::save_agent(&mut transaction, agent_version, &agent).await?;
        Self::insert_claim(&mut transaction, &claim).await?;
        Self::insert_events(
            &mut transaction,
            &[workflow_event, agent_event, handover_event],
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(Some(ClaimState {
            claim,
            workflow,
            agent,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
    };

    use chrono::{Duration, Utc};
    use sessionweft_core::SessionId;
    use sessionweft_execution::{
        AgentManifest, AgentRepository, AgentRole, AgentService, Capability,
    };
    use sessionweft_execution_sqlite::SqliteAgentRepository;
    use sessionweft_orchestration::{
        OrchestrationService, WorkflowDefinition, WorkflowNodeDefinition, WorkflowNodeKind,
        WorkflowNodeStatus,
    };
    use sessionweft_orchestration_sqlite::SqliteOrchestrationRepository;
    use sessionweft_scheduler::{
        ClaimRequest, SchedulerHandoverService, SchedulerPlan, SchedulerRecoveryService,
        SchedulerService, TaskRequirement,
    };

    use super::*;

    #[tokio::test]
    async fn released_claim_moves_to_matching_replacement_agent() {
        let path = std::env::temp_dir().join(format!(
            "sessionweft-scheduler-handover-{}.db",
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
                    name: "handover".into(),
                    version: 1,
                    nodes: vec![WorkflowNodeDefinition {
                        id: "worker".into(),
                        kind: WorkflowNodeKind::Task,
                        dependencies: Vec::new(),
                        max_attempts: 3,
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
        let manifest = |name: &str| AgentManifest {
            name: name.into(),
            role: AgentRole::Worker,
            capabilities: BTreeSet::from([Capability::WorkspaceWrite]),
            heartbeat_timeout_seconds: 5,
        };
        let first = agent_service
            .register(
                session_id,
                manifest("first"),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("first agent");
        let first = agent_service
            .mutate(first.id, first.version, |agent| {
                Ok(vec![agent.start(
                    agent.version,
                    Uuid::new_v4(),
                    Some("test"),
                )?])
            })
            .await
            .expect("start first");
        let replacement = agent_service
            .register(
                session_id,
                manifest("replacement"),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("replacement agent");
        let replacement = agent_service
            .mutate(replacement.id, replacement.version, |agent| {
                Ok(vec![agent.start(
                    agent.version,
                    Uuid::new_v4(),
                    Some("test"),
                )?])
            })
            .await
            .expect("start replacement");

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
        let first_claim = scheduler
            .claim_next(&ClaimRequest {
                workflow_id: workflow.id,
                agent_id: first.id,
                correlation_id: Uuid::new_v4(),
                actor_id: Some("scheduler".into()),
            })
            .await
            .expect("first claim")
            .expect("ready claim");

        let mut stale = agent_repository
            .get(first.id)
            .await
            .expect("load first")
            .expect("first exists");
        stale.heartbeat_at = Utc::now() - Duration::seconds(60);
        stale.updated_at = stale.heartbeat_at;
        agent_repository
            .save(stale.version, &stale, &[])
            .await
            .expect("persist stale agent");
        let released = SchedulerRecoveryService::new(Arc::clone(&scheduler_repository))
            .recover_stale_claims(Utc::now(), 100, Uuid::new_v4(), Some("scheduler"))
            .await
            .expect("recover")
            .remove(0);
        assert_eq!(released.claim.status, TaskClaimStatus::Released);

        let handover = SchedulerHandoverService::new(Arc::clone(&scheduler_repository));
        let request = HandoverRequest {
            previous_claim_id: released.claim.id,
            now: Utc::now(),
            correlation_id: Uuid::new_v4(),
            actor_id: Some("scheduler".into()),
        };
        let assigned = handover
            .handover_released_claim(&request)
            .await
            .expect("handover")
            .expect("replacement available");
        assert_eq!(assigned.agent.id, replacement.id);
        assert_eq!(assigned.claim.attempt, 2);
        assert_ne!(assigned.claim.idempotency_key, first_claim.claim.idempotency_key);
        assert_eq!(
            assigned.workflow.nodes["worker"].status,
            WorkflowNodeStatus::Running
        );
        assert_eq!(
            assigned.agent.current_task_id,
            Some(assigned.claim.task_id.clone())
        );

        let replay = handover
            .handover_released_claim(&request)
            .await
            .expect("idempotent handover")
            .expect("active handover claim");
        assert_eq!(replay.claim.id, assigned.claim.id);
    }
}
