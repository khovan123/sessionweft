use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sessionweft_core::{EventEnvelope, SessionId};
use sessionweft_execution::{AgentRecord, AgentStatus};
use sessionweft_orchestration::{LockLease, LockMode, WorkflowNodeStatus};
use sessionweft_scheduler::{
    ClaimLockFence, ClaimLockFenceSnapshot, ClaimRequest, ClaimState, HandoverRequest,
    RepositoryError, RequiredLock, SchedulerPrerequisiteRepository, TaskClaim, TaskClaimStatus,
    TaskLockRequirement,
};
use sqlx::{Row, Sqlite, Transaction};
use uuid::Uuid;

use super::{SqliteSchedulerRepository, backend, domain};

impl SqliteSchedulerRepository {
    async fn ensure_prerequisite_tables(&self) -> Result<(), RepositoryError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS scheduler_lock_requirements (
                workflow_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                data_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (workflow_id, node_id)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS scheduler_claim_lock_fences (
                claim_id TEXT PRIMARY KEY,
                workflow_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                data_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn lock_requirement_in_transaction(
        transaction: &mut Transaction<'_, Sqlite>,
        workflow_id: Uuid,
        node_id: &str,
    ) -> Result<Option<TaskLockRequirement>, RepositoryError> {
        let row = sqlx::query(
            "SELECT data_json FROM scheduler_lock_requirements WHERE workflow_id = ? AND node_id = ?",
        )
        .bind(workflow_id.to_string())
        .bind(node_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }

    async fn active_claim_for_node_guarded(
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

    async fn insert_fence_snapshot(
        transaction: &mut Transaction<'_, Sqlite>,
        claim: &TaskClaim,
        fence: &ClaimLockFence,
    ) -> Result<ClaimLockFenceSnapshot, RepositoryError> {
        let snapshot = ClaimLockFenceSnapshot {
            claim_id: claim.id,
            workflow_id: claim.workflow_id,
            node_id: claim.node_id.clone(),
            agent_id: claim.agent_id,
            fence: fence.clone(),
            created_at: Utc::now(),
        };
        sqlx::query(
            r#"
            INSERT INTO scheduler_claim_lock_fences (
                claim_id, workflow_id, node_id, agent_id, data_json, created_at
            ) VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(snapshot.claim_id.to_string())
        .bind(snapshot.workflow_id.to_string())
        .bind(&snapshot.node_id)
        .bind(snapshot.agent_id.to_string())
        .bind(serde_json::to_string(&snapshot).map_err(backend)?)
        .bind(snapshot.created_at.to_rfc3339())
        .execute(&mut **transaction)
        .await
        .map_err(backend)?;
        Ok(snapshot)
    }

    async fn replacement_agent_guarded(
        transaction: &mut Transaction<'_, Sqlite>,
        session_id: SessionId,
        excluded_agent_id: Uuid,
        now: DateTime<Utc>,
        requirement: &sessionweft_scheduler::TaskRequirement,
        required_lock: Option<&TaskLockRequirement>,
    ) -> Result<Option<(AgentRecord, Option<ClaimLockFence>)>, RepositoryError> {
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
            if agent.is_stale_at(now) || !requirement.matches(&agent) {
                continue;
            }
            let fence = match required_lock {
                Some(required) => {
                    match lock_fence_for(transaction, session_id, agent.id, &required.required, now)
                        .await?
                    {
                        Some(fence) => Some(fence),
                        None => continue,
                    }
                }
                None => None,
            };
            return Ok(Some((agent, fence)));
        }
        Ok(None)
    }
}

pub(super) async fn lock_fence_for(
    transaction: &mut Transaction<'_, Sqlite>,
    session_id: SessionId,
    agent_id: Uuid,
    required: &RequiredLock,
    now: DateTime<Utc>,
) -> Result<Option<ClaimLockFence>, RepositoryError> {
    let rows = sqlx::query(
        r#"
        SELECT data_json
        FROM lock_leases
        WHERE session_id = ? AND owner_id = ? AND expires_at > ?
        ORDER BY fencing_token DESC
        "#,
    )
    .bind(session_id.to_string())
    .bind(agent_id.to_string())
    .bind(now.to_rfc3339())
    .fetch_all(&mut **transaction)
    .await
    .map_err(backend)?;
    for row in rows {
        let lease =
            serde_json::from_str::<LockLease>(row.get::<&str, _>("data_json")).map_err(backend)?;
        let mode_matches = match required.mode {
            LockMode::Shared => true,
            LockMode::Exclusive => lease.mode == LockMode::Exclusive,
        };
        if mode_matches && lease.resource.overlaps(&required.resource) {
            return Ok(Some(ClaimLockFence {
                lock_id: lease.lock_id,
                resource: lease.resource,
                mode: lease.mode,
                fencing_token: lease.fencing_token,
                expires_at: lease.expires_at,
            }));
        }
    }
    Ok(None)
}

#[async_trait]
impl SchedulerPrerequisiteRepository for SqliteSchedulerRepository {
    async fn register_lock_requirement(
        &self,
        requirement: &TaskLockRequirement,
    ) -> Result<TaskLockRequirement, RepositoryError> {
        self.ensure_prerequisite_tables().await?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let plan = Self::load_plan(&mut transaction, requirement.workflow_id).await?;
        let workflow = Self::load_workflow(&mut transaction, requirement.workflow_id).await?;
        if plan.session_id != requirement.session_id
            || workflow.session_id != requirement.session_id
            || !workflow.definition.nodes.iter().any(|node| {
                node.id == requirement.node_id
                    && node.kind == sessionweft_orchestration::WorkflowNodeKind::Task
            })
        {
            return Err(RepositoryError::SessionMismatch);
        }
        sqlx::query(
            r#"
            INSERT INTO scheduler_lock_requirements (
                workflow_id, node_id, session_id, data_json, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(workflow_id, node_id) DO UPDATE SET
                session_id = excluded.session_id,
                data_json = excluded.data_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(requirement.workflow_id.to_string())
        .bind(&requirement.node_id)
        .bind(requirement.session_id.to_string())
        .bind(serde_json::to_string(requirement).map_err(backend)?)
        .bind(requirement.created_at.to_rfc3339())
        .bind(requirement.updated_at.to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        Ok(requirement.clone())
    }

    async fn get_lock_requirement(
        &self,
        workflow_id: Uuid,
        node_id: &str,
    ) -> Result<Option<TaskLockRequirement>, RepositoryError> {
        self.ensure_prerequisite_tables().await?;
        let row = sqlx::query(
            "SELECT data_json FROM scheduler_lock_requirements WHERE workflow_id = ? AND node_id = ?",
        )
        .bind(workflow_id.to_string())
        .bind(node_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }

    async fn get_claim_lock_fence(
        &self,
        claim_id: Uuid,
    ) -> Result<Option<ClaimLockFenceSnapshot>, RepositoryError> {
        self.ensure_prerequisite_tables().await?;
        let row =
            sqlx::query("SELECT data_json FROM scheduler_claim_lock_fences WHERE claim_id = ?")
                .bind(claim_id.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }

    async fn claim_next_guarded(
        &self,
        request: &ClaimRequest,
        now: DateTime<Utc>,
    ) -> Result<Option<ClaimState>, RepositoryError> {
        self.ensure_prerequisite_tables().await?;
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
            if !plan.requirement_for(&node_id).matches(&agent) {
                continue;
            }
            let required =
                Self::lock_requirement_in_transaction(&mut transaction, workflow.id, &node_id)
                    .await?;
            let fence = match required.as_ref() {
                Some(required) => match lock_fence_for(
                    &mut transaction,
                    workflow.session_id,
                    agent.id,
                    &required.required,
                    now,
                )
                .await?
                {
                    Some(fence) => Some(fence),
                    None => continue,
                },
                None => None,
            };
            selected = Some((node_id, fence));
            break;
        }
        let Some((node_id, fence)) = selected else {
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
        if let Some(fence) = fence.as_ref() {
            Self::insert_fence_snapshot(&mut transaction, &claim, fence).await?;
        }
        let scheduler_event = EventEnvelope::new(
            "scheduler.task_claimed",
            Some(claim.session_id),
            request.correlation_id,
            request.actor_id.as_deref(),
            serde_json::json!({
                "claim_id": claim.id,
                "workflow_id": claim.workflow_id,
                "node_id": claim.node_id,
                "attempt": claim.attempt,
                "agent_id": claim.agent_id,
                "task_id": claim.task_id,
                "idempotency_key": claim.idempotency_key,
                "lock_fence": fence,
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

    async fn handover_released_claim_guarded(
        &self,
        request: &HandoverRequest,
    ) -> Result<Option<ClaimState>, RepositoryError> {
        self.ensure_prerequisite_tables().await?;
        let _guard = self.claim_guard.lock().await;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let previous = Self::load_claim(&mut transaction, request.previous_claim_id).await?;
        if previous.status != TaskClaimStatus::Released {
            return Err(RepositoryError::Conflict(
                "only released claims can be handed over".into(),
            ));
        }
        if let Some(active) = Self::active_claim_for_node_guarded(
            &mut transaction,
            previous.workflow_id,
            &previous.node_id,
        )
        .await?
        {
            return Self::current_state(&mut transaction, active)
                .await
                .map(Some);
        }
        let plan = Self::load_plan(&mut transaction, previous.workflow_id).await?;
        let mut workflow = Self::load_workflow(&mut transaction, previous.workflow_id).await?;
        if workflow.session_id != previous.session_id || plan.session_id != previous.session_id {
            return Err(RepositoryError::SessionMismatch);
        }
        if workflow
            .nodes
            .get(&previous.node_id)
            .map(|state| state.status)
            != Some(WorkflowNodeStatus::Ready)
        {
            return Err(RepositoryError::Conflict(
                "released claim node is not ready for retry".into(),
            ));
        }
        let requirement = plan.requirement_for(&previous.node_id);
        let required_lock =
            Self::lock_requirement_in_transaction(&mut transaction, workflow.id, &previous.node_id)
                .await?;
        let Some((mut agent, fence)) = Self::replacement_agent_guarded(
            &mut transaction,
            previous.session_id,
            previous.agent_id,
            request.now,
            &requirement,
            required_lock.as_ref(),
        )
        .await?
        else {
            transaction.rollback().await.map_err(backend)?;
            return Ok(None);
        };

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
        if let Some(fence) = fence.as_ref() {
            Self::insert_fence_snapshot(&mut transaction, &claim, fence).await?;
        }
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
                "lock_fence": fence,
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
