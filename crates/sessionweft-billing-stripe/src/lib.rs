use std::{collections::BTreeMap, str::FromStr, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use hmac::{Hmac, Mac};
use reqwest::StatusCode;
use serde::Deserialize;
use sessionweft_billing::{
    BillingError, BillingPlan, BillingProvider, MeterName, PlanId, ProviderSubscription,
    ProviderUsageReceipt, ProviderWebhookEvent, Subscription, SubscriptionStatus, UsageRecord,
};
use sessionweft_tenancy::TenantId;
use sha2::Sha256;

const DEFAULT_API_BASE: &str = "https://api.stripe.com";
const DEFAULT_SIGNATURE_TOLERANCE_SECONDS: i64 = 300;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub struct StripeBillingConfig {
    pub secret_key: String,
    pub webhook_secret: String,
    pub api_base: String,
    pub price_ids: BTreeMap<PlanId, String>,
    pub meter_event_names: BTreeMap<MeterName, String>,
    pub request_timeout: Duration,
    pub signature_tolerance: Duration,
}

impl StripeBillingConfig {
    pub fn validate(&self) -> Result<(), BillingError> {
        validate_secret("Stripe secret key", &self.secret_key)?;
        validate_secret("Stripe webhook secret", &self.webhook_secret)?;
        let endpoint = reqwest::Url::parse(&self.api_base).map_err(|error| {
            BillingError::Validation(format!("invalid Stripe API base: {error}"))
        })?;
        if endpoint.scheme() != "https"
            && !(endpoint.scheme() == "http"
                && endpoint
                    .host_str()
                    .is_some_and(|host| matches!(host, "127.0.0.1" | "localhost" | "::1")))
        {
            return Err(BillingError::Validation(
                "Stripe API base must use HTTPS except for loopback tests".into(),
            ));
        }
        if self.request_timeout.is_zero() || self.request_timeout > Duration::from_secs(300) {
            return Err(BillingError::Validation(
                "Stripe request timeout must be between 1 ms and 5 minutes".into(),
            ));
        }
        if self.signature_tolerance.is_zero()
            || self.signature_tolerance > Duration::from_secs(3_600)
        {
            return Err(BillingError::Validation(
                "Stripe signature tolerance must be between 1 second and 1 hour".into(),
            ));
        }
        for (plan, price) in &self.price_ids {
            if price.trim().is_empty() || price.len() > 255 {
                return Err(BillingError::Validation(format!(
                    "Stripe price ID for plan {plan} is invalid"
                )));
            }
        }
        for (meter, event_name) in &self.meter_event_names {
            if event_name.trim().is_empty() || event_name.len() > 255 {
                return Err(BillingError::Validation(format!(
                    "Stripe meter event name for {meter} is invalid"
                )));
            }
        }
        Ok(())
    }
}

impl Default for StripeBillingConfig {
    fn default() -> Self {
        Self {
            secret_key: String::new(),
            webhook_secret: String::new(),
            api_base: DEFAULT_API_BASE.into(),
            price_ids: BTreeMap::new(),
            meter_event_names: BTreeMap::new(),
            request_timeout: Duration::from_secs(30),
            signature_tolerance: Duration::from_secs(
                u64::try_from(DEFAULT_SIGNATURE_TOLERANCE_SECONDS).unwrap_or(300),
            ),
        }
    }
}

#[derive(Clone)]
pub struct StripeBillingProvider {
    config: Arc<StripeBillingConfig>,
    client: reqwest::Client,
}

impl StripeBillingProvider {
    pub fn new(config: StripeBillingConfig) -> Result<Self, BillingError> {
        config.validate()?;
        let client = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .user_agent("sessionweft-billing/0.2")
            .build()
            .map_err(|error| BillingError::Provider(error.to_string()))?;
        Ok(Self {
            config: Arc::new(config),
            client,
        })
    }

    pub fn verify_webhook(
        &self,
        payload: &[u8],
        signature_header: &str,
        now: DateTime<Utc>,
    ) -> Result<ProviderWebhookEvent, BillingError> {
        verify_stripe_signature(
            payload,
            signature_header,
            &self.config.webhook_secret,
            now,
            self.config.signature_tolerance,
        )?;
        normalize_event(payload)
    }

    async fn post_form<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        form: &[(String, String)],
        idempotency_key: &str,
    ) -> Result<T, BillingError> {
        let endpoint = format!("{}{}", self.config.api_base.trim_end_matches('/'), path);
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(&self.config.secret_key)
            .header("Idempotency-Key", idempotency_key)
            .form(form)
            .send()
            .await
            .map_err(map_transport_error)?;
        let status = response.status();
        let request_id = response
            .headers()
            .get("request-id")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("unknown")
            .to_owned();
        let body = response.bytes().await.map_err(map_transport_error)?;
        if !status.is_success() {
            return Err(provider_http_error(status, &request_id, &body));
        }
        serde_json::from_slice(&body).map_err(|error| {
            BillingError::Provider(format!(
                "Stripe response {request_id} was invalid JSON: {error}"
            ))
        })
    }
}

