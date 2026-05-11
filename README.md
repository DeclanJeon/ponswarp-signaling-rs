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
- `GET /ready` - 운영 readiness 체크
- `GET /ws` - WebSocket 엔드포인트
- `GET /api/cloud-plans` - Cloud Drop 무료/유료 플랜 제한 조회
- `POST /api/cloud-share` - Cloudflare R2 24시간 공유 생성 및 업로드 URL 발급
- `POST /api/cloud-share/:share_id/complete` - 공유 업로드 완료 처리
- `GET /api/cloud-share/:share_id` - 공개 다운로드 매니페스트 조회
- `GET /api/cloud-share/:share_id/files/:file_id/download` - 파일 다운로드 URL 리다이렉트

### Cloudflare R2 24시간 공유

Cloud share는 R2 S3 호환 API를 사용합니다. 아래 환경 변수를 설정하면 서버 시작 시 자동으로 활성화됩니다.

```env
PONSWARP_CLOUD_ENABLED=true
R2_ACCOUNT_ID=...
R2_ACCESS_KEY_ID=...
R2_SECRET_ACCESS_KEY=...
R2_BUCKET_NAME=...
# 선택: R2_ENDPOINT=https://<account_id>.r2.cloudflarestorage.com

PONSWARP_CLOUD_PREFIX=ponswarp-cloud
PONSWARP_CLOUD_RETENTION_SECONDS=86400
PONSWARP_CLOUD_UPLOAD_URL_TTL_SECONDS=3600
PONSWARP_CLOUD_DOWNLOAD_URL_TTL_SECONDS=300
PONSWARP_CLOUD_CLEANUP_INTERVAL_SECONDS=300
PONSWARP_CLOUD_CLEANUP_RUN_ON_STARTUP=true
PONSWARP_CLOUD_MAX_FILES=100
PONSWARP_CLOUD_MAX_FILE_BYTES=10737418240
PONSWARP_CLOUD_MAX_TOTAL_BYTES=10737418240
```

서버는 만료된 공유를 주기적으로 삭제합니다. 운영 환경에서는 R2 버킷 라이프사이클도 1일 만료로 같이 설정하는 것을 권장합니다.
브라우저가 presigned PUT/GET 요청을 직접 보내므로 R2 버킷 CORS에는 프론트엔드 Origin과 `PUT`, `GET`, `HEAD` 메서드를 허용해야 합니다.
Cloud Drop은 공유 1건당 최대 10GB입니다. 10GB를 넘는 파일은 직접 P2P 전송을 사용하거나 10GB 단위로 분할해서 공유해야 합니다.

### 운영 안전 설정

명시적으로 Cloud Drop을 켠 상태에서 R2 설정이 불완전하면 서버는 기동에 실패합니다. 유료화 플래그를 켠 상태에서는 Postgres 연결도 필수입니다.

```env
CORS_ORIGINS=https://warp.ponslink.com

# 무료 Cloud Drop은 DB 없이도 R2 manifest fallback으로 동작합니다.
PONSWARP_BILLING_ENABLED=false

# 유료화/권한/사용량 기록을 켤 때 필요합니다.
PONSWARP_PUBLIC_APP_URL=https://warp.ponslink.com
PAYPAL_API_BASE=https://api-m.paypal.com
PAYPAL_CLIENT_ID=...
PAYPAL_CLIENT_SECRET=...
PAYPAL_WEBHOOK_ID=...
PAYPAL_PRO_PLAN_ID=...
DATABASE_URL=postgres://ponswarp:password@127.0.0.1:5432/ponswarp
DATABASE_MAX_CONNECTIONS=5
DATABASE_RUN_MIGRATIONS=true
```

`GET /ready`는 운영 헬스체크에 사용할 수 있습니다. `PONSWARP_BILLING_ENABLED=true`일 때는 Postgres와 PayPal REST 앱 키, webhook id, Pro subscription plan id가 모두 없으면 서버가 기동에 실패합니다.
PayPal webhook URL은 `https://warp.ponslink.com/api/billing/webhook`입니다. `BILLING.SUBSCRIPTION.ACTIVATED`, `BILLING.SUBSCRIPTION.CANCELLED`, `BILLING.SUBSCRIPTION.SUSPENDED`, `BILLING.SUBSCRIPTION.EXPIRED` 이벤트를 보내면 Pro entitlement 상태가 반영됩니다. 단건 Drop Pass는 PayPal 승인 복귀 후 `/api/billing/capture`에서 capture와 entitlement 발급을 완료합니다.

## 메시지 프로토콜

JSON 기반 메시지 프레이밍:

```json
{"type": "JoinRoom", "payload": {"room_id": "abc123"}}
{"type": "Offer", "payload": {"room_id": "abc123", "sdp": "...", "target": null}}
```

## 프론트엔드 통합

`ponswarp/src/services/signaling-adapter.ts` 어댑터를 통해 기존 Socket.io 기반 코드와 호환됩니다.
