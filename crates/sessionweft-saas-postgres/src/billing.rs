use std::str::FromStr;

use async_trait::async_trait;
use chrono::Utc;
use sessionweft_billing::{
    BillingError, BillingPlan, BillingRepository, PlanId, ProviderWebhookEvent, Subscription,
    SubscriptionStatus, UsageRecord, UsageState,
};
use sessionweft_tenancy::TenantId;
use sqlx::Row;
use uuid::Uuid;

use crate::database::SaasPostgresDatabase;

#[derive(Clone)]
pub struct PostgresBillingRepository {
    database: SaasPostgresDatabase,
    tenant_id: TenantId,
}

impl PostgresBillingRepository {
    #[must_use]
    pub fn new(database: SaasPostgresDatabase, tenant_id: TenantId) -> Self {
        Self {
            database,
            tenant_id,
        }
    }

    #[must_use]
    pub const fn tenant_id(&self) -> TenantId {
        self.tenant_id
    }
}

#[async_trait]
impl BillingRepository for PostgresBillingRepository {
    async fn upsert_plan(&self, plan: &BillingPlan) -> Result<(), BillingError> {
        plan.validate()?;
        sqlx::query(
            r#"
            INSERT INTO sessionweft_billing_plans (id, active, data_json, updated_at)
            VALUES ($1, $2, $3, NOW())
            ON CONFLICT (id) DO UPDATE
            SET active = EXCLUDED.active, data_json = EXCLUDED.data_json, updated_at = NOW()
            "#,
        )
        .bind(plan.id.as_str())
        .bind(plan.active)
        .bind(serde_json::to_value(plan).map_err(serialization_error)?)
        .execute(self.database.pool())
        .await
        .map_err(repository_error)?;
        Ok(())
    }

    async fn plan(&self, plan_id: &PlanId) -> Result<Option<BillingPlan>, BillingError> {
        let row = sqlx::query("SELECT data_json FROM sessionweft_billing_plans WHERE id = $1")
            .bind(plan_id.as_str())
            .fetch_optional(self.database.pool())
            .await
            .map_err(repository_error)?;
        row.map(|row| {
            serde_json::from_value(row.try_get("data_json").map_err(repository_error)?)
                .map_err(serialization_error)
        })
        .transpose()
    }

