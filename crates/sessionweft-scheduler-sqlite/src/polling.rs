use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sessionweft_core::SessionId;
use sessionweft_execution::AgentRecord;
use sessionweft_orchestration::{WorkflowExecution, WorkflowNodeStatus};
use sessionweft_scheduler::{
    ReadyWorkflowCandidate, RepositoryError, SchedulerPollingRepository, TaskClaim,
};
use sqlx::Row;
use uuid::Uuid;

use super::{SqliteSchedulerRepository, backend};

#[async_trait]
impl SchedulerPollingRepository for SqliteSchedulerRepository {
    async fn pending_handover_claim_ids(&self, limit: usize) -> Result<Vec<Uuid>, RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT released.claim_id, released.data_json AS claim_json,
                   workflow.data_json AS workflow_json
            FROM scheduler_claims AS released
            JOIN workflow_executions AS workflow ON workflow.id = released.workflow_id
            WHERE released.status = 'released' AND workflow.status = 'running'
              AND NOT EXISTS (
                  SELECT 1 FROM scheduler_claims AS active
                  WHERE active.workflow_id = released.workflow_id
                    AND active.node_id = released.node_id
                    AND active.status = 'active'
              )
            ORDER BY released.updated_at ASC, released.claim_id ASC
            LIMIT ?
            "#,
        )
        .bind(i64::try_from(limit).map_err(backend)?)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;

        let mut claim_ids = Vec::with_capacity(rows.len());
        for row in rows {
            let claim = serde_json::from_str::<TaskClaim>(row.get::<&str, _>("claim_json"))
                .map_err(backend)?;
            let workflow =
                serde_json::from_str::<WorkflowExecution>(row.get::<&str, _>("workflow_json"))
                    .map_err(backend)?;
            let is_ready = workflow
                .nodes
                .get(&claim.node_id)
                .is_some_and(|state| state.status == WorkflowNodeStatus::Ready);
            if is_ready {
                claim_ids.push(claim.id);
            }
        }
        Ok(claim_ids)
    }

    async fn ready_workflows(
        &self,
        limit: usize,
    ) -> Result<Vec<ReadyWorkflowCandidate>, RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT workflow.data_json
            FROM scheduler_plans AS plan
            JOIN workflow_executions AS workflow ON workflow.id = plan.workflow_id
            WHERE workflow.status = 'running'
            ORDER BY workflow.updated_at ASC, workflow.id ASC
            LIMIT ?
            "#,
        )
        .bind(i64::try_from(limit).map_err(backend)?)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;

        let mut candidates = Vec::with_capacity(rows.len());
        for row in rows {
            let workflow =
                serde_json::from_str::<WorkflowExecution>(row.get::<&str, _>("data_json"))
                    .map_err(backend)?;
            if !workflow.ready_nodes().is_empty() {
                candidates.push(ReadyWorkflowCandidate {
                    workflow_id: workflow.id,
                    session_id: workflow.session_id,
                });
            }
        }
        Ok(candidates)
    }

    async fn available_agent_ids(
        &self,
        session_id: SessionId,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<Uuid>, RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT data_json
            FROM agent_records
            WHERE session_id = ? AND status = 'running' AND current_task_id IS NULL
            ORDER BY updated_at ASC, id ASC
            LIMIT ?
            "#,
        )
        .bind(session_id.to_string())
        .bind(i64::try_from(limit).map_err(backend)?)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;

        let mut agent_ids = Vec::with_capacity(rows.len());
        for row in rows {
            let agent = serde_json::from_str::<AgentRecord>(row.get::<&str, _>("data_json"))
                .map_err(backend)?;
            if !agent.is_stale_at(now) {
                agent_ids.push(agent.id);
            }
        }
        Ok(agent_ids)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
    };

    use sessionweft_core::SessionId;
    use sessionweft_execution::{AgentManifest, AgentRole, AgentService, Capability};
    use sessionweft_execution_sqlite::SqliteAgentRepository;
    use sessionweft_orchestration::{
        OrchestrationService, WorkflowDefinition, WorkflowNodeDefinition, WorkflowNodeKind,
        WorkflowNodeStatus,
    };
    use sessionweft_orchestration_sqlite::SqliteOrchestrationRepository;
    use sessionweft_scheduler::{
        PollingConfig, SchedulerPlan, SchedulerPollingService, TaskRequirement,
    };

    use super::*;

    #[tokio::test]
    async fn polling_tick_claims_ready_work_for_available_agent() {
        let path = std::env::temp_dir().join(format!(
            "sessionweft-scheduler-polling-{}.db",
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
                    name: "polling".into(),
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
        let agent_service = AgentService::new(agent_repository);
        let agent = agent_service
            .register(
                session_id,
                AgentManifest {
                    name: "worker".into(),
                    role: AgentRole::Worker,
                    capabilities: BTreeSet::from([Capability::WorkspaceWrite]),
                    heartbeat_timeout_seconds: 30,
                },
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("agent");
        agent_service
            .mutate(agent.id, agent.version, |agent| {
                Ok(vec![agent.start(
                    agent.version,
                    Uuid::new_v4(),
                    Some("test"),
                )?])
            })
            .await
            .expect("start agent");
        sessionweft_scheduler::SchedulerService::new(Arc::clone(&scheduler_repository))
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

        let polling = SchedulerPollingService::new(
            Arc::clone(&scheduler_repository),
            PollingConfig { batch_limit: 100 },
        )
        .expect("polling service");
        let report = polling
            .tick(Utc::now(), Uuid::new_v4(), Some("scheduler"))
            .await
            .expect("polling tick");
        assert_eq!(report.ready_claims_created, 1);
        let workflow = SqliteSchedulerRepository::load_workflow(
            &mut scheduler_repository
                .pool
                .begin()
                .await
                .expect("transaction"),
            workflow.id,
        )
        .await
        .expect("load workflow");
        assert_eq!(workflow.nodes["worker"].status, WorkflowNodeStatus::Running);

        let idle = polling
            .tick(Utc::now(), Uuid::new_v4(), Some("scheduler"))
            .await
            .expect("idle tick");
        assert!(!idle.made_progress());
    }
}
