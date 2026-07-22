use std::{collections::BTreeSet, fmt, str::FromStr, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantId(Uuid);

impl TenantId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for TenantId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for TenantId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PrincipalId(String);

impl PrincipalId {
    pub fn parse(value: impl Into<String>) -> Result<Self, TenancyError> {
        let value = value.into().trim().to_owned();
        if value.is_empty() || value.len() > 256 {
            return Err(TenancyError::Validation(
                "principal ID must be between 1 and 256 bytes".into(),
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PrincipalId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantStatus {
    Active,
    Suspended,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantRole {
    Owner,
    Admin,
    Billing,
    Member,
    Viewer,
}

impl TenantRole {
    #[must_use]
    pub const fn can_manage_members(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }

    #[must_use]
    pub const fn can_manage_billing(self) -> bool {
        matches!(self, Self::Owner | Self::Admin | Self::Billing)
    }

    #[must_use]
    pub const fn can_mutate_runtime(self) -> bool {
        matches!(self, Self::Owner | Self::Admin | Self::Member)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tenant {
    pub id: TenantId,
    pub slug: String,
    pub display_name: String,
    pub status: TenantStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Tenant {
    pub fn new(slug: impl Into<String>, display_name: impl Into<String>) -> Result<Self, TenancyError> {
        let slug = normalize_slug(slug.into())?;
        let display_name = display_name.into().trim().to_owned();
        if display_name.is_empty() || display_name.len() > 256 {
            return Err(TenancyError::Validation(
                "tenant display name must be between 1 and 256 bytes".into(),
            ));
        }
        let now = Utc::now();
        Ok(Self {
            id: TenantId::new(),
            slug,
            display_name,
            status: TenantStatus::Active,
            created_at: now,
            updated_at: now,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    pub tenant_id: TenantId,
    pub principal_id: PrincipalId,
    pub roles: BTreeSet<TenantRole>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Membership {
    pub fn new(
        tenant_id: TenantId,
        principal_id: PrincipalId,
        roles: impl IntoIterator<Item = TenantRole>,
    ) -> Result<Self, TenancyError> {
        let roles = roles.into_iter().collect::<BTreeSet<_>>();
        if roles.is_empty() {
            return Err(TenancyError::Validation(
                "membership must contain at least one role".into(),
            ));
        }
        let now = Utc::now();
        Ok(Self {
            tenant_id,
            principal_id,
            roles,
            created_at: now,
            updated_at: now,
        })
    }

    #[must_use]
    pub fn has_role(&self, role: TenantRole) -> bool {
        self.roles.contains(&role)
    }

    #[must_use]
    pub fn can_manage_members(&self) -> bool {
        self.roles.iter().copied().any(TenantRole::can_manage_members)
    }

    #[must_use]
    pub fn can_manage_billing(&self) -> bool {
        self.roles.iter().copied().any(TenantRole::can_manage_billing)
    }

    #[must_use]
    pub fn can_mutate_runtime(&self) -> bool {
        self.roles.iter().copied().any(TenantRole::can_mutate_runtime)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaDimension {
    Sessions,
    ActiveAgents,
    QueuedTasks,
    IndexedFiles,
    EventBacklog,
    ProviderTokens,
    ToolInvocations,
    StorageBytes,
}

impl fmt::Display for QuotaDimension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Sessions => "sessions",
            Self::ActiveAgents => "active_agents",
            Self::QueuedTasks => "queued_tasks",
            Self::IndexedFiles => "indexed_files",
            Self::EventBacklog => "event_backlog",
            Self::ProviderTokens => "provider_tokens",
            Self::ToolInvocations => "tool_invocations",
            Self::StorageBytes => "storage_bytes",
        };
        formatter.write_str(value)
    }
}

impl FromStr for QuotaDimension {
    type Err = TenancyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "sessions" => Ok(Self::Sessions),
            "active_agents" => Ok(Self::ActiveAgents),
            "queued_tasks" => Ok(Self::QueuedTasks),
            "indexed_files" => Ok(Self::IndexedFiles),
            "event_backlog" => Ok(Self::EventBacklog),
            "provider_tokens" => Ok(Self::ProviderTokens),
            "tool_invocations" => Ok(Self::ToolInvocations),
            "storage_bytes" => Ok(Self::StorageBytes),
            _ => Err(TenancyError::Validation(format!(
                "unknown quota dimension '{value}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantQuota {
    pub tenant_id: TenantId,
    pub dimension: QuotaDimension,
    pub hard_limit: u64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Session,
    Workflow,
    Agent,
    Memory,
    Workspace,
    GitWorktree,
    Plugin,
    ProviderCredential,
    BillingAccount,
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Session => "session",
            Self::Workflow => "workflow",
            Self::Agent => "agent",
            Self::Memory => "memory",
            Self::Workspace => "workspace",
            Self::GitWorktree => "git_worktree",
            Self::Plugin => "plugin",
            Self::ProviderCredential => "provider_credential",
            Self::BillingAccount => "billing_account",
        };
        formatter.write_str(value)
    }
}

impl FromStr for ResourceKind {
    type Err = TenancyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "session" => Ok(Self::Session),
            "workflow" => Ok(Self::Workflow),
            "agent" => Ok(Self::Agent),
            "memory" => Ok(Self::Memory),
            "workspace" => Ok(Self::Workspace),
            "git_worktree" => Ok(Self::GitWorktree),
            "plugin" => Ok(Self::Plugin),
            "provider_credential" => Ok(Self::ProviderCredential),
            "billing_account" => Ok(Self::BillingAccount),
            _ => Err(TenancyError::Validation(format!(
                "unknown resource kind '{value}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantContext {
    pub tenant_id: TenantId,
    pub principal_id: PrincipalId,
    pub roles: BTreeSet<TenantRole>,
    pub correlation_id: Uuid,
}

impl TenantContext {
    #[must_use]
    pub fn from_membership(membership: &Membership, correlation_id: Uuid) -> Self {
        Self {
            tenant_id: membership.tenant_id,
            principal_id: membership.principal_id.clone(),
            roles: membership.roles.clone(),
            correlation_id,
        }
    }

    #[must_use]
    pub fn can_manage_members(&self) -> bool {
        self.roles.iter().copied().any(TenantRole::can_manage_members)
    }

    #[must_use]
    pub fn can_manage_billing(&self) -> bool {
        self.roles.iter().copied().any(TenantRole::can_manage_billing)
    }

    #[must_use]
    pub fn can_mutate_runtime(&self) -> bool {
        self.roles.iter().copied().any(TenantRole::can_mutate_runtime)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaReservation {
    pub tenant_id: TenantId,
    pub dimension: QuotaDimension,
    pub amount: u64,
    pub idempotency_key: String,
    pub used_after: u64,
    pub hard_limit: u64,
}

#[async_trait]
pub trait TenantRepository: Send + Sync {
    async fn create_tenant(
        &self,
        tenant: &Tenant,
        owner: &Membership,
    ) -> Result<Tenant, TenancyError>;
    async fn get_tenant(&self, tenant_id: TenantId) -> Result<Option<Tenant>, TenancyError>;
    async fn upsert_membership(&self, membership: &Membership) -> Result<(), TenancyError>;
    async fn membership(
        &self,
        tenant_id: TenantId,
        principal_id: &PrincipalId,
    ) -> Result<Option<Membership>, TenancyError>;
    async fn set_quota(&self, quota: &TenantQuota) -> Result<(), TenancyError>;
    async fn quota(
        &self,
        tenant_id: TenantId,
        dimension: QuotaDimension,
    ) -> Result<Option<TenantQuota>, TenancyError>;
    async fn bind_resource(
        &self,
        tenant_id: TenantId,
        kind: ResourceKind,
        resource_id: &str,
    ) -> Result<(), TenancyError>;
    async fn owns_resource(
        &self,
        tenant_id: TenantId,
        kind: ResourceKind,
        resource_id: &str,
    ) -> Result<bool, TenancyError>;
    async fn reserve_quota(
        &self,
        tenant_id: TenantId,
        dimension: QuotaDimension,
        amount: u64,
        idempotency_key: &str,
    ) -> Result<QuotaReservation, TenancyError>;
}

pub struct TenantService<R>
where
    R: TenantRepository,
{
    repository: Arc<R>,
}

impl<R> TenantService<R>
where
    R: TenantRepository,
{
    #[must_use]
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    pub async fn bootstrap(
        &self,
        slug: impl Into<String>,
        display_name: impl Into<String>,
        owner_principal: PrincipalId,
    ) -> Result<(Tenant, Membership), TenancyError> {
        let tenant = Tenant::new(slug, display_name)?;
        let owner = Membership::new(tenant.id, owner_principal, [TenantRole::Owner])?;
        let tenant = self.repository.create_tenant(&tenant, &owner).await?;
        Ok((tenant, owner))
    }

    pub async fn context(
        &self,
        tenant_id: TenantId,
        principal_id: &PrincipalId,
        correlation_id: Uuid,
    ) -> Result<TenantContext, TenancyError> {
        let tenant = self
            .repository
            .get_tenant(tenant_id)
            .await?
            .ok_or(TenancyError::TenantNotFound(tenant_id))?;
        if tenant.status != TenantStatus::Active {
            return Err(TenancyError::TenantInactive(tenant_id));
        }
        let membership = self
            .repository
            .membership(tenant_id, principal_id)
            .await?
            .ok_or_else(|| TenancyError::AccessDenied {
                tenant_id,
                principal_id: principal_id.clone(),
            })?;
        Ok(TenantContext::from_membership(&membership, correlation_id))
    }

    pub async fn require_resource(
        &self,
        context: &TenantContext,
        kind: ResourceKind,
        resource_id: &str,
    ) -> Result<(), TenancyError> {
        if self
            .repository
            .owns_resource(context.tenant_id, kind, resource_id)
            .await?
        {
            Ok(())
        } else {
            Err(TenancyError::ResourceNotFound {
                tenant_id: context.tenant_id,
                kind,
                resource_id: resource_id.to_owned(),
            })
        }
    }

    pub async fn reserve(
        &self,
        context: &TenantContext,
        dimension: QuotaDimension,
        amount: u64,
        idempotency_key: &str,
    ) -> Result<QuotaReservation, TenancyError> {
        if !context.can_mutate_runtime() {
            return Err(TenancyError::AccessDenied {
                tenant_id: context.tenant_id,
                principal_id: context.principal_id.clone(),
            });
        }
        validate_idempotency_key(idempotency_key)?;
        self.repository
            .reserve_quota(context.tenant_id, dimension, amount, idempotency_key)
            .await
    }
}

fn normalize_slug(value: String) -> Result<String, TenancyError> {
    let value = value.trim().to_ascii_lowercase();
    if value.len() < 3 || value.len() > 63 {
        return Err(TenancyError::Validation(
            "tenant slug must be between 3 and 63 bytes".into(),
        ));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || value.starts_with('-')
        || value.ends_with('-')
        || value.contains("--")
    {
        return Err(TenancyError::Validation(
            "tenant slug must use lowercase letters, numbers and single internal hyphens".into(),
        ));
    }
    Ok(value)
}

fn validate_idempotency_key(value: &str) -> Result<(), TenancyError> {
    if value.trim().is_empty() || value.len() > 255 {
        return Err(TenancyError::Validation(
            "idempotency key must be between 1 and 255 bytes".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum TenancyError {
    #[error("tenancy validation failed: {0}")]
    Validation(String),
    #[error("tenant {0} was not found")]
    TenantNotFound(TenantId),
    #[error("tenant {0} is not active")]
    TenantInactive(TenantId),
    #[error("principal {principal_id} cannot access tenant {tenant_id}")]
    AccessDenied {
        tenant_id: TenantId,
        principal_id: PrincipalId,
    },
    #[error("resource {kind}/{resource_id} was not found in tenant {tenant_id}")]
    ResourceNotFound {
        tenant_id: TenantId,
        kind: ResourceKind,
        resource_id: String,
    },
    #[error(
        "tenant {tenant_id} quota {dimension} exceeded: requested {requested}, used {used}, limit {limit}"
    )]
    QuotaExceeded {
        tenant_id: TenantId,
        dimension: QuotaDimension,
        requested: u64,
        used: u64,
        limit: u64,
    },
    #[error("tenancy repository conflict: {0}")]
    Conflict(String),
    #[error("tenancy repository failure: {0}")]
    Repository(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_slug_is_canonical() {
        assert_eq!(Tenant::new("Acme-01", "Acme").expect("tenant").slug, "acme-01");
        assert!(Tenant::new("-acme", "Acme").is_err());
        assert!(Tenant::new("acme--west", "Acme").is_err());
    }

    #[test]
    fn roles_are_least_privilege() {
        let tenant_id = TenantId::new();
        let viewer = Membership::new(
            tenant_id,
            PrincipalId::parse("viewer@example.com").expect("principal"),
            [TenantRole::Viewer],
        )
        .expect("membership");
        assert!(!viewer.can_mutate_runtime());
        assert!(!viewer.can_manage_billing());

        let billing = Membership::new(
            tenant_id,
            PrincipalId::parse("billing@example.com").expect("principal"),
            [TenantRole::Billing],
        )
        .expect("membership");
        assert!(billing.can_manage_billing());
        assert!(!billing.can_mutate_runtime());
    }
}
