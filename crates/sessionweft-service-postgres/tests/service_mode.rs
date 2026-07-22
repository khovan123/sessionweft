use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use sessionweft_core::{EventEnvelope, SessionId};
use sessionweft_jetstream::{
    DurableConsumerConfig, EventHandler, JetStreamConfig, JetStreamEventTransport,
};
use sessionweft_orchestration::{
    LockMode, LockRequest, LockResource, OrchestrationRepository, RepositoryError,
};
use sessionweft_service_postgres::{
    PostgresEventInbox, PostgresOrchestrationRepository, PostgresServiceDatabase,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn postgres_url() -> String {
    std::env::var("SESSIONWEFT_TEST_POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://sessionweft:sessionweft@127.0.0.1:5432/sessionweft".into())
}

#[tokio::test]
#[ignore = "requires PostgreSQL service"]
async fn two_runtimes_cannot_claim_the_same_task_or_conflicting_lock() {
    let left = PostgresServiceDatabase::connect(&postgres_url(), "runtime-a")
        .await
        .expect("left database");
    let right = PostgresServiceDatabase::connect(&postgres_url(), "runtime-b")
        .await
        .expect("right database");
    let task_id = format!("task-{}", Uuid::new_v4());
    let (left_claim, right_claim) = tokio::join!(
        left.claim_task(&task_id, "runtime-a", Duration::from_secs(30)),
        right.claim_task(&task_id, "runtime-b", Duration::from_secs(30)),
    );
    let claimed = [
        left_claim.expect("left claim"),
        right_claim.expect("right claim"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    assert_eq!(claimed.len(), 1, "exactly one Runtime must own a task");

    let workspace_id = format!("workspace-{}", Uuid::new_v4());
    let left_repository = PostgresOrchestrationRepository::new(left);
    let right_repository = PostgresOrchestrationRepository::new(right);
    let request = |owner: &str| LockRequest {
        session_id: SessionId::new(),
        owner_id: owner.into(),
        resource: LockResource::File {
            workspace_id: workspace_id.clone(),
            path: "src/lib.rs".into(),
        },
        mode: LockMode::Exclusive,
        ttl_seconds: 30,
    };
    let left_request = request("runtime-a");
    let right_request = request("runtime-b");
    let correlation = Uuid::new_v4();
    let (left_lock, right_lock) = tokio::join!(
        left_repository.acquire_lock(&left_request, correlation, Some("test")),
        right_repository.acquire_lock(&right_request, correlation, Some("test")),
    );
    let successes = [&left_lock, &right_lock]
        .into_iter()
        .filter(|result| result.is_ok())
        .count();
    assert_eq!(
        successes, 1,
        "exactly one Runtime must own a conflicting lock"
    );
    let failures = [left_lock, right_lock]
        .into_iter()
        .filter_map(Result::err)
        .collect::<Vec<_>>();
    assert!(matches!(
        failures.as_slice(),
        [RepositoryError::LockConflict { .. }]
    ));
}

struct CountingHandler {
    calls: AtomicUsize,
    completed: tokio::sync::Notify,
}

#[async_trait]
impl EventHandler for CountingHandler {
    async fn handle(&self, _event: &EventEnvelope) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.completed.notify_one();
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires PostgreSQL and NATS JetStream services"]
async fn jetstream_redelivery_is_idempotent_across_runtime_restart() {
    let database = PostgresServiceDatabase::connect(&postgres_url(), "runtime-consumer")
        .await
        .expect("database");
    let inbox = Arc::new(PostgresEventInbox::new(database).await.expect("inbox"));
    let suffix = Uuid::new_v4().simple().to_string();
    let transport = JetStreamEventTransport::connect(JetStreamConfig {
        server_url: std::env::var("SESSIONWEFT_TEST_NATS_URL")
            .unwrap_or_else(|_| "nats://127.0.0.1:4222".into()),
        stream_name: format!("SESSIONWEFT_{suffix}"),
        subject_prefix: format!("sessionweft.test.{suffix}"),
        ..Default::default()
    })
    .await
    .expect("transport");
    let handler = Arc::new(CountingHandler {
        calls: AtomicUsize::new(0),
        completed: tokio::sync::Notify::new(),
    });
    let cancellation = CancellationToken::new();
    let consumer = {
        let transport = transport.clone();
        let inbox = Arc::clone(&inbox);
        let handler = Arc::clone(&handler);
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            transport
                .durable_consumer(
                    DurableConsumerConfig {
                        durable_name: format!("consumer-{suffix}"),
                        filter_subject: None,
                        dead_letter_subject: format!("sessionweft.test.{suffix}.dlq"),
                        ack_wait: Duration::from_secs(5),
                        retry_delay: Duration::from_millis(100),
                        max_deliveries: 3,
                    },
                    inbox,
                    handler,
                    cancellation,
                )
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(250)).await;
    let event = EventEnvelope::new(
        "task.completed",
        Some(SessionId::new()),
        Uuid::new_v4(),
        Some("integration-test"),
        serde_json::json!({"task_id": Uuid::new_v4()}),
    );
    transport
        .publish_event(&event)
        .await
        .expect("first publish");
    transport
        .publish_event(&event)
        .await
        .expect("duplicate publish");
    tokio::time::timeout(Duration::from_secs(10), handler.completed.notified())
        .await
        .expect("handler completion");
    tokio::time::sleep(Duration::from_millis(500)).await;
    cancellation.cancel();
    consumer.await.expect("consumer task").expect("consumer");
    assert_eq!(handler.calls.load(Ordering::SeqCst), 1);
}
