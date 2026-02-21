# 🚀 Migration: WebSocket → QUIC

## Why QUIC?

| Current Issue (WebSocket)                                  | QUIC Solution                     |
| ---------------------------------------------------------- | --------------------------------- |
| Base64 encoded data → +33% bandwidth                       | Native binary, no encoding needed |
| JSON serialize/deserialize every data frame                | Binary framing, zero-copy         |
| TCP head-of-line blocking (1 blocked stream = all blocked) | Independent QUIC streams          |
| Slow TLS handshake (TCP + TLS = 2-3 RTT)                   | 0-RTT or 1-RTT connection         |
| Manual stream multiplexing implementation                  | Built-in QUIC multiplexing        |
| Reconnect takes 3s + re-register                           | 0-RTT reconnect                   |

## Crates

- **`quinn`** — QUIC implementation for Rust (mature, production-ready)
- **`rustls`** — TLS for QUIC (required, QUIC mandates TLS 1.3)
- **`rcgen`** — Self-signed certificate generation (dev/testing)

---

## Phase 1: Preparation

- [ ] Study `quinn` API (connection, streams, bi-directional streams)
- [ ] Add dependencies to `server/Cargo.toml` (`quinn`, `rustls`, `rcgen`)
- [ ] Add dependencies to `client/src-tauri/Cargo.toml` (`quinn`, `rustls`, `rcgen`, `webpki`)
- [ ] Create TLS certificate generation module (self-signed for dev, option to load cert for prod)
- [ ] Remove unused dependencies (`tokio-tungstenite`, `base64`, `axum` ws feature)

---

## Phase 2: New Protocol Design

### Binary Protocol Format

Control messages (register, connect, tunnel lifecycle):

```
[1 byte: message_type][payload: bincode/msgpack serialized]
```

Data messages (TCP relay):

```
[1 byte: message_type = DATA][8 bytes: session_id][8 bytes: stream_id][raw payload bytes]
```

- [ ] Define new `protocol.rs` — use bincode instead of JSON
- [ ] Define message types:
  - `0x01` Register
  - `0x02` RegisterOk
  - `0x03` Connect
  - `0x04` TunnelRequest
  - `0x05` TunnelAccept
  - `0x06` TunnelReady
  - `0x07` TunnelClose
  - `0x08` StreamOpen
  - `0x09` StreamClose
  - `0x0A` Data (raw binary, no serialization)
  - `0x0B` Ping
  - `0x0C` Pong
  - `0x0D` Error
- [ ] Create shared protocol crate (`tunnel-protocol`) for both server and client

---

## Phase 3: Server — Migrate from Axum/WS to Quinn

### Files to modify/create:

- [ ] **`server/src/main.rs`** — Replace Axum HTTP server with Quinn QUIC endpoint
  - Create `quinn::Endpoint` instead of `tokio::net::TcpListener`
  - Bind UDP socket instead of TCP
  - Accept incoming QUIC connections
  - Keep HTTP API separate (Axum still runs alongside for `/api/agents`)

- [ ] **`server/src/handlers.rs`** — Rewrite connection handler
  - `handle_connection(quinn::Connection)` instead of `handle_connection(WebSocket)`
  - Use `connection.accept_bi()` for control stream (first stream)
  - When `Data` messages arrive → open new QUIC bi-directional stream per TCP stream
  - Relay directly between QUIC streams (no data serialization needed)

- [ ] **`server/src/state.rs`** — Update state types
  - `ClientTx` → replace with `quinn::Connection` handle
  - Add per-connection stream tracking

- [ ] **`server/src/protocol.rs`** — Binary protocol (see Phase 2)

---

## Phase 4: Client — Migrate from tokio-tungstenite to Quinn

### Files to modify/create:

- [ ] **`client/src-tauri/src/agent.rs`** — Rewrite connection loop
  - `quinn::Endpoint::connect()` instead of `connect_async()`
  - Use first bi-directional stream for control messages
  - Each TCP stream → open new QUIC bi-directional stream
  - 0-RTT reconnect when session ticket is available

- [ ] **`client/src-tauri/src/relay.rs`** — Simplify relay
  - `handle_stream_relay(tcp_stream, quic_send, quic_recv)`
  - Remove base64 encode/decode
  - Remove JSON wrapper for data
  - Direct `tokio::io::copy_bidirectional(&mut tcp, &mut quic_stream)`

- [ ] **`client/src-tauri/src/state.rs`** — Update state
  - `ws_tx` → `quinn::Connection`
  - Remove `data_channels` HashMap (QUIC streams are self-managing)

- [ ] **`client/src-tauri/src/protocol.rs`** — Use shared protocol crate

- [ ] **`client/src-tauri/src/commands.rs`** — Update commands
  - `set_server_url` → parse QUIC address instead of WS URL
  - Connect flow uses QUIC instead of WS

---

## Phase 5: Stream Multiplexing (Simplification)

Currently hand-rolled multiplexing over WebSocket. QUIC has native multiplexing:

- [ ] Each tunnel session = 1 QUIC connection (or 1 group of streams)
- [ ] Each TCP connection = 1 QUIC bi-directional stream
- [ ] Remove manual `stream_id` tracking
- [ ] Remove `data_channels` HashMap — each stream has its own send/recv
- [ ] `StreamOpen` / `StreamClose` → replaced by native QUIC stream open/close

---

## Phase 6: TLS & Certificates

- [ ] Server: Generate self-signed cert on startup (or load from file)
- [ ] Client: Option to skip certificate verification (dev mode)
- [ ] Client: Option to trust custom CA (prod mode)
- [ ] UI: Add setting for certificate path (optional)
- [ ] Update `tauri.conf.json` if needed

---

## Phase 7: UI & Config Updates

- [ ] Change server URL format: `ws://host:port/ws` → `host:port` (QUIC has no path)
- [ ] Update default server URL in `state.rs`
- [ ] Update frontend input validation
- [ ] Update connection status events

---

## Phase 8: Testing & Verification

- [ ] Test basic connection: client connects to server via QUIC
- [ ] Test agent registration
- [ ] Test tunnel creation (controller → agent)
- [ ] Test TCP relay through tunnel (SSH, HTTP)
- [ ] Test multiple concurrent streams
- [ ] Test reconnect behavior (kill connection, verify auto-reconnect)
- [ ] Test performance: compare latency/throughput with WebSocket version
- [ ] Test on real network (not localhost)

---

## Phase 9: Cleanup & Documentation

- [ ] Remove old WebSocket code
- [ ] Update `README.md` (architecture diagram, tech stack, port info)
- [ ] Update CI/CD if needed
- [ ] Update systemd service file (port stays 7070 but now UDP)
- [ ] Update firewall docs (UDP instead of TCP)

---

## Important Notes

> ⚠️ **QUIC runs over UDP** — ensure firewall/cloud security groups allow UDP port 7070

> ⚠️ **QUIC requires TLS 1.3** — must generate or provide a certificate

> 💡 **HTTP API can be kept** (`/api/agents`) running alongside on TCP for compatibility

> 💡 **Migration strategy**: Can run both WS and QUIC simultaneously during transition period
