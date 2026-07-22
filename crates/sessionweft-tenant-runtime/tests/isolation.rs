use std::sync::Arc;

use sessionweft_control_plane::OperationContext;
use sessionweft_provider::{EchoProvider, ProviderRegistry};
use sessionweft_tenancy::TenantId;
use sessionweft_tenant_runtime::TenantRuntimeManager;

fn postgres_url() -> String {
    std::env::var("SESSIONWEFT_TEST_POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://sessionweft:sessionweft@127.0.0.1:5432/sessionweft".into())
}

fn providers() -> Arc<ProviderRegistry> {
    let mut providers = ProviderRegistry::new();
    providers.register(EchoProvider);
    Arc::new(providers)
}

#[tokio::test]
#[ignore = "requires PostgreSQL service"]
async fn tenant_runtime_schema_isolation_survives_manager_restart() {
    let tenant_a = TenantId::new();
    let tenant_b = TenantId::new();
    let database_url = postgres_url();
    let manager =
        TenantRuntimeManager::new(&database_url, "isolation-a", providers()).expect("manager");

    let runtime_a = manager.runtime(tenant_a).await.expect("tenant A Runtime");
    let runtime_b = manager.runtime(tenant_b).await.expect("tenant B Runtime");
    assert_ne!(runtime_a.schema(), runtime_b.schema());

    let session = runtime_a
        .control_plane()
        .create_session("tenant A Session", &OperationContext::system("test"))
        .await
        .expect("create Session");
    let loaded_a = runtime_a
        .control_plane()
        .get_session(session.id)
        .await
        .expect("load Session from tenant A");
    assert_eq!(loaded_a.id, session.id);
    assert!(
        runtime_b
            .control_plane()
            .get_session(session.id)
            .await
            .is_err(),
        "tenant B must not resolve a Session stored in tenant A's schema"
    );

    drop(manager);
    let restarted = TenantRuntimeManager::new(&database_url, "isolation-restarted", providers())
        .expect("restarted manager");
    let restarted_a = restarted
        .runtime(tenant_a)
        .await
        .expect("restarted tenant A Runtime");
    let resumed = restarted_a
        .control_plane()
        .get_session(session.id)
        .await
        .expect("resume Session after manager restart");
    assert_eq!(resumed.id, session.id);
    assert_eq!(resumed.title, "tenant A Session");
}
