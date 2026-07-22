from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if text.count(old) != 1:
        raise SystemExit(f"expected one {label}, found {text.count(old)}")
    return text.replace(old, new, 1)


scheduler = Path("crates/sessionweft-scheduler/src/lib.rs")
text = scheduler.read_text()
text = replace_once(
    text,
    """    pub requirements: BTreeMap<String, TaskRequirement>,
    pub created_at: DateTime<Utc>,
""",
    """    pub requirements: BTreeMap<String, TaskRequirement>,
    #[serde(default)]
    pub lock_requirements: BTreeMap<String, RequiredLock>,
    pub created_at: DateTime<Utc>,
""",
    "SchedulerPlan lock requirements field",
)
text = replace_once(
    text,
    """            session_id: workflow.session_id,
            requirements,
            created_at: now,
""",
    """            session_id: workflow.session_id,
            requirements,
            lock_requirements: BTreeMap::new(),
            created_at: now,
""",
    "SchedulerPlan lock requirements initialization",
)
old_methods = """    #[must_use]
    pub fn requirement_for(&self, node_id: &str) -> TaskRequirement {
        self.requirements
            .get(node_id)
            .cloned()
            .unwrap_or(TaskRequirement {
                role: None,
                capabilities: BTreeSet::new(),
            })
    }
"""
new_methods = old_methods + """
    pub fn require_lock(
        &mut self,
        workflow: &WorkflowExecution,
        node_id: &str,
        required: RequiredLock,
    ) -> Result<(), SchedulerError> {
        if workflow.id != self.workflow_id || workflow.session_id != self.session_id {
            return Err(SchedulerError::Validation(
                "lock prerequisite workflow does not match scheduler plan".into(),
            ));
        }
        let is_task = workflow
            .definition
            .nodes
            .iter()
            .any(|node| node.id == node_id && node.kind == WorkflowNodeKind::Task);
        if !is_task {
            return Err(SchedulerError::Validation(format!(
                "lock prerequisite references non-task or missing node '{node_id}'"
            )));
        }
        required.resource.validate().map_err(|error| {
            SchedulerError::Validation(format!("invalid lock prerequisite: {error}"))
        })?;
        self.lock_requirements.insert(node_id.to_owned(), required);
        self.updated_at = Utc::now();
        Ok(())
    }

    #[must_use]
    pub fn required_lock_for(&self, node_id: &str) -> Option<&RequiredLock> {
        self.lock_requirements.get(node_id)
    }
"""
text = replace_once(text, old_methods, new_methods, "SchedulerPlan lock methods")
text = replace_once(
    text,
    """    pub task_id: String,
    pub idempotency_key: String,
    pub status: TaskClaimStatus,
""",
    """    pub task_id: String,
    pub idempotency_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_fence: Option<ClaimLockFence>,
    pub status: TaskClaimStatus,
""",
    "TaskClaim lock fence field",
)
text = replace_once(
    text,
    """            idempotency_key: task_id.clone(),
            task_id,
            status: TaskClaimStatus::Active,
""",
    """            idempotency_key: task_id.clone(),
            task_id,
            lock_fence: None,
            status: TaskClaimStatus::Active,
""",
    "TaskClaim lock fence initialization",
)
scheduler.write_text(text)