    async fn create_subscription(
        &self,
        subscription: &Subscription,
    ) -> Result<Subscription, BillingError> {
        self.require_tenant(subscription.tenant_id)?;
        let mut transaction = self
            .database
            .begin_tenant(self.tenant_id)
            .await
            .map_err(database_error)?;
        sqlx::query(
            r#"
            INSERT INTO sessionweft_subscriptions (
                id, tenant_id, plan_id, provider, provider_customer_id,
                provider_subscription_id, status, period_start, period_end,
                version, data_json, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            "#,
        )
        .bind(subscription.id)
        .bind(subscription.tenant_id.as_uuid())
        .bind(subscription.plan_id.as_str())
        .bind(&subscription.provider)
        .bind(&subscription.provider_customer_id)
        .bind(&subscription.provider_subscription_id)
        .bind(subscription.status.to_string())
        .bind(subscription.period_start)
        .bind(subscription.period_end)
        .bind(as_i64(subscription.version)?)
        .bind(serde_json::to_value(subscription).map_err(serialization_error)?)
        .bind(subscription.created_at)
        .bind(subscription.updated_at)
        .execute(&mut *transaction)
        .await
        .map_err(repository_error)?;
        insert_audit(
            &mut transaction,
            self.tenant_id,
            "billing.subscription_created",
            serde_json::json!({
                "subscription_id": subscription.id,
                "plan_id": subscription.plan_id,
                "provider": subscription.provider,
            }),
        )
        .await?;
        transaction.commit().await.map_err(repository_error)?;
        Ok(subscription.clone())
    }

    async fn subscription(
        &self,
        tenant_id: TenantId,
        subscription_id: Uuid,
    ) -> Result<Option<Subscription>, BillingError> {
        self.require_tenant(tenant_id)?;
        let mut transaction = self
            .database
            .begin_tenant(self.tenant_id)
            .await
            .map_err(database_error)?;
        let row = sqlx::query(
            "SELECT data_json FROM sessionweft_subscriptions WHERE tenant_id = $1 AND id = $2",
        )
        .bind(self.tenant_id.as_uuid())
        .bind(subscription_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(repository_error)?;
        transaction.commit().await.map_err(repository_error)?;
        row.map(subscription_from_row).transpose()
    }

    async fn active_subscription(
        &self,
        tenant_id: TenantId,
    ) -> Result<Option<Subscription>, BillingError> {
        self.require_tenant(tenant_id)?;
        let mut transaction = self
            .database
            .begin_tenant(self.tenant_id)
            .await
            .map_err(database_error)?;
        let row = sqlx::query(
            r#"
            SELECT data_json FROM sessionweft_subscriptions
            WHERE tenant_id = $1 AND status IN ('active', 'trialing')
            ORDER BY updated_at DESC
            LIMIT 1
            "#,
        )
        .bind(self.tenant_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(repository_error)?;
        transaction.commit().await.map_err(repository_error)?;
        row.map(subscription_from_row).transpose()
    }

    async fn save_subscription(
        &self,
        expected_version: u64,
        subscription: &Subscription,
    ) -> Result<Subscription, BillingError> {
        self.require_tenant(subscription.tenant_id)?;
        let mut transaction = self
            .database
            .begin_tenant(self.tenant_id)
            .await
            .map_err(database_error)?;
        let result = sqlx::query(
            r#"
            UPDATE sessionweft_subscriptions
            SET plan_id = $1,
                provider_customer_id = $2,
                provider_subscription_id = $3,
                status = $4,
                period_start = $5,
                period_end = $6,
                version = $7,
                data_json = $8,
                updated_at = $9
            WHERE tenant_id = $10 AND id = $11 AND version = $12
            "#,
        )
        .bind(subscription.plan_id.as_str())
        .bind(&subscription.provider_customer_id)
        .bind(&subscription.provider_subscription_id)
        .bind(subscription.status.to_string())
        .bind(subscription.period_start)
        .bind(subscription.period_end)
        .bind(as_i64(subscription.version)?)
        .bind(serde_json::to_value(subscription).map_err(serialization_error)?)
        .bind(subscription.updated_at)
        .bind(self.tenant_id.as_uuid())
        .bind(subscription.id)
        .bind(as_i64(expected_version)?)
        .execute(&mut *transaction)
        .await
        .map_err(repository_error)?;
        if result.rows_affected() != 1 {
            transaction.rollback().await.map_err(repository_error)?;
            return Err(BillingError::Conflict);
        }
        insert_audit(
            &mut transaction,
            self.tenant_id,
            "billing.subscription_updated",
            serde_json::json!({
                "subscription_id": subscription.id,
                "status": subscription.status,
                "version": subscription.version,
            }),
        )
        .await?;
        transaction.commit().await.map_err(repository_error)?;
        Ok(subscription.clone())
    }

    async fn prepare_usage(&self, usage: &UsageRecord) -> Result<UsageRecord, BillingError> {
        self.require_tenant(usage.tenant_id)?;
        let mut transaction = self
            .database
            .begin_tenant(self.tenant_id)
            .await
            .map_err(database_error)?;
        sqlx::query(
            r#"
            INSERT INTO sessionweft_billing_usage (
                id, tenant_id, subscription_id, meter, quantity, idempotency_key,
                occurred_at, state, provider_event_id, attempts, last_error,
                data_json, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            ON CONFLICT (tenant_id, idempotency_key) DO NOTHING
            "#,
        )
        .bind(usage.id)
        .bind(self.tenant_id.as_uuid())
        .bind(usage.subscription_id)
        .bind(usage.meter.as_str())
        .bind(as_i64(usage.quantity)?)
        .bind(&usage.idempotency_key)
        .bind(usage.occurred_at)
        .bind(usage.state.to_string())
        .bind(&usage.provider_event_id)
        .bind(i32::try_from(usage.attempts).map_err(|_| BillingError::VersionOverflow)?)
        .bind(&usage.last_error)
        .bind(serde_json::to_value(usage).map_err(serialization_error)?)
        .bind(usage.created_at)
        .bind(usage.updated_at)
        .execute(&mut *transaction)
        .await
        .map_err(repository_error)?;
        let row = sqlx::query(
            r#"
            SELECT data_json FROM sessionweft_billing_usage
            WHERE tenant_id = $1 AND idempotency_key = $2
            "#,
        )
        .bind(self.tenant_id.as_uuid())
        .bind(&usage.idempotency_key)
        .fetch_one(&mut *transaction)
        .await
        .map_err(repository_error)?;
        let persisted = usage_from_row(row)?;
        if persisted.subscription_id != usage.subscription_id
            || persisted.meter != usage.meter
            || persisted.quantity != usage.quantity
            || persisted.occurred_at != usage.occurred_at
        {
            transaction.rollback().await.map_err(repository_error)?;
            return Err(BillingError::Validation(
                "billing idempotency key was reused with different usage parameters".into(),
            ));
        }
        transaction.commit().await.map_err(repository_error)?;
        Ok(persisted)
    }

    async fn mark_usage_reporting(&self, usage_id: Uuid) -> Result<UsageRecord, BillingError> {
        self.transition_usage(usage_id, UsageState::Reporting, None, None, true)
            .await
    }

    async fn mark_usage_reported(
        &self,
        usage_id: Uuid,
        provider_event_id: &str,
    ) -> Result<UsageRecord, BillingError> {
        if provider_event_id.trim().is_empty() || provider_event_id.len() > 512 {
            return Err(BillingError::Validation(
                "provider usage event ID must be between 1 and 512 bytes".into(),
            ));
        }
        self.transition_usage(
            usage_id,
            UsageState::Reported,
            Some(provider_event_id),
            None,
            false,
        )
        .await
    }

    async fn mark_usage_failed(
        &self,
        usage_id: Uuid,
        uncertain: bool,
        sanitized_error: &str,
    ) -> Result<UsageRecord, BillingError> {
        self.transition_usage(
            usage_id,
            if uncertain {
                UsageState::Uncertain
            } else {
                UsageState::Failed
            },
            None,
            Some(sanitized_error),
            false,
        )
        .await
    }

    async fn apply_webhook(&self, event: &ProviderWebhookEvent) -> Result<bool, BillingError> {
        self.require_tenant(event.tenant_id)?;
        let mut transaction = self
            .database
            .begin_tenant(self.tenant_id)
            .await
            .map_err(database_error)?;
        let inserted = sqlx::query(
            r#"
            INSERT INTO sessionweft_billing_webhooks (
                provider, event_id, tenant_id, event_type, payload_json,
                occurred_at, processed_at
            ) VALUES ($1, $2, $3, $4, $5, $6, NOW())
            ON CONFLICT (provider, event_id) DO NOTHING
            "#,
        )
        .bind(&event.provider)
        .bind(&event.event_id)
        .bind(self.tenant_id.as_uuid())
        .bind(&event.event_type)
        .bind(&event.payload)
        .bind(event.occurred_at)
        .execute(&mut *transaction)
        .await
        .map_err(repository_error)?;
        if inserted.rows_affected() == 0 {
            transaction.commit().await.map_err(repository_error)?;
            return Ok(false);
        }

        if let (Some(provider_subscription_id), Some(status)) = (
            event.payload.get("provider_subscription_id").and_then(|value| value.as_str()),
            event.payload.get("status").and_then(|value| value.as_str()),
        ) {
            let status = SubscriptionStatus::from_str(status)?;
            let row = sqlx::query(
                r#"
                SELECT data_json FROM sessionweft_subscriptions
                WHERE tenant_id = $1 AND provider = $2 AND provider_subscription_id = $3
                FOR UPDATE
                "#,
            )
            .bind(self.tenant_id.as_uuid())
            .bind(&event.provider)
            .bind(provider_subscription_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(repository_error)?;
            if let Some(row) = row {
                let mut subscription = subscription_from_row(row)?;
                subscription.status = status;
                subscription.version = subscription
                    .version
                    .checked_add(1)
                    .ok_or(BillingError::VersionOverflow)?;
                subscription.updated_at = Utc::now();
                sqlx::query(
                    r#"
                    UPDATE sessionweft_subscriptions
                    SET status = $1, version = $2, data_json = $3, updated_at = $4
                    WHERE tenant_id = $5 AND id = $6
                    "#,
                )
                .bind(subscription.status.to_string())
                .bind(as_i64(subscription.version)?)
                .bind(serde_json::to_value(&subscription).map_err(serialization_error)?)
                .bind(subscription.updated_at)
                .bind(self.tenant_id.as_uuid())
                .bind(subscription.id)
                .execute(&mut *transaction)
                .await
                .map_err(repository_error)?;
            }
        }
        insert_audit(
            &mut transaction,
            self.tenant_id,
            "billing.webhook_applied",
            serde_json::json!({
                "provider": event.provider,
                "event_id": event.event_id,
                "event_type": event.event_type,
            }),
        )
        .await?;
        transaction.commit().await.map_err(repository_error)?;
        Ok(true)
    }
}

impl PostgresBillingRepository {
    async fn transition_usage(
        &self,
        usage_id: Uuid,
        state: UsageState,
        provider_event_id: Option<&str>,
        last_error: Option<&str>,
        increment_attempts: bool,
    ) -> Result<UsageRecord, BillingError> {
        let mut transaction = self
            .database
            .begin_tenant(self.tenant_id)
            .await
            .map_err(database_error)?;
        let row = sqlx::query(
            r#"
            SELECT data_json FROM sessionweft_billing_usage
            WHERE tenant_id = $1 AND id = $2
            FOR UPDATE
            "#,
        )
        .bind(self.tenant_id.as_uuid())
        .bind(usage_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(repository_error)?
        .ok_or_else(|| BillingError::Repository(format!("usage record {usage_id} not found")))?;
        let mut usage = usage_from_row(row)?;
        if usage.state == UsageState::Reported {
            transaction.commit().await.map_err(repository_error)?;
            return Ok(usage);
        }
        if increment_attempts {
            usage.attempts = usage
                .attempts
                .checked_add(1)
                .ok_or(BillingError::VersionOverflow)?;
        }
        usage.state = state;
        usage.provider_event_id = provider_event_id.map(ToOwned::to_owned);
        usage.last_error = last_error.map(|value| value.chars().take(4_096).collect());
        usage.updated_at = Utc::now();
        sqlx::query(
            r#"
            UPDATE sessionweft_billing_usage
            SET state = $1, provider_event_id = $2, attempts = $3,
                last_error = $4, data_json = $5, updated_at = $6
            WHERE tenant_id = $7 AND id = $8
            "#,
        )
        .bind(usage.state.to_string())
        .bind(&usage.provider_event_id)
        .bind(i32::try_from(usage.attempts).map_err(|_| BillingError::VersionOverflow)?)
        .bind(&usage.last_error)
        .bind(serde_json::to_value(&usage).map_err(serialization_error)?)
        .bind(usage.updated_at)
        .bind(self.tenant_id.as_uuid())
        .bind(usage.id)
        .execute(&mut *transaction)
        .await
        .map_err(repository_error)?;
        insert_audit(
            &mut transaction,
            self.tenant_id,
            "billing.usage_transitioned",
            serde_json::json!({
                "usage_id": usage.id,
                "state": usage.state,
                "attempts": usage.attempts,
            }),
        )
        .await?;
        transaction.commit().await.map_err(repository_error)?;
        Ok(usage)
    }

    fn require_tenant(&self, tenant_id: TenantId) -> Result<(), BillingError> {
        if tenant_id == self.tenant_id {
            Ok(())
        } else {
            Err(BillingError::AccessDenied)
        }
    }
}

async fn insert_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<(), BillingError> {
    sqlx::query(
        r#"
        INSERT INTO sessionweft_saas_outbox (
            event_id, tenant_id, event_type, payload_json, correlation_id, created_at
        ) VALUES ($1, $2, $3, $4, $5, NOW())
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(tenant_id.as_uuid())
    .bind(event_type)
    .bind(payload)
    .bind(Uuid::new_v4())
    .execute(&mut **transaction)
    .await
    .map_err(repository_error)?;
    Ok(())
}

fn subscription_from_row(row: sqlx::postgres::PgRow) -> Result<Subscription, BillingError> {
    serde_json::from_value(row.try_get("data_json").map_err(repository_error)?)
        .map_err(serialization_error)
}

fn usage_from_row(row: sqlx::postgres::PgRow) -> Result<UsageRecord, BillingError> {
    serde_json::from_value(row.try_get("data_json").map_err(repository_error)?)
        .map_err(serialization_error)
}

fn as_i64(value: u64) -> Result<i64, BillingError> {
    i64::try_from(value).map_err(|_| BillingError::VersionOverflow)
}

fn repository_error(error: sqlx::Error) -> BillingError {
    BillingError::Repository(error.to_string())
}

fn database_error(error: crate::database::SaasPostgresError) -> BillingError {
    BillingError::Repository(error.to_string())
}

fn serialization_error(error: serde_json::Error) -> BillingError {
    BillingError::Repository(error.to_string())
}
