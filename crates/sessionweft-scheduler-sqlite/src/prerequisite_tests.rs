use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use chrono::Utc;
use sessionweft_core::SessionId;
use sessionweft_execution::{AgentManifest, AgentRole, AgentService, Capability};
use sessionweft_execution_sqlite::SqliteAgentRepository;
use sessionweft_orchestration::{
    LockMode, LockRequest, LockResource, OrchestrationService, WorkflowDefinition,
    WorkflowNodeDefinition, WorkflowNodeKind,
};
use sessionweft_orchestration_sqlite::SqliteOrchestrationRepository;
use sessionweft_scheduler::{
    PollingConfig, RequiredLock, SchedulerPlan, SchedulerPollingService,
    SchedulerPrerequisiteService, SchedulerService, TaskRequirement,
};
use sqlx::Row;
use uuid::Uuid;

use super::*;

async fn repositories() -> (
    Arc<SqliteOrchestrationRepository>,
    Arc<SqliteAgentRepository>,
    Arc<SqliteSchedulerRepository>,
) {
    let path = std::env::temp_dir().join(format!(
        "sessionweft-scheduler-prerequisites-{}.db",
        Uuid::new_v4()
    ));
    let database_url = format!("sqlite://{}", path.display());
    let orchestration = Arc::new(
        SqliteOrchestrationRepository::connect(&database_url)
            .await
            .expect("orchestration repository"),
    );
    let agents = Arc::new(
        SqliteAgentRepository::connect(&database_url)
            .await
            .expect("agent repository"),
    );
    let scheduler = Arc::new(
        SqliteSchedulerRepository::connect(&database_url)
            .await
            .expect("scheduler repository"),
    );
    (orchestration, agents, scheduler)
}

async fn running_worker(
    repository: Arc<SqliteAgentRepository>,
    session_id: SessionId,
) -> sessionweft_execution::AgentRecord {
    let service = AgentService::new(repository);
    let agent = service
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
        .expect("register agent");
    service
        .mutate(agent.id, agent.version, |agent| {
            Ok(vec![agent.start(
                agent.version,
                Uuid::new_v4(),
                Some("test"),
            )?])
        })
        .await
        .expect("start agent")
}

fn task_requirement() -> TaskRequirement {
    TaskRequirement {
        role: Some(AgentRole::Worker),
        capabilities: BTreeSet::from([Capability::WorkspaceWrite]),
    }
}

#[tokio::test]
async fn approval_dependency_blocks_claim_until_granted() {
    let (orchestration_repository, agent_repository, scheduler_repository) = repositories().await;
    let orchestration = OrchestrationService::new(Arc::clone(&orchestration_repository));
    let session_id = SessionId::new();
    let workflow = orchestration
        .create_workflow(
            session_id,
            WorkflowDefinition {
                name: "approval prerequisite".into(),
                version: 1,
                nodes: vec![
                    WorkflowNodeDefinition {
                        id: "review".into(),
                        kind: WorkflowNodeKind::Approval,
                        dependencies: Vec::new(),
                        max_attempts: 1,
                        continue_on_failure: false,
                        fallback: None,
                    },
                    WorkflowNodeDefinition {
                        id: "worker".into(),
                        kind: WorkflowNodeKind::Task,
                        dependencies: vec!["review".into()],
                        max_attempts: 1,
                        continue_on_failure: false,
                        fallback: None,
                    },
                ],
            },
            Uuid::new_v4(),
            Some("test"),
        )
        .await
        .expect("workflow");
    running_worker(agent_repository, session_id).await;
    SchedulerService::new(Arc::clone(&scheduler_repository))
        .register_plan(
            &SchedulerPlan::new(
                &workflow,
                BTreeMap::from([("worker".into(), task_requirement())]),
            )
            .expect("plan"),
        )
        .await
        .expect("register plan");
    let polling = SchedulerPollingService::new(
        Arc::clone(&scheduler_repository),
        PollingConfig { batch_limit: 100 },
    )
    .expect("polling");

    let blocked = polling
        .tick(Utc::now(), Uuid::new_v4(), Some("scheduler"))
        .await
        .expect("blocked tick");
    assert!(!blocked.made_progress());

    orchestration
        .decide_approval(
            workflow.id,
            workflow.version,
            "review",
            true,
            Uuid::new_v4(),
            Some("reviewer"),
        )
        .await
        .expect("approve");
    let claimed = polling
        .tick(Utc::now(), Uuid::new_v4(), Some("scheduler"))
        .await
        .expect("claim tick");
    assert_eq!(claimed.ready_claims_created, 1);
}

