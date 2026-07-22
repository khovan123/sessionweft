use std::sync::Arc;

use sessionweft_core::{Session, SessionId};
use sessionweft_execution::{
    AgentManifest, AgentRecord, AgentRepository, AgentService, ExecutionError,
};
use sessionweft_knowledge::{
    KnowledgeError, MemoryHit, MemoryQuery, MemoryRecord, MemoryRepository, MemoryService,
};
use sessionweft_orchestration::{
    LockLease, LockRequest, OrchestrationError, OrchestrationRepository, OrchestrationService,
    WorkflowDefinition, WorkflowExecution,
};
use sessionweft_runtime::{RuntimeError, RuntimeService};
use sessionweft_storage::SessionRepository;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationContext {
    pub correlation_id: Uuid,
    pub actor_id: Option<String>,
}

impl OperationContext {
    #[must_use]
    pub fn new(correlation_id: Uuid, actor_id: Option<String>) -> Self {
        Self {
            correlation_id,
            actor_id,
        }
    }

    #[must_use]
    pub fn system(actor_id: impl Into<String>) -> Self {
        Self::new(Uuid::new_v4(), Some(actor_id.into()))
    }

    fn actor_id(&self) -> Option<&str> {
        self.actor_id.as_deref()
    }
}

pub struct RuntimeControlPlane<SR, AR, OR, MR>
where
    SR: SessionRepository,
    AR: AgentRepository,
    OR: OrchestrationRepository,
    MR: MemoryRepository,
{
    sessions: RuntimeService<SR>,
    agents: AgentService<AR>,
    orchestration: OrchestrationService<OR>,
    memories: MemoryService<MR>,
}

