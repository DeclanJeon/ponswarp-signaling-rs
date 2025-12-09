//! TURN 자격증명 핸들러

use crate::config::TurnConfig;
use crate::protocol::{IceServer, ServerMessage, TurnConfigData};
use crate::state::AppState;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::UnboundedSender;

type HmacSha1 = Hmac<Sha1>;

/// TURN 설정 요청 처리
pub async fn handle_turn_config_request(
    state: Arc<AppState>,
    sender: &UnboundedSender<ServerMessage>,
    room_id: &str,
) {
    let turn_config = &state.config.turn;

    if turn_config.url.is_empty() || turn_config.secret.is_empty() {
        let _ = sender.send(ServerMessage::TurnConfig {
            success: false,
            data: None,
            error: Some("TURN server not configured".to_string()),
        });
        return;
    }

    let credentials = generate_credentials(turn_config);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let _ = sender.send(ServerMessage::TurnConfig {
        success: true,
        data: Some(TurnConfigData {
            ice_servers: credentials,
            ttl: turn_config.credential_ttl,
            timestamp: now,
            room_id: room_id.to_string(),
        }),
        error: None,
    });

    tracing::info!(room_id = %room_id, "TURN config sent");
}

/// TURN 자격증명 생성 (RFC 5766 HMAC-SHA1)
fn generate_credentials(config: &TurnConfig) -> Vec<IceServer> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let expiry_time = now + config.credential_ttl;

    // username 생성
    let random: u64 = rand::random();
    let base_username = format!("user_{}_{:x}", now, random);
    let credential_username = format!("{}:{}", base_username, expiry_time);

    // HMAC-SHA1 해시 생성
    let password = generate_hmac_hash(&credential_username, &config.secret);

    // ICE 서버 목록 생성
    build_ice_servers(config, &credential_username, &password)
}

fn generate_hmac_hash(username: &str, secret: &str) -> String {
    let mut mac =
        HmacSha1::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(username.as_bytes());
    let result = mac.finalize();
    BASE64.encode(result.into_bytes())
}

fn build_ice_servers(config: &TurnConfig, username: &str, password: &str) -> Vec<IceServer> {
    let mut servers = Vec::new();
    let mut turn_urls = Vec::new();

    if config.enable_udp {
        turn_urls.push(format!("turn:{}:{}", config.url, config.ports.udp));
    }
    if config.enable_tcp {
        turn_urls.push(format!("turn:{}:{}", config.url, config.ports.tcp));
    }
    if config.enable_tls {
        turn_urls.push(format!(
            "turns:{}:{}?transport=tcp",
            config.url, config.ports.tls
        ));
    }

    // 폴백 서버 추가
    for fallback in &config.fallback_servers {
        if config.enable_tls {
            turn_urls.push(format!(
                "turns:{}:{}?transport=tcp",
                fallback, config.ports.tls
            ));
        } else {
            turn_urls.push(format!("turn:{}:{}", fallback, config.ports.udp));
        }
    }

    // TURN 서버 (인증 필요)
    for url in &turn_urls {
        servers.push(IceServer {
            urls: vec![url.clone()],
            username: Some(username.to_string()),
            credential: Some(password.to_string()),
            credential_type: Some("password".to_string()),
        });
    }

    // STUN 서버 (인증 불필요)
    if config.enable_udp {
        servers.push(IceServer {
            urls: vec![format!("stun:{}:{}", config.url, config.ports.udp)],
            username: None,
            credential: None,
            credential_type: None,
        });
    }

    servers
}

/// 자격증명 유효성 검증
pub fn validate_credentials(username: &str) -> bool {
    if let Some(expiry_str) = username.split(':').last() {
        if let Ok(expiry_time) = expiry_str.parse::<u64>() {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            return expiry_time > now;
        }
    }
    false
}
