# 🚇 Tunnel — Remote Access like TeamViewer

Phần mềm tunnel cho phép truy cập từ xa giữa các máy tính thông qua một relay server trung gian. Client đóng vai trò **Agent** — tự động kết nối tới server, sẵn sàng nhận tunnel request từ bất kỳ ai biết Agent ID.

## Kiến trúc tổng quan

```
┌─────────────────┐         ┌─────────────────┐         ┌─────────────────┐
│   Agent (máy A) │◄───────►│   Relay Server   │◄───────►│ Controller (B)  │
│   Tauri App     │   WS    │   Rust / Axum    │   WS    │   Tauri App     │
└─────────────────┘         └─────────────────┘         └─────────────────┘
        │                           │                           │
   TCP Listener              Agent Registry              Nhập Agent ID
   (local ports)            Session Manager              → tạo tunnel
```

### Thành phần

| Thành phần | Công nghệ | Vai trò |
|-----------|-----------|---------|
| **Server** | Rust (Axum + Tokio) | Relay server — quản lý agents, chuyển tiếp dữ liệu |
| **Client** | Rust (Tauri v2) + React | Vừa là Agent (nhận kết nối) vừa là Controller (kết nối tới agent khác) |

## Cách hoạt động

### 1. Agent Registration (Đăng ký Agent)

```
Agent                         Server
  │                              │
  │── WebSocket Connect ────────►│
  │── Register {agent_id} ─────►│  ← Lưu vào Agent Registry
  │◄─ RegisterOk ───────────────│
  │                              │
  │◄─── Ping ───────────────────│  ← Heartbeat mỗi 30s
  │──── Pong ──────────────────►│
```

Khi Client (Tauri app) khởi động:
1. **Tạo Agent ID** — UUID ngắn 8 ký tự (ví dụ: `A3F8-B2C1`), lưu persistent
2. **Kết nối WebSocket** tới server (`ws://server:7070/ws`)
3. **Gửi Register** — server lưu agent vào registry
4. **Heartbeat** — ping/pong mỗi 30s, nếu miss 3 lần → disconnect → auto-reconnect

### 2. Tunnel Establishment (Thiết lập Tunnel)

```
Controller           Server              Agent
    │                   │                   │
    │── Connect ──────►│                   │
    │  {target_id}     │── TunnelRequest ─►│
    │                   │◄─ TunnelAccept ──│
    │◄─ TunnelReady ───│                   │
    │                   │                   │
    │══ Data ═════════►│══ Data ══════════►│ ── TCP ──► localhost:22
    │◄═ Data ══════════│◄═ Data ═══════════│ ◄─ TCP ─── localhost:22
```

Khi Controller muốn truy cập Agent:
1. **Nhập Agent ID** của máy đích + cấu hình port (ví dụ: forward port 22 của agent)
2. **Gửi Connect** tới server, server tìm agent trong registry
3. **Server thông báo** agent có tunnel request
4. **Agent chấp nhận** → server tạo session, bắt đầu relay
5. **Dữ liệu TCP** được đóng gói và chuyển tiếp qua WebSocket frames

### 3. Data Relay (Chuyển tiếp dữ liệu)

```
┌──────────┐     ┌──────────┐     ┌──────────┐     ┌──────────┐
│ App local │     │Controller│     │  Server  │     │  Agent   │     ┌──────────┐
│ (browser) │     │          │     │  (relay) │     │          │     │ Service  │
│           │     │          │     │          │     │          │     │ local    │
│  :8080 ◄──┼─TCP─┤  encode  ├─WS──┤ forward  ├─WS──┤  decode  ├─TCP─┤ :3000   │
│           │     │  base64  │     │  binary  │     │  base64  │     │          │
└──────────┘     └──────────┘     └──────────┘     └──────────┘     └──────────┘
```

- Controller mở TCP listener trên `local_port` (ví dụ `:8080`)
- Khi có connection tới `:8080`, dữ liệu được encode base64 → gửi qua WebSocket
- Server chuyển tiếp tới Agent dựa trên `session_id`
- Agent decode → gửi tới `remote_host:remote_port` (ví dụ `localhost:3000`)
- Response đi ngược lại theo cùng đường

## Protocol (WebSocket Messages)

Tất cả messages đều là JSON, truyền qua WebSocket text frames.

### Control Messages

```jsonc
// Agent → Server: Đăng ký
{"type": "register", "agent_id": "A3F8-B2C1"}

// Server → Agent: Xác nhận đăng ký
{"type": "register_ok"}

// Controller → Server: Yêu cầu tunnel
{"type": "connect", "target_id": "A3F8-B2C1", "remote_host": "127.0.0.1", "remote_port": 3000}

// Server → Agent: Thông báo tunnel request
{"type": "tunnel_request", "session_id": "sess-uuid", "remote_host": "127.0.0.1", "remote_port": 3000}

// Agent → Server: Chấp nhận tunnel
{"type": "tunnel_accept", "session_id": "sess-uuid"}

// Server → Controller: Tunnel sẵn sàng
{"type": "tunnel_ready", "session_id": "sess-uuid"}

// Any → Any: Ngắt tunnel
{"type": "tunnel_close", "session_id": "sess-uuid"}
```

### Data Messages

```jsonc
// Truyền dữ liệu TCP qua tunnel
{"type": "data", "session_id": "sess-uuid", "payload": "<base64-encoded-bytes>"}
```

### Heartbeat

```jsonc
{"type": "ping"}
{"type": "pong"}
```

### Error

```jsonc
{"type": "error", "message": "Agent not found"}
```

## Cấu trúc thư mục

```
tunnel/
├── server/                    # Relay Server
│   ├── Cargo.toml
│   └── src/
│       └── main.rs            # Axum WebSocket server + relay logic
│
├── client/                    # Tauri App (Agent + Controller)
│   ├── package.json
│   ├── index.html
│   ├── src/                   # React Frontend
│   │   ├── main.tsx
│   │   ├── App.tsx            # Dashboard UI
│   │   └── App.css            # Dark theme styles
│   └── src-tauri/             # Rust Backend
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs        # Tauri entry point
│           └── lib.rs         # Agent logic + Tauri commands
│
└── README.md                  # ← Bạn đang đọc file này
```

## Use Cases

### SSH qua tunnel
```
Controller                              Agent (máy remote)
┌──────────┐                           ┌──────────┐
│ ssh -p   │                           │ sshd     │
│ 2222     │  ← tunnel qua server →   │ :22      │
│ localhost│                           │          │
└──────────┘                           └──────────┘

# Trên Controller: forward local port 2222 → agent port 22
# Sau đó: ssh -p 2222 user@localhost
```

### Web app qua tunnel
```
Controller                              Agent (máy remote)
┌──────────┐                           ┌──────────┐
│ Browser  │                           │ Web App  │
│ :8080    │  ← tunnel qua server →   │ :3000    │
│ localhost│                           │          │
└──────────┘                           └──────────┘

# Trên Controller: forward local port 8080 → agent port 3000
# Sau đó mở browser: http://localhost:8080
```

## Tech Stack

- **Server**: Rust, Axum, Tokio, WebSocket (tokio-tungstenite)
- **Client Backend**: Rust, Tauri v2, Tokio
- **Client Frontend**: React, TypeScript, Vite
- **Protocol**: WebSocket + JSON control messages + base64 data payload

## Chạy development

```bash
# 1. Start server
cd server && cargo run
# Server sẽ listen trên 0.0.0.0:7070

# 2. Start client (dev mode)  
cd client && npm run tauri dev
# App sẽ mở ra, tự động kết nối tới server
```