sqlite = Path("crates/sessionweft-scheduler-sqlite/src/lib.rs")
text = sqlite.read_text()
old_selection = """        let mut selected = None;
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
"""
new_selection = """        let mut selected = None;
        for node_id in workflow.ready_nodes() {
            if Self::active_claim_exists(&mut transaction, workflow.id, &node_id).await? {
                continue;
            }
            if !plan.requirement_for(&node_id).matches(&agent) {
                continue;
            }
            let lock_fence = match plan.required_lock_for(&node_id) {
                Some(required) => match prerequisites::lock_fence_for(
                    &mut transaction,
                    workflow.session_id,
                    agent.id,
                    required,
                    chrono::Utc::now(),
                )
                .await?
                {
                    Some(fence) => Some(fence),
                    None => continue,
                },
                None => None,
            };
            selected = Some((node_id, lock_fence));
            break;
        }
        let Some((node_id, lock_fence)) = selected else {
            transaction.rollback().await.map_err(backend)?;
            return Ok(None);
        };
"""
text = replace_once(text, old_selection, new_selection, "claim prerequisite selection")
text = replace_once(
    text,
    """        let claim = TaskClaim::new(&workflow, node_id, attempt, &agent);
        let scheduler_event = EventEnvelope::new(
""",
    """        let mut claim = TaskClaim::new(&workflow, node_id, attempt, &agent);
        claim.lock_fence = lock_fence;
        let scheduler_event = EventEnvelope::new(
""",
    "claim fence snapshot",
)
text = replace_once(
    text,
    """                "idempotency_key": claim.idempotency_key,
            }),
""",
    """                "idempotency_key": claim.idempotency_key,
                "lock_fence": &claim.lock_fence,
            }),
""",
    "claim event fence",
)
sqlite.write_text(text)

handover = Path("crates/sessionweft-scheduler-sqlite/src/handover.rs")
text = handover.read_text()
text = replace_once(
    text,
    """    ClaimState, HandoverRequest, RepositoryError, SchedulerHandoverRepository, TaskClaim,
    TaskClaimStatus,
""",
    """    ClaimLockFence, ClaimState, HandoverRequest, RepositoryError, RequiredLock,
    SchedulerHandoverRepository, TaskClaim, TaskClaimStatus,
""",
    "handover prerequisite imports",
)
text = replace_once(
    text,
    """        now: chrono::DateTime<chrono::Utc>,
        requirement: &sessionweft_scheduler::TaskRequirement,
    ) -> Result<Option<AgentRecord>, RepositoryError> {
""",
    """        now: chrono::DateTime<chrono::Utc>,
        requirement: &sessionweft_scheduler::TaskRequirement,
        required_lock: Option<&RequiredLock>,
    ) -> Result<Option<(AgentRecord, Option<ClaimLockFence>)>, RepositoryError> {
""",
    "handover replacement signature",
)
text = replace_once(
    text,
    """            if !agent.is_stale_at(now) && requirement.matches(&agent) {
                return Ok(Some(agent));
            }
""",
    """            if agent.is_stale_at(now) || !requirement.matches(&agent) {
                continue;
            }
            let lock_fence = match required_lock {
                Some(required) => match super::prerequisites::lock_fence_for(
                    transaction,
                    session_id,
                    agent.id,
                    required,
                    now,
                )
                .await?
                {
                    Some(fence) => Some(fence),
                    None => continue,
                },
                None => None,
            };
            return Ok(Some((agent, lock_fence)));
""",
    "handover replacement lock evaluation",
)
text = replace_once(
    text,
    """        let requirement = plan.requirement_for(&previous.node_id);
        let Some(mut agent) = Self::replacement_agent(
""",
    """        let requirement = plan.requirement_for(&previous.node_id);
        let required_lock = plan.required_lock_for(&previous.node_id);
        let Some((mut agent, lock_fence)) = Self::replacement_agent(
""",
    "handover replacement destructure",
)
text = replace_once(
    text,
    """            request.now,
            &requirement,
        )
""",
    """            request.now,
            &requirement,
            required_lock,
        )
""",
    "handover replacement call",
)
text = replace_once(
    text,
    """        let claim = TaskClaim::new(&workflow, previous.node_id.clone(), attempt, &agent);
        let handover_event = EventEnvelope::new(
""",
    """        let mut claim = TaskClaim::new(&workflow, previous.node_id.clone(), attempt, &agent);
        claim.lock_fence = lock_fence;
        let handover_event = EventEnvelope::new(
""",
    "handover claim fence snapshot",
)
text = replace_once(
    text,
    """                "idempotency_key": claim.idempotency_key,
            }),
""",
    """                "idempotency_key": claim.idempotency_key,
                "lock_fence": &claim.lock_fence,
            }),
""",
    "handover event fence",
)
handover.write_text(text)
