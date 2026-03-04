# Architecture

## Overview

Tunnel is a remote access system between computers through a central relay server, similar to TeamViewer.

```
┌─────────────────┐          ┌─────────────────┐          ┌─────────────────┐
│   Agent (PC A)  │◄──QUIC──►│  Relay Server   │◄──QUIC──►│ Controller (B)  │
│   Tauri App     │          │  Rust / Axum    │          │  Tauri App      │
└────────┬────────┘          └─────────────────┘          └────────┬────────┘
         │                                                         │
    TCP to local                                              TCP listener
    services                                                  on local port
```

### Components

| Component       | Role                                                    |
| --------------- | ------------------------------------------------------ |
| **Agent**       | Machine to access remotely, runs Tauri app             |
| **Controller**  | Machine accessing Agent's services, also runs Tauri app|
| **Relay Server**| Central hub connecting Agents and Controllers           |

---

## Protocol

### Message Types

| Tag   | Message                                    | Direction           |
| ----- | ----------------------------------------- | ------------------ |
| 0x01  | `Register`                                | Client → Server    |
| 0x02  | `RegisterOk { agent_id }`                 | Server → Client    |
| 0x03  | `Connect { target_id, remote_host, remote_port }` | Controller → Server |
| 0x04  | `TunnelRequest { session_id, remote_host, remote_port }` | Server → Agent |
| 0x05  | `TunnelAccept { session_id }`            | Agent → Server     |
| 0x06  | `TunnelReady { session_id }`             | Server → Controller |
| 0x07  | `TunnelClose { session_id }`             | Any → Server       |
| 0x08  | `StreamOpen { session_id, stream_id }`  | Any → Server       |
| 0x09  | `StreamClose { session_id, stream_id }`  | Any → Server       |
| 0x0A  | `Data` (raw bytes)                       | Any → Server       |
| 0x0B  | `Ping`                                    | Client → Server    |
| 0x0C  | `Pong`                                    | Server → Client    |
| 0x0D  | `Error { message }`                      | Server → Client    |

### Serialization

- **Control messages**: `[1-byte tag][bincode payload]`
- **Data messages**: `[1-byte tag 0x0A][8-byte session_id][8-byte stream_id][payload]`

### QUIC Streams

- Each connection uses **1 control stream** (first stream, bidirectional) for control messages
- Additional **data streams** (bidirectional) are opened when relaying data
- 4-byte length-prefixed framing is used for the control stream

---

## Server (`server/`)

### Components

| File          | Description                                                        |
| --------------| ------------------------------------------------------------------ |
| `main.rs`     | Initialize Axum HTTP server (TCP 7070) + Quinn QUIC server (UDP 7070) |
| `state.rs`    | Shared state using `DashMap`: agents, connections, sessions        |
| `handlers.rs` | Handle QUIC connections: control stream, data streams, message routing |

### HTTP API

| Endpoint      | Method | Description                        |
| ------------- | ------ | ---------------------------------- |
| `/api/agents` | GET    | List connected agents (JSON array) |

### Connection Flow

1. Client connects QUIC → Server accepts
2. Server accepts first stream as **control stream**
3. Client sends `Register` → Server creates agent_id → sends `RegisterOk`
4. Controller sends `Connect{target_id, remote_port}` → Server looks up agent
5. Server sends `TunnelRequest` to Agent
6. Agent auto-accepts → sends `TunnelAccept`
7. Server sends `TunnelReady` to Controller
8. Controller opens TCP listener on local_port
9. User connects to localhost:local_port → Controller opens QUIC stream + sends `StreamOpen`
10. Agent receives `StreamOpen` → connects TCP to local service → relays data

### Auto-Reconnect

- Agent auto-reconnects every 3 seconds when disconnected
- Heartbeat ping every 30 seconds

---

## Client (`client/`)

### Backend (`src-tauri/`)

#### Tauri Commands

| Command             | Description                                              |
| ------------------- | -------------------------------------------------------- |
| `get_agent_info`   | Returns `{agent_id, connected, server_url}`             |
| `set_server_url`   | Update relay server address                             |
| `connect_to_agent` | Create tunnel: target_id, remote_host, remote_port, local_port |
| `disconnect_tunnel`| Close tunnel by session_id                              |
| `get_tunnels`      | List active tunnels                                     |

#### Dual-Role Operation

The client operates simultaneously in two roles:

**Agent Mode** (receiving tunnel requests):
- Registers with server, receives agent_id
- Auto-accepts all incoming tunnel requests
- Listens for `StreamOpen` → connects TCP to local service → relays data

**Controller Mode** (creating tunnels):
- Sends `Connect` with target agent ID
- Opens TCP listener on local_port
- Each incoming TCP connection → opens QUIC stream → sends `StreamOpen` → relays data

### Frontend (`src/`)

#### React Components

| Component           | Description                                    |
| ------------------- | --------------------------------------------- |
| **Header**         | App branding ("Tunnel Agent")                  |
| **Server Settings**| Configure relay server IP/port                 |
| **Your Agent**     | Display agent ID + copy button + status badge |
| **Connect to Agent**| Tunnel creation form (target ID, target port, local port) |
| **Active Tunnels** | Tunnel list + disconnect button               |

#### Events (Backend → Frontend)

| Event               | Payload    | Action                           |
| ------------------- | ---------- | -------------------------------- |
| `connection-status` | `boolean`  | Update status badge              |
| `registered`        | `string`   | Update displayed agent ID       |
| `tunnels-updated`   | —          | Refresh tunnel list              |
| `server-error`      | `string`   | Show error toast (5s)            |

---

## Tunnel Protocol Library (`tunnel-protocol/`)

Shared library between server and client, defining:

- All message structs (`Register`, `RegisterOk`, `Connect`, etc.)
- Message tag constants (0x01 - 0x0D)
- Serialization/deserialization with `bincode`

---

## Detailed Data Flow

```
1. Agent connects → QUIC handshake → Server assigns agent_id (format: XXXX-XXXX)
2. Controller connects → QUIC handshake → Server assigns agent_id
3. User enters target Agent ID + target port + local port
4. Controller sends: Connect{target_id, remote_host, remote_port}
5. Server looks up target agent in registry
6. Server sends: TunnelRequest{session_id, remote_host, remote_port} to Agent
7. Agent auto-accepts → sends: TunnelAccept{session_id}
8. Server sends: TunnelReady{session_id} to Controller
9. Controller starts TCP listener on local_port
10. User connects to localhost:local_port
11. Controller accepts TCP connection → opens QUIC data stream
12. Controller sends: StreamOpen{session_id, stream_id}
13. Agent receives → connects TCP to remote_host:remote_port
14. Both sides relay: TCP ↔ QUIC Stream ↔ TCP
15. On close: StreamClose → TunnelClose → cleanup
```
