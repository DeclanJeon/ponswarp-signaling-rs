use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::state::AppState;
const MAX_MESH_JSON_BYTES: usize = 64 * 1024;

#[derive(Debug, Default)]
pub struct MeshState {
    pub workspaces: DashMap<String, MeshWorkspace>,
    pub nodes: DashMap<(String, String), MeshNode>,
    pub presence: DashMap<(String, String), MeshPresence>,
    pub files: DashMap<(String, String), MeshFile>,
    pub availability: DashMap<(String, String, String), MeshAvailability>,
    pub events: DashMap<String, MeshEvent>,
    pub shares: DashMap<String, MeshShare>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshWorkspace {
    pub workspace_id: String,
    pub name: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshNode {
    pub workspace_id: String,
    pub node_id: String,
    pub display_name: String,
    pub public_key: String,
    pub status: String,
    #[serde(default)]
    pub capabilities: Value,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshPresence {
    pub workspace_id: String,
    pub node_id: String,
    pub online: bool,
    #[serde(default)]
    pub endpoint_hints: Value,
    #[serde(default)]
    pub load: Value,
    pub updated_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshFile {
    pub workspace_id: String,
    pub file_id: String,
    pub name: String,
    pub size_bytes: u64,
    pub piece_size: u64,
    pub piece_count: u64,
    #[serde(default)]
    pub manifest: Value,
    #[serde(default)]
    pub tags: Value,
    pub created_by_node_id: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshAvailability {
    pub workspace_id: String,
    pub file_id: String,
    pub node_id: String,
    pub complete: bool,
    #[serde(default)]
    pub verified_ranges: Value,
    pub updated_at: u64,
    pub advertise_until: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshEvent {
    pub event_id: String,
    pub workspace_id: String,
    pub event_type: String,
    #[serde(default)]
    pub payload: Value,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshShare {
    pub code: String,
    pub workspace_id: String,
    pub file_id: String,
    pub created_by_node_id: String,
    pub expires_at: u64,
    pub revoked_at: Option<u64>,
    #[serde(default)]
    pub capabilities: Value,
    pub created_at: u64,
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkspaceRequest {
    #[serde(rename = "workspaceId")]
    pub workspace_id: Option<String>,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct RegisterNodeRequest {
    #[serde(rename = "nodeId")]
    pub node_id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "publicKey")]
    pub public_key: String,
    #[serde(default)]
    pub capabilities: Value,
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatRequest {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, rename = "endpointHints")]
    pub endpoint_hints: Value,
    #[serde(default)]
    pub load: Value,
    #[serde(default, rename = "ttlSeconds")]
    pub ttl_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct PublishFileRequest {
    pub manifest: Value,
    #[serde(default)]
    pub availability: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAvailabilityRequest {
    #[serde(default)]
    pub complete: bool,
    #[serde(default, rename = "verifiedRanges")]
    pub verified_ranges: Value,
    #[serde(default, rename = "advertiseUntil")]
    pub advertise_until: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct RecordEventRequest {
    #[serde(rename = "eventType")]
    pub event_type: String,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Deserialize)]
pub struct CreateShareRequest {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(rename = "fileId")]
    pub file_id: String,
    #[serde(default, rename = "createdByNodeId")]
    pub created_by_node_id: Option<String>,
    #[serde(default, rename = "ttlSeconds")]
    pub ttl_seconds: Option<u64>,
    #[serde(default)]
    pub capabilities: Value,
}

#[derive(Debug, Deserialize)]
pub struct ShareEventRequest {
    #[serde(rename = "eventType")]
    pub event_type: String,
    #[serde(default)]
    pub payload: Value,
}

pub async fn mesh_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if !state.config.mesh.enabled {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "enabled": false, "status": "disabled", "error": "mesh_disabled" })),
        );
    }
    (
        StatusCode::OK,
        Json(json!({ "enabled": true, "status": "ok" })),
    )
}

pub async fn mesh_ready(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if !state.config.mesh.enabled {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "enabled": false, "status": "disabled", "error": "mesh_disabled" })),
        );
    }
    (
        StatusCode::OK,
        Json(json!({ "enabled": true, "status": "ready", "storage": "memory" })),
    )
}