impl<SR, AR, OR, MR> RuntimeControlPlane<SR, AR, OR, MR>
where
    SR: SessionRepository,
    AR: AgentRepository,
    OR: OrchestrationRepository,
    MR: MemoryRepository,
{
    #[must_use]
    pub fn new(
        sessions: RuntimeService<SR>,
        agent_repository: Arc<AR>,
        orchestration_repository: Arc<OR>,
        memory_repository: Arc<MR>,
    ) -> Self {
        Self {
            sessions,
            agents: AgentService::new(agent_repository),
            orchestration: OrchestrationService::new(orchestration_repository),
            memories: MemoryService::new(memory_repository),
        }
    }

    pub async fn create_session(
        &self,
        title: impl Into<String>,
        context: &OperationContext,
    ) -> Result<Session, ControlPlaneError> {
        self.sessions
            .create_session(title, context.actor_id(), context.correlation_id)
            .await
            .map_err(ControlPlaneError::Runtime)
    }

    pub async fn get_session(&self, session_id: SessionId) -> Result<Session, ControlPlaneError> {
        self.sessions
            .get_session(session_id)
            .await
            .map_err(ControlPlaneError::Runtime)
    }

    pub async fn register_agent(
        &self,
        session_id: SessionId,
        manifest: AgentManifest,
        context: &OperationContext,
    ) -> Result<AgentRecord, ControlPlaneError> {
        self.ensure_session(session_id).await?;
        self.agents
            .register(
                session_id,
                manifest,
                context.correlation_id,
                context.actor_id(),
            )
            .await
            .map_err(ControlPlaneError::Execution)
    }

    pub async fn get_agent(
        &self,
        session_id: SessionId,
        agent_id: Uuid,
    ) -> Result<AgentRecord, ControlPlaneError> {
        self.ensure_session(session_id).await?;
        let agent = self
            .agents
            .get(agent_id)
            .await
            .map_err(ControlPlaneError::Execution)?;
        ensure_scope("agent", session_id, agent.session_id)?;
        Ok(agent)
    }

    pub async fn start_agent(
        &self,
        session_id: SessionId,
        agent_id: Uuid,
        expected_version: u64,
        context: &OperationContext,
    ) -> Result<AgentRecord, ControlPlaneError> {
        self.get_agent(session_id, agent_id).await?;
        let actor_id = context.actor_id.clone();
        let correlation_id = context.correlation_id;
        self.agents
            .mutate(agent_id, expected_version, move |agent| {
                Ok(vec![agent.start(
                    expected_version,
                    correlation_id,
                    actor_id.as_deref(),
                )?])
            })
            .await
            .map_err(ControlPlaneError::Execution)
    }

    pub async fn heartbeat_agent(
        &self,
        session_id: SessionId,
        agent_id: Uuid,
        expected_version: u64,
        context: &OperationContext,
    ) -> Result<AgentRecord, ControlPlaneError> {
        self.get_agent(session_id, agent_id).await?;
        let actor_id = context.actor_id.clone();
        let correlation_id = context.correlation_id;
        self.agents
            .mutate(agent_id, expected_version, move |agent| {
                Ok(vec![agent.heartbeat(
                    expected_version,
                    correlation_id,
                    actor_id.as_deref(),
                )?])
            })
            .await
            .map_err(ControlPlaneError::Execution)
    }

    pub async fn stop_agent(
        &self,
        session_id: SessionId,
        agent_id: Uuid,
        expected_version: u64,
        context: &OperationContext,
    ) -> Result<AgentRecord, ControlPlaneError> {
        self.get_agent(session_id, agent_id).await?;
        let actor_id = context.actor_id.clone();
        let correlation_id = context.correlation_id;
        self.agents
            .mutate(agent_id, expected_version, move |agent| {
                Ok(vec![agent.stop(
                    expected_version,
                    correlation_id,
                    actor_id.as_deref(),
                )?])
            })
            .await
            .map_err(ControlPlaneError::Execution)
    }

    pub async fn create_workflow(
        &self,
        session_id: SessionId,
        definition: WorkflowDefinition,
        context: &OperationContext,
    ) -> Result<WorkflowExecution, ControlPlaneError> {
        self.ensure_session(session_id).await?;
        self.orchestration
            .create_workflow(
                session_id,
                definition,
                context.correlation_id,
                context.actor_id(),
            )
            .await
            .map_err(ControlPlaneError::Orchestration)
    }

    pub async fn get_workflow(
        &self,
        session_id: SessionId,
        workflow_id: Uuid,
    ) -> Result<WorkflowExecution, ControlPlaneError> {
        self.ensure_session(session_id).await?;
        let workflow = self
            .orchestration
            .get_workflow(workflow_id)
            .await
            .map_err(ControlPlaneError::Orchestration)?;
        ensure_scope("workflow", session_id, workflow.session_id)?;
        Ok(workflow)
    }

    pub async fn start_workflow_node(
        &self,
        session_id: SessionId,
        workflow_id: Uuid,
        expected_version: u64,
        node_id: &str,
        owner_id: impl Into<String>,
        context: &OperationContext,
    ) -> Result<WorkflowExecution, ControlPlaneError> {
        self.get_workflow(session_id, workflow_id).await?;
        self.orchestration
            .start_node(
                workflow_id,
                expected_version,
                node_id,
                owner_id,
                context.correlation_id,
                context.actor_id(),
            )
            .await
            .map_err(ControlPlaneError::Orchestration)
    }

    pub async fn complete_workflow_node(
        &self,
        session_id: SessionId,
        workflow_id: Uuid,
        expected_version: u64,
        node_id: &str,
        context: &OperationContext,
    ) -> Result<WorkflowExecution, ControlPlaneError> {
        self.get_workflow(session_id, workflow_id).await?;
        self.orchestration
            .complete_node(
                workflow_id,
                expected_version,
                node_id,
                context.correlation_id,
                context.actor_id(),
            )
            .await
            .map_err(ControlPlaneError::Orchestration)
    }

    pub async fn fail_workflow_node(
        &self,
        session_id: SessionId,
        workflow_id: Uuid,
        expected_version: u64,
        node_id: &str,
        sanitized_error: impl Into<String>,
        context: &OperationContext,
    ) -> Result<WorkflowExecution, ControlPlaneError> {
        self.get_workflow(session_id, workflow_id).await?;
        self.orchestration
            .fail_node(
                workflow_id,
                expected_version,
                node_id,
                sanitized_error,
                context.correlation_id,
                context.actor_id(),
            )
            .await
            .map_err(ControlPlaneError::Orchestration)
    }

    pub async fn decide_workflow_approval(
        &self,
        session_id: SessionId,
        workflow_id: Uuid,
        expected_version: u64,
        node_id: &str,
        approved: bool,
        context: &OperationContext,
    ) -> Result<WorkflowExecution, ControlPlaneError> {
        self.get_workflow(session_id, workflow_id).await?;
        self.orchestration
            .decide_approval(
                workflow_id,
                expected_version,
                node_id,
                approved,
                context.correlation_id,
                context.actor_id(),
            )
            .await
            .map_err(ControlPlaneError::Orchestration)
    }

    pub async fn acquire_lock(
        &self,
        request: &LockRequest,
        context: &OperationContext,
    ) -> Result<LockLease, ControlPlaneError> {
        self.ensure_session(request.session_id).await?;
        self.orchestration
            .acquire_lock(request, context.correlation_id, context.actor_id())
            .await
            .map_err(ControlPlaneError::Orchestration)
    }

    pub async fn heartbeat_lock(
        &self,
        session_id: SessionId,
        lock_id: Uuid,
        owner_id: &str,
        fencing_token: u64,
        ttl_seconds: u32,
        context: &OperationContext,
    ) -> Result<LockLease, ControlPlaneError> {
        self.ensure_session(session_id).await?;
        self.orchestration
            .heartbeat_lock(
                lock_id,
                owner_id,
                fencing_token,
                ttl_seconds,
                context.correlation_id,
                context.actor_id(),
            )
            .await
            .map_err(ControlPlaneError::Orchestration)
    }

    pub async fn release_lock(
        &self,
        session_id: SessionId,
        lock_id: Uuid,
        owner_id: &str,
        fencing_token: u64,
        context: &OperationContext,
    ) -> Result<(), ControlPlaneError> {
        self.ensure_session(session_id).await?;
        self.orchestration
            .release_lock(
                lock_id,
                owner_id,
                fencing_token,
                context.correlation_id,
                context.actor_id(),
            )
            .await
            .map_err(ControlPlaneError::Orchestration)
    }

    pub async fn list_locks(
        &self,
        session_id: SessionId,
        workspace_id: &str,
    ) -> Result<Vec<LockLease>, ControlPlaneError> {
        self.ensure_session(session_id).await?;
        self.orchestration
            .list_locks(workspace_id)
            .await
            .map_err(ControlPlaneError::Orchestration)
    }

    pub async fn remember(
        &self,
        record: MemoryRecord,
        context: &OperationContext,
    ) -> Result<MemoryRecord, ControlPlaneError> {
        self.ensure_session(record.session_id).await?;
        self.memories
            .remember(record, context.correlation_id, context.actor_id())
            .await
            .map_err(ControlPlaneError::Knowledge)
    }

    pub async fn search_memories(
        &self,
        query: &MemoryQuery,
    ) -> Result<Vec<MemoryHit>, ControlPlaneError> {
        self.ensure_session(query.session_id).await?;
        self.memories
            .search(query)
            .await
            .map_err(ControlPlaneError::Knowledge)
    }

    pub async fn forget_memory(
        &self,
        session_id: SessionId,
        memory_id: Uuid,
        context: &OperationContext,
    ) -> Result<(), ControlPlaneError> {
        self.ensure_session(session_id).await?;
        self.memories
            .forget(
                session_id,
                memory_id,
                context.correlation_id,
                context.actor_id(),
            )
            .await
            .map_err(ControlPlaneError::Knowledge)
    }

    async fn ensure_session(&self, session_id: SessionId) -> Result<(), ControlPlaneError> {
        self.get_session(session_id).await.map(|_| ())
    }
}

