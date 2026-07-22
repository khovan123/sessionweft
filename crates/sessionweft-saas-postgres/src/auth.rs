use chrono::{DateTime, Utc};
use sessionweft_tenancy::{PrincipalId, TenancyError, TenantId};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::database::SaasPostgresDatabase;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedTenantToken {
    pub id: Uuid,
    pub tenant_id: TenantId,
    pub principal_id: PrincipalId,
    pub label: String,
    pub raw_token: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTenantToken {
    pub id: Uuid,
    pub tenant_id: TenantId,
    pub principal_id: PrincipalId,
    pub label: String,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct PostgresTenantAuthRepository {
    database: SaasPostgresDatabase,
}

impl PostgresTenantAuthRepository {
    pub async fn new(database: SaasPostgresDatabase) -> Result<Self, TenancyError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessionweft_tenant_api_tokens (
                id UUID PRIMARY KEY,
                tenant_id UUID NOT NULL REFERENCES sessionweft_tenants(id) ON DELETE CASCADE,
                principal_id TEXT NOT NULL,
                label TEXT NOT NULL,
                token_hash BYTEA NOT NULL UNIQUE,
                expires_at TIMESTAMPTZ,
                revoked_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ NOT NULL,
                last_used_at TIMESTAMPTZ
            )
            "#,
        )
        .execute(database.pool())
        .await
        .map_err(repository_error)?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_sessionweft_tenant_api_tokens_tenant
            ON sessionweft_tenant_api_tokens (tenant_id, created_at DESC)
            "#,
        )
        .execute(database.pool())
        .await
        .map_err(repository_error)?;
        Ok(Self { database })
    }

    pub async fn issue(
        &self,
        tenant_id: TenantId,
        principal_id: PrincipalId,
        label: impl Into<String>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<IssuedTenantToken, TenancyError> {
        let label = label.into().trim().to_owned();
        if label.is_empty() || label.len() > 128 {
            return Err(TenancyError::Validation(
                "API token label must be between 1 and 128 bytes".into(),
            ));
        }
        if expires_at.is_some_and(|value| value <= Utc::now()) {
            return Err(TenancyError::Validation(
                "API token expiry must be in the future".into(),
            ));
        }
        let id = Uuid::new_v4();
        let raw_token = format!(
            "swt_{}_{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        let token_hash = hash_token(&raw_token);
        let created_at = Utc::now();
        let mut transaction = self
            .database
            .begin_tenant(tenant_id)
            .await
            .map_err(database_error)?;
        let membership_exists = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM sessionweft_tenant_memberships
                WHERE tenant_id = $1 AND principal_id = $2
            )
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(principal_id.as_str())
        .fetch_one(&mut *transaction)
        .await
        .map_err(repository_error)?;
        if !membership_exists {
            transaction.rollback().await.map_err(repository_error)?;
            return Err(TenancyError::AccessDenied {
                tenant_id,
                principal_id,
            });
        }
        sqlx::query(
            r#"
            INSERT INTO sessionweft_tenant_api_tokens (
                id, tenant_id, principal_id, label, token_hash, expires_at, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(id)
        .bind(tenant_id.as_uuid())
        .bind(principal_id.as_str())
        .bind(&label)
        .bind(token_hash.as_slice())
        .bind(expires_at)
        .bind(created_at)
        .execute(&mut *transaction)
        .await
        .map_err(repository_error)?;
        transaction.commit().await.map_err(repository_error)?;
        Ok(IssuedTenantToken {
            id,
            tenant_id,
            principal_id,
            label,
            raw_token,
            expires_at,
            created_at,
        })
    }

    pub async fn resolve(&self, raw_token: &str) -> Result<Option<ResolvedTenantToken>, TenancyError> {
        if !raw_token.starts_with("swt_") || raw_token.len() > 256 {
            return Ok(None);
        }
        let token_hash = hash_token(raw_token);
        let row = sqlx::query(
            r#"
            UPDATE sessionweft_tenant_api_tokens
            SET last_used_at = NOW()
            WHERE token_hash = $1
              AND revoked_at IS NULL
              AND (expires_at IS NULL OR expires_at > NOW())
            RETURNING id, tenant_id, principal_id, label, expires_at
            "#,
        )
        .bind(token_hash.as_slice())
        .fetch_optional(self.database.pool())
        .await
        .map_err(repository_error)?;
        row.map(|row| {
            Ok(ResolvedTenantToken {
                id: row.try_get("id").map_err(repository_error)?,
                tenant_id: TenantId::from_uuid(
                    row.try_get("tenant_id").map_err(repository_error)?,
                ),
                principal_id: PrincipalId::parse(
                    row.try_get::<String, _>("principal_id")
                        .map_err(repository_error)?,
                )?,
                label: row.try_get("label").map_err(repository_error)?,
                expires_at: row.try_get("expires_at").map_err(repository_error)?,
            })
        })
        .transpose()
    }

    pub async fn revoke(
        &self,
        tenant_id: TenantId,
        token_id: Uuid,
    ) -> Result<bool, TenancyError> {
        let mut transaction = self
            .database
            .begin_tenant(tenant_id)
            .await
            .map_err(database_error)?;
        let result = sqlx::query(
            r#"
            UPDATE sessionweft_tenant_api_tokens
            SET revoked_at = COALESCE(revoked_at, NOW())
            WHERE id = $1 AND tenant_id = $2
            "#,
        )
        .bind(token_id)
        .bind(tenant_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(repository_error)?;
        transaction.commit().await.map_err(repository_error)?;
        Ok(result.rows_affected() == 1)
    }
}

fn hash_token(raw_token: &str) -> [u8; 32] {
    Sha256::digest(raw_token.as_bytes()).into()
}

fn repository_error(error: sqlx::Error) -> TenancyError {
    TenancyError::Repository(error.to_string())
}

fn database_error(error: crate::database::SaasPostgresError) -> TenancyError {
    TenancyError::Repository(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_hash_is_stable_and_raw_token_is_not_stored() {
        assert_eq!(hash_token("swt_example"), hash_token("swt_example"));
        assert_ne!(hash_token("swt_example"), hash_token("swt_other"));
    }
}
