//! 애플리케이션 상태 관리

use crate::config::Config;
use crate::protocol::ServerMessage;
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
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            rooms: DashMap::new(),
            peers: DashMap::new(),
            config: Arc::new(config),
        }
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
