use std::{collections::BTreeMap, fmt, str::FromStr, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sessionweft_tenancy::{TenantContext, TenantId};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlanId(String);

impl PlanId {
    pub fn parse(value: impl Into<String>) -> Result<Self, BillingError> {
        let value = value.into().trim().to_ascii_lowercase();
        if value.is_empty()
            || value.len() > 128
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.')
            })
        {
            return Err(BillingError::Validation(
                "plan ID must contain 1-128 lowercase letters, numbers, dots, hyphens or underscores"
                    .into(),
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PlanId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MeterName(String);

impl MeterName {
    pub fn parse(value: impl Into<String>) -> Result<Self, BillingError> {
        let value = value.into().trim().to_ascii_lowercase();
        if value.is_empty()
            || value.len() > 128
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.')
            })
        {
            return Err(BillingError::Validation(
                "meter name must contain 1-128 lowercase letters, numbers, dots, hyphens or underscores"
                    .into(),
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MeterName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Money {
    pub currency: String,
    pub minor_units: i64,
}

impl Money {
    pub fn new(currency: impl Into<String>, minor_units: i64) -> Result<Self, BillingError> {
        let currency = currency.into().trim().to_ascii_uppercase();
        if currency.len() != 3 || !currency.bytes().all(|byte| byte.is_ascii_uppercase()) {
            return Err(BillingError::Validation(
                "currency must be a three-letter ISO-style code".into(),
            ));
        }
        if minor_units < 0 {
            return Err(BillingError::Validation(
                "money amount cannot be negative".into(),
            ));
        }
        Ok(Self {
            currency,
            minor_units,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeterDefinition {
    pub name: MeterName,
    pub unit_name: String,
    pub unit_price: Money,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingPlan {
    pub id: PlanId,
    pub display_name: String,
    pub base_price: Money,
    pub interval: BillingInterval,
    pub entitlements: BTreeMap<String, u64>,
    pub meters: BTreeMap<MeterName, MeterDefinition>,
    pub active: bool,
}

impl BillingPlan {
    pub fn validate(&self) -> Result<(), BillingError> {
        if self.display_name.trim().is_empty() || self.display_name.len() > 256 {
            return Err(BillingError::Validation(
                "plan display name must be between 1 and 256 bytes".into(),
            ));
        }
        for (name, definition) in &self.meters {
            if name != &definition.name {
                return Err(BillingError::Validation(format!(
                    "meter map key '{name}' does not match definition '{}'",
                    definition.name
                )));
            }
            if definition.unit_name.trim().is_empty() || definition.unit_name.len() > 64 {
                return Err(BillingError::Validation(format!(
                    "meter '{name}' unit name must be between 1 and 64 bytes"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingInterval {
    Monthly,
    Annual,
}

impl fmt::Display for BillingInterval {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Monthly => "monthly",
            Self::Annual => "annual",
        })
    }
}

impl FromStr for BillingInterval {
    type Err = BillingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "monthly" => Ok(Self::Monthly),
            "annual" => Ok(Self::Annual),
            _ => Err(BillingError::Validation(format!(
                "unknown billing interval '{value}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionStatus {
    Pending,
    Trialing,
    Active,
    PastDue,
    Paused,
    Cancelled,
}

impl fmt::Display for SubscriptionStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Pending => "pending",
            Self::Trialing => "trialing",
            Self::Active => "active",
            Self::PastDue => "past_due",
            Self::Paused => "paused",
            Self::Cancelled => "cancelled",
        })
    }
}

impl FromStr for SubscriptionStatus {
    type Err = BillingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "trialing" => Ok(Self::Trialing),
            "active" => Ok(Self::Active),
            "past_due" => Ok(Self::PastDue),
            "paused" => Ok(Self::Paused),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(BillingError::Validation(format!(
                "unknown subscription status '{value}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub id: Uuid,
    pub tenant_id: TenantId,
    pub plan_id: PlanId,
    pub provider: String,
    pub provider_customer_id: Option<String>,
    pub provider_subscription_id: Option<String>,
    pub status: SubscriptionStatus,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Subscription {
    pub fn pending(
        tenant_id: TenantId,
        plan_id: PlanId,
        provider: impl Into<String>,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
    ) -> Result<Self, BillingError> {
        if period_end <= period_start {
            return Err(BillingError::Validation(
                "subscription period end must be after period start".into(),
            ));
        }
        let provider = provider.into().trim().to_owned();
        if provider.is_empty() || provider.len() > 64 {
            return Err(BillingError::Validation(
                "billing provider name must be between 1 and 64 bytes".into(),
            ));
        }
        let now = Utc::now();
        Ok(Self {
            id: Uuid::new_v4(),
            tenant_id,
            plan_id,
            provider,
            provider_customer_id: None,
            provider_subscription_id: None,
            status: SubscriptionStatus::Pending,
            period_start,
            period_end,
            version: 0,
            created_at: now,
            updated_at: now,
        })
    }

    #[must_use]
    pub const fn grants_entitlements(&self) -> bool {
        matches!(
            self.status,
            SubscriptionStatus::Trialing | SubscriptionStatus::Active
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageState {
    Prepared,
    Reporting,
    Reported,
    Failed,
    Uncertain,
}

impl fmt::Display for UsageState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Prepared => "prepared",
            Self::Reporting => "reporting",
            Self::Reported => "reported",
            Self::Failed => "failed",
            Self::Uncertain => "uncertain",
        })
    }
}

impl FromStr for UsageState {
    type Err = BillingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "reporting" => Ok(Self::Reporting),
            "reported" => Ok(Self::Reported),
            "failed" => Ok(Self::Failed),
            "uncertain" => Ok(Self::Uncertain),
            _ => Err(BillingError::Validation(format!(
                "unknown usage state '{value}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub id: Uuid,
    pub tenant_id: TenantId,
    pub subscription_id: Uuid,
    pub meter: MeterName,
    pub quantity: u64,
    pub idempotency_key: String,
    pub occurred_at: DateTime<Utc>,
    pub state: UsageState,
    pub provider_event_id: Option<String>,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl UsageRecord {
    pub fn new(
        tenant_id: TenantId,
        subscription_id: Uuid,
        meter: MeterName,
        quantity: u64,
        idempotency_key: impl Into<String>,
        occurred_at: DateTime<Utc>,
    ) -> Result<Self, BillingError> {
        if quantity == 0 {
            return Err(BillingError::Validation(
                "usage quantity must be greater than zero".into(),
            ));
        }
        let idempotency_key = idempotency_key.into().trim().to_owned();
        if idempotency_key.is_empty() || idempotency_key.len() > 255 {
            return Err(BillingError::Validation(
                "usage idempotency key must be between 1 and 255 bytes".into(),
            ));
        }
        let now = Utc::now();
        Ok(Self {
            id: Uuid::new_v4(),
            tenant_id,
            subscription_id,
            meter,
            quantity,
            idempotency_key,
            occurred_at,
            state: UsageState::Prepared,
            provider_event_id: None,
            attempts: 0,
            last_error: None,
            created_at: now,
            updated_at: now,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSubscription {
    pub customer_id: String,
    pub subscription_id: String,
    pub status: SubscriptionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderUsageReceipt {
    pub provider_event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderWebhookEvent {
    pub provider: String,
    pub event_id: String,
    pub event_type: String,
    pub tenant_id: TenantId,
    pub payload: serde_json::Value,
    pub occurred_at: DateTime<Utc>,
}

#[async_trait]
pub trait BillingProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn create_subscription(
        &self,
        tenant_id: TenantId,
        plan: &BillingPlan,
        idempotency_key: &str,
    ) -> Result<ProviderSubscription, BillingError>;

    async fn report_usage(
        &self,
        subscription: &Subscription,
        usage: &UsageRecord,
    ) -> Result<ProviderUsageReceipt, BillingError>;
}

#[async_trait]
pub trait BillingRepository: Send + Sync {
    async fn upsert_plan(&self, plan: &BillingPlan) -> Result<(), BillingError>;
    async fn plan(&self, plan_id: &PlanId) -> Result<Option<BillingPlan>, BillingError>;
    async fn create_subscription(
        &self,
        subscription: &Subscription,
    ) -> Result<Subscription, BillingError>;
    async fn subscription(
        &self,
        tenant_id: TenantId,
        subscription_id: Uuid,
    ) -> Result<Option<Subscription>, BillingError>;
    async fn active_subscription(
        &self,
        tenant_id: TenantId,
    ) -> Result<Option<Subscription>, BillingError>;
    async fn save_subscription(
        &self,
        expected_version: u64,
        subscription: &Subscription,
    ) -> Result<Subscription, BillingError>;
    async fn prepare_usage(&self, usage: &UsageRecord) -> Result<UsageRecord, BillingError>;
    async fn mark_usage_reporting(&self, usage_id: Uuid) -> Result<UsageRecord, BillingError>;
    async fn mark_usage_reported(
        &self,
        usage_id: Uuid,
        provider_event_id: &str,
    ) -> Result<UsageRecord, BillingError>;
    async fn mark_usage_failed(
        &self,
        usage_id: Uuid,
        uncertain: bool,
        sanitized_error: &str,
    ) -> Result<UsageRecord, BillingError>;
    async fn apply_webhook(&self, event: &ProviderWebhookEvent) -> Result<bool, BillingError>;
}

pub struct BillingService<R, P>
where
    R: BillingRepository,
    P: BillingProvider,
{
    repository: Arc<R>,
    provider: Arc<P>,
}

impl<R, P> BillingService<R, P>
where
    R: BillingRepository,
    P: BillingProvider,
{
    #[must_use]
    pub fn new(repository: Arc<R>, provider: Arc<P>) -> Self {
        Self {
            repository,
            provider,
        }
    }

    pub async fn subscribe(
        &self,
        context: &TenantContext,
        plan_id: &PlanId,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
        idempotency_key: &str,
    ) -> Result<Subscription, BillingError> {
        if !context.can_manage_billing() {
            return Err(BillingError::AccessDenied);
        }
        validate_idempotency_key(idempotency_key)?;
        let plan = self
            .repository
            .plan(plan_id)
            .await?
            .ok_or_else(|| BillingError::PlanNotFound(plan_id.clone()))?;
        if !plan.active {
            return Err(BillingError::PlanInactive(plan_id.clone()));
        }
        let subscription = Subscription::pending(
            context.tenant_id,
            plan.id.clone(),
            self.provider.name(),
            period_start,
            period_end,
        )?;
        let mut subscription = self.repository.create_subscription(&subscription).await?;
        let provider_subscription = self
            .provider
            .create_subscription(context.tenant_id, &plan, idempotency_key)
            .await?;
        let expected_version = subscription.version;
        subscription.provider_customer_id = Some(provider_subscription.customer_id);
        subscription.provider_subscription_id = Some(provider_subscription.subscription_id);
        subscription.status = provider_subscription.status;
        subscription.version = subscription
            .version
            .checked_add(1)
            .ok_or(BillingError::VersionOverflow)?;
        subscription.updated_at = Utc::now();
        self.repository
            .save_subscription(expected_version, &subscription)
            .await
    }

    pub async fn entitlement(
        &self,
        tenant_id: TenantId,
        name: &str,
    ) -> Result<Option<u64>, BillingError> {
        let subscription = match self.repository.active_subscription(tenant_id).await? {
            Some(subscription) if subscription.grants_entitlements() => subscription,
            _ => return Ok(None),
        };
        let plan = self
            .repository
            .plan(&subscription.plan_id)
            .await?
            .ok_or_else(|| BillingError::PlanNotFound(subscription.plan_id.clone()))?;
        Ok(plan.entitlements.get(name).copied())
    }

    pub async fn record_usage(
        &self,
        context: &TenantContext,
        meter: MeterName,
        quantity: u64,
        idempotency_key: &str,
        occurred_at: DateTime<Utc>,
    ) -> Result<UsageRecord, BillingError> {
        if !context.can_mutate_runtime() {
            return Err(BillingError::AccessDenied);
        }
        let subscription = self
            .repository
            .active_subscription(context.tenant_id)
            .await?
            .filter(Subscription::grants_entitlements)
            .ok_or(BillingError::NoActiveSubscription(context.tenant_id))?;
        let plan = self
            .repository
            .plan(&subscription.plan_id)
            .await?
            .ok_or_else(|| BillingError::PlanNotFound(subscription.plan_id.clone()))?;
        if !plan.meters.contains_key(&meter) {
            return Err(BillingError::MeterNotInPlan {
                meter,
                plan_id: plan.id,
            });
        }
        let prepared = self
            .repository
            .prepare_usage(&UsageRecord::new(
                context.tenant_id,
                subscription.id,
                meter,
                quantity,
                idempotency_key,
                occurred_at,
            )?)
            .await?;
        if prepared.state == UsageState::Reported {
            return Ok(prepared);
        }
        let reporting = self.repository.mark_usage_reporting(prepared.id).await?;
        match self.provider.report_usage(&subscription, &reporting).await {
            Ok(receipt) => {
                self.repository
                    .mark_usage_reported(reporting.id, &receipt.provider_event_id)
                    .await
            }
            Err(error) => {
                let uncertain = matches!(error, BillingError::ProviderUncertain(_));
                let sanitized = error.to_string();
                let _ = self
                    .repository
                    .mark_usage_failed(reporting.id, uncertain, &sanitized)
                    .await;
                Err(error)
            }
        }
    }
}

fn validate_idempotency_key(value: &str) -> Result<(), BillingError> {
    if value.trim().is_empty() || value.len() > 255 {
        return Err(BillingError::Validation(
            "billing idempotency key must be between 1 and 255 bytes".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum BillingError {
    #[error("billing validation failed: {0}")]
    Validation(String),
    #[error("billing access denied")]
    AccessDenied,
    #[error("billing plan {0} was not found")]
    PlanNotFound(PlanId),
    #[error("billing plan {0} is inactive")]
    PlanInactive(PlanId),
    #[error("tenant {0} has no active subscription")]
    NoActiveSubscription(TenantId),
    #[error("meter {meter} is not available in plan {plan_id}")]
    MeterNotInPlan { meter: MeterName, plan_id: PlanId },
    #[error("billing optimistic concurrency conflict")]
    Conflict,
    #[error("billing version overflow")]
    VersionOverflow,
    #[error("billing provider rejected the operation: {0}")]
    Provider(String),
    #[error("billing provider result is uncertain: {0}")]
    ProviderUncertain(String),
    #[error("billing repository failure: {0}")]
    Repository(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_canonical() {
        assert_eq!(PlanId::parse("Team.Monthly").expect("plan").as_str(), "team.monthly");
        assert_eq!(MeterName::parse("Provider_Tokens").expect("meter").as_str(), "provider_tokens");
        assert!(PlanId::parse("bad plan").is_err());
    }

    #[test]
    fn only_active_or_trialing_subscriptions_grant_entitlements() {
        let now = Utc::now();
        let mut subscription = Subscription::pending(
            TenantId::new(),
            PlanId::parse("team").expect("plan"),
            "stripe",
            now,
            now + chrono::Duration::days(30),
        )
        .expect("subscription");
        assert!(!subscription.grants_entitlements());
        subscription.status = SubscriptionStatus::Trialing;
        assert!(subscription.grants_entitlements());
        subscription.status = SubscriptionStatus::PastDue;
        assert!(!subscription.grants_entitlements());
    }
}