#[async_trait]
impl BillingProvider for StripeBillingProvider {
    fn name(&self) -> &str {
        "stripe"
    }

    async fn create_subscription(
        &self,
        tenant_id: TenantId,
        plan: &BillingPlan,
        idempotency_key: &str,
    ) -> Result<ProviderSubscription, BillingError> {
        let price_id = self
            .config
            .price_ids
            .get(&plan.id)
            .ok_or_else(|| {
                BillingError::Validation(format!(
                    "Stripe price ID is not configured for plan {}",
                    plan.id
                ))
            })?
            .clone();
        let customer: StripeIdResponse = self
            .post_form(
                "/v1/customers",
                &[
                    ("name".into(), format!("SessionWeft tenant {tenant_id}")),
                    ("metadata[tenant_id]".into(), tenant_id.to_string()),
                ],
                &format!("{idempotency_key}:customer"),
            )
            .await?;
        let subscription: StripeSubscriptionResponse = self
            .post_form(
                "/v1/subscriptions",
                &[
                    ("customer".into(), customer.id.clone()),
                    ("items[0][price]".into(), price_id),
                    ("payment_behavior".into(), "default_incomplete".into()),
                    ("metadata[tenant_id]".into(), tenant_id.to_string()),
                    ("metadata[sessionweft_plan_id]".into(), plan.id.to_string()),
                ],
                &format!("{idempotency_key}:subscription"),
            )
            .await?;
        Ok(ProviderSubscription {
            customer_id: customer.id,
            subscription_id: subscription.id,
            status: map_subscription_status(&subscription.status)?,
        })
    }

    async fn report_usage(
        &self,
        subscription: &Subscription,
        usage: &UsageRecord,
    ) -> Result<ProviderUsageReceipt, BillingError> {
        let customer_id = subscription
            .provider_customer_id
            .as_deref()
            .ok_or_else(|| {
                BillingError::Validation("Stripe customer ID is missing from subscription".into())
            })?;
        let event_name = self
            .config
            .meter_event_names
            .get(&usage.meter)
            .ok_or_else(|| {
                BillingError::Validation(format!(
                    "Stripe event name is not configured for meter {}",
                    usage.meter
                ))
            })?;
        let response: StripeMeterEventResponse = self
            .post_form(
                "/v1/billing/meter_events",
                &[
                    ("event_name".into(), event_name.clone()),
                    ("payload[stripe_customer_id]".into(), customer_id.to_owned()),
                    ("payload[value]".into(), usage.quantity.to_string()),
                    ("identifier".into(), usage.idempotency_key.clone()),
                    (
                        "timestamp".into(),
                        usage.occurred_at.timestamp().to_string(),
                    ),
                ],
                &usage.idempotency_key,
            )
            .await?;
        Ok(ProviderUsageReceipt {
            provider_event_id: response
                .identifier
                .unwrap_or_else(|| usage.idempotency_key.clone()),
        })
    }
}

#[derive(Debug, Deserialize)]
struct StripeIdResponse {
    id: String,
}

#[derive(Debug, Deserialize)]
struct StripeSubscriptionResponse {
    id: String,
    status: String,
}

#[derive(Debug, Deserialize)]
struct StripeMeterEventResponse {
    identifier: Option<String>,
}

fn verify_stripe_signature(
    payload: &[u8],
    signature_header: &str,
    secret: &str,
    now: DateTime<Utc>,
    tolerance: Duration,
) -> Result<(), BillingError> {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in signature_header.split(',') {
        let Some((key, value)) = part.trim().split_once('=') else {
            continue;
        };
        match key {
            "t" => {
                timestamp = Some(value.parse::<i64>().map_err(|_| {
                    BillingError::Validation("Stripe signature timestamp is invalid".into())
                })?);
            }
            "v1" => signatures.push(value.to_owned()),
            _ => {}
        }
    }
    let timestamp = timestamp.ok_or_else(|| {
        BillingError::Validation("Stripe signature header is missing timestamp".into())
    })?;
    if signatures.is_empty() {
        return Err(BillingError::Validation(
            "Stripe signature header is missing v1 signature".into(),
        ));
    }
    let tolerance = i64::try_from(tolerance.as_secs())
        .map_err(|_| BillingError::Validation("Stripe signature tolerance is too large".into()))?;
    if (now.timestamp() - timestamp).abs() > tolerance {
        return Err(BillingError::Validation(
            "Stripe webhook signature timestamp is outside tolerance".into(),
        ));
    }
    let mut signed = timestamp.to_string().into_bytes();
    signed.push(b'.');
    signed.extend_from_slice(payload);
    let valid = signatures.into_iter().any(|signature| {
        let Ok(expected) = hex::decode(signature) else {
            return false;
        };
        let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
            return false;
        };
        mac.update(&signed);
        mac.verify_slice(&expected).is_ok()
    });
    if valid {
        Ok(())
    } else {
        Err(BillingError::AccessDenied)
    }
}

