use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sessionweft_orchestration::{LockMode, LockResource};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredLock {
    pub resource: LockResource,
    pub mode: LockMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimLockFence {
    pub lock_id: Uuid,
    pub resource: LockResource,
    pub mode: LockMode,
    pub fencing_token: u64,
    pub expires_at: DateTime<Utc>,
}
