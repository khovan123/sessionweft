use std::time::Duration;

use sqlx::postgres::PgPoolOptions;

use crate::{PostgresServiceDatabase, ServiceDatabaseError};

impl PostgresServiceDatabase {
    pub async fn connect_in_schema(
        database_url: &str,
        instance_id: impl Into<String>,
        schema: &str,
    ) -> Result<Self, ServiceDatabaseError> {
        let instance_id = validate_instance_id(instance_id.into())?;
        validate_schema_name(schema)?;
        let quoted_schema = format!("\"{schema}\"");

        let admin = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(10))
            .connect(database_url)
            .await?;
        sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {quoted_schema}"))
            .execute(&admin)
            .await?;
        admin.close().await;

        let search_path = format!("SET search_path TO {quoted_schema}, public");
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(move |connection, _metadata| {
                let search_path = search_path.clone();
                Box::pin(async move {
                    sqlx::Executor::execute(&mut *connection, search_path.as_str()).await?;
                    Ok(())
                })
            })
            .connect(database_url)
            .await?;
        let database = Self {
            pool,
            instance_id,
            outbox_claim_ttl: Duration::from_secs(30),
        };
        database.migrate().await?;
        Ok(database)
    }
}

fn validate_instance_id(value: String) -> Result<String, ServiceDatabaseError> {
    let value = value.trim().to_owned();
    if value.is_empty() || value.len() > 256 {
        return Err(ServiceDatabaseError::Validation(
            "runtime instance ID must be between 1 and 256 bytes".into(),
        ));
    }
    Ok(value)
}

fn validate_schema_name(value: &str) -> Result<(), ServiceDatabaseError> {
    if value.len() < 3
        || value.len() > 63
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        || !value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
    {
        return Err(ServiceDatabaseError::Validation(
            "PostgreSQL schema name must be 3-63 lowercase letters, numbers or underscores and start with a letter".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_names_are_restricted_to_safe_identifiers() {
        assert!(validate_schema_name("tenant_0123abcd").is_ok());
        assert!(validate_schema_name("Tenant_0123").is_err());
        assert!(validate_schema_name("tenant;drop schema public").is_err());
        assert!(validate_schema_name("1tenant").is_err());
    }
}