fn normalize_event(payload: &[u8]) -> Result<ProviderWebhookEvent, BillingError> {
    let value: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|error| BillingError::Validation(format!("invalid Stripe event JSON: {error}")))?;
    let event_id = string_at(&value, &["id"])?;
    let event_type = string_at(&value, &["type"])?;
    let created = value
        .get("created")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| {
            BillingError::Validation("Stripe event created timestamp is missing".into())
        })?;
    let object = value
        .pointer("/data/object")
        .ok_or_else(|| BillingError::Validation("Stripe event data.object is missing".into()))?;
    let tenant_id = object
        .pointer("/metadata/tenant_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| BillingError::Validation("Stripe object tenant metadata is missing".into()))?
        .parse::<TenantId>()
        .map_err(|error| BillingError::Validation(format!("invalid tenant metadata: {error}")))?;
    let mut normalized = serde_json::Map::new();
    if let Some(id) = object.get("id").and_then(serde_json::Value::as_str) {
        normalized.insert(
            "provider_subscription_id".into(),
            serde_json::Value::String(id.to_owned()),
        );
    }
    if let Some(status) = object.get("status").and_then(serde_json::Value::as_str) {
        if let Ok(status) = map_subscription_status(status) {
            normalized.insert(
                "status".into(),
                serde_json::Value::String(status.to_string()),
            );
        }
    }
    Ok(ProviderWebhookEvent {
        provider: "stripe".into(),
        event_id,
        event_type,
        tenant_id,
        payload: serde_json::Value::Object(normalized),
        occurred_at: Utc
            .timestamp_opt(created, 0)
            .single()
            .ok_or_else(|| BillingError::Validation("Stripe event timestamp is invalid".into()))?,
    })
}

fn string_at(value: &serde_json::Value, path: &[&str]) -> Result<String, BillingError> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment).ok_or_else(|| {
            BillingError::Validation(format!("Stripe event field {} is missing", path.join(".")))
        })?;
    }
    current.as_str().map(ToOwned::to_owned).ok_or_else(|| {
        BillingError::Validation(format!(
            "Stripe event field {} is not a string",
            path.join(".")
        ))
    })
}

fn map_subscription_status(value: &str) -> Result<SubscriptionStatus, BillingError> {
    match value {
        "trialing" => Ok(SubscriptionStatus::Trialing),
        "active" => Ok(SubscriptionStatus::Active),
        "past_due" | "unpaid" | "incomplete" | "incomplete_expired" => {
            Ok(SubscriptionStatus::PastDue)
        }
        "paused" => Ok(SubscriptionStatus::Paused),
        "canceled" | "cancelled" => Ok(SubscriptionStatus::Cancelled),
        "pending" => Ok(SubscriptionStatus::Pending),
        _ => SubscriptionStatus::from_str(value),
    }
}

fn validate_secret(name: &str, value: &str) -> Result<(), BillingError> {
    if value.trim().len() < 8 || value.len() > 512 {
        return Err(BillingError::Validation(format!(
            "{name} must be between 8 and 512 bytes"
        )));
    }
    Ok(())
}

fn map_transport_error(error: reqwest::Error) -> BillingError {
    if error.is_timeout() || error.is_connect() || error.is_request() {
        BillingError::ProviderUncertain(error.to_string())
    } else {
        BillingError::Provider(error.to_string())
    }
}

fn provider_http_error(status: StatusCode, request_id: &str, body: &[u8]) -> BillingError {
    let summary = String::from_utf8_lossy(body)
        .chars()
        .take(1_024)
        .collect::<String>();
    BillingError::Provider(format!(
        "Stripe request {request_id} returned {status}: {summary}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_signature_is_verified_and_normalized() {
        let tenant_id = TenantId::new();
        let payload = serde_json::json!({
            "id": "evt_test",
            "type": "customer.subscription.updated",
            "created": 1_700_000_000_i64,
            "data": {
                "object": {
                    "id": "sub_test",
                    "status": "active",
                    "metadata": {"tenant_id": tenant_id.to_string()}
                }
            }
        })
        .to_string();
        let secret = "whsec_test_secret";
        let signed = format!("{}.{}", 1_700_000_000_i64, payload);
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC key");
        mac.update(signed.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());
        let header = format!("t=1700000000,v1={signature}");
        verify_stripe_signature(
            payload.as_bytes(),
            &header,
            secret,
            Utc.timestamp_opt(1_700_000_010, 0).single().expect("time"),
            Duration::from_secs(300),
        )
        .expect("signature");
        let event = normalize_event(payload.as_bytes()).expect("event");
        assert_eq!(event.tenant_id, tenant_id);
        assert_eq!(event.payload["status"], "active");
    }

    #[test]
    fn stale_webhook_signature_is_rejected() {
        let payload = b"{}";
        let secret = "whsec_test_secret";
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC key");
        mac.update(b"100.{}");
        let header = format!("t=100,v1={}", hex::encode(mac.finalize().into_bytes()));
        assert!(
            verify_stripe_signature(
                payload,
                &header,
                secret,
                Utc.timestamp_opt(1_000, 0).single().expect("time"),
                Duration::from_secs(300),
            )
            .is_err()
        );
    }
}
