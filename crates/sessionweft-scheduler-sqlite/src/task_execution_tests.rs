use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use chrono::{Duration, Utc};
use sessionweft_core::{MessageRole, ProviderMessage, SessionId};
use sessionweft_execution::{
    AgentManifest, AgentRole, AgentService, Capability, Permission, RiskLevel, ToolDescriptor,
};
use sessionweft_execution_sqlite::SqliteAgentRepository;
use sessionweft_orchestration::{
    LockMode, LockRequest, LockResource, OrchestrationService, WorkflowDefinition,
    WorkflowNodeDefinition, WorkflowNodeKind,
};
use sessionweft_orchestration_sqlite::SqliteOrchestrationRepository;
use sessionweft_scheduler::{
    ClaimRequest, RequiredLock, SchedulerPlan, SchedulerPrerequisiteRepository,
    SchedulerPrerequisiteService, SchedulerService, TaskAction, TaskExecutionRepository,
    TaskExecutionService, TaskExecutionSpec, TaskExecutionStatus, TaskRequirement,
    ToolExecutionApproval,
};
use uuid::Uuid;

use super::*;

struct Fixture {
    orchestration: OrchestrationService<SqliteOrchestrationRepository>,
    agents: AgentService<SqliteAgentRepository>,
    scheduler: Arc<SqliteSchedulerRepository>,
    session_id: SessionId,
    workflow: sessionweft_orchestration::WorkflowExecution,
}

