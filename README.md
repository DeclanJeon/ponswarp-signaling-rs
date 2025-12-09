# PonsWarp Signaling Server (Rust)

고성능 WebRTC 시그널링 서버 - Rust + Tokio + Axum 기반

## 특징

- **고성능**: Rust의 제로 코스트 추상화와 Tokio 비동기 런타임
- **Thread-safe**: DashMap을 활용한 동시성 안전 상태 관리
- **WebSocket**: axum-ws 기반 실시간 통신
- **TURN 지원**: RFC 5766 HMAC-SHA1 자격증명 생성

## 빌드 및 실행

```bash
# 개발 모드
cargo run

# 릴리즈 빌드
cargo build --release
./target/release/ponswarp-signaling-rs
```

## 환경 변수

`.env.example`을 `.env`로 복사 후 설정:

```bash
cp .env.example .env
```

## API

- `GET /` - 서버 정보
- `GET /health` - 헬스 체크
- `GET /ws` - WebSocket 엔드포인트

## 메시지 프로토콜

JSON 기반 메시지 프레이밍:

```json
{"type": "JoinRoom", "payload": {"room_id": "abc123"}}
{"type": "Offer", "payload": {"room_id": "abc123", "sdp": "...", "target": null}}
```

## 프론트엔드 통합

`ponswarp/src/services/signaling-adapter.ts` 어댑터를 통해 기존 Socket.io 기반 코드와 호환됩니다.
