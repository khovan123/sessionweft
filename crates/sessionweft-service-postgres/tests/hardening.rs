use std::time::{Duration, Instant};

use chrono::Utc;
use sessionweft_core::SessionId;
use sessionweft_service_postgres::{
    PostgresServiceDatabase, PostgresSessionRepository,
};
use sessionweft_storage::{SessionRepository, StorageError};
use uuid::Uuid;

fn postgres_url() -> String {
    std::env::var("SESSIONWEFT_TEST_POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://sessionweft:sessionweft@127.0.0.1:5432/sessionweft".into())
}

#[tokio::test]
#[ignore = "requires PostgreSQL service"]
async fn expired_task_claim_is_recovered_within_failover_slo() {
    let left = PostgresServiceDatabase::connect(&postgres_url(), "hardening-left")
        .await
        .expect("left database");
    let right = PostgresServiceDatabase::connect(&postgres_url(), "hardening-right")
        .await
        .expect("right database");
    let task_id = format!("hardening-task-{}", Uuid::new_v4());
    let first = left
        .claim_task(&task_id, "runtime-left", Duration::from_millis(400))
        .await
        .expect("first claim")
        .expect("claim owner");
    assert!(
        right
            .claim_task(&task_id, "runtime-right", Duration::from_secs(5))
            .await
            .expect("blocked claim")
            .is_none(),
        "a live claim must not be stolen"
    );

    tokio::time::sleep(Duration::from_millis(650)).await;
    let started = Instant::now();
    let recovered = right
        .claim_task(&task_id, "runtime-right", Duration::from_secs(5))
        .await
        .expect("recovery claim")
        .expect("replacement owner");
    assert!(
        started.elapsed() <= Duration::from_secs(5),
        "claim recovery exceeded the five-second RC gate"
    );
    assert_ne!(first.claim_token, recovered.claim_token);
    assert_eq!(recovered.owner_id, "runtime-right");
}

#[tokio::test]
#[ignore = "requires PostgreSQL service"]
async fn malformed_persisted_session_is_rejected_without_process_failure() {
    let database = PostgresServiceDatabase::connect(&postgres_url(), "hardening-corruption")
        .await
        .expect("database");
    let repository = PostgresSessionRepository::new(database.clone());
    let session_id = SessionId::new();
    let now = Utc::now();
    sqlx::query(
        r#"
        INSERT INTO sessionweft_sessions (
            id, version, status, title, data_json, created_at, updated_at
        ) VALUES ($1, 0, 'active', 'corrupt-fixture', $2, $3, $3)
        "#,
    )
    .bind(session_id.to_string())
    .bind(serde_json::json!({"invalid_session_shape": true}))
    .bind(now)
    .execute(database.pool())
    .await
    .expect("insert corrupt fixture");

    let error = repository
        .get(session_id)
        .await
        .expect_err("corrupt JSON shape must be rejected");
    assert!(matches!(error, StorageError::Serialization(_)));

    sqlx::query("DELETE FROM sessionweft_sessions WHERE id = $1")
        .bind(session_id.to_string())
        .execute(database.pool())
        .await
        .expect("cleanup corrupt fixture");
}

#[tokio::test]
#[ignore = "requires PostgreSQL service"]
async fn migrations_are_idempotent_across_runtime_restart() {
    let instance = format!("migration-{}", Uuid::new_v4());
    let first = PostgresServiceDatabase::connect(&postgres_url(), &instance)
        .await
        .expect("first startup");
    first.migrate().await.expect("first migration pass");
    drop(first);

    let restarted = PostgresServiceDatabase::connect(&postgres_url(), &instance)
        .await
        .expect("restart");
    restarted
        .migrate()
        .await
        .expect("second migration pass");
    let required_tables = sqlx::query_scalar::<_, String>(
        r#"
        SELECT table_name
        FROM information_schema.tables
        WHERE table_schema = 'public'
          AND table_name IN (
            'sessionweft_sessions',
            'sessionweft_outbox',
            'sessionweft_inbox',
            'sessionweft_task_claims'
          )
        ORDER BY table_name
        "#,
    )
    .fetch_all(restarted.pool())
    .await
    .expect("table inventory");
    assert_eq!(required_tables.len(), 4);
}