pub async fn create_workspace(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> impl IntoResponse {
    if let Some(disabled) = mesh_disabled_response(&state) {
        return disabled;
    }
    let workspace_id = req
        .workspace_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("ws_{}", uuid::Uuid::new_v4().simple()));
    let workspace = MeshWorkspace {
        workspace_id: workspace_id.clone(),
        name: req.name,
        created_at: now_seconds(),
    };
    state
        .mesh
        .workspaces
        .insert(workspace_id.clone(), workspace.clone());
    (
        StatusCode::OK,
        Json(json!({ "workspaceId": workspace_id, "name": workspace.name })),
    )
}

pub async fn register_node(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
    Json(req): Json<RegisterNodeRequest>,
) -> impl IntoResponse {
    if let Some(disabled) = mesh_disabled_response(&state) {
        return disabled;
    }
    if !workspace_exists(&state, &workspace_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "workspace_not_found" })),
        );
    }
    let status = if state.config.mesh.auto_approve_nodes {
        "approved"
    } else {
        "pending"
    }
    .to_string();
    let node = MeshNode {
        workspace_id: workspace_id.clone(),
        node_id: req.node_id.clone(),
        display_name: req.display_name,
        public_key: req.public_key,
        status: status.clone(),
        capabilities: req.capabilities,
        created_at: now_seconds(),
    };
    state
        .mesh
        .nodes
        .insert((workspace_id, req.node_id.clone()), node);
    (
        StatusCode::OK,
        Json(json!({ "nodeId": req.node_id, "status": status })),
    )
}

pub async fn heartbeat(
    State(state): State<Arc<AppState>>,
    Path((workspace_id, node_id)): Path<(String, String)>,
    Json(req): Json<HeartbeatRequest>,
) -> impl IntoResponse {
    if let Some(disabled) = mesh_disabled_response(&state) {
        return disabled;
    }
    if !node_exists(&state, &workspace_id, &node_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "node_not_found" })),
        );
    }
    let now = now_seconds();
    let ttl = req
        .ttl_seconds
        .unwrap_or(state.config.mesh.presence_ttl_seconds)
        .max(1);
    let presence = MeshPresence {
        workspace_id: workspace_id.clone(),
        node_id: node_id.clone(),
        online: req.status.as_deref().unwrap_or("online") == "online",
        endpoint_hints: req.endpoint_hints,
        load: req.load,
        updated_at: now,
        expires_at: now + ttl,
    };
    state
        .mesh
        .presence
        .insert((workspace_id, node_id.clone()), presence.clone());
    (
        StatusCode::OK,
        Json(
            json!({ "nodeId": node_id, "online": presence.online, "expiresAt": presence.expires_at }),
        ),
    )
}

pub async fn publish_file(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
    Json(req): Json<PublishFileRequest>,
) -> impl IntoResponse {
    if let Some(disabled) = mesh_disabled_response(&state) {
        return disabled;
    }
    if !workspace_exists(&state, &workspace_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "workspace_not_found" })),
        );
    }
    let manifest = req.manifest;
    if json_size(&manifest) > MAX_MESH_JSON_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({ "error": "manifest_too_large" })),
        );
    }
    let requested_availability = req.availability;
    let availability_node_id = requested_availability
        .as_ref()
        .and_then(|value| string_field(value, "nodeId"));
    if let Some(node_id) = availability_node_id.as_deref() {
        if !node_exists(&state, &workspace_id, node_id) {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "node_not_found" })),
            );
        }
    }
    let file_id = string_field(&manifest, "fileId")
        .unwrap_or_else(|| format!("file_{}", uuid::Uuid::new_v4().simple()));
    let file = MeshFile {
        workspace_id: workspace_id.clone(),
        file_id: file_id.clone(),
        name: string_field(&manifest, "name").unwrap_or_else(|| file_id.clone()),
        size_bytes: u64_field(&manifest, "size").unwrap_or(0),
        piece_size: u64_field(&manifest, "pieceSize").unwrap_or(0),
        piece_count: u64_field(&manifest, "pieceCount").unwrap_or(0),
        created_by_node_id: availability_node_id.clone().unwrap_or_default(),
        manifest,
        tags: Value::Array(vec![]),
        created_at: now_seconds(),
    };
    state
        .mesh
        .files
        .insert((workspace_id.clone(), file_id.clone()), file.clone());
    if let Some(availability) = requested_availability {
        if let Some(node_id) = availability_node_id {
            let availability = MeshAvailability {
                workspace_id: workspace_id.clone(),
                file_id: file_id.clone(),
                node_id: node_id.clone(),
                complete: availability
                    .get("complete")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                verified_ranges: availability
                    .get("verifiedRanges")
                    .cloned()
                    .unwrap_or(Value::Array(vec![])),
                updated_at: now_seconds(),
                advertise_until: None,
            };
            state.mesh.availability.insert(
                (workspace_id.clone(), file_id.clone(), node_id),
                availability,
            );
        }
    }
    (
        StatusCode::OK,
        Json(json!({ "fileId": file_id, "name": file.name, "sizeBytes": file.size_bytes })),
    )
}

