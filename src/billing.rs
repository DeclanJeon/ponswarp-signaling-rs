//! PayPal Checkout and webhook handling for Cloud Drop entitlements.

use crate::config::Config;
use crate::database::{
    CloudDatabase, PayPalOrderEntitlementInput, PayPalSubscriptionEntitlementInput,
};
use crate::state::AppState;
use anyhow::{bail, Result};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct BillingClient {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
    webhook_id: String,
    api_base: String,
    currency: String,
    pro_plan_id: String,
    public_app_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutRequest {
    mode: CheckoutMode,
    sku: String,
    return_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRequest {
    order_id: String,
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
    checkout_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureResponse {
    entitlement_token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BillingErrorBody {
    error: String,
}

#[derive(Default)]
struct JsonPostOptions {
    prefer: Option<&'static str>,
    paypal_request_id: Option<String>,
}

impl JsonPostOptions {
    fn representation() -> Self {
        Self {
            prefer: Some("return=representation"),
            paypal_request_id: None,
        }
    }

    fn with_request_id(mut self, request_id: String) -> Self {
        self.paypal_request_id = Some(request_id);
        self
    }
}

impl BillingClient {
    pub fn from_config(config: &Config) -> Result<Option<Self>> {
        if !config.cloud.billing_enabled {
            return Ok(None);
        }

        if config.billing.paypal_client_id.trim().is_empty() {
            bail!("PONSWARP_BILLING_ENABLED=true requires PAYPAL_CLIENT_ID");
        }
        if config.billing.paypal_client_secret.trim().is_empty() {
            bail!("PONSWARP_BILLING_ENABLED=true requires PAYPAL_CLIENT_SECRET");
        }
        if config.billing.paypal_webhook_id.trim().is_empty() {
            bail!("PONSWARP_BILLING_ENABLED=true requires PAYPAL_WEBHOOK_ID");
        }
        if config.billing.paypal_pro_plan_id.trim().is_empty() {
            bail!("PONSWARP_BILLING_ENABLED=true requires PAYPAL_PRO_PLAN_ID");
        }
        if config.billing.public_app_url.trim().is_empty() {
            bail!("PONSWARP_BILLING_ENABLED=true requires PONSWARP_PUBLIC_APP_URL");
        }

        Ok(Some(Self {
            http: reqwest::Client::new(),
            client_id: config.billing.paypal_client_id.clone(),
            client_secret: config.billing.paypal_client_secret.clone(),
            webhook_id: config.billing.paypal_webhook_id.clone(),
            api_base: config
                .billing
                .paypal_api_base
                .trim_end_matches('/')
                .to_string(),
            currency: config.billing.paypal_currency.to_uppercase(),
            pro_plan_id: config.billing.paypal_pro_plan_id.clone(),
            public_app_url: config
                .billing
                .public_app_url
                .trim_end_matches('/')
                .to_string(),
        }))
    }

    async fn create_checkout(
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

        match request.mode {
            CheckoutMode::Payment => self.create_order_checkout(plan, &return_url).await,
            CheckoutMode::Subscription => {
                self.create_subscription_checkout(plan, &return_url).await
            }
        }
    }

    async fn create_order_checkout(
        &self,
        plan: PaidPlan,
        return_url: &str,
    ) -> Result<CheckoutResponse, BillingError> {
        let access_token = self.access_token().await?;
        let payload = json!({
            "intent": "CAPTURE",
            "purchase_units": [{
                "reference_id": plan.sku,
                "custom_id": plan.sku,
                "description": plan.label,
                "amount": {
                    "currency_code": self.currency.as_str(),
                    "value": plan.paypal_amount(&self.currency),
                },
            }],
            "payment_source": {
                "paypal": {
                    "experience_context": {
                        "brand_name": "PonsWarp",
                        "landing_page": "LOGIN",
                        "shipping_preference": "NO_SHIPPING",
                        "user_action": "PAY_NOW",
                        "return_url": append_query(return_url, "checkout=success"),
                        "cancel_url": append_query(return_url, "checkout=cancelled"),
                    },
                },
            },
        });

        let value = self
            .post_json_with_options(
                "/v2/checkout/orders",
                &access_token,
                &payload,
                JsonPostOptions::representation(),
            )
            .await?;
        let checkout_id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| BillingError::internal("PayPal did not return a checkout id"))?
            .to_string();
        approval_url(&value).map(|checkout_url| CheckoutResponse {
            checkout_url,
            checkout_id,
        })
    }

    async fn create_subscription_checkout(
        &self,
        plan: PaidPlan,
        return_url: &str,
    ) -> Result<CheckoutResponse, BillingError> {
        let access_token = self.access_token().await?;
        let payload = json!({
            "plan_id": self.pro_plan_id.as_str(),
            "custom_id": plan.sku,
            "application_context": {
                "brand_name": "PonsWarp",
                "user_action": "SUBSCRIBE_NOW",
                "return_url": append_query(return_url, "checkout=success"),
                "cancel_url": append_query(return_url, "checkout=cancelled"),
            },
        });

        let value = self
            .post_json("/v1/billing/subscriptions", &access_token, &payload)
            .await?;
        let checkout_id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| BillingError::internal("PayPal did not return a checkout id"))?
            .to_string();
        approval_url(&value).map(|checkout_url| CheckoutResponse {
            checkout_url,
            checkout_id,
        })
    }

    async fn capture_order(
        &self,
        database: &CloudDatabase,
        order_id: &str,
    ) -> Result<CaptureResponse, BillingError> {
        let order_id = order_id.trim();
        if order_id.is_empty() {
            return Err(BillingError::bad_request("Missing PayPal order id"));
        }

        if database
            .resolve_entitlement(order_id, unix_now())
            .await
            .map_err(BillingError::internal)?
            .is_some()
        {
            return Ok(CaptureResponse {
                entitlement_token: order_id.to_string(),
            });
        }

        let access_token = self.access_token().await?;
        let value = self
            .post_json_with_options(
                &format!("/v2/checkout/orders/{}/capture", url_path(order_id)),
                &access_token,
                &json!({}),
                JsonPostOptions::representation().with_request_id(capture_request_id(order_id)),
            )
            .await?;
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if status != "COMPLETED" {
            return Err(BillingError::bad_request(
                "PayPal order has not completed payment",
            ));
        }

        let unit = value
            .pointer("/purchase_units/0")
            .ok_or_else(|| BillingError::bad_request("PayPal order is missing purchase unit"))?;
        let sku = unit
            .get("reference_id")
            .and_then(Value::as_str)
            .or_else(|| unit.get("custom_id").and_then(Value::as_str))
            .ok_or_else(|| BillingError::bad_request("PayPal order is missing Cloud Drop sku"))?;
        let Some(plan) = paid_plan(sku) else {
            return Err(BillingError::bad_request(
                "PayPal order has unknown Cloud Drop sku",
            ));
        };
        if plan.checkout_mode != CheckoutMode::Payment {
            return Err(BillingError::bad_request(
                "PayPal order cannot activate a subscription plan",
            ));
        }
        let capture_id = unit
            .pointer("/payments/captures/0/id")
            .and_then(Value::as_str)
            .ok_or_else(|| BillingError::bad_request("PayPal order is missing capture id"))?;
        let email = value
            .pointer("/payer/email_address")
            .and_then(Value::as_str);

        database
            .upsert_paypal_order_entitlement(PayPalOrderEntitlementInput {
                paypal_order_id: order_id,
                paypal_capture_id: capture_id,
                email,
                sku: plan.sku,
                max_total_bytes: plan.max_total_bytes,
                max_file_bytes: plan.max_file_bytes,
                retention_seconds: plan.retention_seconds,
                created_at: unix_now(),
            })
            .await
            .map_err(BillingError::internal)?;

        Ok(CaptureResponse {
            entitlement_token: order_id.to_string(),
        })
    }

    async fn validate_webhook(
        &self,
        headers: &HeaderMap,
        event: &Value,
    ) -> Result<(), BillingError> {
        let access_token = self.access_token().await?;
        let payload = json!({
            "auth_algo": required_header(headers, "paypal-auth-algo")?,
            "cert_url": required_header(headers, "paypal-cert-url")?,
            "transmission_id": required_header(headers, "paypal-transmission-id")?,
            "transmission_sig": required_header(headers, "paypal-transmission-sig")?,
            "transmission_time": required_header(headers, "paypal-transmission-time")?,
            "webhook_id": self.webhook_id.as_str(),
            "webhook_event": event,
        });
        let value = self
            .post_json(
                "/v1/notifications/verify-webhook-signature",
                &access_token,
                &payload,
            )
            .await?;
        let status = value
            .get("verification_status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if status == "SUCCESS" {
            return Ok(());
        }

        Err(BillingError::bad_webhook(
            "PayPal webhook signature verification failed",
        ))
    }

    async fn access_token(&self) -> Result<String, BillingError> {
        let value: Value = self
            .http
            .post(format!("{}/v1/oauth2/token", self.api_base))
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .map_err(BillingError::internal)?
            .error_for_status()
            .map_err(BillingError::internal)?
            .json()
            .await
            .map_err(BillingError::internal)?;

        value
            .get("access_token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| BillingError::internal("PayPal did not return an access token"))
    }

    async fn post_json(
        &self,
        path: &str,
        access_token: &str,
        payload: &Value,
    ) -> Result<Value, BillingError> {
        self.post_json_with_options(path, access_token, payload, JsonPostOptions::default())
            .await
    }

    async fn post_json_with_options(
        &self,
        path: &str,
        access_token: &str,
        payload: &Value,
        options: JsonPostOptions,
    ) -> Result<Value, BillingError> {
        let mut request = self
            .http
            .post(format!("{}{}", self.api_base, path))
            .bearer_auth(access_token)
            .json(payload);
        if let Some(prefer) = options.prefer {
            request = request.header("Prefer", prefer);
        }
        if let Some(request_id) = options.paypal_request_id {
            request = request.header("PayPal-Request-Id", request_id);
        }

        request
            .send()
            .await
            .map_err(BillingError::internal)?
            .error_for_status()
            .map_err(BillingError::internal)?
            .json()
            .await
            .map_err(BillingError::internal)
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

    match billing.create_checkout(request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => error.into_response(),
    }
}

pub async fn capture_checkout(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CaptureRequest>,
) -> Response {
    let Some(billing) = state.billing.as_ref() else {
        return BillingError::unavailable("Billing is not enabled").into_response();
    };
    let Some(database) = state.cloud_db.as_ref() else {
        return BillingError::unavailable("Billing database is not available").into_response();
    };

    match billing.capture_order(database, &request.order_id).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => error.into_response(),
    }
}

pub async fn paypal_webhook(
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

    let event = match serde_json::from_slice::<Value>(&body) {
        Ok(event) => event,
        Err(error) => return BillingError::bad_webhook(error.to_string()).into_response(),
    };
    if let Err(error) = billing.validate_webhook(&headers, &event).await {
        return error.into_response();
    }

    match process_paypal_event(database, &event).await {
        Ok(()) => Json(json!({ "received": true })).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn process_paypal_event(database: &CloudDatabase, event: &Value) -> Result<(), BillingError> {
    let event_id = event
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| BillingError::bad_webhook("PayPal event is missing id"))?;
    let event_type = event
        .get("event_type")
        .and_then(Value::as_str)
        .ok_or_else(|| BillingError::bad_webhook("PayPal event is missing event_type"))?;
    let event_created = unix_now();

    let is_new = database
        .try_record_paypal_event(event_id, event_type, event_created, unix_now())
        .await
        .map_err(BillingError::internal)?;
    if !is_new {
        return Ok(());
    }

    let resource = event
        .get("resource")
        .ok_or_else(|| BillingError::bad_webhook("PayPal event is missing resource"))?;

    match event_type {
        "BILLING.SUBSCRIPTION.ACTIVATED" => {
            let subscription_id = resource
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| BillingError::bad_webhook("PayPal subscription is missing id"))?;
            let sku = resource
                .get("custom_id")
                .and_then(Value::as_str)
                .unwrap_or("pro_monthly_krw_9900");
            if paid_plan(sku).is_none() {
                return Err(BillingError::bad_webhook(
                    "PayPal subscription has unknown sku",
                ));
            }
            let email = resource
                .pointer("/subscriber/email_address")
                .and_then(Value::as_str);
            let paypal_plan_id = resource.get("plan_id").and_then(Value::as_str);

            database
                .upsert_paypal_subscription_entitlement(PayPalSubscriptionEntitlementInput {
                    paypal_subscription_id: subscription_id,
                    paypal_plan_id,
                    email,
                    created_at: event_created,
                })
                .await
                .map_err(BillingError::internal)?;
        }
        "BILLING.SUBSCRIPTION.CANCELLED"
        | "BILLING.SUBSCRIPTION.SUSPENDED"
        | "BILLING.SUBSCRIPTION.EXPIRED" => {
            let subscription_id = resource
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| BillingError::bad_webhook("PayPal subscription is missing id"))?;
            database
                .set_paypal_subscription_status(subscription_id, "inactive", event_created)
                .await
                .map_err(BillingError::internal)?;
        }
        _ => {}
    }

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
    fn paypal_amount(&self, currency: &str) -> String {
        match currency {
            "KRW" => self.price_krw.to_string(),
            "USD" => format!("{:.2}", self.price_krw as f64 / 1000.0),
            _ => self.price_krw.to_string(),
        }
    }
}

fn approval_url(value: &Value) -> Result<String, BillingError> {
    value
        .get("links")
        .and_then(Value::as_array)
        .and_then(|links| {
            links.iter().find_map(|link| {
                let rel = link.get("rel").and_then(Value::as_str);
                if rel == Some("approve") || rel == Some("payer-action") {
                    link.get("href").and_then(Value::as_str)
                } else {
                    None
                }
            })
        })
        .map(str::to_string)
        .ok_or_else(|| BillingError::internal("PayPal did not return an approval URL"))
}

fn required_header(headers: &HeaderMap, name: &'static str) -> Result<String, BillingError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .ok_or_else(|| BillingError::bad_webhook(format!("Missing {name} header")))
}

fn append_query(url: &str, query: &str) -> String {
    if url.contains('?') {
        format!("{url}&{query}")
    } else {
        format!("{url}?{query}")
    }
}

fn url_path(value: &str) -> String {
    value.replace('/', "%2F")
}

fn capture_request_id(order_id: &str) -> String {
    format!("ponswarp-capture-{order_id}")
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
    fn append_query_adds_first_query_parameter() {
        assert_eq!(
            append_query("https://warp.ponslink.com", "checkout=success"),
            "https://warp.ponslink.com?checkout=success"
        );
    }

    #[test]
    fn append_query_preserves_existing_query_parameters() {
        assert_eq!(
            append_query(
                "https://warp.ponslink.com/cloud?mode=drop",
                "checkout=success"
            ),
            "https://warp.ponslink.com/cloud?mode=drop&checkout=success"
        );
    }

    #[test]
    fn paid_plan_modes_match_checkout_kind() {
        assert_eq!(
            paid_plan("drop_1tb_7d").unwrap().checkout_mode,
            CheckoutMode::Payment
        );
        assert_eq!(
            paid_plan("pro_monthly_krw_9900").unwrap().checkout_mode,
            CheckoutMode::Subscription
        );
    }

    #[test]
    fn paypal_amount_formats_configured_currency() {
        let plan = paid_plan("pro_monthly_krw_9900").unwrap();

        assert_eq!(plan.paypal_amount("KRW"), "9900");
        assert_eq!(plan.paypal_amount("USD"), "9.90");
    }

    #[test]
    fn capture_request_id_is_stable_for_order_retries() {
        assert_eq!(
            capture_request_id("5O190127TN364715T"),
            "ponswarp-capture-5O190127TN364715T"
        );
    }

    #[test]
    fn approval_url_accepts_payer_action_links() {
        let value = json!({
            "links": [
                {
                    "rel": "payer-action",
                    "href": "https://www.sandbox.paypal.com/checkoutnow?token=test"
                }
            ]
        });

        assert_eq!(
            approval_url(&value).unwrap(),
            "https://www.sandbox.paypal.com/checkoutnow?token=test"
        );
    }
}
