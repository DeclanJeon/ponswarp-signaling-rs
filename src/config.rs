//! 환경 변수 기반 설정 관리

use std::env;

/// 서버 설정
#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub host: String,
    #[allow(dead_code)]
    pub cors_origins: Vec<String>,
    pub room: RoomConfig,
    pub turn: TurnConfig,
    pub log_level: String,
}

/// 방 설정
#[derive(Debug, Clone)]
pub struct RoomConfig {
    pub max_size: usize,
    pub timeout_ms: u64,
}

/// TURN 서버 설정
#[derive(Debug, Clone)]
pub struct TurnConfig {
    pub url: String,
    pub secret: String,
    #[allow(dead_code)]
    pub realm: String,
    pub enable_tls: bool,
    pub enable_udp: bool,
    pub enable_tcp: bool,
    pub ports: TurnPorts,
    pub credential_ttl: u64,
    pub fallback_servers: Vec<String>,
}

/// TURN 포트 설정
#[derive(Debug, Clone)]
pub struct TurnPorts {
    pub udp: u16,
    pub tcp: u16,
    pub tls: u16,
}

impl Config {
    /// 환경 변수에서 설정 로드
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();

        Self {
            port: env::var("PORT")
                .unwrap_or_else(|_| "5502".to_string())
                .parse()
                .unwrap_or(5502),
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            cors_origins: env::var("CORS_ORIGINS")
                .unwrap_or_else(|_| "http://localhost:3500".to_string())
                .split(',')
                .map(|s| s.trim().to_string())
                .collect(),
            room: RoomConfig {
                max_size: env::var("MAX_ROOM_SIZE")
                    .unwrap_or_else(|_| "4".to_string())
                    .parse()
                    .unwrap_or(4),
                timeout_ms: env::var("ROOM_TIMEOUT")
                    .unwrap_or_else(|_| "3600000".to_string())
                    .parse()
                    .unwrap_or(3600000),
            },
            turn: TurnConfig {
                url: env::var("TURN_SERVER_URL").unwrap_or_default(),
                secret: env::var("TURN_SECRET").unwrap_or_default(),
                realm: env::var("TURN_REALM").unwrap_or_default(),
                enable_tls: env::var("TURN_ENABLE_TLS")
                    .map(|v| v == "true")
                    .unwrap_or(false),
                enable_udp: env::var("TURN_ENABLE_UDP")
                    .map(|v| v != "false")
                    .unwrap_or(true),
                enable_tcp: env::var("TURN_ENABLE_TCP")
                    .map(|v| v != "false")
                    .unwrap_or(true),
                ports: TurnPorts {
                    udp: env::var("TURN_PORT_UDP")
                        .unwrap_or_else(|_| "3478".to_string())
                        .parse()
                        .unwrap_or(3478),
                    tcp: env::var("TURN_PORT_TCP")
                        .unwrap_or_else(|_| "3478".to_string())
                        .parse()
                        .unwrap_or(3478),
                    tls: env::var("TURN_PORT_TLS")
                        .unwrap_or_else(|_| "443".to_string())
                        .parse()
                        .unwrap_or(443),
                },
                credential_ttl: env::var("TURN_CREDENTIAL_TTL")
                    .unwrap_or_else(|_| "3600".to_string())
                    .parse()
                    .unwrap_or(3600),
                fallback_servers: env::var("TURN_FALLBACK_SERVERS")
                    .unwrap_or_default()
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.trim().to_string())
                    .collect(),
            },
            log_level: env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
        }
    }
}
