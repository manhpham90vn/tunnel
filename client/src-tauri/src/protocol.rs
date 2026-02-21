//! # WebSocket Protocol Messages (Client)
//!
//! Defines all message types exchanged between the client and the relay server.
//! This enum **must stay in sync** with the server's `WsMessage` enum in
//! `server/src/protocol.rs` — any changes to one must be mirrored in the other.

use serde::{Deserialize, Serialize};

/// All possible WebSocket messages in the tunnel protocol.
///
/// Uses serde's internally-tagged representation: each message is serialized
/// as a JSON object with a `"type"` field (e.g., `{"type": "register", ...}`).
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    // ── Registration ──────────────────────────────────────────────
    /// Request registration as an agent on the relay server.
    /// The server will generate and assign a unique Agent ID.
    Register,

    /// Server confirms successful registration and assigns an Agent ID.
    RegisterOk { agent_id: String },

    // ── Tunnel Lifecycle ──────────────────────────────────────────
    /// Request a tunnel to a target agent (sent by controller).
    Connect {
        target_id: String,
        remote_host: String,
        remote_port: u16,
    },

    /// Incoming tunnel request from a controller (received by agent).
    TunnelRequest {
        session_id: String,
        remote_host: String,
        remote_port: u16,
    },

    /// Accept an incoming tunnel request (sent by agent).
    TunnelAccept { session_id: String },

    /// Tunnel is established and ready for data relay (received by controller).
    TunnelReady { session_id: String },

    /// Tear down a tunnel session (sent/received by either side).
    TunnelClose { session_id: String },

    // ── Stream Multiplexing ───────────────────────────────────────
    /// Open a new TCP stream within an existing tunnel session.
    StreamOpen {
        session_id: String,
        stream_id: String,
    },

    /// Close a specific stream within a tunnel session.
    StreamClose {
        session_id: String,
        stream_id: String,
    },

    // ── Data Relay ────────────────────────────────────────────────
    /// Carry base64-encoded TCP data through the tunnel.
    /// `role` indicates who sent the data ("agent" or "controller").
    Data {
        session_id: String,
        stream_id: String,
        role: String,
        payload: String,
    },

    // ── Heartbeat ─────────────────────────────────────────────────
    /// Heartbeat request.
    Ping,

    /// Heartbeat response.
    Pong,

    // ── Error ─────────────────────────────────────────────────────
    /// Error notification from the server.
    Error { message: String },
}
