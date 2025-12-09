//! WebRTC 시그널링 핸들러

use crate::protocol::ServerMessage;
use crate::state::AppState;
use std::sync::Arc;

/// Offer 처리
pub async fn handle_offer(
    state: Arc<AppState>,
    from_peer_id: &str,
    room_id: &str,
    sdp: &str,
    target: Option<&str>,
) {
    let message = ServerMessage::Offer {
        from: from_peer_id.to_string(),
        sdp: sdp.to_string(),
    };

    if let Some(target_id) = target {
        send_to_peer(&state, target_id, message).await;
    } else {
        broadcast_to_room_except(&state, room_id, from_peer_id, message).await;
    }

    tracing::debug!(
        from = %from_peer_id,
        room_id = %room_id,
        target = ?target,
        "Relayed offer"
    );
}

/// Answer 처리
pub async fn handle_answer(
    state: Arc<AppState>,
    from_peer_id: &str,
    room_id: &str,
    sdp: &str,
    target: Option<&str>,
) {
    let message = ServerMessage::Answer {
        from: from_peer_id.to_string(),
        sdp: sdp.to_string(),
    };

    if let Some(target_id) = target {
        send_to_peer(&state, target_id, message).await;
    } else {
        broadcast_to_room_except(&state, room_id, from_peer_id, message).await;
    }

    tracing::debug!(
        from = %from_peer_id,
        room_id = %room_id,
        target = ?target,
        "Relayed answer"
    );
}

/// ICE Candidate 처리
pub async fn handle_ice_candidate(
    state: Arc<AppState>,
    from_peer_id: &str,
    room_id: &str,
    candidate: &str,
    target: Option<&str>,
) {
    let message = ServerMessage::IceCandidate {
        from: from_peer_id.to_string(),
        candidate: candidate.to_string(),
    };

    if let Some(target_id) = target {
        send_to_peer(&state, target_id, message).await;
    } else {
        broadcast_to_room_except(&state, room_id, from_peer_id, message).await;
    }

    tracing::debug!(
        from = %from_peer_id,
        room_id = %room_id,
        target = ?target,
        "Relayed ICE candidate"
    );
}

/// 특정 피어에게 메시지 전송
async fn send_to_peer(state: &AppState, peer_id: &str, message: ServerMessage) {
    if let Some(session) = state.peers.get(peer_id) {
        let _ = session.sender.send(message);
    }
}

/// 방의 특정 피어를 제외하고 브로드캐스트
async fn broadcast_to_room_except(
    state: &AppState,
    room_id: &str,
    except_peer_id: &str,
    message: ServerMessage,
) {
    if let Some(room) = state.rooms.get(room_id) {
        let users = room.users.read().await;
        for peer_id in users.iter() {
            if peer_id != except_peer_id {
                if let Some(session) = state.peers.get(peer_id) {
                    let _ = session.sender.send(message.clone());
                }
            }
        }
    }
}
