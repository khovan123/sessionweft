use chrono::{DateTime, Utc};
use sessionweft_core::SessionId;
use sessionweft_orchestration::{LockLease, LockMode};
use sessionweft_scheduler::{ClaimLockFence, RepositoryError, RequiredLock};
use sqlx::{Row, Sqlite, Transaction};
use uuid::Uuid;

use super::backend;

pub(super) async fn lock_fence_for(
    transaction: &mut Transaction<'_, Sqlite>,
    session_id: SessionId,
    agent_id: Uuid,
    required: &RequiredLock,
    now: DateTime<Utc>,
) -> Result<Option<ClaimLockFence>, RepositoryError> {
    let rows = sqlx::query(
        r#"
        SELECT data_json
        FROM lock_leases
        WHERE session_id = ? AND owner_id = ? AND expires_at > ?
        ORDER BY fencing_token DESC
        "#,
    )
    .bind(session_id.to_string())
    .bind(agent_id.to_string())
    .bind(now.to_rfc3339())
    .fetch_all(&mut **transaction)
    .await
    .map_err(backend)?;

    for row in rows {
        let lease = serde_json::from_str::<LockLease>(row.get::<&str, _>("data_json"))
            .map_err(backend)?;
        let mode_matches = match required.mode {
            LockMode::Shared => true,
            LockMode::Exclusive => lease.mode == LockMode::Exclusive,
        };
        if mode_matches && lease.resource.overlaps(&required.resource) {
            return Ok(Some(ClaimLockFence {
                lock_id: lease.lock_id,
                resource: lease.resource,
                mode: lease.mode,
                fencing_token: lease.fencing_token,
                expires_at: lease.expires_at,
            }));
        }
    }
    Ok(None)
}
