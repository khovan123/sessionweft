use std::time::Duration;

use sessionweft_tenancy::TenantId;
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use thiserror::Error;

#[derive(Clone)]
pub struct SaasPostgresDatabase {
    pool: PgPool,
}

impl SaasPostgresDatabase {
    pub async fn connect(database_url: &str) -> Result<Self, SaasPostgresError> {
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(Duration::from_secs(10))
            .connect(database_url)
            .await?;
        let database = Self { pool };
        database.migrate().await?;
        Ok(database)
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn migrate(&self) -> Result<(), SaasPostgresError> {
        for statement in MIGRATIONS {
            sqlx::query(statement).execute(&self.pool).await?;
        }
        Ok(())
    }

    pub async fn begin_tenant(
        &self,
        tenant_id: TenantId,
    ) -> Result<Transaction<'_, Postgres>, SaasPostgresError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT set_config('sessionweft.tenant_id', $1, true)")
            .bind(tenant_id.to_string())
            .execute(&mut *transaction)
            .await?;
        Ok(transaction)
    }
}

#[derive(Debug, Error)]
pub enum SaasPostgresError {
    #[error("PostgreSQL failure: {0}")]
    Database(#[from] sqlx::Error),
    #[error("serialization failure: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid persisted data: {0}")]
    Corrupt(String),
}

const MIGRATIONS: &[&str] = &[
    r#"CREATE TABLE IF NOT EXISTS sessionweft_tenants (
        id UUID PRIMARY KEY,
        slug TEXT NOT NULL UNIQUE,
        display_name TEXT NOT NULL,
        status TEXT NOT NULL,
        created_at TIMESTAMPTZ NOT NULL,
        updated_at TIMESTAMPTZ NOT NULL
    )"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_tenant_memberships (
        tenant_id UUID NOT NULL REFERENCES sessionweft_tenants(id) ON DELETE CASCADE,
        principal_id TEXT NOT NULL,
        roles JSONB NOT NULL,
        created_at TIMESTAMPTZ NOT NULL,
        updated_at TIMESTAMPTZ NOT NULL,
        PRIMARY KEY (tenant_id, principal_id)
    )"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_tenant_quotas (
        tenant_id UUID NOT NULL REFERENCES sessionweft_tenants(id) ON DELETE CASCADE,
        dimension TEXT NOT NULL,
        hard_limit BIGINT NOT NULL CHECK (hard_limit >= 0),
        updated_at TIMESTAMPTZ NOT NULL,
        PRIMARY KEY (tenant_id, dimension)
    )"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_tenant_usage (
        tenant_id UUID NOT NULL REFERENCES sessionweft_tenants(id) ON DELETE CASCADE,
        dimension TEXT NOT NULL,
        used BIGINT NOT NULL DEFAULT 0 CHECK (used >= 0),
        updated_at TIMESTAMPTZ NOT NULL,
        PRIMARY KEY (tenant_id, dimension)
    )"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_quota_reservations (
        tenant_id UUID NOT NULL REFERENCES sessionweft_tenants(id) ON DELETE CASCADE,
        dimension TEXT NOT NULL,
        idempotency_key TEXT NOT NULL,
        amount BIGINT NOT NULL CHECK (amount > 0),
        used_after BIGINT NOT NULL CHECK (used_after >= 0),
        hard_limit BIGINT NOT NULL CHECK (hard_limit >= 0),
        created_at TIMESTAMPTZ NOT NULL,
        PRIMARY KEY (tenant_id, dimension, idempotency_key)
    )"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_tenant_resources (
        tenant_id UUID NOT NULL REFERENCES sessionweft_tenants(id) ON DELETE CASCADE,
        resource_kind TEXT NOT NULL,
        resource_id TEXT NOT NULL,
        created_at TIMESTAMPTZ NOT NULL,
        PRIMARY KEY (resource_kind, resource_id),
        UNIQUE (tenant_id, resource_kind, resource_id)
    )"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_billing_plans (
        id TEXT PRIMARY KEY,
        active BOOLEAN NOT NULL,
        data_json JSONB NOT NULL,
        updated_at TIMESTAMPTZ NOT NULL
    )"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_subscriptions (
        id UUID PRIMARY KEY,
        tenant_id UUID NOT NULL REFERENCES sessionweft_tenants(id) ON DELETE CASCADE,
        plan_id TEXT NOT NULL REFERENCES sessionweft_billing_plans(id),
        provider TEXT NOT NULL,
        provider_customer_id TEXT,
        provider_subscription_id TEXT,
        status TEXT NOT NULL,
        period_start TIMESTAMPTZ NOT NULL,
        period_end TIMESTAMPTZ NOT NULL,
        version BIGINT NOT NULL,
        data_json JSONB NOT NULL,
        created_at TIMESTAMPTZ NOT NULL,
        updated_at TIMESTAMPTZ NOT NULL
    )"#,
    r#"CREATE UNIQUE INDEX IF NOT EXISTS idx_sessionweft_subscription_provider
        ON sessionweft_subscriptions (provider, provider_subscription_id)
        WHERE provider_subscription_id IS NOT NULL"#,
    r#"CREATE INDEX IF NOT EXISTS idx_sessionweft_subscription_tenant_status
        ON sessionweft_subscriptions (tenant_id, status, updated_at DESC)"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_billing_usage (
        id UUID PRIMARY KEY,
        tenant_id UUID NOT NULL REFERENCES sessionweft_tenants(id) ON DELETE CASCADE,
        subscription_id UUID NOT NULL REFERENCES sessionweft_subscriptions(id) ON DELETE RESTRICT,
        meter TEXT NOT NULL,
        quantity BIGINT NOT NULL CHECK (quantity > 0),
        idempotency_key TEXT NOT NULL,
        occurred_at TIMESTAMPTZ NOT NULL,
        state TEXT NOT NULL,
        provider_event_id TEXT,
        attempts INTEGER NOT NULL DEFAULT 0,
        last_error TEXT,
        data_json JSONB NOT NULL,
        created_at TIMESTAMPTZ NOT NULL,
        updated_at TIMESTAMPTZ NOT NULL,
        UNIQUE (tenant_id, idempotency_key)
    )"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_billing_webhooks (
        provider TEXT NOT NULL,
        event_id TEXT NOT NULL,
        tenant_id UUID NOT NULL REFERENCES sessionweft_tenants(id) ON DELETE CASCADE,
        event_type TEXT NOT NULL,
        payload_json JSONB NOT NULL,
        occurred_at TIMESTAMPTZ NOT NULL,
        processed_at TIMESTAMPTZ NOT NULL,
        PRIMARY KEY (provider, event_id)
    )"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_saas_outbox (
        event_id UUID PRIMARY KEY,
        tenant_id UUID NOT NULL REFERENCES sessionweft_tenants(id) ON DELETE CASCADE,
        event_type TEXT NOT NULL,
        payload_json JSONB NOT NULL,
        correlation_id UUID NOT NULL,
        created_at TIMESTAMPTZ NOT NULL,
        published_at TIMESTAMPTZ
    )"#,
    r#"ALTER TABLE sessionweft_tenants ENABLE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_tenants FORCE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_tenant_memberships ENABLE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_tenant_memberships FORCE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_tenant_quotas ENABLE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_tenant_quotas FORCE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_tenant_usage ENABLE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_tenant_usage FORCE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_quota_reservations ENABLE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_quota_reservations FORCE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_tenant_resources ENABLE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_tenant_resources FORCE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_subscriptions ENABLE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_subscriptions FORCE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_billing_usage ENABLE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_billing_usage FORCE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_billing_webhooks ENABLE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_billing_webhooks FORCE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_saas_outbox ENABLE ROW LEVEL SECURITY"#,
    r#"ALTER TABLE sessionweft_saas_outbox FORCE ROW LEVEL SECURITY"#,
    r#"DO $$
    DECLARE table_name TEXT;
    BEGIN
      FOREACH table_name IN ARRAY ARRAY[
        'sessionweft_tenants',
        'sessionweft_tenant_memberships',
        'sessionweft_tenant_quotas',
        'sessionweft_tenant_usage',
        'sessionweft_quota_reservations',
        'sessionweft_tenant_resources',
        'sessionweft_subscriptions',
        'sessionweft_billing_usage',
        'sessionweft_billing_webhooks',
        'sessionweft_saas_outbox'
      ] LOOP
        IF NOT EXISTS (
          SELECT 1 FROM pg_policies
          WHERE schemaname = current_schema()
            AND tablename = table_name
            AND policyname = 'sessionweft_tenant_isolation'
        ) THEN
          EXECUTE format(
            'CREATE POLICY sessionweft_tenant_isolation ON %I USING (tenant_id = NULLIF(current_setting(''sessionweft.tenant_id'', true), '''')::uuid) WITH CHECK (tenant_id = NULLIF(current_setting(''sessionweft.tenant_id'', true), '''')::uuid)',
            table_name
          );
        END IF;
      END LOOP;
    END $$"#,
];
