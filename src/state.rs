//! 애플리케이션 상태 관리

use crate::billing::BillingClient;
use crate::config::Config;
use crate::database::CloudDatabase;
use crate::protocol::ServerMessage;
use anyhow::{bail, Result};
use aws_config::BehaviorVersion;
use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::Client;
use dashmap::DashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc::UnboundedSender, RwLock};

/// 전역 애플리케이션 상태
pub struct AppState {
    /// 방 정보 (room_id -> Room)
    pub rooms: DashMap<String, Room>,
    /// 피어 세션 (peer_id -> PeerSession)
    pub peers: DashMap<String, PeerSession>,
    /// 설정
    pub config: Arc<Config>,
    /// Cloudflare R2 임시 파일 공유 저장소
    pub cloud: Option<Arc<CloudStorage>>,
    /// Optional DB-backed Cloud Drop persistence and entitlement state
    pub cloud_db: Option<Arc<CloudDatabase>>,
    /// Optional Stripe billing client
    pub billing: Option<Arc<BillingClient>>,
}

impl AppState {
    pub async fn new(config: Config) -> Result<Self> {
        let cloud = CloudStorage::from_config(&config).await?.map(Arc::new);
        let cloud_db = CloudDatabase::from_config(&config).await?.map(Arc::new);
        let billing = BillingClient::from_config(&config)?.map(Arc::new);
        Ok(Self {
            rooms: DashMap::new(),
            peers: DashMap::new(),
            config: Arc::new(config),
            cloud,
            cloud_db,
            billing,
        })
    }

    pub fn cloud_storage(
        &self,
    ) -> Result<&CloudStorage, crate::handlers::cloud_share::CloudShareError> {
        self.cloud.as_deref().ok_or_else(|| {
            crate::handlers::cloud_share::CloudShareError::bad_request(
                "Cloud share is not configured",
            )
        })
    }
}

/// Cloudflare R2 S3 API client.
pub struct CloudStorage {
    pub client: Client,
    pub bucket: String,
    pub prefix: String,
}

impl CloudStorage {
    async fn from_config(config: &Config) -> Result<Option<Self>> {
        let cloud = &config.cloud;
        if !cloud.enabled {
            tracing::info!("Cloud share disabled");
            return Ok(None);
        }
        if cloud.bucket.is_empty()
            || cloud.endpoint.is_empty()
            || cloud.access_key_id.is_empty()
            || cloud.secret_access_key.is_empty()
        {
            bail!("PONSWARP_CLOUD_ENABLED=true requires complete R2 configuration");
        }

        let credentials = Credentials::new(
            cloud.access_key_id.clone(),
            cloud.secret_access_key.clone(),
            None,
            None,
            "ponswarp-r2-env",
        );
        let shared_config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(cloud.region.clone()))
            .credentials_provider(credentials)
            .load()
            .await;
        let s3_config = aws_sdk_s3::config::Builder::from(&shared_config)
            .endpoint_url(cloud.endpoint.clone())
            .force_path_style(true)
            .build();

        Ok(Some(Self {
            client: Client::from_conf(s3_config),
            bucket: cloud.bucket.clone(),
            prefix: cloud.prefix.trim_matches('/').to_string(),
        }))
    }

    pub fn manifest_prefix(&self) -> String {
        format!("{}/manifests/", self.prefix)
    }

    pub fn manifest_key(&self, share_id: &str) -> String {
        format!("{}{share_id}.json", self.manifest_prefix())
    }

    pub fn file_key(&self, share_id: &str, file_id: &str) -> String {
        format!("{}/shares/{share_id}/files/{file_id}", self.prefix)
    }
}

/// 방 정보
pub struct Room {
    #[allow(dead_code)]
    pub id: String,
    pub users: RwLock<HashSet<String>>,
    pub created_at: Instant,
}

impl Room {
    pub fn new(id: String) -> Self {
        Self {
            id,
            users: RwLock::new(HashSet::new()),
            created_at: Instant::now(),
        }
    }
}

/// 피어 세션 정보
pub struct PeerSession {
    #[allow(dead_code)]
    pub id: String,
    pub room_id: RwLock<Option<String>>,
    pub sender: UnboundedSender<ServerMessage>,
    #[allow(dead_code)]
    pub connected_at: Instant,
}
