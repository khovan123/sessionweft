use std::time::{Duration, Instant};

use sessionweft_core::SessionId;
use sessionweft_orchestration::{LockMode, LockRequest, LockResource, OrchestrationRepository};
use sessionweft_service_postgres::{PostgresOrchestrationRepository, PostgresServiceDatabase};
use tokio::task::JoinSet;
use uuid::Uuid;

fn postgres_url() -> String {
    std::env::var("SESSIONWEFT_TEST_POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://sessionweft:sessionweft@127.0.0.1:5432/sessionweft".into())
}

#[tokio::test]
#[ignore = "requires PostgreSQL service"]
async fn concurrent_runtimes_create_one_task_owner_with_bounded_latency() {
    let task_id = format!("contention-task-{}", Uuid::new_v4());
    let started = Instant::now();
    let mut tasks = JoinSet::new();
    for index in 0..64 {
        let url = postgres_url();
        let task_id = task_id.clone();
        tasks.spawn(async move {
            let owner = format!("runtime-contention-{index}");
            let database = PostgresServiceDatabase::connect(&url, &owner)
                .await
                .expect("database");
            database
                .claim_task(&task_id, &owner, Duration::from_secs(30))
                .await
                .expect("claim")
        });
    }
    let mut owners = Vec::new();
    while let Some(result) = tasks.join_next().await {
        if let Some(claim) = result.expect("task") {
            owners.push(claim.owner_id);
        }
    }
    assert_eq!(owners.len(), 1);
    assert!(started.elapsed() <= Duration::from_secs(10));
}

#[tokio::test]
#[ignore = "requires PostgreSQL service"]
async fn concurrent_runtimes_create_one_exclusive_lock_owner_with_bounded_latency() {
    let session_id = SessionId::new();
    let workspace_id = format!("contention-workspace-{}", Uuid::new_v4());
    let correlation_id = Uuid::new_v4();
    let started = Instant::now();
    let mut tasks = JoinSet::new();
    for index in 0..64 {
        let url = postgres_url();
        let workspace_id = workspace_id.clone();
        tasks.spawn(async move {
            let owner = format!("runtime-lock-{index}");
            let database = PostgresServiceDatabase::connect(&url, &owner)
                .await
                .expect("database");
            let repository = PostgresOrchestrationRepository::new(database);
            repository
                .acquire_lock(
                    &LockRequest {
                        session_id,
                        owner_id: owner.clone(),
                        resource: LockResource::File {
                            workspace_id,
                            path: "src/lib.rs".into(),
                        },
                        mode: LockMode::Exclusive,
                        ttl_seconds: 30,
                    },
                    correlation_id,
                    Some("contention-test"),
                )
                .await
                .map(|lease| lease.owner_id)
        });
    }
    let mut owners = Vec::new();
    while let Some(result) = tasks.join_next().await {
        if let Ok(owner) = result.expect("task") {
            owners.push(owner);
        }
    }
    assert_eq!(owners.len(), 1);
    assert!(started.elapsed() <= Duration::from_secs(10));
}
