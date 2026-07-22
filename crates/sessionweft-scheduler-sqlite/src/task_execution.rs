use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sessionweft_core::EventEnvelope;
use sessionweft_execution::{AgentStatus, Capability};
use sessionweft_scheduler::{
    ClaimLockFenceSnapshot, RepositoryError, TaskAction, TaskExecutionRecord,
    TaskExecutionRepository, TaskExecutionSpec, TaskExecutionStatus, TaskClaimStatus,
    ToolExecutionApproval,
};
use sqlx::{Row, Sqlite, Transaction};
use uuid::Uuid;

use super::{SqliteSchedulerRepository, backend};

impl SqliteSchedulerRepository {
    async fn ensure_execution_tables(&self) -> Result<(), RepositoryError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS scheduler_execution_specs (
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
            CREATE TABLE IF NOT EXISTS scheduler_tool_approvals (
                approval_id TEXT PRIMARY KEY,
                claim_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                data_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_scheduler_tool_approvals_claim
            ON scheduler_tool_approvals (claim_id, tool_name, expires_at)
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS scheduler_task_executions (
                execution_id TEXT PRIMARY KEY,
                claim_id TEXT NOT NULL UNIQUE,
                session_id TEXT NOT NULL,
                workflow_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                idempotency_key TEXT NOT NULL UNIQUE,
                status TEXT NOT NULL,
                claim_finalized INTEGER NOT NULL DEFAULT 0,
                data_json TEXT NOT NULL,
                prepared_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_scheduler_task_executions_status
            ON scheduler_task_executions (status, claim_finalized, updated_at)
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
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

    async fn load_execution(
        transaction: &mut Transaction<'_, Sqlite>,
        execution_id: Uuid,
    ) -> Result<TaskExecutionRecord, RepositoryError> {
        let row = sqlx::query(
            "SELECT data_json FROM scheduler_task_executions WHERE execution_id = ?",
        )
        .bind(execution_id.to_string())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(backend)?
        .ok_or_else(|| RepositoryError::Conflict(format!("execution {execution_id} not found")))?;
        serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend)
    }

    async fn execution_for_claim(
        transaction: &mut Transaction<'_, Sqlite>,
        claim_id: Uuid,
    ) -> Result<Option<TaskExecutionRecord>, RepositoryError> {
        let row = sqlx::query(
            "SELECT data_json FROM scheduler_task_executions WHERE claim_id = ?",
        )
        .bind(claim_id.to_string())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }

    async fn execution_spec(
        transaction: &mut Transaction<'_, Sqlite>,
        workflow_id: Uuid,
        node_id: &str,
    ) -> Result<Option<TaskExecutionSpec>, RepositoryError> {
        let row = sqlx::query(
            "SELECT data_json FROM scheduler_execution_specs WHERE workflow_id = ? AND node_id = ?",
        )
        .bind(workflow_id.to_string())
        .bind(node_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }

    async fn approval_exists(
        transaction: &mut Transaction<'_, Sqlite>,
        claim_id: Uuid,
        session_id: sessionweft_core::SessionId,
        agent_id: Uuid,
        tool_name: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, RepositoryError> {
        let count = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COUNT(*) FROM scheduler_tool_approvals
            WHERE claim_id = ? AND session_id = ? AND agent_id = ?
              AND tool_name = ? AND expires_at > ?
            "#,
        )
        .bind(claim_id.to_string())
        .bind(session_id.to_string())
        .bind(agent_id.to_string())
        .bind(tool_name)
        .bind(now.to_rfc3339())
        .fetch_one(&mut **transaction)
        .await
        .map_err(backend)?;
        Ok(count > 0)
    }

    async fn validate_fence_snapshot(
        transaction: &mut Transaction<'_, Sqlite>,
        claim_id: Uuid,
        session_id: sessionweft_core::SessionId,
        agent_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        let requirement = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COUNT(*)
            FROM scheduler_claims AS claim
            JOIN scheduler_lock_requirements AS requirement
              ON requirement.workflow_id = claim.workflow_id
             AND requirement.node_id = claim.node_id
            WHERE claim.claim_id = ?
            "#,
        )
        .bind(claim_id.to_string())
        .fetch_one(&mut **transaction)
        .await
        .map_err(backend)?;
        if requirement == 0 {
            return Ok(());
        }
        let row = sqlx::query(
            "SELECT data_json FROM scheduler_claim_lock_fences WHERE claim_id = ?",
        )
        .bind(claim_id.to_string())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(backend)?
        .ok_or_else(|| RepositoryError::Conflict("required claim lock fence is missing".into()))?;
        let snapshot = serde_json::from_str::<ClaimLockFenceSnapshot>(
            row.get::<&str, _>("data_json"),
        )
        .map_err(backend)?;
        if snapshot.agent_id != agent_id || snapshot.fence.expires_at <= now {
            return Err(RepositoryError::Conflict(
                "claim lock fence is expired or belongs to another Agent".into(),
            ));
        }
        let lease_row = sqlx::query("SELECT data_json FROM lock_leases WHERE lock_id = ?")
            .bind(snapshot.fence.lock_id.to_string())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(backend)?
            .ok_or_else(|| RepositoryError::Conflict("claim lock lease no longer exists".into()))?;
        let lease = serde_json::from_str::<sessionweft_orchestration::LockLease>(
            lease_row.get::<&str, _>("data_json"),
        )
        .map_err(backend)?;
        if lease.session_id != session_id
            || lease.owner_id != agent_id.to_string()
            || lease.fencing_token != snapshot.fence.fencing_token
            || lease.expires_at <= now
            || !lease.resource.overlaps(&snapshot.fence.resource)
        {
            return Err(RepositoryError::Conflict(
                "claim lock fence failed execution-time revalidation".into(),
            ));
        }
        Ok(())
    }

    async fn save_execution(
        transaction: &mut Transaction<'_, Sqlite>,
        execution: &TaskExecutionRecord,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            r#"
            UPDATE scheduler_task_executions
            SET status = ?, data_json = ?, updated_at = ?
            WHERE execution_id = ?
            "#,
        )
        .bind(execution_status(execution.status))
        .bind(serde_json::to_string(execution).map_err(backend)?)
        .bind(execution.updated_at.to_rfc3339())
        .bind(execution.id.to_string())
        .execute(&mut **transaction)
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn execution_event(
        transaction: &mut Transaction<'_, Sqlite>,
        execution: &TaskExecutionRecord,
        event_type: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<(), RepositoryError> {
        let event = EventEnvelope::new(
            event_type,
            Some(execution.session_id),
            correlation_id,
            actor_id,
            serde_json::json!({
                "execution_id": execution.id,
                "claim_id": execution.claim_id,
                "workflow_id": execution.workflow_id,
                "node_id": execution.node_id,
                "agent_id": execution.agent_id,
                "idempotency_key": execution.idempotency_key,
                "status": execution.status,
                "action": execution.action.action_name(),
            }),
        );
        Self::insert_events(transaction, &[event]).await
    }
}

#[async_trait]
impl TaskExecutionRepository for SqliteSchedulerRepository {
    async fn register_spec(
        &self,
        spec: &TaskExecutionSpec,
    ) -> Result<TaskExecutionSpec, RepositoryError> {
        self.ensure_execution_tables().await?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let workflow = Self::load_workflow(&mut transaction, spec.workflow_id).await?;
        if workflow.session_id != spec.session_id
            || !workflow.definition.nodes.iter().any(|node| {
                node.id == spec.node_id
                    && node.kind == sessionweft_orchestration::WorkflowNodeKind::Task
            })
        {
            return Err(RepositoryError::SessionMismatch);
        }
        spec.action
            .validate()
            .map_err(|error| RepositoryError::Conflict(error.to_string()))?;
        sqlx::query(
            r#"
            INSERT INTO scheduler_execution_specs (
                workflow_id, node_id, session_id, data_json, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(workflow_id, node_id) DO UPDATE SET
                session_id = excluded.session_id,
                data_json = excluded.data_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(spec.workflow_id.to_string())
        .bind(&spec.node_id)
        .bind(spec.session_id.to_string())
        .bind(serde_json::to_string(spec).map_err(backend)?)
        .bind(spec.created_at.to_rfc3339())
        .bind(spec.updated_at.to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        Ok(spec.clone())
    }

    async fn get_spec(
        &self,
        workflow_id: Uuid,
        node_id: &str,
    ) -> Result<Option<TaskExecutionSpec>, RepositoryError> {
        self.ensure_execution_tables().await?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        Self::execution_spec(&mut transaction, workflow_id, node_id).await
    }

    async fn grant_tool_approval(
        &self,
        approval: &ToolExecutionApproval,
    ) -> Result<ToolExecutionApproval, RepositoryError> {
        self.ensure_execution_tables().await?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let claim = Self::load_claim(&mut transaction, approval.claim_id).await?;
        if claim.session_id != approval.session_id || claim.agent_id != approval.agent_id {
            return Err(RepositoryError::SessionMismatch);
        }
        sqlx::query(
            r#"
            INSERT INTO scheduler_tool_approvals (
                approval_id, claim_id, session_id, agent_id, tool_name,
                expires_at, data_json, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(approval.id.to_string())
        .bind(approval.claim_id.to_string())
        .bind(approval.session_id.to_string())
        .bind(approval.agent_id.to_string())
        .bind(&approval.tool_name)
        .bind(approval.expires_at.to_rfc3339())
        .bind(serde_json::to_string(approval).map_err(backend)?)
        .bind(approval.created_at.to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        Ok(approval.clone())
    }

    async fn prepare_claim_execution(
        &self,
        claim_id: Uuid,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Option<TaskExecutionRecord>, RepositoryError> {
        self.ensure_execution_tables().await?;
        let _guard = self.claim_guard.lock().await;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        if let Some(existing) = Self::execution_for_claim(&mut transaction, claim_id).await? {
            return Ok(Some(existing));
        }
        let claim = Self::load_claim(&mut transaction, claim_id).await?;
        if claim.status != TaskClaimStatus::Active {
            return Err(RepositoryError::ClaimNotActive(claim_id));
        }
        let agent = Self::load_agent(&mut transaction, claim.agent_id).await?;
        if agent.status != AgentStatus::Running
            || agent.current_task_id.as_deref() != Some(claim.task_id.as_str())
            || agent.is_stale_at(now)
        {
            return Err(RepositoryError::AgentUnavailable);
        }
        let Some(spec) = Self::execution_spec(
            &mut transaction,
            claim.workflow_id,
            &claim.node_id,
        )
        .await?
        else {
            transaction.rollback().await.map_err(backend)?;
            return Ok(None);
        };
        if spec.session_id != claim.session_id {
            return Err(RepositoryError::SessionMismatch);
        }
        match &spec.action {
            TaskAction::Provider { .. } => {
                if !agent.manifest.capabilities.contains(&Capability::Provider) {
                    return Err(RepositoryError::Conflict(
                        "Agent lacks Provider capability".into(),
                    ));
                }
            }
            TaskAction::Tool { descriptor, .. } => {
                descriptor
                    .validate()
                    .map_err(|error| RepositoryError::Conflict(error.to_string()))?;
                if descriptor
                    .permissions
                    .iter()
                    .any(|permission| !agent.allows(permission))
                {
                    return Err(RepositoryError::Conflict(
                        "Agent lacks one or more Tool permissions".into(),
                    ));
                }
                if spec.action.requires_explicit_approval()
                    && !Self::approval_exists(
                        &mut transaction,
                        claim.id,
                        claim.session_id,
                        claim.agent_id,
                        &descriptor.name,
                        now,
                    )
                    .await?
                {
                    transaction.rollback().await.map_err(backend)?;
                    return Ok(None);
                }
            }
        }
        Self::validate_fence_snapshot(
            &mut transaction,
            claim.id,
            claim.session_id,
            claim.agent_id,
            now,
        )
        .await?;
        let execution = TaskExecutionRecord::prepared(
            claim.id,
            claim.session_id,
            claim.workflow_id,
            claim.node_id,
            claim.agent_id,
            claim.idempotency_key,
            spec.action,
            now,
        );
        sqlx::query(
            r#"
            INSERT INTO scheduler_task_executions (
                execution_id, claim_id, session_id, workflow_id, node_id,
                agent_id, idempotency_key, status, claim_finalized,
                data_json, prepared_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?)
            "#,
        )
        .bind(execution.id.to_string())
        .bind(execution.claim_id.to_string())
        .bind(execution.session_id.to_string())
        .bind(execution.workflow_id.to_string())
        .bind(&execution.node_id)
        .bind(execution.agent_id.to_string())
        .bind(&execution.idempotency_key)
        .bind(execution_status(execution.status))
        .bind(serde_json::to_string(&execution).map_err(backend)?)
        .bind(execution.prepared_at.to_rfc3339())
        .bind(execution.updated_at.to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        Self::execution_event(
            &mut transaction,
            &execution,
            "scheduler.execution_prepared",
            correlation_id,
            actor_id,
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(Some(execution))
    }

    async fn mark_execution_running(
        &self,
        execution_id: Uuid,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<TaskExecutionRecord, RepositoryError> {
        self.ensure_execution_tables().await?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut execution = Self::load_execution(&mut transaction, execution_id).await?;
        if execution.status != TaskExecutionStatus::Prepared {
            return Err(RepositoryError::Conflict(format!(
                "execution {execution_id} is not prepared"
            )));
        }
        execution.status = TaskExecutionStatus::Running;
        execution.started_at = Some(now);
        execution.updated_at = now;
        Self::save_execution(&mut transaction, &execution).await?;
        Self::execution_event(
            &mut transaction,
            &execution,
            "scheduler.execution_started",
            correlation_id,
            actor_id,
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(execution)
    }

    async fn succeed_execution(
        &self,
        execution_id: Uuid,
        output: Value,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<TaskExecutionRecord, RepositoryError> {
        self.ensure_execution_tables().await?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut execution = Self::load_execution(&mut transaction, execution_id).await?;
        if execution.status == TaskExecutionStatus::Succeeded {
            return Ok(execution);
        }
        if execution.status != TaskExecutionStatus::Running {
            return Err(RepositoryError::Conflict(format!(
                "execution {execution_id} is not running"
            )));
        }
        execution.status = TaskExecutionStatus::Succeeded;
        execution.output = Some(output);
        execution.completed_at = Some(now);
        execution.updated_at = now;
        Self::save_execution(&mut transaction, &execution).await?;
        Self::execution_event(
            &mut transaction,
            &execution,
            "scheduler.execution_succeeded",
            correlation_id,
            actor_id,
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(execution)
    }

    async fn fail_execution(
        &self,
        execution_id: Uuid,
        sanitized_error: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<TaskExecutionRecord, RepositoryError> {
        self.ensure_execution_tables().await?;
        if sanitized_error.trim().is_empty() {
            return Err(RepositoryError::Conflict(
                "execution failure requires a sanitized error".into(),
            ));
        }
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut execution = Self::load_execution(&mut transaction, execution_id).await?;
        if execution.status == TaskExecutionStatus::Failed {
            return Ok(execution);
        }
        if execution.status != TaskExecutionStatus::Running {
            return Err(RepositoryError::Conflict(format!(
                "execution {execution_id} is not running"
            )));
        }
        execution.status = TaskExecutionStatus::Failed;
        execution.sanitized_error = Some(sanitized_error.to_owned());
        execution.completed_at = Some(now);
        execution.updated_at = now;
        Self::save_execution(&mut transaction, &execution).await?;
        Self::execution_event(
            &mut transaction,
            &execution,
            "scheduler.execution_failed",
            correlation_id,
            actor_id,
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(execution)
    }

    async fn mark_stale_running_uncertain(
        &self,
        stale_before: DateTime<Utc>,
        limit: usize,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Vec<TaskExecutionRecord>, RepositoryError> {
        self.ensure_execution_tables().await?;
        let rows = sqlx::query(
            r#"
            SELECT execution_id FROM scheduler_task_executions
            WHERE status = 'running' AND updated_at < ?
            ORDER BY updated_at ASC LIMIT ?
            "#,
        )
        .bind(stale_before.to_rfc3339())
        .bind(i64::try_from(limit.clamp(1, 1_000)).map_err(backend)?)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        let mut uncertain = Vec::with_capacity(rows.len());
        for row in rows {
            let execution_id = Uuid::parse_str(row.get::<&str, _>("execution_id"))
                .map_err(backend)?;
            let mut transaction = self.pool.begin().await.map_err(backend)?;
            let mut execution = Self::load_execution(&mut transaction, execution_id).await?;
            if execution.status != TaskExecutionStatus::Running {
                continue;
            }
            execution.status = TaskExecutionStatus::Uncertain;
            execution.sanitized_error = Some(
                "worker stopped while external side effect status was unknown".into(),
            );
            execution.updated_at = Utc::now();
            Self::save_execution(&mut transaction, &execution).await?;
            Self::execution_event(
                &mut transaction,
                &execution,
                "scheduler.execution_uncertain",
                correlation_id,
                actor_id,
            )
            .await?;
            transaction.commit().await.map_err(backend)?;
            uncertain.push(execution);
        }
        Ok(uncertain)
    }

    async fn get_execution(
        &self,
        execution_id: Uuid,
    ) -> Result<Option<TaskExecutionRecord>, RepositoryError> {
        self.ensure_execution_tables().await?;
        let row = sqlx::query(
            "SELECT data_json FROM scheduler_task_executions WHERE execution_id = ?",
        )
        .bind(execution_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }

    async fn prepared_executions(
        &self,
        limit: usize,
    ) -> Result<Vec<TaskExecutionRecord>, RepositoryError> {
        execution_list(&self.pool, "prepared", false, limit).await
    }

    async fn succeeded_unfinalized_executions(
        &self,
        limit: usize,
    ) -> Result<Vec<TaskExecutionRecord>, RepositoryError> {
        execution_list(&self.pool, "succeeded", true, limit).await
    }

    async fn mark_claim_finalized(
        &self,
        execution_id: Uuid,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<(), RepositoryError> {
        self.ensure_execution_tables().await?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let execution = Self::load_execution(&mut transaction, execution_id).await?;
        if execution.status != TaskExecutionStatus::Succeeded
            && execution.status != TaskExecutionStatus::Failed
        {
            return Err(RepositoryError::Conflict(
                "only terminal executions can finalize claims".into(),
            ));
        }
        sqlx::query(
            "UPDATE scheduler_task_executions SET claim_finalized = 1 WHERE execution_id = ?",
        )
        .bind(execution_id.to_string())
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        Self::execution_event(
            &mut transaction,
            &execution,
            "scheduler.execution_claim_finalized",
            correlation_id,
            actor_id,
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(())
    }
}

async fn execution_list(
    pool: &sqlx::SqlitePool,
    status: &str,
    unfinalized: bool,
    limit: usize,
) -> Result<Vec<TaskExecutionRecord>, RepositoryError> {
    let rows = sqlx::query(
        r#"
        SELECT data_json FROM scheduler_task_executions
        WHERE status = ? AND (? = 0 OR claim_finalized = 0)
        ORDER BY updated_at ASC LIMIT ?
        "#,
    )
    .bind(status)
    .bind(if unfinalized { 1_i64 } else { 0_i64 })
    .bind(i64::try_from(limit.clamp(1, 1_000)).map_err(backend)?)
    .fetch_all(pool)
    .await
    .map_err(backend)?;
    rows.into_iter()
        .map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
        .collect()
}

fn execution_status(status: TaskExecutionStatus) -> &'static str {
    match status {
        TaskExecutionStatus::Prepared => "prepared",
        TaskExecutionStatus::Running => "running",
        TaskExecutionStatus::Succeeded => "succeeded",
        TaskExecutionStatus::Failed => "failed",
        TaskExecutionStatus::Uncertain => "uncertain",
    }
}