#[tokio::test]
async fn required_lock_blocks_claim_and_persists_fencing_snapshot() {
    let (orchestration_repository, agent_repository, scheduler_repository) = repositories().await;
    let orchestration = OrchestrationService::new(Arc::clone(&orchestration_repository));
    let session_id = SessionId::new();
    let workflow = orchestration
        .create_workflow(
            session_id,
            WorkflowDefinition {
                name: "lock prerequisite".into(),
                version: 1,
                nodes: vec![WorkflowNodeDefinition {
                    id: "worker".into(),
                    kind: WorkflowNodeKind::Task,
                    dependencies: Vec::new(),
                    max_attempts: 1,
                    continue_on_failure: false,
                    fallback: None,
                }],
            },
            Uuid::new_v4(),
            Some("test"),
        )
        .await
        .expect("workflow");
    let agent = running_worker(agent_repository, session_id).await;
    let resource = LockResource::File {
        workspace_id: "workspace".into(),
        path: "src/lib.rs".into(),
    };
    let plan = SchedulerPlan::new(
        &workflow,
        BTreeMap::from([("worker".into(), task_requirement())]),
    )
    .expect("plan");
    SchedulerService::new(Arc::clone(&scheduler_repository))
        .register_plan(&plan)
        .await
        .expect("register plan");
    let prerequisites = SchedulerPrerequisiteService::new(Arc::clone(&scheduler_repository));
    prerequisites
        .require_lock(
            &workflow,
            "worker",
            RequiredLock {
                resource: resource.clone(),
                mode: LockMode::Exclusive,
            },
        )
        .await
        .expect("lock requirement");
    let polling = SchedulerPollingService::new(
        Arc::clone(&scheduler_repository),
        PollingConfig { batch_limit: 100 },
    )
    .expect("polling");

    let blocked = polling
        .tick(Utc::now(), Uuid::new_v4(), Some("scheduler"))
        .await
        .expect("blocked tick");
    assert!(!blocked.made_progress());

    let lease = orchestration
        .acquire_lock(
            &LockRequest {
                session_id,
                owner_id: agent.id.to_string(),
                resource,
                mode: LockMode::Exclusive,
                ttl_seconds: 60,
            },
            Uuid::new_v4(),
            Some("agent"),
        )
        .await
        .expect("lock lease");
    let claimed = polling
        .tick(Utc::now(), Uuid::new_v4(), Some("scheduler"))
        .await
        .expect("claim tick");
    assert_eq!(claimed.ready_claims_created, 1);

    let row = sqlx::query("SELECT claim_id FROM scheduler_claims WHERE status = 'active'")
        .fetch_one(&scheduler_repository.pool)
        .await
        .expect("active claim");
    let claim_id = Uuid::parse_str(row.get::<&str, _>("claim_id")).expect("claim ID");
    let snapshot = prerequisites
        .claim_lock_fence(claim_id)
        .await
        .expect("fence lookup")
        .expect("claim fence snapshot");
    assert_eq!(snapshot.fence.lock_id, lease.lock_id);
    assert_eq!(snapshot.fence.fencing_token, lease.fencing_token);
    assert_eq!(snapshot.agent_id, agent.id);
}
