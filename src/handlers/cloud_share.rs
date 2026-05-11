//! Cloudflare R2 backed temporary file sharing handlers.

use crate::config::CloudConfig;
use crate::state::{AppState, CloudStorage};
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCloudShareRequest {
    pub root_name: String,
    pub files: Vec<CreateCloudFileRequest>,
    pub entitlement_token: Option<String>,
    pub retention_seconds: Option<u64>,
    pub password: Option<String>,
    pub download_limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCloudFileRequest {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub content_type: Option<String>,
    pub last_modified: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteCloudShareRequest {
    pub uploaded_file_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CloudShareManifest {
    pub share_id: String,
    pub root_name: String,
    pub total_size: u64,
    pub total_files: usize,
    pub created_at: u64,
    pub expires_at: u64,
    pub completed: bool,
    pub files: Vec<CloudFileManifest>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CloudFileManifest {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub content_type: String,
    pub last_modified: Option<u64>,
    pub object_key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCloudShareResponse {
    pub share_id: String,
    pub share_url: String,
    pub expires_at: u64,
    pub upload_url_ttl_seconds: u64,
    pub files: Vec<CloudUploadTarget>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudUploadTarget {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub upload_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicCloudShareResponse {
    pub share_id: String,
    pub root_name: String,
    pub total_size: u64,
    pub total_files: usize,
    pub created_at: u64,
    pub expires_at: u64,
    pub seconds_until_expiry: u64,
    pub completed: bool,
    pub files: Vec<PublicCloudFile>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicCloudFile {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub content_type: String,
    pub last_modified: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPlansResponse {
    pub direct_p2p: DirectP2pPlan,
    pub free: CloudPlanLimit,
    pub passes: Vec<DropPassPlan>,
    pub pro: ProPlan,
    pub checkout_enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectP2pPlan {
    pub label: String,
    pub unlimited: bool,
    pub price_krw: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPlanLimit {
    pub sku: String,
    pub label: String,
    pub price_krw: u64,
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
    pub retention_seconds: u64,
    pub download_limit: Option<u32>,
    pub available: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DropPassPlan {
    #[serde(flatten)]
    pub limit: CloudPlanLimit,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProPlan {
    #[serde(flatten)]
    pub limit: CloudPlanLimit,
    pub monthly_quota_bytes: u64,
    pub concurrent_storage_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiErrorBody {
    error: String,
}

pub async fn create_cloud_share(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateCloudShareRequest>,
) -> Response {
    match create_cloud_share_inner(state, request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => error.into_response(),
    }
}

pub async fn get_cloud_plans(State(state): State<Arc<AppState>>) -> Json<CloudPlansResponse> {
    Json(cloud_plans_response(&state.config.cloud))
}

pub async fn get_cloud_share(
    State(state): State<Arc<AppState>>,
    Path(share_id): Path<String>,
) -> Response {
    match read_public_share(state, &share_id).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => error.into_response(),
    }
}

pub async fn complete_cloud_share(
    State(state): State<Arc<AppState>>,
    Path(share_id): Path<String>,
    Json(request): Json<CompleteCloudShareRequest>,
) -> Response {
    match complete_cloud_share_inner(state, &share_id, request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => error.into_response(),
    }
}

pub async fn download_cloud_file(
    State(state): State<Arc<AppState>>,
    Path((share_id, file_id)): Path<(String, String)>,
) -> Response {
    match download_cloud_file_inner(state, &share_id, &file_id).await {
        Ok(redirect) => redirect.into_response(),
        Err(error) => error.into_response(),
    }
}

fn cloud_plans_response(config: &CloudConfig) -> CloudPlansResponse {
    const GB: u64 = 1024 * 1024 * 1024;
    const TB: u64 = 1024 * GB;

    let billing_enabled = config.billing_enabled;
    CloudPlansResponse {
        direct_p2p: DirectP2pPlan {
            label: "Direct P2P".to_string(),
            unlimited: true,
            price_krw: 0,
        },
        free: CloudPlanLimit {
            sku: "free_cloud_10gb_24h".to_string(),
            label: "Free Cloud Drop".to_string(),
            price_krw: 0,
            max_total_bytes: config.max_total_bytes,
            max_file_bytes: config.max_file_bytes,
            retention_seconds: config.retention_seconds,
            download_limit: None,
            available: true,
        },
        passes: vec![
            DropPassPlan {
                limit: CloudPlanLimit {
                    sku: "drop_100gb_3d".to_string(),
                    label: "100GB Drop Pass".to_string(),
                    price_krw: 1_900,
                    max_total_bytes: 100 * GB,
                    max_file_bytes: 100 * GB,
                    retention_seconds: 3 * 24 * 60 * 60,
                    download_limit: Some(10),
                    available: billing_enabled,
                },
            },
            DropPassPlan {
                limit: CloudPlanLimit {
                    sku: "drop_500gb_7d".to_string(),
                    label: "500GB Drop Pass".to_string(),
                    price_krw: 4_900,
                    max_total_bytes: 500 * GB,
                    max_file_bytes: 500 * GB,
                    retention_seconds: 7 * 24 * 60 * 60,
                    download_limit: Some(20),
                    available: billing_enabled,
                },
            },
            DropPassPlan {
                limit: CloudPlanLimit {
                    sku: "drop_1tb_7d".to_string(),
                    label: "1TB Drop Pass".to_string(),
                    price_krw: 9_900,
                    max_total_bytes: TB,
                    max_file_bytes: TB,
                    retention_seconds: 7 * 24 * 60 * 60,
                    download_limit: Some(30),
                    available: billing_enabled,
                },
            },
        ],
        pro: ProPlan {
            limit: CloudPlanLimit {
                sku: "pro_monthly_krw_9900".to_string(),
                label: "PonsWarp Pro".to_string(),
                price_krw: 9_900,
                max_total_bytes: TB,
                max_file_bytes: TB,
                retention_seconds: 7 * 24 * 60 * 60,
                download_limit: Some(30),
                available: billing_enabled,
            },
            monthly_quota_bytes: 2 * TB,
            concurrent_storage_bytes: TB,
        },
        checkout_enabled: billing_enabled,
    }
}

struct ResolvedCloudPolicy {
    retention_seconds: u64,
    plan_snapshot: serde_json::Value,
    password_hash: Option<String>,
    download_limit: Option<u32>,
}

impl From<ResolvedCloudPolicy> for crate::database::CloudSharePolicyRecord {
    fn from(policy: ResolvedCloudPolicy) -> Self {
        Self {
            plan_snapshot: policy.plan_snapshot,
            owner_user_id: None,
            drop_pass_id: None,
            password_hash: policy.password_hash,
            download_limit: policy.download_limit,
        }
    }
}

fn resolve_cloud_policy(
    config: &CloudConfig,
    request: &CreateCloudShareRequest,
) -> Result<ResolvedCloudPolicy, CloudShareError> {
    if request
        .entitlement_token
        .as_deref()
        .is_some_and(|token| !token.trim().is_empty())
    {
        return Err(CloudShareError::bad_request(
            "Paid Cloud Drop checkout is not available yet",
        ));
    }

    if request
        .password
        .as_deref()
        .is_some_and(|password| !password.trim().is_empty())
    {
        return Err(CloudShareError::bad_request(
            "Password-protected Cloud Drop links are not available yet",
        ));
    }

    if request.download_limit.is_some() {
        return Err(CloudShareError::bad_request(
            "Download limits are not available yet",
        ));
    }

    let retention_seconds = request
        .retention_seconds
        .unwrap_or(config.retention_seconds)
        .clamp(1, config.retention_seconds);

    Ok(ResolvedCloudPolicy {
        retention_seconds,
        plan_snapshot: free_plan_snapshot(config, retention_seconds),
        password_hash: None,
        download_limit: None,
    })
}

fn free_plan_snapshot(config: &CloudConfig, retention_seconds: u64) -> serde_json::Value {
    json!({
        "sku": "free_cloud_10gb_24h",
        "label": "Free Cloud Drop",
        "priceKrw": 0,
        "maxTotalBytes": config.max_total_bytes,
        "maxFileBytes": config.max_file_bytes,
        "retentionSeconds": retention_seconds
    })
}

async fn create_cloud_share_inner(
    state: Arc<AppState>,
    request: CreateCloudShareRequest,
) -> Result<CreateCloudShareResponse, CloudShareError> {
    let storage = state.cloud_storage()?;
    validate_create_request(&state.config.cloud, &request)?;
    let policy = resolve_cloud_policy(&state.config.cloud, &request)?;

    let now = unix_now();
    let expires_at = now + policy.retention_seconds;
    let share_id = Uuid::new_v4().simple().to_string();
    let root_name = clamp_name(&request.root_name, "Cloud Drop");

    let mut manifest_files = Vec::with_capacity(request.files.len());
    let mut upload_targets = Vec::with_capacity(request.files.len());

    for file in request.files {
        let id = Uuid::new_v4().simple().to_string();
        let object_key = storage.file_key(&share_id, &id);
        let content_type = file
            .content_type
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let name = clamp_name(&file.name, "download.bin");
        let path = clamp_path(&file.path, &name);

        manifest_files.push(CloudFileManifest {
            id: id.clone(),
            name: name.clone(),
            path: path.clone(),
            size: file.size,
            content_type,
            last_modified: file.last_modified,
            object_key: object_key.clone(),
        });

        let upload_url = presign_put(
            storage,
            &object_key,
            state.config.cloud.upload_url_ttl_seconds,
        )
        .await?;
        upload_targets.push(CloudUploadTarget {
            id,
            name,
            path,
            size: file.size,
            upload_url,
        });
    }

    let manifest = CloudShareManifest {
        total_size: manifest_files.iter().map(|file| file.size).sum(),
        total_files: manifest_files.len(),
        share_id: share_id.clone(),
        root_name,
        created_at: now,
        expires_at,
        completed: false,
        files: manifest_files,
    };

    if let Some(database) = state.cloud_db.as_ref() {
        database
            .insert_cloud_share(&manifest, policy.into())
            .await
            .map_err(CloudShareError::internal)?;
    }
    if let Err(error) = write_manifest(storage, &manifest).await {
        if let Some(database) = state.cloud_db.as_ref() {
            let _ = database
                .mark_cloud_share_deleted(&manifest.share_id, unix_now())
                .await;
        }
        return Err(error);
    }

    Ok(CreateCloudShareResponse {
        share_url: format!("/cloud/{}", share_id),
        share_id,
        expires_at,
        upload_url_ttl_seconds: state.config.cloud.upload_url_ttl_seconds,
        files: upload_targets,
    })
}

async fn complete_cloud_share_inner(
    state: Arc<AppState>,
    share_id: &str,
    request: CompleteCloudShareRequest,
) -> Result<PublicCloudShareResponse, CloudShareError> {
    let storage = state.cloud_storage()?;
    let mut manifest = read_share_manifest(&state, storage, share_id).await?;
    reject_expired(&manifest)?;

    let expected: std::collections::HashSet<&str> =
        manifest.files.iter().map(|file| file.id.as_str()).collect();
    let uploaded: std::collections::HashSet<&str> = request
        .uploaded_file_ids
        .iter()
        .map(String::as_str)
        .collect();
    if expected != uploaded {
        return Err(CloudShareError::bad_request(
            "Uploaded file list does not match the created share",
        ));
    }

    for file in &manifest.files {
        ensure_object_exists(storage, &file.object_key, file.size).await?;
    }

    manifest.completed = true;
    if let Some(database) = state.cloud_db.as_ref() {
        database
            .mark_cloud_share_completed(&manifest.share_id, manifest.total_size, unix_now())
            .await
            .map_err(CloudShareError::internal)?;
    }
    write_manifest(storage, &manifest).await?;
    Ok(public_share(manifest))
}

async fn read_public_share(
    state: Arc<AppState>,
    share_id: &str,
) -> Result<PublicCloudShareResponse, CloudShareError> {
    let storage = state.cloud_storage()?;
    let manifest = read_share_manifest(&state, storage, share_id).await?;
    reject_expired(&manifest)?;
    Ok(public_share(manifest))
}

async fn download_cloud_file_inner(
    state: Arc<AppState>,
    share_id: &str,
    file_id: &str,
) -> Result<Redirect, CloudShareError> {
    let storage = state.cloud_storage()?;
    let manifest = read_share_manifest(&state, storage, share_id).await?;
    reject_expired(&manifest)?;
    if !manifest.completed {
        return Err(CloudShareError::conflict(
            "Share upload is not complete yet",
        ));
    }

    let file = manifest
        .files
        .iter()
        .find(|file| file.id == file_id)
        .ok_or_else(|| CloudShareError::not_found("File not found"))?;

    let presigned = storage
        .client
        .get_object()
        .bucket(&storage.bucket)
        .key(&file.object_key)
        .response_content_disposition(format!(
            "attachment; filename=\"{}\"",
            sanitize_ascii_filename(&file.name)
        ))
        .presigned(presign_config(state.config.cloud.download_url_ttl_seconds)?)
        .await
        .map_err(CloudShareError::internal)?;

    Ok(Redirect::temporary(&presigned.uri().to_string()))
}

pub async fn cleanup_expired_cloud_shares(state: Arc<AppState>) {
    let Some(storage) = state.cloud.as_ref() else {
        return;
    };

    let manifest_prefix = storage.manifest_prefix();
    let mut continuation_token = None;

    loop {
        let response = storage
            .client
            .list_objects_v2()
            .bucket(&storage.bucket)
            .prefix(&manifest_prefix)
            .set_continuation_token(continuation_token)
            .send()
            .await;

        let Ok(response) = response else {
            tracing::warn!("Cloud share cleanup list_objects_v2 failed");
            return;
        };

        for object in response.contents() {
            let Some(key) = object.key() else {
                continue;
            };

            let Ok(manifest) = read_manifest_by_key(storage, key).await else {
                continue;
            };

            if manifest.expires_at > unix_now() {
                continue;
            }

            for file in &manifest.files {
                let _ = storage
                    .client
                    .delete_object()
                    .bucket(&storage.bucket)
                    .key(&file.object_key)
                    .send()
                    .await;
            }
            let _ = storage
                .client
                .delete_object()
                .bucket(&storage.bucket)
                .key(key)
                .send()
                .await;
            if let Some(database) = state.cloud_db.as_ref() {
                let _ = database
                    .mark_cloud_share_deleted(&manifest.share_id, unix_now())
                    .await;
            }
            tracing::info!(share_id = %manifest.share_id, "Expired cloud share deleted");
        }

        if response.is_truncated().unwrap_or(false) {
            continuation_token = response.next_continuation_token().map(str::to_string);
        } else {
            break;
        }
    }
}

fn validate_create_request(
    config: &CloudConfig,
    request: &CreateCloudShareRequest,
) -> Result<(), CloudShareError> {
    if request.files.is_empty() {
        return Err(CloudShareError::bad_request(
            "At least one file is required",
        ));
    }
    if request.files.len() > config.max_files {
        return Err(CloudShareError::bad_request(
            "Too many files in one cloud share",
        ));
    }
    let total_size: u64 = request.files.iter().map(|file| file.size).sum();
    if total_size > config.max_total_bytes {
        return Err(CloudShareError::bad_request(format!(
            "Cloud Drop is limited to {}. Use direct P2P for unlimited transfer or split files into 10GB batches.",
            format_bytes(config.max_total_bytes)
        )));
    }
    if request
        .files
        .iter()
        .any(|file| file.size > config.max_file_bytes)
    {
        return Err(CloudShareError::bad_request(format!(
            "Each Cloud Drop file is limited to {}. Use direct P2P for unlimited transfer or split the file.",
            format_bytes(config.max_file_bytes)
        )));
    }
    if request.files.iter().any(|file| file.size == 0) {
        return Err(CloudShareError::bad_request(
            "Empty files are not supported",
        ));
    }
    Ok(())
}

async fn presign_put(
    storage: &CloudStorage,
    object_key: &str,
    ttl_seconds: u64,
) -> Result<String, CloudShareError> {
    let presigned = storage
        .client
        .put_object()
        .bucket(&storage.bucket)
        .key(object_key)
        .presigned(presign_config(ttl_seconds)?)
        .await
        .map_err(CloudShareError::internal)?;

    Ok(presigned.uri().to_string())
}

async fn ensure_object_exists(
    storage: &CloudStorage,
    object_key: &str,
    expected_size: u64,
) -> Result<(), CloudShareError> {
    let output = storage
        .client
        .head_object()
        .bucket(&storage.bucket)
        .key(object_key)
        .send()
        .await
        .map_err(|error| {
            let message = error.to_string();
            if message.contains("NoSuchKey") || message.contains("NotFound") {
                CloudShareError::conflict("Uploaded object is not available yet")
            } else {
                CloudShareError::internal(error)
            }
        })?;
    if output.content_length().unwrap_or_default() < 0
        || output.content_length().unwrap_or_default() as u64 != expected_size
    {
        return Err(CloudShareError::conflict(
            "Uploaded object size does not match the created share",
        ));
    }
    Ok(())
}

fn presign_config(ttl_seconds: u64) -> Result<PresigningConfig, CloudShareError> {
    PresigningConfig::builder()
        .expires_in(Duration::from_secs(ttl_seconds.min(604_800)))
        .build()
        .map_err(CloudShareError::internal)
}

async fn write_manifest(
    storage: &CloudStorage,
    manifest: &CloudShareManifest,
) -> Result<(), CloudShareError> {
    let body = serde_json::to_vec(manifest).map_err(CloudShareError::internal)?;
    storage
        .client
        .put_object()
        .bucket(&storage.bucket)
        .key(storage.manifest_key(&manifest.share_id))
        .content_type("application/json")
        .body(ByteStream::from(body))
        .send()
        .await
        .map_err(CloudShareError::internal)?;
    Ok(())
}

async fn read_manifest(
    storage: &CloudStorage,
    share_id: &str,
) -> Result<CloudShareManifest, CloudShareError> {
    read_manifest_by_key(storage, &storage.manifest_key(share_id)).await
}

async fn read_share_manifest(
    state: &AppState,
    storage: &CloudStorage,
    share_id: &str,
) -> Result<CloudShareManifest, CloudShareError> {
    if let Some(database) = state.cloud_db.as_ref() {
        match database
            .read_cloud_share(share_id)
            .await
            .map_err(CloudShareError::internal)?
        {
            Some(manifest) => return Ok(manifest),
            None => {
                tracing::debug!(share_id = %share_id, "Cloud share not found in DB; falling back to R2 manifest");
            }
        }
    }

    read_manifest(storage, share_id).await
}

async fn read_manifest_by_key(
    storage: &CloudStorage,
    key: &str,
) -> Result<CloudShareManifest, CloudShareError> {
    let output = storage
        .client
        .get_object()
        .bucket(&storage.bucket)
        .key(key)
        .send()
        .await
        .map_err(|error| {
            let message = error.to_string();
            if message.contains("NoSuchKey") || message.contains("NotFound") {
                CloudShareError::not_found("Share not found")
            } else {
                CloudShareError::internal(error)
            }
        })?;
    let bytes = output
        .body
        .collect()
        .await
        .map_err(CloudShareError::internal)?
        .into_bytes();
    serde_json::from_slice(&bytes).map_err(CloudShareError::internal)
}

fn public_share(manifest: CloudShareManifest) -> PublicCloudShareResponse {
    let now = unix_now();
    PublicCloudShareResponse {
        share_id: manifest.share_id,
        root_name: manifest.root_name,
        total_size: manifest.total_size,
        total_files: manifest.total_files,
        created_at: manifest.created_at,
        expires_at: manifest.expires_at,
        seconds_until_expiry: manifest.expires_at.saturating_sub(now),
        completed: manifest.completed,
        files: manifest
            .files
            .into_iter()
            .map(|file| PublicCloudFile {
                id: file.id,
                name: file.name,
                path: file.path,
                size: file.size,
                content_type: file.content_type,
                last_modified: file.last_modified,
            })
            .collect(),
    }
}

fn reject_expired(manifest: &CloudShareManifest) -> Result<(), CloudShareError> {
    if manifest.expires_at <= unix_now() {
        return Err(CloudShareError::gone("Share has expired"));
    }
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn clamp_name(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.chars().take(180).collect()
    }
}

fn clamp_path(path: &str, fallback_name: &str) -> String {
    let cleaned = path
        .split('/')
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
        .collect::<Vec<_>>()
        .join("/");
    if cleaned.is_empty() {
        fallback_name.to_string()
    } else {
        cleaned.chars().take(512).collect()
    }
}

fn sanitize_ascii_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '"' | '\\' | '\r' | '\n' => '_',
            c if c.is_ascii_graphic() || c == ' ' => c,
            _ => '_',
        })
        .collect()
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.0} {}", size, UNITS[unit])
    }
}

#[derive(Debug)]
pub struct CloudShareError {
    status: StatusCode,
    message: String,
}

impl CloudShareError {
    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    pub(crate) fn gone(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::GONE,
            message: message.into(),
        }
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    pub(crate) fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(error = %error, "Cloud share operation failed");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Cloud share service failed".to_string(),
        }
    }
}

impl IntoResponse for CloudShareError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorBody {
                error: self.message,
            }),
        )
            .into_response()
    }
}
