//! Admin dashboard API foundation.

use crate::auth::{current_session_user, UserIdentity};
use crate::database::AdminMemberRecord;
use crate::state::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminMeResponse {
    authenticated: bool,
    user: AdminUserResponse,
    admin: AdminRoleResponse,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminUserResponse {
    id: String,
    email: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminRoleResponse {
    role: String,
    status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminOverviewResponse {
    total_users: i64,
    active_subscriptions: i64,
    active_drop_passes: i64,
    active_cloud_shares: i64,
    stored_cloud_bytes: i64,
    billing_events: i64,
}

#[derive(Debug, Serialize)]
struct AdminErrorBody {
    error: String,
}

pub async fn me(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some((user, admin)) = require_admin(&state, &headers).await else {
        return admin_error(StatusCode::FORBIDDEN, "Admin access is required");
    };

    Json(AdminMeResponse {
        authenticated: true,
        user: AdminUserResponse {
            id: user.id.to_string(),
            email: user.email,
        },
        admin: AdminRoleResponse {
            role: admin.role,
            status: admin.status,
        },
    })
    .into_response()
}

pub async fn overview(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some((_user, _admin)) = require_admin(&state, &headers).await else {
        return admin_error(StatusCode::FORBIDDEN, "Admin access is required");
    };
    let Some(database) = state.cloud_db.as_ref() else {
        return admin_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Admin database is not available",
        );
    };

    match database.admin_overview(unix_now()).await {
        Ok(overview) => Json(AdminOverviewResponse {
            total_users: overview.total_users,
            active_subscriptions: overview.active_subscriptions,
            active_drop_passes: overview.active_drop_passes,
            active_cloud_shares: overview.active_cloud_shares,
            stored_cloud_bytes: overview.stored_cloud_bytes,
            billing_events: overview.billing_events,
        })
        .into_response(),
        Err(error) => {
            tracing::error!(error = %error, "Failed to load admin overview");
            admin_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to load admin overview",
            )
        }
    }
}

async fn require_admin(
    state: &AppState,
    headers: &HeaderMap,
) -> Option<(UserIdentity, AdminMemberRecord)> {
    let user = match current_session_user(state, headers).await {
        Ok(Some(user)) => user,
        Ok(None) => return None,
        Err(error) => {
            tracing::error!(?error, "Failed to resolve current admin session");
            return None;
        }
    };
    let database = state.cloud_db.as_ref()?;

    match database.admin_for_user(user.id).await {
        Ok(Some(admin)) => return Some((user, admin)),
        Ok(None) => {}
        Err(error) => {
            tracing::error!(error = %error, user_id = %user.id, "Failed to load admin member");
            return None;
        }
    }

    if !state
        .config
        .admin
        .bootstrap_emails
        .iter()
        .any(|email| email.eq_ignore_ascii_case(&user.email))
    {
        return None;
    }

    if let Err(error) = database.ensure_bootstrap_admin(user.id, unix_now()).await {
        tracing::error!(error = %error, user_id = %user.id, "Failed to bootstrap admin member");
        return None;
    }

    match database.admin_for_user(user.id).await {
        Ok(Some(admin)) => Some((user, admin)),
        Ok(None) => None,
        Err(error) => {
            tracing::error!(error = %error, user_id = %user.id, "Failed to reload bootstrap admin");
            None
        }
    }
}

fn admin_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(AdminErrorBody {
            error: message.to_string(),
        }),
    )
        .into_response()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