pub async fn list_files(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
) -> impl IntoResponse {
    if let Some(disabled) = mesh_disabled_response(&state) {
        return disabled;
    }
    if !workspace_exists(&state, &workspace_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "workspace_not_found" })),
        );
    }
    let files: Vec<_> = state.mesh.files.iter()
        .filter(|entry| entry.key().0 == workspace_id)
        .map(|entry| {
            let file = entry.value();
            let online_providers = count_online_providers(&state, &workspace_id, &file.file_id);
            json!({ "fileId": file.file_id, "name": file.name, "sizeBytes": file.size_bytes, "pieceCount": file.piece_count, "onlineProviders": online_providers })
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({ "workspaceId": workspace_id, "files": files })),
    )
}

pub async fn get_file(
    State(state): State<Arc<AppState>>,
    Path((workspace_id, file_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Some(disabled) = mesh_disabled_response(&state) {
        return disabled;
    }
    let Some(file) = state
        .mesh
        .files
        .get(&(workspace_id.clone(), file_id.clone()))
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "file_not_found" })),
        );
    };
    (
        StatusCode::OK,
        Json(
            json!({ "fileId": file.file_id, "name": file.name, "sizeBytes": file.size_bytes, "pieceCount": file.piece_count }),
        ),
    )
}

pub async fn update_availability(
    State(state): State<Arc<AppState>>,
    Path((workspace_id, file_id, node_id)): Path<(String, String, String)>,
    Json(req): Json<UpdateAvailabilityRequest>,
) -> impl IntoResponse {
    if let Some(disabled) = mesh_disabled_response(&state) {
        return disabled;
    }
    if !node_exists(&state, &workspace_id, &node_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "node_not_found" })),
        );
    }
    if !state
        .mesh
        .files
        .contains_key(&(workspace_id.clone(), file_id.clone()))
    {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "file_not_found" })),
        );
    }
    let availability = MeshAvailability {
        workspace_id: workspace_id.clone(),
        file_id: file_id.clone(),
        node_id: node_id.clone(),
        complete: req.complete,
        verified_ranges: req.verified_ranges,
        updated_at: now_seconds(),
        advertise_until: req.advertise_until,
    };
    state
        .mesh
        .availability
        .insert((workspace_id, file_id, node_id.clone()), availability);
    (
        StatusCode::OK,
        Json(json!({ "nodeId": node_id, "updated": true })),
    )
}

