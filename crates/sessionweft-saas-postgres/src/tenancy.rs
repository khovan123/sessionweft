use std::{collections::BTreeSet, str::FromStr};

use async_trait::async_trait;
use sessionweft_tenancy::{
    Membership, PrincipalId, QuotaDimension, QuotaReservation, ResourceKind, TenancyError, Tenant,
    TenantId, TenantQuota, TenantRepository, TenantRole, TenantStatus,
};
use sqlx::Row;
use uuid::Uuid;

use crate::database::SaasPostgresDatabase;

#[derive(Clone)]
pub struct PostgresTenantRepository {
    database: SaasPostgresDatabase,
}

impl PostgresTenantRepository {
    #[must_use]
    pub fn new(database: SaasPostgresDatabase) -> Self {
        Self { database }
    }

    #[must_use]
    pub fn database(&self) -> &SaasPostgresDatabase {
        &self.database
    }
}

#[async_trait]
impl TenantRepository for PostgresTenantRepository {
    async fn create_tenant(
        &self,
        tenant: &Tenant,
        owner: &Membership,
    ) -> Result<Tenant, TenancyError> {
        if owner.tenant_id != tenant.id || !owner.has_role(TenantRole::Owner) {
            return Err(TenancyError::Validation(
                "tenant bootstrap membership must be an owner for the same tenant".into(),
            ));
        }
        let mut transaction = self
            .database
            .begin_tenant(tenant.id)
            .await
            .map_err(repository_error)?;
        sqlx::query(
            r#"
            INSERT INTO sessionweft_tenants (
                id, slug, display_name, status, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(tenant.id.as_uuid())
        .bind(&tenant.slug)
        .bind(&tenant.display_name)
        .bind(status_text(tenant.status))
        .bind(tenant.created_at)
        .bind(tenant.updated_at)
        .execute(&mut *transaction)
        .await
        .map_err(repository_error)?;
        insert_membership(&mut transaction, owner).await?;
        insert_audit(
            &mut transaction,
            tenant.id,
            "tenant.created",
            serde_json::json!({
                "tenant_id": tenant.id,
                "slug": tenant.slug,
                "owner_principal_id": owner.principal_id,
            }),
        )
        .await?;
        transaction.commit().await.map_err(repository_error)?;
        Ok(tenant.clone())
    }

    async fn get_tenant(&self, tenant_id: TenantId) -> Result<Option<Tenant>, TenancyError> {
        let mut transaction = self
            .database
            .begin_tenant(tenant_id)
            .await
            .map_err(repository_error)?;
        let row = sqlx::query(
            "SELECT id, slug, display_name, status, created_at, updated_at FROM sessionweft_tenants WHERE id = $1",
        )
        .bind(tenant_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(repository_error)?;
        transaction.commit().await.map_err(repository_error)?;
        row.map(tenant_from_row).transpose()
    }

    async fn upsert_membership(&self, membership: &Membership) -> Result<(), TenancyError> {
        let mut transaction = self
            .database
            .begin_tenant(membership.tenant_id)
            .await
            .map_err(repository_error)?;
        insert_membership(&mut transaction, membership).await?;
        insert_audit(
            &mut transaction,
            membership.tenant_id,
            "tenant.membership_updated",
            serde_json::json!({
                "principal_id": membership.principal_id,
                "roles": membership.roles,
            }),
        )
        .await?;
        transaction.commit().await.map_err(repository_error)
    }

    async fn membership(
        &self,
        tenant_id: TenantId,
        principal_id: &PrincipalId,
    ) -> Result<Option<Membership>, TenancyError> {
        let mut transaction = self
            .database
            .begin_tenant(tenant_id)
            .await
            .map_err(repository_error)?;
        let row = sqlx::query(
            r#"
            SELECT tenant_id, principal_id, roles, created_at, updated_at
            FROM sessionweft_tenant_memberships
            WHERE tenant_id = $1 AND principal_id = $2
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(principal_id.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(repository_error)?;
        transaction.commit().await.map_err(repository_error)?;
        row.map(membership_from_row).transpose()
    }

    async fn set_quota(&self, quota: &TenantQuota) -> Result<(), TenancyError> {
        let hard_limit = i64::try_from(quota.hard_limit).map_err(|_| {
            TenancyError::Validation("quota hard limit exceeds PostgreSQL BIGINT".into())
        })?;
        let mut transaction = self
            .database
            .begin_tenant(quota.tenant_id)
            .await
            .map_err(repository_error)?;
        sqlx::query(
            r#"
            INSERT INTO sessionweft_tenant_quotas (tenant_id, dimension, hard_limit, updated_at)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (tenant_id, dimension) DO UPDATE
            SET hard_limit = EXCLUDED.hard_limit, updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(quota.tenant_id.as_uuid())
        .bind(quota.dimension.to_string())
        .bind(hard_limit)
        .bind(quota.updated_at)
        .execute(&mut *transaction)
        .await
        .map_err(repository_error)?;
        insert_audit(
            &mut transaction,
            quota.tenant_id,
            "tenant.quota_updated",
            serde_json::json!({
                "dimension": quota.dimension,
                "hard_limit": quota.hard_limit,
            }),
        )
        .await?;
        transaction.commit().await.map_err(repository_error)
    }

    async fn quota(
        &self,
        tenant_id: TenantId,
        dimension: QuotaDimension,
    ) -> Result<Option<TenantQuota>, TenancyError> {
        let mut transaction = self
            .database
            .begin_tenant(tenant_id)
            .await
            .map_err(repository_error)?;
        let row = sqlx::query(
            r#"
            SELECT tenant_id, dimension, hard_limit, updated_at
            FROM sessionweft_tenant_quotas
            WHERE tenant_id = $1 AND dimension = $2
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(dimension.to_string())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(repository_error)?;
        transaction.commit().await.map_err(repository_error)?;
        row.map(quota_from_row).transpose()
    }

    async fn bind_resource(
        &self,
        tenant_id: TenantId,
        kind: ResourceKind,
        resource_id: &str,
    ) -> Result<(), TenancyError> {
        validate_resource_id(resource_id)?;
        let mut transaction = self
            .database
            .begin_tenant(tenant_id)
            .await
            .map_err(repository_error)?;
        let result = sqlx::query(
            r#"
            INSERT INTO sessionweft_tenant_resources (
                tenant_id, resource_kind, resource_id, created_at
            ) VALUES ($1, $2, $3, NOW())
            ON CONFLICT (resource_kind, resource_id) DO NOTHING
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(kind.to_string())
        .bind(resource_id)
        .execute(&mut *transaction)
        .await
        .map_err(repository_error)?;
        if result.rows_affected() != 1 {
            transaction.rollback().await.map_err(repository_error)?;
            return Err(TenancyError::Conflict(format!(
                "resource {kind}/{resource_id} is already bound"
            )));
        }
        insert_audit(
            &mut transaction,
            tenant_id,
            "tenant.resource_bound",
            serde_json::json!({"kind": kind, "resource_id": resource_id}),
        )
        .await?;
        transaction.commit().await.map_err(repository_error)
    }

    async fn owns_resource(
        &self,
        tenant_id: TenantId,
        kind: ResourceKind,
        resource_id: &str,
    ) -> Result<bool, TenancyError> {
        validate_resource_id(resource_id)?;
        let mut transaction = self
            .database
            .begin_tenant(tenant_id)
            .await
            .map_err(repository_error)?;
        let owned = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM sessionweft_tenant_resources
                WHERE tenant_id = $1 AND resource_kind = $2 AND resource_id = $3
            )
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(kind.to_string())
        .bind(resource_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(repository_error)?;
        transaction.commit().await.map_err(repository_error)?;
        Ok(owned)
    }

    async fn reserve_quota(
        &self,
        tenant_id: TenantId,
        dimension: QuotaDimension,
        amount: u64,
        idempotency_key: &str,
    ) -> Result<QuotaReservation, TenancyError> {
        if amount == 0 {
            return Err(TenancyError::Validation(
                "quota reservation amount must be greater than zero".into(),
            ));
        }
        let amount_i64 = i64::try_from(amount).map_err(|_| {
            TenancyError::Validation("quota reservation exceeds PostgreSQL BIGINT".into())
        })?;
        let mut transaction = self
            .database
            .begin_tenant(tenant_id)
            .await
            .map_err(repository_error)?;

        if let Some(row) = sqlx::query(
            r#"
            SELECT amount, used_after, hard_limit
            FROM sessionweft_quota_reservations
            WHERE tenant_id = $1 AND dimension = $2 AND idempotency_key = $3
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(dimension.to_string())
        .bind(idempotency_key)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(repository_error)?
        {
            let reservation = QuotaReservation {
                tenant_id,
                dimension,
                amount: as_u64(row.try_get::<i64, _>("amount").map_err(repository_error)?)?,
                idempotency_key: idempotency_key.to_owned(),
                used_after: as_u64(
                    row.try_get::<i64, _>("used_after")
                        .map_err(repository_error)?,
                )?,
                hard_limit: as_u64(
                    row.try_get::<i64, _>("hard_limit")
                        .map_err(repository_error)?,
                )?,
            };
            transaction.commit().await.map_err(repository_error)?;
            return Ok(reservation);
        }

        let limit = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT hard_limit FROM sessionweft_tenant_quotas
            WHERE tenant_id = $1 AND dimension = $2
            FOR UPDATE
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(dimension.to_string())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(repository_error)?
        .unwrap_or(0);

        sqlx::query(
            r#"
            INSERT INTO sessionweft_tenant_usage (tenant_id, dimension, used, updated_at)
            VALUES ($1, $2, 0, NOW())
            ON CONFLICT (tenant_id, dimension) DO NOTHING
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(dimension.to_string())
        .execute(&mut *transaction)
        .await
        .map_err(repository_error)?;

        let used = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT used FROM sessionweft_tenant_usage
            WHERE tenant_id = $1 AND dimension = $2
            FOR UPDATE
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(dimension.to_string())
        .fetch_one(&mut *transaction)
        .await
        .map_err(repository_error)?;
        let used_after = used
            .checked_add(amount_i64)
            .ok_or_else(|| TenancyError::Validation("quota usage overflow".into()))?;
        if used_after > limit {
            transaction.rollback().await.map_err(repository_error)?;
            return Err(TenancyError::QuotaExceeded {
                tenant_id,
                dimension,
                requested: amount,
                used: as_u64(used)?,
                limit: as_u64(limit)?,
            });
        }

        sqlx::query(
            r#"
            UPDATE sessionweft_tenant_usage
            SET used = $3, updated_at = NOW()
            WHERE tenant_id = $1 AND dimension = $2
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(dimension.to_string())
        .bind(used_after)
        .execute(&mut *transaction)
        .await
        .map_err(repository_error)?;
        sqlx::query(
            r#"
            INSERT INTO sessionweft_quota_reservations (
                tenant_id, dimension, idempotency_key, amount, used_after, hard_limit, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, NOW())
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(dimension.to_string())
        .bind(idempotency_key)
        .bind(amount_i64)
        .bind(used_after)
        .bind(limit)
        .execute(&mut *transaction)
        .await
        .map_err(repository_error)?;
        insert_audit(
            &mut transaction,
            tenant_id,
            "tenant.quota_reserved",
            serde_json::json!({
                "dimension": dimension,
                "amount": amount,
                "used_after": used_after,
                "hard_limit": limit,
                "idempotency_key": idempotency_key,
            }),
        )
        .await?;
        transaction.commit().await.map_err(repository_error)?;
        Ok(QuotaReservation {
            tenant_id,
            dimension,
            amount,
            idempotency_key: idempotency_key.to_owned(),
            used_after: as_u64(used_after)?,
            hard_limit: as_u64(limit)?,
        })
    }
}

async fn insert_membership(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    membership: &Membership,
) -> Result<(), TenancyError> {
    sqlx::query(
        r#"
        INSERT INTO sessionweft_tenant_memberships (
            tenant_id, principal_id, roles, created_at, updated_at
        ) VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (tenant_id, principal_id) DO UPDATE
        SET roles = EXCLUDED.roles, updated_at = EXCLUDED.updated_at
        "#,
    )
    .bind(membership.tenant_id.as_uuid())
    .bind(membership.principal_id.as_str())
    .bind(serde_json::to_value(&membership.roles).map_err(serialization_error)?)
    .bind(membership.created_at)
    .bind(membership.updated_at)
    .execute(&mut **transaction)
    .await
    .map_err(repository_error)?;
    Ok(())
}

async fn insert_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<(), TenancyError> {
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

fn tenant_from_row(row: sqlx::postgres::PgRow) -> Result<Tenant, TenancyError> {
    Ok(Tenant {
        id: TenantId::from_uuid(row.try_get("id").map_err(repository_error)?),
        slug: row.try_get("slug").map_err(repository_error)?,
        display_name: row.try_get("display_name").map_err(repository_error)?,
        status: status_from_text(
            &row.try_get::<String, _>("status")
                .map_err(repository_error)?,
        )?,
        created_at: row.try_get("created_at").map_err(repository_error)?,
        updated_at: row.try_get("updated_at").map_err(repository_error)?,
    })
}

fn membership_from_row(row: sqlx::postgres::PgRow) -> Result<Membership, TenancyError> {
    let roles: BTreeSet<TenantRole> = serde_json::from_value(
        row.try_get::<serde_json::Value, _>("roles")
            .map_err(repository_error)?,
    )
    .map_err(serialization_error)?;
    Ok(Membership {
        tenant_id: TenantId::from_uuid(row.try_get("tenant_id").map_err(repository_error)?),
        principal_id: PrincipalId::parse(
            row.try_get::<String, _>("principal_id")
                .map_err(repository_error)?,
        )?,
        roles,
        created_at: row.try_get("created_at").map_err(repository_error)?,
        updated_at: row.try_get("updated_at").map_err(repository_error)?,
    })
}

fn quota_from_row(row: sqlx::postgres::PgRow) -> Result<TenantQuota, TenancyError> {
    Ok(TenantQuota {
        tenant_id: TenantId::from_uuid(row.try_get("tenant_id").map_err(repository_error)?),
        dimension: QuotaDimension::from_str(
            &row.try_get::<String, _>("dimension")
                .map_err(repository_error)?,
        )?,
        hard_limit: as_u64(row.try_get("hard_limit").map_err(repository_error)?)?,
        updated_at: row.try_get("updated_at").map_err(repository_error)?,
    })
}

fn status_text(status: TenantStatus) -> &'static str {
    match status {
        TenantStatus::Active => "active",
        TenantStatus::Suspended => "suspended",
        TenantStatus::Closed => "closed",
    }
}

fn status_from_text(value: &str) -> Result<TenantStatus, TenancyError> {
    match value {
        "active" => Ok(TenantStatus::Active),
        "suspended" => Ok(TenantStatus::Suspended),
        "closed" => Ok(TenantStatus::Closed),
        _ => Err(TenancyError::Repository(format!(
            "unknown persisted tenant status '{value}'"
        ))),
    }
}

fn validate_resource_id(value: &str) -> Result<(), TenancyError> {
    if value.trim().is_empty() || value.len() > 512 {
        return Err(TenancyError::Validation(
            "resource ID must be between 1 and 512 bytes".into(),
        ));
    }
    Ok(())
}

fn as_u64(value: i64) -> Result<u64, TenancyError> {
    u64::try_from(value)
        .map_err(|_| TenancyError::Repository(format!("invalid negative numeric value {value}")))
}

fn repository_error(error: impl std::fmt::Display) -> TenancyError {
    TenancyError::Repository(error.to_string())
}

fn serialization_error(error: serde_json::Error) -> TenancyError {
    TenancyError::Repository(error.to_string())
}
