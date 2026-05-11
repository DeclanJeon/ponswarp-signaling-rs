//! Stripe Checkout and webhook handling for Cloud Drop entitlements.

use crate::config::Config;
use crate::database::{CloudDatabase, PaidEntitlementInput};
use crate::state::AppState;
use anyhow::{bail, Result};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct BillingClient {
    http: reqwest::Client,
    stripe_secret_key: String,
    stripe_webhook_secret: String,
    public_app_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutRequest {
    mode: CheckoutMode,
    sku: String,
    return_url: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum CheckoutMode {
    Payment,
    Subscription,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutResponse {
    checkout_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BillingErrorBody {
    error: String,
}

impl BillingClient {
    pub fn from_config(config: &Config) -> Result<Option<Self>> {
        if !config.cloud.billing_enabled {
            return Ok(None);
        }

        if config.billing.stripe_secret_key.trim().is_empty() {
            bail!("PONSWARP_BILLING_ENABLED=true requires STRIPE_SECRET_KEY");
        }
        if config.billing.stripe_webhook_secret.trim().is_empty() {
            bail!("PONSWARP_BILLING_ENABLED=true requires STRIPE_WEBHOOK_SECRET");
        }
        if config.billing.public_app_url.trim().is_empty() {
            bail!("PONSWARP_BILLING_ENABLED=true requires PONSWARP_PUBLIC_APP_URL");
        }

        Ok(Some(Self {
            http: reqwest::Client::new(),
            stripe_secret_key: config.billing.stripe_secret_key.clone(),
            stripe_webhook_secret: config.billing.stripe_webhook_secret.clone(),
            public_app_url: config
                .billing
                .public_app_url
                .trim_end_matches('/')
                .to_string(),
        }))
    }

    async fn create_checkout_session(
        &self,
        request: CheckoutRequest,
    ) -> Result<CheckoutResponse, BillingError> {
        let plan = paid_plan(&request.sku)
            .ok_or_else(|| BillingError::bad_request("Unknown Cloud Drop plan"))?;
        if request.mode != plan.checkout_mode {
            return Err(BillingError::bad_request(
                "Checkout mode does not match plan",
            ));
        }
        let return_url = self.validate_return_url(&request.return_url)?;
        let success_url = append_query(
            &return_url,
            "checkout=success&cloudEntitlement={CHECKOUT_SESSION_ID}",
        );
        let cancel_url = append_query(&return_url, "checkout=cancelled");

        let mut form = vec![
            ("mode".to_string(), plan.stripe_mode().to_string()),
            ("success_url".to_string(), success_url),
            ("cancel_url".to_string(), cancel_url),
            ("line_items[0][quantity]".to_string(), "1".to_string()),
            (
                "line_items[0][price_data][currency]".to_string(),
                "krw".to_string(),
            ),
            (
                "line_items[0][price_data][unit_amount]".to_string(),
                plan.price_krw.to_string(),
            ),
            (
                "line_items[0][price_data][product_data][name]".to_string(),
                plan.label.to_string(),
            ),
            ("metadata[sku]".to_string(), plan.sku.to_string()),
            (
                "metadata[kind]".to_string(),
                "ponswarp_cloud_drop".to_string(),
            ),
        ];
        if request.mode == CheckoutMode::Subscription {
            form.push((
                "line_items[0][price_data][recurring][interval]".to_string(),
                "month".to_string(),
            ));
        }

        let value: Value = self
            .http
            .post("https://api.stripe.com/v1/checkout/sessions")
            .bearer_auth(&self.stripe_secret_key)
            .form(&form)
            .send()
            .await
            .map_err(BillingError::internal)?
            .error_for_status()
            .map_err(BillingError::internal)?
            .json()
            .await
            .map_err(BillingError::internal)?;

        let Some(url) = value.get("url").and_then(Value::as_str) else {
            return Err(BillingError::internal(
                "Stripe did not return a checkout URL",
            ));
        };

        Ok(CheckoutResponse {
            checkout_url: url.to_string(),
        })
    }

    fn validate_webhook(&self, body: &[u8], signature: &str) -> Result<Value, BillingError> {
        verify_stripe_signature(body, signature, &self.stripe_webhook_secret)?;
        serde_json::from_slice(body).map_err(|error| BillingError::bad_webhook(error.to_string()))
    }

    fn validate_return_url(&self, return_url: &str) -> Result<String, BillingError> {
        let trimmed = return_url.trim();
        if !trimmed.starts_with(&self.public_app_url) {
            return Err(BillingError::bad_request(
                "Checkout returnUrl is not allowed for this deployment",
            ));
        }
        Ok(trimmed.to_string())
    }
}

pub async fn create_checkout(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CheckoutRequest>,
) -> Response {
    let Some(billing) = state.billing.as_ref() else {
        return BillingError::unavailable("Billing is not enabled").into_response();
    };
    if state.cloud_db.is_none() {
        return BillingError::unavailable("Billing database is not available").into_response();
    }

    match billing.create_checkout_session(request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => error.into_response(),
    }
}

pub async fn stripe_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(billing) = state.billing.as_ref() else {
        return BillingError::unavailable("Billing is not enabled").into_response();
    };
    let Some(database) = state.cloud_db.as_ref() else {
        return BillingError::unavailable("Billing database is not available").into_response();
    };
    let Some(signature) = headers
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok())
    else {
        return BillingError::bad_webhook("Missing Stripe-Signature header").into_response();
    };

    let event = match billing.validate_webhook(&body, signature) {
        Ok(event) => event,
        Err(error) => return error.into_response(),
    };

    match process_stripe_event(database, &event).await {
        Ok(()) => Json(serde_json::json!({ "received": true })).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn process_stripe_event(database: &CloudDatabase, event: &Value) -> Result<(), BillingError> {
    let event_id = event
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| BillingError::bad_webhook("Stripe event is missing id"))?;
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| BillingError::bad_webhook("Stripe event is missing type"))?;
    let event_created = event
        .get("created")
        .and_then(Value::as_u64)
        .unwrap_or_else(unix_now);

    let is_new = database
        .try_record_stripe_event(event_id, event_type, event_created, unix_now())
        .await
        .map_err(BillingError::internal)?;
    if !is_new {
        return Ok(());
    }

    if event_type != "checkout.session.completed" {
        return Ok(());
    }

    let session = event
        .pointer("/data/object")
        .ok_or_else(|| BillingError::bad_webhook("Stripe event is missing checkout session"))?;
    let session_id = session
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| BillingError::bad_webhook("Checkout session is missing id"))?;
    let sku = session
        .pointer("/metadata/sku")
        .and_then(Value::as_str)
        .ok_or_else(|| BillingError::bad_webhook("Checkout session is missing sku metadata"))?;
    let Some(plan) = paid_plan(sku) else {
        return Err(BillingError::bad_webhook(
            "Checkout session has unknown sku",
        ));
    };
    let email = session
        .pointer("/customer_details/email")
        .and_then(Value::as_str)
        .or_else(|| session.get("customer_email").and_then(Value::as_str));

    database
        .upsert_paid_entitlement(PaidEntitlementInput {
            checkout_session_id: session_id,
            email,
            sku: plan.sku,
            max_total_bytes: plan.max_total_bytes,
            max_file_bytes: plan.max_file_bytes,
            retention_seconds: plan.retention_seconds,
            payment_intent_id: session.get("payment_intent").and_then(Value::as_str),
            subscription_id: session.get("subscription").and_then(Value::as_str),
            created_at: event_created,
        })
        .await
        .map_err(BillingError::internal)?;

    Ok(())
}

#[derive(Clone, Copy)]
struct PaidPlan {
    sku: &'static str,
    label: &'static str,
    price_krw: u64,
    max_total_bytes: u64,
    max_file_bytes: u64,
    retention_seconds: u64,
    checkout_mode: CheckoutMode,
}

fn paid_plan(sku: &str) -> Option<PaidPlan> {
    const GB: u64 = 1024 * 1024 * 1024;
    const TB: u64 = 1024 * GB;
    match sku {
        "drop_100gb_3d" => Some(PaidPlan {
            sku: "drop_100gb_3d",
            label: "100GB Drop Pass",
            price_krw: 1_900,
            max_total_bytes: 100 * GB,
            max_file_bytes: 100 * GB,
            retention_seconds: 3 * 24 * 60 * 60,
            checkout_mode: CheckoutMode::Payment,
        }),
        "drop_500gb_7d" => Some(PaidPlan {
            sku: "drop_500gb_7d",
            label: "500GB Drop Pass",
            price_krw: 4_900,
            max_total_bytes: 500 * GB,
            max_file_bytes: 500 * GB,
            retention_seconds: 7 * 24 * 60 * 60,
            checkout_mode: CheckoutMode::Payment,
        }),
        "drop_1tb_7d" => Some(PaidPlan {
            sku: "drop_1tb_7d",
            label: "1TB Drop Pass",
            price_krw: 9_900,
            max_total_bytes: TB,
            max_file_bytes: TB,
            retention_seconds: 7 * 24 * 60 * 60,
            checkout_mode: CheckoutMode::Payment,
        }),
        "pro_monthly_krw_9900" => Some(PaidPlan {
            sku: "pro_monthly_krw_9900",
            label: "PonsWarp Pro",
            price_krw: 9_900,
            max_total_bytes: TB,
            max_file_bytes: TB,
            retention_seconds: 7 * 24 * 60 * 60,
            checkout_mode: CheckoutMode::Subscription,
        }),
        _ => None,
    }
}

impl PaidPlan {
    fn stripe_mode(&self) -> &'static str {
        match self.checkout_mode {
            CheckoutMode::Payment => "payment",
            CheckoutMode::Subscription => "subscription",
        }
    }
}