pub async fn candidates(
    State(state): State<Arc<AppState>>,
    Path((workspace_id, file_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Some(disabled) = mesh_disabled_response(&state) {
        return disabled;
    }
    if !state
        .mesh
        .files
        .contains_key(&(workspace_id.clone(), file_id.clone()))
    {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "file_not_found" })),
        );
    }
    let now = now_seconds();
    let providers: Vec<_> = state.mesh.availability.iter()
        .filter(|entry| entry.key().0 == workspace_id && entry.key().1 == file_id)
        .filter_map(|entry| {
            let node_id = entry.key().2.clone();
            let presence = state.mesh.presence.get(&(workspace_id.clone(), node_id.clone()))?;
            let online = presence.online && presence.expires_at >= now;
            Some(json!({ "nodeId": node_id, "online": online, "verifiedRanges": entry.value().verified_ranges, "score": if online { 1.0 } else { 0.0 }, "endpointHints": presence.endpoint_hints }))
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({ "fileId": file_id, "providers": providers })),
    )
}

pub async fn record_event(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
    Json(req): Json<RecordEventRequest>,
) -> impl IntoResponse {
    if let Some(disabled) = mesh_disabled_response(&state) {
        return disabled;
    }
    if !workspace_exists(&state, &workspace_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "workspace_not_found" })),
        );
    }
    if json_size(&req.payload) > MAX_MESH_JSON_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({ "error": "event_too_large" })),
        );
    }
    let event_id = uuid::Uuid::new_v4().to_string();
    let event = MeshEvent {
        event_id: event_id.clone(),
        workspace_id,
        event_type: req.event_type,
        payload: req.payload,
        created_at: now_seconds(),
    };
    state.mesh.events.insert(event_id.clone(), event);
    (StatusCode::OK, Json(json!({ "eventId": event_id })))
}

pub async fn create_share(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
    Json(req): Json<CreateShareRequest>,
) -> impl IntoResponse {
    if let Some(disabled) = mesh_disabled_response(&state) {
        return disabled;
    }
    if !workspace_exists(&state, &workspace_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "workspace_not_found" })),
        );
    }
    if !state
        .mesh
        .files
        .contains_key(&(workspace_id.clone(), req.file_id.clone()))
    {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "file_not_found" })),
        );
    }
    let created_by_node_id = req.created_by_node_id.unwrap_or_default();
    if !created_by_node_id.is_empty() && !node_exists(&state, &workspace_id, &created_by_node_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "node_not_found" })),
        );
    }
    if json_size(&req.capabilities) > MAX_MESH_JSON_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({ "error": "share_too_large" })),
        );
    }
    let now = now_seconds();
    let code = req
        .code
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(generate_share_code);
    let share = MeshShare {
        code: code.clone(),
        workspace_id: workspace_id.clone(),
        file_id: req.file_id,
        created_by_node_id,
        expires_at: now + req.ttl_seconds.unwrap_or(86_400).max(1),
        revoked_at: None,
        capabilities: req.capabilities,
        created_at: now,
    };
    state.mesh.shares.insert(code.clone(), share.clone());
    (
        StatusCode::OK,
        Json(json!({
            "code": code,
            "workspaceId": workspace_id,
            "fileId": share.file_id,
            "createdByNodeId": share.created_by_node_id,
            "expiresAt": share.expires_at,
            "revokedAt": share.revoked_at,
            "capabilities": share.capabilities
        })),
    )
}

pub async fn resolve_share(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
) -> impl IntoResponse {
    if let Some(disabled) = mesh_disabled_response(&state) {
        return disabled;
    }
    let Some(share) = state.mesh.shares.get(&code) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "share_not_found" })),
        );
    };
    let share = share.clone();
    if let Some(revoked_at) = share.revoked_at {
        return (
            StatusCode::GONE,
            Json(json!({ "error": "share_revoked", "revokedAt": revoked_at })),
        );
    }
    let now = now_seconds();
    if share.expires_at < now {
        return (
            StatusCode::GONE,
            Json(json!({ "error": "share_expired", "expiresAt": share.expires_at })),
        );
    }
    let Some(file) = state
        .mesh
        .files
        .get(&(share.workspace_id.clone(), share.file_id.clone()))
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "file_not_found" })),
        );
    };
    (
        StatusCode::OK,
        Json(json!({
            "code": share.code,
            "workspaceId": share.workspace_id,
            "fileId": share.file_id,
            "createdByNodeId": share.created_by_node_id,
            "name": file.name,
            "sizeBytes": file.size_bytes,
            "pieceSize": file.piece_size,
            "pieceCount": file.piece_count,
            "expiresAt": share.expires_at,
            "revokedAt": share.revoked_at,
            "capabilities": share.capabilities
        })),
    )
}