fn ensure_scope(
    resource: &'static str,
    expected: SessionId,
    actual: SessionId,
) -> Result<(), ControlPlaneError> {
    if expected == actual {
        return Ok(());
    }
    Err(ControlPlaneError::SessionScopeMismatch {
        resource,
        expected,
        actual,
    })
}

#[derive(Debug, Error)]
pub enum ControlPlaneError {
    #[error("runtime operation failed: {0}")]
    Runtime(RuntimeError),
    #[error("agent operation failed: {0}")]
    Execution(ExecutionError),
    #[error("orchestration operation failed: {0}")]
    Orchestration(OrchestrationError),
    #[error("knowledge operation failed: {0}")]
    Knowledge(KnowledgeError),
    #[error("{resource} belongs to session {actual}, expected session {expected}")]
    SessionScopeMismatch {
        resource: &'static str,
        expected: SessionId,
        actual: SessionId,
    },
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use sessionweft_execution::{AgentRole, AgentStatus};
    use sessionweft_execution_sqlite::SqliteAgentRepository;
    use sessionweft_knowledge::{MemoryClass, MemorySource};
    use sessionweft_knowledge_sqlite::SqliteMemoryRepository;
    use sessionweft_orchestration::{
        LockMode, LockResource, WorkflowNodeDefinition, WorkflowNodeKind, WorkflowStatus,
    };
    use sessionweft_orchestration_sqlite::SqliteOrchestrationRepository;
    use sessionweft_provider::{EchoProvider, ProviderRegistry};
    use sessionweft_storage::SqliteSessionRepository;

    use super::*;

    async fn control_plane() -> RuntimeControlPlane<
        SqliteSessionRepository,
        SqliteAgentRepository,
        SqliteOrchestrationRepository,
        SqliteMemoryRepository,
    > {
        let session_repository = Arc::new(
            SqliteSessionRepository::connect("sqlite::memory:")
                .await
                .expect("session repository"),
        );
        let agent_repository = Arc::new(
            SqliteAgentRepository::connect("sqlite::memory:")
                .await
                .expect("agent repository"),
        );
        let orchestration_repository = Arc::new(
            SqliteOrchestrationRepository::connect("sqlite::memory:")
                .await
                .expect("orchestration repository"),
        );
        let memory_repository = Arc::new(
            SqliteMemoryRepository::connect("sqlite::memory:")
                .await
                .expect("memory repository"),
        );
        let mut providers = ProviderRegistry::new();
        providers.register(EchoProvider);
        RuntimeControlPlane::new(
            RuntimeService::new(session_repository, Arc::new(providers)),
            agent_repository,
            orchestration_repository,
            memory_repository,
        )
    }

