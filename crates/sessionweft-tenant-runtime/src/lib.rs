use std::{collections::HashMap, sync::Arc};

use sessionweft_control_plane::RuntimeControlPlane;
use sessionweft_provider::ProviderRegistry;
use sessionweft_runtime::RuntimeService;
use sessionweft_service_postgres::{
    PostgresAgentRepository, PostgresMemoryRepository, PostgresOrchestrationRepository,
    PostgresServiceDatabase, PostgresSessionRepository, ServiceDatabaseError,
};
use sessionweft_tenancy::TenantId;
use thiserror::Error;
use tokio::sync::RwLock;

pub type TenantControlPlane = RuntimeControlPlane<
    PostgresSessionRepository,
    PostgresAgentRepository,
    PostgresOrchestrationRepository,
    PostgresMemoryRepository,
>;

pub struct TenantRuntime {
    tenant_id: TenantId,
    schema: String,
    database: PostgresServiceDatabase,
    control_plane: Arc<TenantControlPlane>,
}

impl TenantRuntime {
    #[must_use]
    pub const fn tenant_id(&self) -> TenantId {
        self.tenant_id
    }

    #[must_use]
    pub fn schema(&self) -> &str {
        &self.schema
    }

    #[must_use]
    pub fn database(&self) -> &PostgresServiceDatabase {
        &self.database
    }

    #[must_use]
    pub fn control_plane(&self) -> &Arc<TenantControlPlane> {
        &self.control_plane
    }
}

pub struct TenantRuntimeManager {
    database_url: String,
    instance_id: String,
    providers: Arc<ProviderRegistry>,
    runtimes: RwLock<HashMap<TenantId, Arc<TenantRuntime>>>,
}

impl TenantRuntimeManager {
    pub fn new(
        database_url: impl Into<String>,
        instance_id: impl Into<String>,
        providers: Arc<ProviderRegistry>,
    ) -> Result<Self, TenantRuntimeError> {
        let database_url = database_url.into().trim().to_owned();
        let instance_id = instance_id.into().trim().to_owned();
        if database_url.is_empty() {
            return Err(TenantRuntimeError::Validation(
                "tenant Runtime database URL cannot be empty".into(),
            ));
        }
        if instance_id.is_empty() || instance_id.len() > 128 {
            return Err(TenantRuntimeError::Validation(
                "tenant Runtime instance ID must be between 1 and 128 bytes".into(),
            ));
        }
        Ok(Self {
            database_url,
            instance_id,
            providers,
            runtimes: RwLock::new(HashMap::new()),
        })
    }

    pub async fn runtime(
        &self,
        tenant_id: TenantId,
    ) -> Result<Arc<TenantRuntime>, TenantRuntimeError> {
        if let Some(runtime) = self.runtimes.read().await.get(&tenant_id).cloned() {
            return Ok(runtime);
        }
        let mut runtimes = self.runtimes.write().await;
        if let Some(runtime) = runtimes.get(&tenant_id).cloned() {
            return Ok(runtime);
        }
        let runtime = Arc::new(self.build_runtime(tenant_id).await?);
        runtimes.insert(tenant_id, Arc::clone(&runtime));
        Ok(runtime)
    }

    pub async fn evict(&self, tenant_id: TenantId) -> bool {
        self.runtimes.write().await.remove(&tenant_id).is_some()
    }

    pub async fn loaded_tenants(&self) -> Vec<TenantId> {
        let mut tenants = self
            .runtimes
            .read()
            .await
            .keys()
            .copied()
            .collect::<Vec<_>>();
        tenants.sort_unstable();
        tenants
    }

    async fn build_runtime(
        &self,
        tenant_id: TenantId,
    ) -> Result<TenantRuntime, TenantRuntimeError> {
        let schema = tenant_schema(tenant_id);
        let database = PostgresServiceDatabase::connect_in_schema(
            &self.database_url,
            format!("{}:{tenant_id}", self.instance_id),
            &schema,
        )
        .await?;
        let session_repository = Arc::new(PostgresSessionRepository::new(database.clone()));
        let runtime_service = RuntimeService::new(session_repository, Arc::clone(&self.providers));
        let control_plane = RuntimeControlPlane::new(
            runtime_service,
            Arc::new(PostgresAgentRepository::new(database.clone())),
            Arc::new(PostgresOrchestrationRepository::new(database.clone())),
            Arc::new(PostgresMemoryRepository::new(database.clone())),
        );
        Ok(TenantRuntime {
            tenant_id,
            schema,
            database,
            control_plane: Arc::new(control_plane),
        })
    }
}

#[must_use]
pub fn tenant_schema(tenant_id: TenantId) -> String {
    format!("tenant_{}", tenant_id.as_uuid().simple())
}

#[derive(Debug, Error)]
pub enum TenantRuntimeError {
    #[error("tenant Runtime validation failed: {0}")]
    Validation(String),
    #[error("tenant Runtime database failed: {0}")]
    Database(#[from] ServiceDatabaseError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use sessionweft_provider::EchoProvider;

    #[test]
    fn schema_name_is_stable_and_safe() {
        let tenant_id = TenantId::from_uuid(
            uuid::Uuid::parse_str("01234567-89ab-cdef-0123-456789abcdef").expect("UUID"),
        );
        assert_eq!(
            tenant_schema(tenant_id),
            "tenant_0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn manager_requires_database_and_instance_identity() {
        let mut providers = ProviderRegistry::new();
        providers.register(EchoProvider);
        assert!(TenantRuntimeManager::new("", "runtime", Arc::new(providers)).is_err());
    }
}