pub async fn revoke_share(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
) -> impl IntoResponse {
    if let Some(disabled) = mesh_disabled_response(&state) {
        return disabled;
    }
    let Some(mut share) = state.mesh.shares.get_mut(&code) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "share_not_found" })),
        );
    };
    let now = now_seconds();
    share.revoked_at = Some(now);
    (
        StatusCode::OK,
        Json(json!({ "code": code, "revokedAt": now })),
    )
}

pub async fn share_candidates(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
) -> impl IntoResponse {
    if let Some(disabled) = mesh_disabled_response(&state) {
        return disabled;
    }
    let Some(share) = active_share(&state, &code) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "share_not_found_or_inactive" })),
        );
    };
    let now = now_seconds();
    let providers: Vec<_> = state
        .mesh
        .availability
        .iter()
        .filter(|entry| entry.key().0 == share.workspace_id && entry.key().1 == share.file_id)
        .filter_map(|entry| {
            let node_id = entry.key().2.clone();
            let presence = state
                .mesh
                .presence
                .get(&(share.workspace_id.clone(), node_id.clone()))?;
            let online = presence.online && presence.expires_at >= now;
            Some(json!({
                "nodeId": node_id,
                "online": online,
                "verifiedRanges": entry.value().verified_ranges,
                "score": if online { 1.0 } else { 0.0 },
                "endpointHints": presence.endpoint_hints
            }))
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({ "code": code, "fileId": share.file_id, "providers": providers })),
    )
}

pub async fn record_share_event(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
    Json(req): Json<ShareEventRequest>,
) -> impl IntoResponse {
    if let Some(disabled) = mesh_disabled_response(&state) {
        return disabled;
    }
    let Some(share) = active_share(&state, &code) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "share_not_found_or_inactive" })),
        );
    };
    if json_size(&req.payload) > MAX_MESH_JSON_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({ "error": "event_too_large" })),
        );
    }
    let event_id = uuid::Uuid::new_v4().to_string();
    let event = MeshEvent {
        event_id: event_id.clone(),
        workspace_id: share.workspace_id,
        event_type: req.event_type,
        payload: json!({ "shareCode": code, "payload": req.payload }),
        created_at: now_seconds(),
    };
    state.mesh.events.insert(event_id.clone(), event);
    (StatusCode::OK, Json(json!({ "eventId": event_id })))
}

fn mesh_disabled_response(state: &AppState) -> Option<(StatusCode, Json<Value>)> {
    (!state.config.mesh.enabled).then(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "mesh_disabled" })),
        )
    })
}

fn workspace_exists(state: &AppState, workspace_id: &str) -> bool {
    state.mesh.workspaces.contains_key(workspace_id)
}

fn node_exists(state: &AppState, workspace_id: &str, node_id: &str) -> bool {
    state
        .mesh
        .nodes
        .contains_key(&(workspace_id.to_string(), node_id.to_string()))
}