    fn agent_manifest() -> AgentManifest {
        AgentManifest {
            name: "worker".into(),
            role: AgentRole::Worker,
            capabilities: BTreeSet::new(),
            heartbeat_timeout_seconds: 30,
        }
    }

    fn workflow_definition() -> WorkflowDefinition {
        WorkflowDefinition {
            name: "single-task".into(),
            version: 1,
            nodes: vec![WorkflowNodeDefinition {
                id: "task".into(),
                kind: WorkflowNodeKind::Task,
                dependencies: Vec::new(),
                max_attempts: 1,
                continue_on_failure: false,
                fallback: None,
            }],
        }
    }

    #[tokio::test]
    async fn control_plane_coordinates_session_scoped_operations() {
        let control_plane = control_plane().await;
        let context = OperationContext::system("test");
        let session = control_plane
            .create_session("control-plane", &context)
            .await
            .expect("session");

        let agent = control_plane
            .register_agent(session.id, agent_manifest(), &context)
            .await
            .expect("agent");
        let agent = control_plane
            .start_agent(session.id, agent.id, agent.version, &context)
            .await
            .expect("start agent");
        assert_eq!(agent.status, AgentStatus::Running);

        let workflow = control_plane
            .create_workflow(session.id, workflow_definition(), &context)
            .await
            .expect("workflow");
        let workflow = control_plane
            .start_workflow_node(
                session.id,
                workflow.id,
                workflow.version,
                "task",
                agent.id.to_string(),
                &context,
            )
            .await
            .expect("start workflow node");
        let workflow = control_plane
            .complete_workflow_node(
                session.id,
                workflow.id,
                workflow.version,
                "task",
                &context,
            )
            .await
            .expect("complete workflow node");
        assert_eq!(workflow.status, WorkflowStatus::Succeeded);

        let lease = control_plane
            .acquire_lock(
                &LockRequest {
                    session_id: session.id,
                    owner_id: agent.id.to_string(),
                    resource: LockResource::Workspace {
                        workspace_id: "workspace".into(),
                    },
                    mode: LockMode::Exclusive,
                    ttl_seconds: 30,
                },
                &context,
            )
            .await
            .expect("lock");
        let lease = control_plane
            .heartbeat_lock(
                session.id,
                lease.lock_id,
                &lease.owner_id,
                lease.fencing_token,
                30,
                &context,
            )
            .await
            .expect("heartbeat lock");
        assert_eq!(control_plane.list_locks(session.id, "workspace").await.expect("list locks").len(), 1);
        control_plane
            .release_lock(
                session.id,
                lease.lock_id,
                &lease.owner_id,
                lease.fencing_token,
                &context,
            )
            .await
            .expect("release lock");
        assert!(control_plane
            .list_locks(session.id, "workspace")
            .await
            .expect("list released locks")
            .is_empty());

        let memory = MemoryRecord::new(
            session.id,
            MemoryClass::Decision,
            "Runtime owns durable state",
            MemorySource {
                kind: "test".into(),
                locator: "control-plane".into(),
                revision: None,
            },
            ["runtime".into()],
        )
        .expect("memory record");
        let memory = control_plane
            .remember(memory, &context)
            .await
            .expect("remember");
        let hits = control_plane
            .search_memories(&MemoryQuery {
                session_id: session.id,
                text: "Runtime state".into(),
                classes: BTreeSet::from([MemoryClass::Decision]),
                tags: BTreeSet::new(),
                limit: 10,
            })
            .await
            .expect("search memories");
        assert_eq!(hits.len(), 1);
        control_plane
            .forget_memory(session.id, memory.id, &context)
            .await
            .expect("forget memory");
        let hits = control_plane
            .search_memories(&MemoryQuery {
                session_id: session.id,
                text: "Runtime state".into(),
                classes: BTreeSet::from([MemoryClass::Decision]),
                tags: BTreeSet::new(),
                limit: 10,
            })
            .await
            .expect("search deleted memory");
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn missing_session_blocks_dependent_resources() {
        let control_plane = control_plane().await;
        let context = OperationContext::system("test");
        let result = control_plane
            .register_agent(SessionId::new(), agent_manifest(), &context)
            .await;
        assert!(matches!(result, Err(ControlPlaneError::Runtime(_))));
    }
}
