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
    pub cloud: CloudConfig,
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

/// Cloudflare R2 backed temporary file share 설정
#[derive(Debug, Clone)]
pub struct CloudConfig {
    pub enabled: bool,
    pub bucket: String,
    pub endpoint: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub region: String,
    pub prefix: String,
    pub retention_seconds: u64,
    pub upload_url_ttl_seconds: u64,
    pub download_url_ttl_seconds: u64,
    pub cleanup_interval_seconds: u64,
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
}

impl Config {
    /// 환경 변수에서 설정 로드
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();

        let r2_account_id = env::var("R2_ACCOUNT_ID").unwrap_or_default();
        let r2_endpoint = env::var("R2_ENDPOINT")
            .or_else(|_| env::var("CLOUDFLARE_R2_ENDPOINT"))
            .unwrap_or_else(|_| {
                if r2_account_id.is_empty() {
                    String::new()
                } else {
                    format!("https://{}.r2.cloudflarestorage.com", r2_account_id)
                }
            });
        let r2_bucket = env::var("R2_BUCKET_NAME")
            .or_else(|_| env::var("CLOUDFLARE_R2_BUCKET"))
            .unwrap_or_default();
        let r2_access_key = env::var("R2_ACCESS_KEY_ID")
            .or_else(|_| env::var("CLOUDFLARE_R2_ACCESS_KEY_ID"))
            .unwrap_or_default();
        let r2_secret_key = env::var("R2_SECRET_ACCESS_KEY")
            .or_else(|_| env::var("CLOUDFLARE_R2_SECRET_ACCESS_KEY"))
            .unwrap_or_default();
        let cloud_enabled = env::var("PONSWARP_CLOUD_ENABLED")
            .map(|v| v != "false")
            .unwrap_or_else(|_| {
                !r2_endpoint.is_empty()
                    && !r2_bucket.is_empty()
                    && !r2_access_key.is_empty()
                    && !r2_secret_key.is_empty()
            });

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
            cloud: CloudConfig {
                enabled: cloud_enabled,
                bucket: r2_bucket,
                endpoint: r2_endpoint,
                access_key_id: r2_access_key,
                secret_access_key: r2_secret_key,
                region: env::var("R2_REGION")
                    .or_else(|_| env::var("CLOUDFLARE_R2_REGION"))
                    .unwrap_or_else(|_| "auto".to_string()),
                prefix: env::var("PONSWARP_CLOUD_PREFIX")
                    .unwrap_or_else(|_| "ponswarp-cloud".to_string()),
                retention_seconds: env::var("PONSWARP_CLOUD_RETENTION_SECONDS")
                    .unwrap_or_else(|_| "86400".to_string())
                    .parse()
                    .unwrap_or(86400),
                upload_url_ttl_seconds: env::var("PONSWARP_CLOUD_UPLOAD_URL_TTL_SECONDS")
                    .or_else(|_| env::var("R2_SIGNED_URL_TTL_SECONDS"))
                    .unwrap_or_else(|_| "3600".to_string())
                    .parse()
                    .unwrap_or(3600),
                download_url_ttl_seconds: env::var("PONSWARP_CLOUD_DOWNLOAD_URL_TTL_SECONDS")
                    .unwrap_or_else(|_| "300".to_string())
                    .parse()
                    .unwrap_or(300),
                cleanup_interval_seconds: env::var("PONSWARP_CLOUD_CLEANUP_INTERVAL_SECONDS")
                    .unwrap_or_else(|_| "300".to_string())
                    .parse()
                    .unwrap_or(300),
                max_files: env::var("PONSWARP_CLOUD_MAX_FILES")
                    .unwrap_or_else(|_| "100".to_string())
                    .parse()
                    .unwrap_or(100),
                max_file_bytes: env::var("PONSWARP_CLOUD_MAX_FILE_BYTES")
                    .unwrap_or_else(|_| "10737418240".to_string())
                    .parse()
                    .unwrap_or(10 * 1024 * 1024 * 1024),
                max_total_bytes: env::var("PONSWARP_CLOUD_MAX_TOTAL_BYTES")
                    .unwrap_or_else(|_| "10737418240".to_string())
                    .parse()
                    .unwrap_or(10 * 1024 * 1024 * 1024),
            },
            log_level: env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
        }
    }
}
