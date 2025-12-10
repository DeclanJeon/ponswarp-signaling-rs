//! PonsWarp Rust 시그널링 서버

mod config;
mod handlers;
mod protocol;
mod state;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use config::Config;
use futures::{SinkExt, StreamExt};
use protocol::{ClientMessage, ServerMessage};
use state::AppState;
use std::sync::Arc;
use tokio::sync::mpsc;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    let config = Config::from_env();

    // 로깅 초기화
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(&config.log_level))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let state = Arc::new(AppState::new(config.clone()));

    // 방 정리 스케줄러
    let cleanup_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(300));
        loop {
            interval.tick().await;
            handlers::cleanup_old_rooms(cleanup_state.clone()).await;
        }
    });

    // CORS 설정
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // 라우터 설정
    let app = Router::new()
        .route("/", get(index_handler))
        .route("/health", get(health_handler))
        .route("/ws", get(ws_handler))
        .layer(cors)
        .with_state(state.clone());

    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();

    tracing::info!("🚀 PonsWarp Rust Signaling Server started");
    tracing::info!("Address: {}", addr);
    tracing::info!("WebSocket: ws://{}/ws", addr);

    axum::serve(listener, app).await.unwrap();
}

async fn index_handler() -> Html<&'static str> {
    Html("<h1>PonsWarp Signaling Server (Rust)</h1><p>WebSocket endpoint: /ws</p>")
}

async fn health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "server": "ponswarp-signaling-rs",
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMessage>();

    // 연결 처리
    let peer_id = handlers::handle_connection(state.clone(), tx.clone()).await;

    // 송신 태스크
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if ws_sender.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
    });

    // 수신 처리
    let state_clone = state.clone();
    let peer_id_clone = peer_id.clone();
    let tx_clone = tx.clone();

    while let Some(result) = ws_receiver.next().await {
        match result {
            Ok(Message::Text(text)) => {
                if let Ok(msg) = serde_json::from_str::<ClientMessage>(&text) {
                    handle_client_message(&state_clone, &peer_id_clone, &tx_clone, msg).await;
                }
            }
            Ok(Message::Close(_)) => break,
            Err(_) => break,
            _ => {}
        }
    }

    // 연결 해제
    handlers::handle_disconnect(state, &peer_id).await;
    send_task.abort();
}

async fn handle_client_message(
    state: &Arc<AppState>,
    peer_id: &str,
    sender: &mpsc::UnboundedSender<ServerMessage>,
    msg: ClientMessage,
) {
    match msg {
        ClientMessage::Heartbeat => {
            handlers::handle_heartbeat(sender);
        }
        ClientMessage::JoinRoom { room_id } => {
            handlers::handle_join_room(state.clone(), peer_id, &room_id).await;
        }
        ClientMessage::LeaveRoom => {
            handlers::handle_leave_room(state.clone(), peer_id).await;
        }
        ClientMessage::Offer { room_id, sdp, target } => {
            handlers::handle_offer(
                state.clone(),
                peer_id,
                &room_id,
                &sdp,
                target.as_deref(),
            )
            .await;
        }
        ClientMessage::Answer { room_id, sdp, target } => {
            handlers::handle_answer(
                state.clone(),
                peer_id,
                &room_id,
                &sdp,
                target.as_deref(),
            )
            .await;
        }
        ClientMessage::IceCandidate { room_id, candidate, target } => {
            handlers::handle_ice_candidate(
                state.clone(),
                peer_id,
                &room_id,
                &candidate,
                target.as_deref(),
            )
            .await;
        }
        ClientMessage::Manifest { room_id, manifest, target } => {
            handlers::handle_manifest(
                state.clone(),
                peer_id,
                &room_id,
                &manifest,
                target.as_deref(),
            )
            .await;
        }
        ClientMessage::TransferReady { room_id, target } => {
            handlers::handle_transfer_ready(
                state.clone(),
                peer_id,
                &room_id,
                target.as_deref(),
            )
            .await;
        }
        ClientMessage::TransferComplete { room_id, target } => {
            handlers::handle_transfer_complete(
                state.clone(),
                peer_id,
                &room_id,
                target.as_deref(),
            )
            .await;
        }
        ClientMessage::RequestTurnConfig { room_id, .. } => {
            handlers::handle_turn_config_request(state.clone(), sender, &room_id).await;
        }
        ClientMessage::RefreshTurnCredentials { room_id, current_username } => {
            if handlers::validate_credentials(&current_username) {
                let _ = sender.send(ServerMessage::TurnConfig {
                    success: true,
                    data: None,
                    error: Some("Credentials still valid".to_string()),
                });
            } else {
                handlers::handle_turn_config_request(state.clone(), sender, &room_id).await;
            }
        }
        ClientMessage::CheckTurnServerStatus => {
            let _ = sender.send(ServerMessage::TurnServerStatusUpdate {
                room_id: String::new(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            });
        }
    }
}