fn verify_stripe_signature(
    body: &[u8],
    signature_header: &str,
    secret: &str,
) -> Result<(), BillingError> {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in signature_header.split(',') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        match key {
            "t" => timestamp = Some(value),
            "v1" => signatures.push(value),
            _ => {}
        }
    }
    let Some(timestamp) = timestamp else {
        return Err(BillingError::bad_webhook(
            "Stripe signature is missing timestamp",
        ));
    };
    if signatures.is_empty() {
        return Err(BillingError::bad_webhook("Stripe signature is missing v1"));
    }

    let mut signed_payload = timestamp.as_bytes().to_vec();
    signed_payload.push(b'.');
    signed_payload.extend_from_slice(body);

    for signature in signatures {
        let Ok(expected) = hex::decode(signature) else {
            continue;
        };
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|_| BillingError::bad_webhook("Invalid webhook secret"))?;
        mac.update(&signed_payload);
        if mac.verify_slice(&expected).is_ok() {
            return Ok(());
        }
    }

    Err(BillingError::bad_webhook(
        "Stripe webhook signature verification failed",
    ))
}

fn append_query(url: &str, query: &str) -> String {
    if url.contains('?') {
        format!("{url}&{query}")
    } else {
        format!("{url}?{query}")
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug)]
struct BillingError {
    status: StatusCode,
    message: String,
}

impl BillingError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: message.into(),
        }
    }

    fn bad_webhook(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(error = %error, "Billing error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Billing request failed".to_string(),
        }
    }
}

impl IntoResponse for BillingError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(BillingErrorBody {
                error: self.message,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stripe_signature_accepts_valid_hmac() {
        let body = br#"{"id":"evt_test"}"#;
        let secret = "whsec_test";
        let timestamp = "1710000000";
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(body);
        let signature = hex::encode(mac.finalize().into_bytes());
        let header = format!("t={timestamp},v1={signature}");

        verify_stripe_signature(body, &header, secret).unwrap();
    }

    #[test]
    fn stripe_signature_rejects_tampered_payload() {
        let body = br#"{"id":"evt_test"}"#;
        let secret = "whsec_test";
        let header = "t=1710000000,v1=000000";

        assert!(verify_stripe_signature(body, header, secret).is_err());
    }
}