async fn fixture(capabilities: BTreeSet<Capability>) -> Fixture {
    let path = std::env::temp_dir().join(format!(
        "sessionweft-task-execution-{}.db",
        Uuid::new_v4()
    ));
    let database_url = format!("sqlite://{}", path.display());
    let orchestration_repository = Arc::new(
        SqliteOrchestrationRepository::connect(&database_url)
            .await
            .expect("orchestration repository"),
    );
    let agent_repository = Arc::new(
        SqliteAgentRepository::connect(&database_url)
            .await
            .expect("agent repository"),
    );
    let scheduler = Arc::new(
        SqliteSchedulerRepository::connect(&database_url)
            .await
            .expect("scheduler repository"),
    );
    let session_id = SessionId::new();
    let orchestration = OrchestrationService::new(orchestration_repository);
    let workflow = orchestration
        .create_workflow(
            session_id,
            WorkflowDefinition {
                name: "execution".into(),
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
    let agents = AgentService::new(agent_repository);
    let agent = agents
        .register(
            session_id,
            AgentManifest {
                name: "worker".into(),
                role: AgentRole::Worker,
                capabilities,
                heartbeat_timeout_seconds: 30,
            },
            Uuid::new_v4(),
            Some("test"),
        )
        .await
        .expect("agent");
    agents
        .mutate(agent.id, agent.version, |agent| {
            Ok(vec![agent.start(
                agent.version,
                Uuid::new_v4(),
                Some("test"),
            )?])
        })
        .await
        .expect("start agent");
    SchedulerService::new(Arc::clone(&scheduler))
        .register_plan(
            &SchedulerPlan::new(
                &workflow,
                BTreeMap::from([(
                    "worker".into(),
                    TaskRequirement {
                        role: Some(AgentRole::Worker),
                        capabilities: BTreeSet::new(),
                    },
                )]),
            )
            .expect("plan"),
        )
        .await
        .expect("register plan");
    Fixture {
        orchestration,
        agents,
        scheduler,
        session_id,
        workflow,
    }
}

async fn claim(fixture: &Fixture) -> sessionweft_scheduler::ClaimState {
    let agent = fixture
        .agents
        .get(
            fixture
                .scheduler
                .available_agent_ids(fixture.session_id, Utc::now(), 1)
                .await
                .expect("agent IDs")[0],
        )
        .await
        .expect("agent");
    SchedulerService::new(Arc::clone(&fixture.scheduler))
        .claim_next(&ClaimRequest {
            workflow_id: fixture.workflow.id,
            agent_id: agent.id,
            correlation_id: Uuid::new_v4(),
            actor_id: Some("scheduler".into()),
        })
        .await
        .expect("claim")
        .expect("ready claim")
}

#[tokio::test]
async fn provider_execution_is_prepared_once_and_finishes_durably() {
    let fixture = fixture(BTreeSet::from([Capability::Provider])).await;
    let claimed = claim(&fixture).await;
    let service = TaskExecutionService::new(Arc::clone(&fixture.scheduler));
    service
        .register_spec(
            &TaskExecutionSpec::new(
                &fixture.workflow,
                "worker",
                TaskAction::Provider {
                    provider: "echo".into(),
                    model: "test".into(),
                    messages: vec![ProviderMessage {
                        role: MessageRole::User,
                        content: "hello".into(),
                    }],
                },
            )
            .expect("spec"),
        )
        .await
        .expect("register spec");

    let prepared = service
        .prepare_claim(
            claimed.claim.id,
            Utc::now(),
            Uuid::new_v4(),
            Some("execution-worker"),
        )
        .await
        .expect("prepare")
        .expect("execution spec");
    let replay = service
        .prepare_claim(
            claimed.claim.id,
            Utc::now(),
            Uuid::new_v4(),
            Some("execution-worker"),
        )
        .await
        .expect("prepare replay")
        .expect("existing execution");
    assert_eq!(prepared.id, replay.id);
    assert_eq!(prepared.status, TaskExecutionStatus::Prepared);

    fixture
        .scheduler
        .mark_execution_running(
            prepared.id,
            Utc::now(),
            Uuid::new_v4(),
            Some("execution-worker"),
        )
        .await
        .expect("running");
    let succeeded = fixture
        .scheduler
        .succeed_execution(
            prepared.id,
            serde_json::json!({"text": "done"}),
            Utc::now(),
            Uuid::new_v4(),
            Some("execution-worker"),
        )
        .await
        .expect("succeed");
    assert_eq!(succeeded.status, TaskExecutionStatus::Succeeded);
    assert_eq!(
        fixture
            .scheduler
            .succeeded_unfinalized_executions(100)
            .await
            .expect("unfinalized")
            .len(),
        1
    );
    fixture
        .scheduler
        .mark_claim_finalized(
            prepared.id,
            Uuid::new_v4(),
            Some("execution-worker"),
        )
        .await
        .expect("finalize marker");
    assert!(fixture
        .scheduler
        .succeeded_unfinalized_executions(100)
        .await
        .expect("finalized list")
        .is_empty());
}

#[tokio::test]
async fn high_risk_tool_requires_claim_scoped_approval() {
    let fixture = fixture(BTreeSet::from([Capability::Tool("echo".into())])).await;
    let claimed = claim(&fixture).await;
    let service = TaskExecutionService::new(Arc::clone(&fixture.scheduler));
    let descriptor = ToolDescriptor {
        name: "echo".into(),
        version: "1".into(),
        permissions: BTreeSet::from([Permission::Tool("echo".into())]),
        risk: RiskLevel::High,
        input_schema: serde_json::json!({"type": "object"}),
    };
    service
        .register_spec(
            &TaskExecutionSpec::new(
                &fixture.workflow,
                "worker",
                TaskAction::Tool {
                    descriptor,
                    input: serde_json::json!({"value": 1}),
                },
            )
            .expect("spec"),
        )
        .await
        .expect("register spec");
    assert!(service
        .prepare_claim(
            claimed.claim.id,
            Utc::now(),
            Uuid::new_v4(),
            Some("execution-worker"),
        )
        .await
        .expect("blocked prepare")
        .is_none());

    service
        .grant_tool_approval(
            &ToolExecutionApproval::new(
                claimed.claim.id,
                claimed.claim.session_id,
                claimed.claim.agent_id,
                "echo",
                Utc::now() + Duration::minutes(5),
            )
            .expect("approval"),
        )
        .await
        .expect("grant approval");
    assert!(service
        .prepare_claim(
            claimed.claim.id,
            Utc::now(),
            Uuid::new_v4(),
            Some("execution-worker"),
        )
        .await
        .expect("approved prepare")
        .is_some());
}

#[tokio::test]
async fn released_lock_blocks_execution_time_prepare() {
    let fixture = fixture(BTreeSet::from([
        Capability::Provider,
        Capability::WorkspaceWrite,
    ]))
    .await;
    let agent_id = fixture
        .scheduler
        .available_agent_ids(fixture.session_id, Utc::now(), 1)
        .await
        .expect("agent IDs")[0];
    let resource = LockResource::File {
        workspace_id: "workspace".into(),
        path: "src/lib.rs".into(),
    };
    SchedulerPrerequisiteService::new(Arc::clone(&fixture.scheduler))
        .require_lock(
            &fixture.workflow,
            "worker",
            RequiredLock {
                resource: resource.clone(),
                mode: LockMode::Exclusive,
            },
        )
        .await
        .expect("lock requirement");
    let lease = fixture
        .orchestration
        .acquire_lock(
            &LockRequest {
                session_id: fixture.session_id,
                owner_id: agent_id.to_string(),
                resource,
                mode: LockMode::Exclusive,
                ttl_seconds: 60,
            },
            Uuid::new_v4(),
            Some("agent"),
        )
        .await
        .expect("lease");
    let claimed = fixture
        .scheduler
        .claim_next_guarded(
            &ClaimRequest {
                workflow_id: fixture.workflow.id,
                agent_id,
                correlation_id: Uuid::new_v4(),
                actor_id: Some("scheduler".into()),
            },
            Utc::now(),
        )
        .await
        .expect("guarded claim")
        .expect("claim");
    TaskExecutionService::new(Arc::clone(&fixture.scheduler))
        .register_spec(
            &TaskExecutionSpec::new(
                &fixture.workflow,
                "worker",
                TaskAction::Provider {
                    provider: "echo".into(),
                    model: "test".into(),
                    messages: vec![ProviderMessage {
                        role: MessageRole::User,
                        content: "hello".into(),
                    }],
                },
            )
            .expect("spec"),
        )
        .await
        .expect("register spec");
    fixture
        .orchestration
        .release_lock(
            lease.lock_id,
            &lease.owner_id,
            lease.fencing_token,
            Uuid::new_v4(),
            Some("agent"),
        )
        .await
        .expect("release lease");
    let error = fixture
        .scheduler
        .prepare_claim_execution(
            claimed.claim.id,
            Utc::now(),
            Uuid::new_v4(),
            Some("execution-worker"),
        )
        .await
        .expect_err("released lock must block execution");
    assert!(matches!(error, sessionweft_scheduler::RepositoryError::Conflict(_)));
}
