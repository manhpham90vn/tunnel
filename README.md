# 🚇 Tunnel — Remote Access like TeamViewer

A tunnel application that enables remote access between computers through a central relay server. The client acts as an **Agent** — it automatically connects to the server and is ready to receive tunnel requests from anyone who knows its Agent ID.

## Architecture Overview

```
┌─────────────────┐         ┌─────────────────┐         ┌─────────────────┐
│   Agent (PC A)  │◄───────►│   Relay Server   │◄───────►│ Controller (B)  │
│   Tauri App     │   WS    │   Rust / Axum    │   WS    │   Tauri App     │
└─────────────────┘         └─────────────────┘         └─────────────────┘
        │                           │                           │
   TCP Listener              Agent Registry              Enter Agent ID
   (local ports)            Session Manager              → create tunnel
```

### Components

| Component | Technology | Role |
|-----------|------------|------|
| **Server** | Rust (Axum + Tokio) | Relay server — manages agents, forwards data |
| **Client** | Rust (Tauri v2) + React | Acts as both Agent (receives connections) and Controller (connects to other agents) |

## How It Works

### 1. Agent Registration

```
Agent                         Server
  │                              │
  │── WebSocket Connect ────────►│
  │── Register {agent_id} ─────►│  ← Store in Agent Registry
  │◄─ RegisterOk ───────────────│
  │                              │
  │◄─── Ping ───────────────────│  ← Heartbeat every 30s
  │──── Pong ──────────────────►│
```

When the client (Tauri app) starts:
1. **Generate Agent ID** — A short 8-character UUID (e.g., `A3F8-B2C1`), stored persistently
2. **Connect via WebSocket** to the server (`ws://server:7070/ws`)
3. **Send Register** — the server stores the agent in its registry
4. **Heartbeat** — ping/pong every 30s; 3 missed pings → disconnect → auto-reconnect

### 2. Tunnel Establishment

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

When a Controller wants to access an Agent:
1. **Enter the Agent ID** of the target machine + configure ports (e.g., forward agent's port 22)
2. **Send Connect** to the server; the server looks up the agent in the registry
3. **Server notifies** the agent of the tunnel request
4. **Agent accepts** → server creates a session and begins relaying
5. **TCP data** is encapsulated and forwarded through WebSocket frames

### 3. Data Relay

```
┌──────────┐     ┌──────────┐     ┌──────────┐     ┌──────────┐
│ Local App │     │Controller│     │  Server  │     │  Agent   │     ┌──────────┐
│ (browser) │     │          │     │  (relay) │     │          │     │ Local    │
│           │     │          │     │          │     │          │     │ Service  │
│  :8080 ◄──┼─TCP─┤  encode  ├─WS──┤ forward  ├─WS──┤  decode  ├─TCP─┤ :3000   │
│           │     │  base64  │     │  binary  │     │  base64  │     │          │
└──────────┘     └──────────┘     └──────────┘     └──────────┘     └──────────┘
```

- The Controller opens a TCP listener on `local_port` (e.g., `:8080`)
- When a connection arrives on `:8080`, data is base64-encoded → sent via WebSocket
- The Server forwards it to the Agent based on the `session_id`
- The Agent decodes → sends to `remote_host:remote_port` (e.g., `localhost:3000`)
- Responses travel back along the same path

## Protocol (WebSocket Messages)

All messages are JSON, transmitted via WebSocket text frames.

### Control Messages

```jsonc
// Agent → Server: Register
{"type": "register", "agent_id": "A3F8-B2C1"}

// Server → Agent: Registration confirmed
{"type": "register_ok"}

// Controller → Server: Request tunnel
{"type": "connect", "target_id": "A3F8-B2C1", "remote_host": "127.0.0.1", "remote_port": 3000}

// Server → Agent: Tunnel request notification
{"type": "tunnel_request", "session_id": "sess-uuid", "remote_host": "127.0.0.1", "remote_port": 3000}

// Agent → Server: Accept tunnel
{"type": "tunnel_accept", "session_id": "sess-uuid"}

// Server → Controller: Tunnel ready
{"type": "tunnel_ready", "session_id": "sess-uuid"}

// Any → Any: Close tunnel
{"type": "tunnel_close", "session_id": "sess-uuid"}
```

### Data Messages

```jsonc
// TCP data transmitted through the tunnel
{"type": "data", "session_id": "sess-uuid", "stream_id": "stream-uuid", "role": "controller", "payload": "<base64-encoded-bytes>"}
```

### Stream Multiplexing

```jsonc
// Open a new stream (one per TCP connection)
{"type": "stream_open", "session_id": "sess-uuid", "stream_id": "stream-uuid"}

// Close a stream
{"type": "stream_close", "session_id": "sess-uuid", "stream_id": "stream-uuid"}
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

## Project Structure

```
tunnel/
├── server/                    # Relay Server
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs            # Entry point — router setup, server start
│       ├── protocol.rs        # WebSocket message types
│       ├── state.rs           # Shared state (agents, sessions)
│       ├── handlers.rs        # WebSocket handlers + message dispatch
│       └── api.rs             # REST API endpoints
│
├── client/                    # Tauri App (Agent + Controller)
│   ├── package.json
│   ├── index.html
│   ├── src/                   # React Frontend
│   │   ├── main.tsx           # React entry point
│   │   ├── App.tsx            # Dashboard UI
│   │   └── App.css            # Dark theme styles
│   └── src-tauri/             # Rust Backend
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs        # Tauri binary entry point
│           ├── lib.rs         # App setup + module declarations
│           ├── protocol.rs    # WebSocket message types
│           ├── state.rs       # Agent state + data types
│           ├── commands.rs    # Tauri IPC commands
│           ├── agent.rs       # WebSocket connection loop
│           └── relay.rs       # TCP ↔ WebSocket relay
│
├── .github/workflows/
│   └── release.yml            # CI/CD: build + release pipeline
│
└── README.md
```

## Use Cases

### SSH through a tunnel

```
Controller                              Agent (remote machine)
┌──────────┐                           ┌──────────┐
│ ssh -p   │                           │ sshd     │
│ 2222     │  ← tunnel via server →    │ :22      │
│ localhost│                           │          │
└──────────┘                           └──────────┘

# On Controller: forward local port 2222 → agent port 22
# Then run: ssh -p 2222 user@localhost
```

### Web app through a tunnel

```
Controller                              Agent (remote machine)
┌──────────┐                           ┌──────────┐
│ Browser  │                           │ Web App  │
│ :8080    │  ← tunnel via server →    │ :3000    │
│ localhost│                           │          │
└──────────┘                           └──────────┘

# On Controller: forward local port 8080 → agent port 3000
# Then open browser: http://localhost:8080
```

## Tech Stack

- **Server**: Rust, Axum, Tokio, WebSocket (tokio-tungstenite)
- **Client Backend**: Rust, Tauri v2, Tokio
- **Client Frontend**: React, TypeScript, Vite
- **Protocol**: WebSocket + JSON control messages + base64 data payload

## Development

```bash
# 1. Start the relay server
cd server && cargo run
# Server will listen on 0.0.0.0:7070

# 2. Start the client (dev mode)
cd client && npm run tauri dev
# The app will open and automatically connect to the server
```

## Release

The project uses GitHub Actions for CI/CD. Pushing a tag matching `v*` triggers a multi-platform build:

- **macOS**: Universal binary (aarch64 + x86_64) → `.dmg`
- **Linux**: `.deb` + `.AppImage`
- **Windows**: `.exe` (NSIS installer)
- **Server**: Linux binary

All artifacts are uploaded to a GitHub Release automatically.
