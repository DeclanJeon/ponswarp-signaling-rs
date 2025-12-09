//! 클라이언트-서버 메시지 프로토콜 정의

use serde::{Deserialize, Serialize};

/// 클라이언트 → 서버 메시지
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum ClientMessage {
    // Connection
    Heartbeat,

    // Room Management
    JoinRoom { room_id: String },
    LeaveRoom,

    // WebRTC Signaling
    Offer {
        room_id: String,
        sdp: String,
        target: Option<String>,
    },
    Answer {
        room_id: String,
        sdp: String,
        target: Option<String>,
    },
    IceCandidate {
        room_id: String,
        candidate: String,
        target: Option<String>,
    },

    // TURN
    RequestTurnConfig {
        room_id: String,
        force_refresh: Option<bool>,
    },
    RefreshTurnCredentials {
        room_id: String,
        current_username: String,
    },
    CheckTurnServerStatus,
}

/// 서버 → 클라이언트 메시지
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum ServerMessage {
    // Connection
    Connected { socket_id: String },
    HeartbeatAck,
    Error { code: String, message: String },

    // Room Events
    JoinedRoom {
        room_id: String,
        socket_id: String,
        user_count: usize,
    },
    RoomUsers {
        users: Vec<String>,
    },
    PeerJoined {
        socket_id: String,
        room_id: String,
    },
    UserLeft {
        socket_id: String,
    },
    RoomFull {
        room_id: String,
    },

    // WebRTC Signaling
    Offer {
        from: String,
        sdp: String,
    },
    Answer {
        from: String,
        sdp: String,
    },
    IceCandidate {
        from: String,
        candidate: String,
    },

    // TURN
    TurnConfig {
        success: bool,
        data: Option<TurnConfigData>,
        error: Option<String>,
    },
    TurnServerStatusUpdate {
        room_id: String,
        timestamp: u64,
    },
}

/// TURN 설정 데이터
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnConfigData {
    pub ice_servers: Vec<IceServer>,
    pub ttl: u64,
    pub timestamp: u64,
    pub room_id: String,
}

/// ICE 서버 설정
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_type: Option<String>,
}
