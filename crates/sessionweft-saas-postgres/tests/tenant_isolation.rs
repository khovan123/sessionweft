use std::{collections::BTreeMap, sync::Arc};

use chrono::{Duration, Utc};
use sessionweft_billing::{
    BillingInterval, BillingPlan, BillingRepository, Money, PlanId, Subscription,
};
use sessionweft_saas_postgres::{
    PostgresBillingRepository, PostgresTenantAuthRepository, PostgresTenantRepository,
    SaasPostgresDatabase,
};
use sessionweft_tenancy::{
    PrincipalId, QuotaDimension, ResourceKind, TenantQuota, TenantRepository, TenantService,
};
use uuid::Uuid;

fn postgres_url() -> String {
    std::env::var("SESSIONWEFT_TEST_POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://sessionweft:sessionweft@127.0.0.1:5432/sessionweft".into())
}

#[tokio::test]
#[ignore = "requires PostgreSQL service"]
async fn rls_resource_ownership_and_quota_reservations_are_tenant_isolated() {
    let database = SaasPostgresDatabase::connect(&postgres_url())
        .await
        .expect("database");
    let repository = Arc::new(PostgresTenantRepository::new(database.clone()));
    let service = TenantService::new(Arc::clone(&repository));
    let suffix = Uuid::new_v4().simple().to_string();
    let (left, left_owner) = service
        .bootstrap(
            format!("left-{suffix}"),
            "Left tenant",
            PrincipalId::parse(format!("left-{suffix}@example.com")).expect("principal"),
        )
        .await
        .expect("left tenant");
    let (right, _) = service
        .bootstrap(
            format!("right-{suffix}"),
            "Right tenant",
            PrincipalId::parse(format!("right-{suffix}@example.com")).expect("principal"),
        )
        .await
        .expect("right tenant");

    repository
        .set_quota(&TenantQuota {
            tenant_id: left.id,
            dimension: QuotaDimension::ProviderTokens,
            hard_limit: 10,
            updated_at: Utc::now(),
        })
        .await
        .expect("quota");
    let context = service
        .context(left.id, &left_owner.principal_id, Uuid::new_v4())
        .await
        .expect("context");
    let first = service
        .reserve(&context, QuotaDimension::ProviderTokens, 7, "usage-1")
        .await
        .expect("first reservation");
    let replay = service
        .reserve(&context, QuotaDimension::ProviderTokens, 7, "usage-1")
        .await
        .expect("replay");
    assert_eq!(first, replay);
    assert!(
        service
            .reserve(&context, QuotaDimension::ProviderTokens, 4, "usage-2")
            .await
            .is_err()
    );

    let resource_id = format!("session-{suffix}");
    repository
        .bind_resource(left.id, ResourceKind::Session, &resource_id)
        .await
        .expect("bind resource");
    assert!(
        repository
            .owns_resource(left.id, ResourceKind::Session, &resource_id)
            .await
            .expect("left ownership")
    );
    assert!(
        !repository
            .owns_resource(right.id, ResourceKind::Session, &resource_id)
            .await
            .expect("right ownership")
    );

    let mut right_transaction = database
        .begin_tenant(right.id)
        .await
        .expect("right transaction");
    let visible =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sessionweft_tenants WHERE id = $1")
            .bind(left.id.as_uuid())
            .fetch_one(&mut *right_transaction)
            .await
            .expect("RLS query");
    right_transaction.commit().await.expect("commit");
    assert_eq!(visible, 0, "RLS must hide another tenant's row");
}

#[tokio::test]
#[ignore = "requires PostgreSQL service"]
async fn billing_repository_cannot_cross_tenant_boundary() {
    let database = SaasPostgresDatabase::connect(&postgres_url())
        .await
        .expect("database");
    let tenant_repository = Arc::new(PostgresTenantRepository::new(database.clone()));
    let service = TenantService::new(Arc::clone(&tenant_repository));
    let suffix = Uuid::new_v4().simple().to_string();
    let (left, _) = service
        .bootstrap(
            format!("bill-left-{suffix}"),
            "Billing left",
            PrincipalId::parse(format!("owner-left-{suffix}")).expect("principal"),
        )
        .await
        .expect("left tenant");
    let (right, _) = service
        .bootstrap(
            format!("bill-right-{suffix}"),
            "Billing right",
            PrincipalId::parse(format!("owner-right-{suffix}")).expect("principal"),
        )
        .await
        .expect("right tenant");
    let left_billing = PostgresBillingRepository::new(database.clone(), left.id);
    let right_billing = PostgresBillingRepository::new(database, right.id);
    let plan = BillingPlan {
        id: PlanId::parse(format!("team-{suffix}")).expect("plan"),
        display_name: "Team".into(),
        base_price: Money::new("USD", 2_000).expect("money"),
        interval: BillingInterval::Monthly,
        entitlements: BTreeMap::from([("active_agents".into(), 50)]),
        meters: BTreeMap::new(),
        active: true,
    };
    left_billing.upsert_plan(&plan).await.expect("plan upsert");
    let now = Utc::now();
    let subscription =
        Subscription::pending(left.id, plan.id, "stripe", now, now + Duration::days(30))
            .expect("subscription");
    let subscription = left_billing
        .create_subscription(&subscription)
        .await
        .expect("create subscription");
    assert!(
        right_billing
            .subscription(left.id, subscription.id)
            .await
            .is_err(),
        "tenant-bound billing repository must reject another tenant ID"
    );
    assert!(
        right_billing
            .subscription(right.id, subscription.id)
            .await
            .expect("right lookup")
            .is_none(),
        "RLS must hide the left subscription from the right tenant"
    );
}

#[tokio::test]
#[ignore = "requires PostgreSQL service"]
async fn tenant_token_is_returned_once_resolved_by_hash_and_revoked() {
    let database = SaasPostgresDatabase::connect(&postgres_url())
        .await
        .expect("database");
    let tenant_repository = Arc::new(PostgresTenantRepository::new(database.clone()));
    let service = TenantService::new(Arc::clone(&tenant_repository));
    let auth = PostgresTenantAuthRepository::new(database.clone())
        .await
        .expect("auth repository");
    let suffix = Uuid::new_v4().simple().to_string();
    let principal = PrincipalId::parse(format!("token-owner-{suffix}")).expect("principal");
    let (tenant, _) = service
        .bootstrap(
            format!("token-{suffix}"),
            "Token tenant",
            principal.clone(),
        )
        .await
        .expect("tenant");
    let issued = auth
        .issue(tenant.id, principal.clone(), "integration", None)
        .await
        .expect("issue token");
    assert!(issued.raw_token.starts_with("swt_"));

    let raw_is_persisted = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM sessionweft_tenant_api_tokens WHERE encode(token_hash, 'hex') = encode($1::bytea, 'hex'))",
    )
    .bind(issued.raw_token.as_bytes())
    .fetch_one(database.pool())
    .await
    .expect("raw token persistence check");
    assert!(!raw_is_persisted, "raw tenant token must never be stored");

    let resolved = auth
        .resolve(&issued.raw_token)
        .await
        .expect("resolve token")
        .expect("resolved token");
    assert_eq!(resolved.tenant_id, tenant.id);
    assert_eq!(resolved.principal_id, principal);
    assert!(auth.revoke(tenant.id, issued.id).await.expect("revoke"));
    assert!(
        auth.resolve(&issued.raw_token)
            .await
            .expect("resolve revoked token")
            .is_none()
    );
}
