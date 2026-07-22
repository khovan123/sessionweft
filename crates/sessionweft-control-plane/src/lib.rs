use std::sync::Arc;

use sessionweft_core::{Session, SessionId};
use sessionweft_execution::{
    AgentManifest, AgentRecord, AgentRepository, AgentService, ExecutionError,
};
use sessionweft_knowledge::{
    KnowledgeError, MemoryRecord, MemoryRepository, MemoryService,
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
        LockMode, LockResource, WorkflowNodeDefinition, WorkflowNodeKind,
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
            .create_workflow(
                session.id,
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
                },
                &context,
            )
            .await
            .expect("workflow");
        assert_eq!(workflow.session_id, session.id);

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
        assert_eq!(lease.session_id, session.id);

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
        assert_eq!(memory.session_id, session.id);
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
