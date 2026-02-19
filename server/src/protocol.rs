//! # WebSocket Protocol Messages
//!
//! Defines all message types exchanged between clients and the relay server
//! over WebSocket connections. Messages are serialized as JSON text frames
//! using serde's internally-tagged representation (`"type": "..."` field).

use serde::{Deserialize, Serialize};

/// All possible WebSocket messages in the tunnel protocol.
///
/// The `#[serde(tag = "type")]` attribute means each variant is serialized
/// as a JSON object with a `"type"` field whose value is the snake_case
/// variant name. For example, `WsMessage::RegisterOk` serializes to
/// `{"type": "register_ok"}`.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    // ── Registration ──────────────────────────────────────────────

    /// Sent by a client to request registration as an agent.
    /// The server will generate a unique Agent ID and respond
    /// with `RegisterOk { agent_id }`.
    Register,

    /// Server's acknowledgment that the agent was successfully registered.
    /// Contains the server-assigned unique Agent ID.
    RegisterOk { agent_id: String },

    // ── Tunnel Lifecycle ──────────────────────────────────────────

    /// Sent by a controller to request a tunnel to a specific agent.
    /// The server looks up `target_id` in the agent registry and, if found,
    /// forwards a `TunnelRequest` to that agent.
    Connect {
        target_id: String,
        remote_host: String,
        remote_port: u16,
    },

    /// Forwarded by the server to the target agent when a controller
    /// wants to establish a tunnel. Contains the session ID assigned
    /// by the server and the target address the agent should connect to.
    TunnelRequest {
        session_id: String,
        remote_host: String,
        remote_port: u16,
    },

    /// Sent by the agent to accept an incoming tunnel request.
    /// The server will then notify the controller with `TunnelReady`.
    TunnelAccept { session_id: String },

    /// Sent by the server to the controller to confirm the tunnel
    /// is established and data relay can begin.
    TunnelReady { session_id: String },

    /// Sent by either side (or the server) to tear down the tunnel.
    /// Triggers cleanup of all associated streams and resources.
    TunnelClose { session_id: String },

    // ── Stream Multiplexing ───────────────────────────────────────
    // A single tunnel session can carry multiple independent TCP
    // connections (streams). Each stream has its own `stream_id`.

    /// Opens a new stream within an existing tunnel session.
    /// The controller sends this when a new TCP connection arrives
    /// on the local listener; the agent then opens a TCP connection
    /// to the target service.
    StreamOpen {
        session_id: String,
        stream_id: String,
    },

    /// Closes a specific stream within a tunnel session.
    /// Triggers cleanup of the per-stream data channel.
    StreamClose {
        session_id: String,
        stream_id: String,
    },

    // ── Data Relay ────────────────────────────────────────────────

    /// Carries TCP data through the tunnel. The payload is base64-encoded
    /// bytes. The `role` field ("agent" or "controller") indicates who
    /// sent the data, allowing the server to route it to the other side.
    Data {
        session_id: String,
        stream_id: String,
        role: String,
        payload: String,
    },

    // ── Heartbeat ─────────────────────────────────────────────────

    /// Heartbeat request, sent periodically to keep the connection alive.
    Ping,

    /// Heartbeat response.
    Pong,

    // ── Error ─────────────────────────────────────────────────────

    /// Error notification with a human-readable message.
    Error { message: String },
}
