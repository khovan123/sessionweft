use std::{str::FromStr, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::json;
use sessionweft_core::EventEnvelope;
use sessionweft_execution::{AgentRecord, AgentStatus};
use sessionweft_orchestration::WorkflowExecution;
use sessionweft_scheduler::{
    ClaimRequest, ClaimState, RepositoryError, SchedulerPlan, SchedulerRepository, TaskClaim,
    TaskClaimStatus,
};
use sqlx::{
    Row, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub struct SqliteSchedulerRepository {
    pool: SqlitePool,
    claim_guard: Arc<Mutex<()>>,
}

impl SqliteSchedulerRepository {
    pub async fn connect(database_url: &str) -> Result<Self, RepositoryError> {
        let is_memory = database_url.contains(":memory:");
        let mut options = SqliteConnectOptions::from_str(database_url)
            .map_err(backend)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        if !is_memory {
            options = options.journal_mode(SqliteJournalMode::Wal);
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(if is_memory { 1 } else { 5 })
            .connect_with(options)
            .await
            .map_err(backend)?;
        let repository = Self {
            pool,
            claim_guard: Arc::new(Mutex::new(())),
        };
        repository.migrate().await?;
        Ok(repository)
    }

    async fn migrate(&self) -> Result<(), RepositoryError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS scheduler_plans (
                workflow_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                data_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS scheduler_claims (
                claim_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                workflow_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                attempt INTEGER NOT NULL,
                agent_id TEXT NOT NULL,
                idempotency_key TEXT NOT NULL UNIQUE,
                status TEXT NOT NULL,
                data_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query(
            r#"
            CREATE UNIQUE INDEX IF NOT EXISTS idx_scheduler_active_claim
            ON scheduler_claims (workflow_id, node_id)
            WHERE status = 'active'
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_scheduler_agent_claims ON scheduler_claims (agent_id, status, updated_at)",
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS outbox (
                event_id TEXT PRIMARY KEY,
                session_id TEXT,
                event_type TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                payload_json TEXT NOT NULL,
                correlation_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                published_at TEXT,
                publish_attempts INTEGER NOT NULL DEFAULT 0,
                last_error TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn load_plan(
        transaction: &mut Transaction<'_, Sqlite>,
        workflow_id: Uuid,
    ) -> Result<SchedulerPlan, RepositoryError> {
        let row = sqlx::query("SELECT data_json FROM scheduler_plans WHERE workflow_id = ?")
            .bind(workflow_id.to_string())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(backend)?
            .ok_or(RepositoryError::PlanNotFound(workflow_id))?;
        serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend)
    }

    async fn load_workflow(
        transaction: &mut Transaction<'_, Sqlite>,
        workflow_id: Uuid,
    ) -> Result<WorkflowExecution, RepositoryError> {
        let row = sqlx::query("SELECT data_json FROM workflow_executions WHERE id = ?")
            .bind(workflow_id.to_string())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(backend)?
            .ok_or(RepositoryError::WorkflowNotFound(workflow_id))?;
        serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend)
    }

    async fn load_agent(
        transaction: &mut Transaction<'_, Sqlite>,
        agent_id: Uuid,
    ) -> Result<AgentRecord, RepositoryError> {
        let row = sqlx::query("SELECT data_json FROM agent_records WHERE id = ?")
            .bind(agent_id.to_string())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(backend)?
            .ok_or(RepositoryError::AgentNotFound(agent_id))?;
        serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend)
    }

    async fn load_claim(
        transaction: &mut Transaction<'_, Sqlite>,
        claim_id: Uuid,
    ) -> Result<TaskClaim, RepositoryError> {
        let row = sqlx::query("SELECT data_json FROM scheduler_claims WHERE claim_id = ?")
            .bind(claim_id.to_string())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(backend)?
            .ok_or(RepositoryError::ClaimNotFound(claim_id))?;
        serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend)
    }

    async fn active_claim_exists(
        transaction: &mut Transaction<'_, Sqlite>,
        workflow_id: Uuid,
        node_id: &str,
    ) -> Result<bool, RepositoryError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM scheduler_claims WHERE workflow_id = ? AND node_id = ? AND status = 'active'",
        )
        .bind(workflow_id.to_string())
        .bind(node_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(backend)?;
        Ok(count > 0)
    }

    async fn save_workflow(
        transaction: &mut Transaction<'_, Sqlite>,
        expected_version: u64,
        workflow: &WorkflowExecution,
    ) -> Result<(), RepositoryError> {
        let result = sqlx::query(
            r#"
            UPDATE workflow_executions
            SET version = ?, status = ?, data_json = ?, updated_at = ?
            WHERE id = ? AND version = ?
            "#,
        )
        .bind(to_i64(workflow.version)?)
        .bind(format!("{:?}", workflow.status).to_lowercase())
        .bind(serde_json::to_string(workflow).map_err(backend)?)
        .bind(workflow.updated_at.to_rfc3339())
        .bind(workflow.id.to_string())
        .bind(to_i64(expected_version)?)
        .execute(&mut **transaction)
        .await
        .map_err(backend)?;
        if result.rows_affected() != 1 {
            return Err(RepositoryError::Conflict(format!(
                "workflow {} changed while claiming task",
                workflow.id
            )));
        }
        Ok(())
    }

    async fn save_agent(
        transaction: &mut Transaction<'_, Sqlite>,
        expected_version: u64,
        agent: &AgentRecord,
    ) -> Result<(), RepositoryError> {
        let result = sqlx::query(
            r#"
            UPDATE agent_records
            SET version = ?, status = ?, heartbeat_at = ?, current_task_id = ?,
                updated_at = ?, data_json = ?
            WHERE id = ? AND version = ?
            "#,
        )
        .bind(to_i64(agent.version)?)
        .bind(format!("{:?}", agent.status).to_lowercase())
        .bind(agent.heartbeat_at.to_rfc3339())
        .bind(&agent.current_task_id)
        .bind(agent.updated_at.to_rfc3339())
        .bind(serde_json::to_string(agent).map_err(backend)?)
        .bind(agent.id.to_string())
        .bind(to_i64(expected_version)?)
        .execute(&mut **transaction)
        .await
        .map_err(backend)?;
        if result.rows_affected() != 1 {
            return Err(RepositoryError::Conflict(format!(
                "agent {} changed while claiming task",
                agent.id
            )));
        }
        Ok(())
    }

    async fn insert_claim(
        transaction: &mut Transaction<'_, Sqlite>,
        claim: &TaskClaim,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            r#"
            INSERT INTO scheduler_claims (
                claim_id, session_id, workflow_id, node_id, attempt, agent_id,
                idempotency_key, status, data_json, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(claim.id.to_string())
        .bind(claim.session_id.to_string())
        .bind(claim.workflow_id.to_string())
        .bind(&claim.node_id)
        .bind(i64::from(claim.attempt))
        .bind(claim.agent_id.to_string())
        .bind(&claim.idempotency_key)
        .bind(status_name(claim.status))
        .bind(serde_json::to_string(claim).map_err(backend)?)
        .bind(claim.created_at.to_rfc3339())
        .bind(claim.updated_at.to_rfc3339())
        .execute(&mut **transaction)
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn save_claim(
        transaction: &mut Transaction<'_, Sqlite>,
        claim: &TaskClaim,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE scheduler_claims SET status = ?, data_json = ?, updated_at = ? WHERE claim_id = ?",
        )
        .bind(status_name(claim.status))
        .bind(serde_json::to_string(claim).map_err(backend)?)
        .bind(claim.updated_at.to_rfc3339())
        .bind(claim.id.to_string())
        .execute(&mut **transaction)
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn insert_events(
        transaction: &mut Transaction<'_, Sqlite>,
        events: &[EventEnvelope],
    ) -> Result<(), RepositoryError> {
        for event in events {
            sqlx::query(
                r#"
                INSERT INTO outbox (
                    event_id, session_id, event_type, schema_version,
                    payload_json, correlation_id, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(event.event_id.to_string())
            .bind(event.session_id.map(|value| value.to_string()))
            .bind(&event.event_type)
            .bind(i64::from(event.schema_version))
            .bind(serde_json::to_string(event).map_err(backend)?)
            .bind(event.correlation_id.to_string())
            .bind(event.occurred_at.to_rfc3339())
            .execute(&mut **transaction)
            .await
            .map_err(backend)?;
        }
        Ok(())
    }

    async fn current_state(
        transaction: &mut Transaction<'_, Sqlite>,
        claim: TaskClaim,
    ) -> Result<ClaimState, RepositoryError> {
        let workflow = Self::load_workflow(transaction, claim.workflow_id).await?;
        let agent = Self::load_agent(transaction, claim.agent_id).await?;
        Ok(ClaimState {
            claim,
            workflow,
            agent,
        })
    }

    async fn finish_claim(
        &self,
        claim_id: Uuid,
        sanitized_error: Option<&str>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<ClaimState, RepositoryError> {
        let _guard = self.claim_guard.lock().await;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut claim = Self::load_claim(&mut transaction, claim_id).await?;
        match (claim.status, sanitized_error) {
            (TaskClaimStatus::Completed, None) | (TaskClaimStatus::Failed, Some(_)) => {
                return Self::current_state(&mut transaction, claim).await;
            }
            (TaskClaimStatus::Active, _) => {}
            _ => return Err(RepositoryError::ClaimNotActive(claim_id)),
        }

        let mut workflow = Self::load_workflow(&mut transaction, claim.workflow_id).await?;
        let mut agent = Self::load_agent(&mut transaction, claim.agent_id).await?;
        if agent.current_task_id.as_deref() != Some(claim.task_id.as_str()) {
            return Err(RepositoryError::Conflict(
                "agent task ownership does not match scheduler claim".into(),
            ));
        }
        let workflow_version = workflow.version;
        let agent_version = agent.version;
        let mut events = match sanitized_error {
            Some(error) => workflow
                .fail_node(
                    workflow_version,
                    &claim.node_id,
                    error,
                    correlation_id,
                    actor_id,
                )
                .map_err(domain)?,
            None => workflow
                .complete_node(workflow_version, &claim.node_id, correlation_id, actor_id)
                .map_err(domain)?,
        };
        events.push(
            agent
                .release_task(agent_version, correlation_id, actor_id)
                .map_err(domain)?,
        );
        if sanitized_error.is_some() {
            claim.fail(workflow.version, agent.version);
        } else {
            claim.complete(workflow.version, agent.version);
        }
        events.push(EventEnvelope::new(
            if sanitized_error.is_some() {
                "scheduler.claim_failed"
            } else {
                "scheduler.claim_completed"
            },
            Some(claim.session_id),
            correlation_id,
            actor_id,
            json!({
                "claim_id": claim.id,
                "workflow_id": claim.workflow_id,
                "node_id": claim.node_id,
                "agent_id": claim.agent_id,
                "idempotency_key": claim.idempotency_key,
            }),
        ));

        Self::save_workflow(&mut transaction, workflow_version, &workflow).await?;
        Self::save_agent(&mut transaction, agent_version, &agent).await?;
        Self::save_claim(&mut transaction, &claim).await?;
        Self::insert_events(&mut transaction, &events).await?;
        transaction.commit().await.map_err(backend)?;
        Ok(ClaimState {
            claim,
            workflow,
            agent,
        })
    }
}

#[async_trait]
impl SchedulerRepository for SqliteSchedulerRepository {
    async fn register_plan(&self, plan: &SchedulerPlan) -> Result<SchedulerPlan, RepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let workflow = Self::load_workflow(&mut transaction, plan.workflow_id).await?;
        if workflow.session_id != plan.session_id {
            return Err(RepositoryError::SessionMismatch);
        }
        sqlx::query(
            r#"
            INSERT INTO scheduler_plans (
                workflow_id, session_id, data_json, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(workflow_id) DO UPDATE SET
                session_id = excluded.session_id,
                data_json = excluded.data_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(plan.workflow_id.to_string())
        .bind(plan.session_id.to_string())
        .bind(serde_json::to_string(plan).map_err(backend)?)
        .bind(plan.created_at.to_rfc3339())
        .bind(plan.updated_at.to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        Ok(plan.clone())
    }

    async fn get_plan(&self, workflow_id: Uuid) -> Result<Option<SchedulerPlan>, RepositoryError> {
        let row = sqlx::query("SELECT data_json FROM scheduler_plans WHERE workflow_id = ?")
            .bind(workflow_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }

    async fn claim_next(
        &self,
        request: &ClaimRequest,
    ) -> Result<Option<ClaimState>, RepositoryError> {
        let _guard = self.claim_guard.lock().await;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let plan = Self::load_plan(&mut transaction, request.workflow_id).await?;
        let mut workflow = Self::load_workflow(&mut transaction, request.workflow_id).await?;
        let mut agent = Self::load_agent(&mut transaction, request.agent_id).await?;
        if workflow.session_id != agent.session_id || plan.session_id != workflow.session_id {
            return Err(RepositoryError::SessionMismatch);
        }
        if agent.status != AgentStatus::Running || agent.current_task_id.is_some() {
            return Err(RepositoryError::AgentUnavailable);
        }

        let mut selected = None;
        for node_id in workflow.ready_nodes() {
            if Self::active_claim_exists(&mut transaction, workflow.id, &node_id).await? {
                continue;
            }
            if plan.requirement_for(&node_id).matches(&agent) {
                selected = Some(node_id);
                break;
            }
        }
        let Some(node_id) = selected else {
            transaction.rollback().await.map_err(backend)?;
            return Ok(None);
        };

        let workflow_version = workflow.version;
        let agent_version = agent.version;
        let workflow_event = workflow
            .start_node(
                workflow_version,
                &node_id,
                agent.id.to_string(),
                request.correlation_id,
                request.actor_id.as_deref(),
            )
            .map_err(domain)?;
        let attempt = workflow.nodes[&node_id].attempts;
        let task_id = format!("{}:{node_id}:{attempt}", workflow.id);
        let agent_event = agent
            .assign_task(
                agent_version,
                task_id,
                request.correlation_id,
                request.actor_id.as_deref(),
            )
            .map_err(domain)?;
        let claim = TaskClaim::new(&workflow, node_id, attempt, &agent);
        let scheduler_event = EventEnvelope::new(
            "scheduler.task_claimed",
            Some(claim.session_id),
            request.correlation_id,
            request.actor_id.as_deref(),
            json!({
                "claim_id": claim.id,
                "workflow_id": claim.workflow_id,
                "node_id": claim.node_id,
                "attempt": claim.attempt,
                "agent_id": claim.agent_id,
                "task_id": claim.task_id,
                "idempotency_key": claim.idempotency_key,
            }),
        );

        Self::save_workflow(&mut transaction, workflow_version, &workflow).await?;
        Self::save_agent(&mut transaction, agent_version, &agent).await?;
        Self::insert_claim(&mut transaction, &claim).await?;
        Self::insert_events(
            &mut transaction,
            &[workflow_event, agent_event, scheduler_event],
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(Some(ClaimState {
            claim,
            workflow,
            agent,
        }))
    }

    async fn complete_claim(
        &self,
        claim_id: Uuid,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<ClaimState, RepositoryError> {
        self.finish_claim(claim_id, None, correlation_id, actor_id)
            .await
    }

    async fn fail_claim(
        &self,
        claim_id: Uuid,
        sanitized_error: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<ClaimState, RepositoryError> {
        self.finish_claim(claim_id, Some(sanitized_error), correlation_id, actor_id)
            .await
    }

    async fn get_claim(&self, claim_id: Uuid) -> Result<Option<TaskClaim>, RepositoryError> {
        let row = sqlx::query("SELECT data_json FROM scheduler_claims WHERE claim_id = ?")
            .bind(claim_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }
}

const fn status_name(status: TaskClaimStatus) -> &'static str {
    match status {
        TaskClaimStatus::Active => "active",
        TaskClaimStatus::Completed => "completed",
        TaskClaimStatus::Failed => "failed",
        TaskClaimStatus::Released => "released",
    }
}

fn backend(error: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::Backend(error.to_string())
}

fn domain(error: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::Conflict(error.to_string())
}

fn to_i64(value: u64) -> Result<i64, RepositoryError> {
    i64::try_from(value).map_err(backend)
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
        WorkflowNodeStatus, WorkflowStatus,
    };
    use sessionweft_orchestration_sqlite::SqliteOrchestrationRepository;
    use sessionweft_scheduler::{SchedulerService, TaskRequirement};

    use super::*;

    async fn setup() -> (
        SchedulerService<SqliteSchedulerRepository>,
        WorkflowExecution,
        AgentRecord,
        String,
    ) {
        let path =
            std::env::temp_dir().join(format!("sessionweft-scheduler-{}.db", Uuid::new_v4()));
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
                    name: "claim".into(),
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
        let scheduler = SchedulerService::new(scheduler_repository);
        let plan = SchedulerPlan::new(
            &workflow,
            BTreeMap::from([(
                "worker".into(),
                TaskRequirement {
                    role: Some(AgentRole::Worker),
                    capabilities: BTreeSet::from([Capability::WorkspaceWrite]),
                },
            )]),
        )
        .expect("plan");
        scheduler.register_plan(&plan).await.expect("register plan");
        (scheduler, workflow, agent, path.display().to_string())
    }

    #[tokio::test]
    async fn claim_and_completion_update_workflow_agent_and_outbox_atomically() {
        let (scheduler, workflow, agent, _path) = setup().await;
        let request = ClaimRequest {
            workflow_id: workflow.id,
            agent_id: agent.id,
            correlation_id: Uuid::new_v4(),
            actor_id: Some("scheduler".into()),
        };
        let claimed = scheduler
            .claim_next(&request)
            .await
            .expect("claim")
            .expect("ready claim");
        assert_eq!(claimed.claim.status, TaskClaimStatus::Active);
        assert_eq!(
            claimed.agent.current_task_id,
            Some(claimed.claim.task_id.clone())
        );
        assert_eq!(
            claimed.workflow.nodes["worker"].status,
            WorkflowNodeStatus::Running
        );
        assert!(scheduler.claim_next(&request).await.is_err());

        let completed = scheduler
            .complete_claim(claimed.claim.id, Uuid::new_v4(), Some("scheduler"))
            .await
            .expect("complete claim");
        assert_eq!(completed.claim.status, TaskClaimStatus::Completed);
        assert_eq!(completed.workflow.status, WorkflowStatus::Succeeded);
        assert!(completed.agent.current_task_id.is_none());

        let replay = scheduler
            .complete_claim(completed.claim.id, Uuid::new_v4(), Some("scheduler"))
            .await
            .expect("idempotent completion");
        assert_eq!(replay.claim.status, TaskClaimStatus::Completed);
    }

    #[tokio::test]
    async fn unmatched_capability_leaves_ready_node_unclaimed() {
        let (scheduler, workflow, agent, _path) = setup().await;
        let incompatible_plan = SchedulerPlan::new(
            &workflow,
            BTreeMap::from([(
                "worker".into(),
                TaskRequirement {
                    role: Some(AgentRole::Worker),
                    capabilities: BTreeSet::from([Capability::Network]),
                },
            )]),
        )
        .expect("incompatible plan");
        scheduler
            .register_plan(&incompatible_plan)
            .await
            .expect("replace plan");
        let request = ClaimRequest {
            workflow_id: workflow.id,
            agent_id: agent.id,
            correlation_id: Uuid::new_v4(),
            actor_id: Some("scheduler".into()),
        };
        let result = scheduler.claim_next(&request).await.expect("claim result");
        assert!(result.is_none());
    }
}
