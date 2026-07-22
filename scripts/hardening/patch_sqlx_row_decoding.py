from pathlib import Path

path = Path("crates/sessionweft-service-postgres/src/database.rs")
text = path.read_text()

text = text.replace(
    "use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};",
    "use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgPoolOptions};",
    1,
)

claimed_old = '''#[derive(Debug, sqlx::FromRow)]
struct ClaimedOutboxRow {
    event_id: Uuid,
    payload_json: serde_json::Value,
    publish_attempts: i32,
}
'''
claimed_new = '''#[derive(Debug)]
struct ClaimedOutboxRow {
    event_id: Uuid,
    payload_json: serde_json::Value,
    publish_attempts: i32,
}

impl<'row> sqlx::FromRow<'row, sqlx::postgres::PgRow> for ClaimedOutboxRow {
    fn from_row(row: &'row sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            event_id: row.try_get("event_id")?,
            payload_json: row.try_get("payload_json")?,
            publish_attempts: row.try_get("publish_attempts")?,
        })
    }
}
'''
if claimed_old in text:
    text = text.replace(claimed_old, claimed_new, 1)
elif claimed_new not in text:
    raise SystemExit("ClaimedOutboxRow marker not found")

task_old = '''#[derive(Debug, sqlx::FromRow)]
struct TaskClaimRow {
    task_id: String,
    owner_id: String,
    claim_token: Uuid,
    expires_at: DateTime<Utc>,
}
'''
task_new = '''#[derive(Debug)]
struct TaskClaimRow {
    task_id: String,
    owner_id: String,
    claim_token: Uuid,
    expires_at: DateTime<Utc>,
}

impl<'row> sqlx::FromRow<'row, sqlx::postgres::PgRow> for TaskClaimRow {
    fn from_row(row: &'row sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            task_id: row.try_get("task_id")?,
            owner_id: row.try_get("owner_id")?,
            claim_token: row.try_get("claim_token")?,
            expires_at: row.try_get("expires_at")?,
        })
    }
}
'''
if task_old in text:
    text = text.replace(task_old, task_new, 1)
elif task_new not in text:
    raise SystemExit("TaskClaimRow marker not found")

path.write_text(text)
