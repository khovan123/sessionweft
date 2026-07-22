use sessionweft_service_postgres::PostgresServiceDatabase;
use sqlx::{PgPool, postgres::PgPoolOptions};

fn database_url() -> String {
    std::env::var("SESSIONWEFT_MIGRATION_TEST_POSTGRES_URL").unwrap_or_else(|_| {
        "postgres://sessionweft:sessionweft@127.0.0.1:5432/sessionweft_compat_drill".into()
    })
}

async fn legacy_fixture(pool: &PgPool) {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS hardening_legacy_sentinel (
            id INTEGER PRIMARY KEY,
            value TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await
    .expect("create legacy sentinel");
    sqlx::query(
        r#"
        INSERT INTO hardening_legacy_sentinel (id, value)
        VALUES (1, 'preserve-me')
        ON CONFLICT (id) DO UPDATE SET value = EXCLUDED.value
        "#,
    )
    .execute(pool)
    .await
    .expect("insert legacy sentinel");
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database"]
async fn additive_migrations_preserve_legacy_data_and_are_idempotent() {
    let url = database_url();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("legacy database");
    legacy_fixture(&pool).await;
    pool.close().await;

    let first = PostgresServiceDatabase::connect(&url, "migration-runtime-a")
        .await
        .expect("first migration");
    let required_tables = [
        "sessionweft_sessions",
        "sessionweft_workflows",
        "sessionweft_agents",
        "sessionweft_memories",
        "sessionweft_locks",
        "sessionweft_outbox",
        "sessionweft_inbox",
        "sessionweft_task_claims",
    ];
    for table in required_tables {
        let qualified = format!("public.{table}");
        let exists = sqlx::query_scalar::<_, Option<String>>("SELECT to_regclass($1)::TEXT")
            .bind(qualified)
            .fetch_one(first.pool())
            .await
            .expect("table lookup");
        assert_eq!(exists.as_deref(), Some(table));
    }
    let sentinel =
        sqlx::query_scalar::<_, String>("SELECT value FROM hardening_legacy_sentinel WHERE id = 1")
            .fetch_one(first.pool())
            .await
            .expect("legacy data");
    assert_eq!(sentinel, "preserve-me");
    drop(first);

    let second = PostgresServiceDatabase::connect(&url, "migration-runtime-b")
        .await
        .expect("idempotent migration");
    let sentinel =
        sqlx::query_scalar::<_, String>("SELECT value FROM hardening_legacy_sentinel WHERE id = 1")
            .fetch_one(second.pool())
            .await
            .expect("legacy data after second migration");
    assert_eq!(sentinel, "preserve-me");
}