fn json_size(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

fn count_online_providers(state: &AppState, workspace_id: &str, file_id: &str) -> usize {
    let now = now_seconds();
    state
        .mesh
        .availability
        .iter()
        .filter(|entry| entry.key().0 == workspace_id && entry.key().1 == file_id)
        .filter(|entry| {
            state
                .mesh
                .presence
                .get(&(workspace_id.to_string(), entry.key().2.clone()))
                .is_some_and(|presence| presence.online && presence.expires_at >= now)
        })
        .count()
}

fn active_share(state: &AppState, code: &str) -> Option<MeshShare> {
    let share = state.mesh.shares.get(code)?.clone();
    if share.revoked_at.is_some() || share.expires_at < now_seconds() {
        return None;
    }
    Some(share)
}

fn generate_share_code() -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    format!(
        "{}-{}",
        &raw[0..4].to_uppercase(),
        &raw[4..8].to_uppercase()
    )
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn u64_field(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, MeshConfig};

    #[test]
    fn mesh_config_defaults_disabled() {
        let config = Config::from_env_with_mesh(MeshConfig::default());
        assert!(!config.mesh.enabled);
    }

    #[test]
    fn create_workspace_request_accepts_explicit_workspace_id() {
        let req: CreateWorkspaceRequest =
            serde_json::from_value(json!({ "workspaceId": "ws_cli", "name": "CLI Workspace" }))
                .expect("valid workspace request");
        assert_eq!(req.workspace_id.as_deref(), Some("ws_cli"));
        assert_eq!(req.name, "CLI Workspace");
    }

    #[test]
    fn create_share_request_accepts_optional_code_and_ttl() {
        let req: CreateShareRequest = serde_json::from_value(json!({
            "code": "ABCD-1234",
            "fileId": "file",
            "createdByNodeId": "node",
            "ttlSeconds": 120,
            "capabilities": ["grid", "resume"]
        }))
        .expect("valid share request");
        assert_eq!(req.code.as_deref(), Some("ABCD-1234"));
        assert_eq!(req.file_id, "file");
        assert_eq!(req.created_by_node_id.as_deref(), Some("node"));
        assert_eq!(req.ttl_seconds, Some(120));
    }

    #[test]
    fn active_share_excludes_revoked_and_expired_shares() {
        let state = AppState::new_for_test_with_mesh(true);
        let now = now_seconds();
        state.mesh.shares.insert(
            "OKAY-0001".into(),
            MeshShare {
                code: "OKAY-0001".into(),
                workspace_id: "ws".into(),
                file_id: "file".into(),
                created_by_node_id: "node".into(),
                expires_at: now + 60,
                revoked_at: None,
                capabilities: json!([]),
                created_at: now,
            },
        );
        state.mesh.shares.insert(
            "OLD-0001".into(),
            MeshShare {
                code: "OLD-0001".into(),
                workspace_id: "ws".into(),
                file_id: "file".into(),
                created_by_node_id: "node".into(),
                expires_at: now.saturating_sub(1),
                revoked_at: None,
                capabilities: json!([]),
                created_at: now,
            },
        );
        state.mesh.shares.insert(
            "NOPE-0001".into(),
            MeshShare {
                code: "NOPE-0001".into(),
                workspace_id: "ws".into(),
                file_id: "file".into(),
                created_by_node_id: "node".into(),
                expires_at: now + 60,
                revoked_at: Some(now),
                capabilities: json!([]),
                created_at: now,
            },
        );

        assert_eq!(
            active_share(&state, "OKAY-0001").map(|share| share.code),
            Some("OKAY-0001".into())
        );
        assert!(active_share(&state, "OLD-0001").is_none());
        assert!(active_share(&state, "NOPE-0001").is_none());
    }

    #[test]
    fn generated_share_codes_are_short_human_codes() {
        let code = generate_share_code();
        assert_eq!(code.len(), 9);
        assert_eq!(code.as_bytes()[4], b'-');
        assert!(code.chars().all(|ch| ch == '-' || ch.is_ascii_hexdigit()));
    }

    #[test]
    fn mesh_state_counts_only_online_unexpired_providers() {
        let state = AppState::new_for_test_with_mesh(true);
        state.mesh.availability.insert(
            ("ws".into(), "file".into(), "node".into()),
            MeshAvailability {
                workspace_id: "ws".into(),
                file_id: "file".into(),
                node_id: "node".into(),
                complete: true,
                verified_ranges: json!([[0, 1]]),
                updated_at: now_seconds(),
                advertise_until: None,
            },
        );
        state.mesh.presence.insert(
            ("ws".into(), "node".into()),
            MeshPresence {
                workspace_id: "ws".into(),
                node_id: "node".into(),
                online: true,
                endpoint_hints: json!([]),
                load: json!({}),
                updated_at: now_seconds(),
                expires_at: now_seconds() + 60,
            },
        );
        assert_eq!(count_online_providers(&state, "ws", "file"), 1);
    }
}
