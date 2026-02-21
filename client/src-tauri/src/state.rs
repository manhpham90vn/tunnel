//! # Agent State
//!
//! Contains all state types for the tunnel client application:
//! - [`AgentState`] — the central state object shared across all Tauri commands
//!   and background tasks
//! - [`TunnelInfo`] — UI-facing tunnel information
//! - [`AgentStatus`] — agent connection status for the frontend
//! - [`PendingConnect`] — temporary storage for outgoing tunnel parameters
//! - [`AgentTunnelInfo`] — agent-side tunnel target address

use serde::Serialize;
use std::collections::HashMap;
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use tracing::info;

use crate::protocol::WsMessage;

// ─── Data Types ─────────────────────────────────────────────────

/// Information about a single tunnel, displayed in the frontend UI.
#[derive(Debug, Clone, Serialize)]
pub struct TunnelInfo {
    /// Unique session identifier.
    pub session_id: String,

    /// The remote host being tunneled to (e.g., "127.0.0.1").
    pub remote_host: String,

    /// The remote port being tunneled to (e.g., 22).
    pub remote_port: u16,

    /// The local port being listened on (controller side only).
    pub local_port: u16,

    /// Direction: "incoming" (agent receiving) or "outgoing" (controller initiating).
    pub direction: String,

    /// Current status: "connecting", "active", or "error".
    pub status: String,
}

/// Agent connection status, returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct AgentStatus {
    /// This agent's unique ID (e.g., "A3F8-B2C1").
    pub agent_id: String,

    /// Whether the agent is currently connected to the relay server.
    pub connected: bool,

    /// The relay server URL this agent connects to.
    pub server_url: String,
}

/// Temporary storage for a pending outgoing tunnel connection.
/// Stored while waiting for the server to confirm the tunnel is ready.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PendingConnect {
    /// The local port to listen on once the tunnel is established.
    pub local_port: u16,

    /// The remote host the agent should connect to.
    pub remote_host: String,

    /// The remote port the agent should connect to.
    pub remote_port: u16,
}

/// Agent-side information about an active tunnel's target address.
/// Used when the agent needs to open TCP connections to the target
/// service in response to `StreamOpen` messages.
#[derive(Debug, Clone)]
pub struct AgentTunnelInfo {
    /// Target host (e.g., "127.0.0.1").
    pub remote_host: String,

    /// Target port (e.g., 3000).
    pub remote_port: u16,
}

/// Default relay server URL. Used when no custom URL is set.
pub const DEFAULT_SERVER_URL: &str = "ws://127.0.0.1:7070/ws";

// ─── Central Agent State ────────────────────────────────────────

/// The main application state, shared across all Tauri commands
/// and background tasks via `Arc<AgentState>`.
///
/// All mutable fields are protected by `RwLock` for safe concurrent access.
pub struct AgentState {
    /// This agent's unique identifier, assigned by the server on registration.
    /// Empty string until the server responds with RegisterOk.
    pub agent_id: RwLock<String>,

    /// The relay server WebSocket URL (e.g., "ws://1.2.3.4:7070/ws").
    /// Can be changed at runtime from the UI.
    pub server_url: RwLock<String>,

    /// Whether we're currently connected to the relay server.
    pub connected: RwLock<bool>,

    /// Channel to send outbound WebSocket messages to the server.
    /// `None` when not connected.
    pub ws_tx: RwLock<Option<mpsc::UnboundedSender<WsMessage>>>,

    /// List of active tunnels (displayed in the UI).
    pub tunnels: RwLock<Vec<TunnelInfo>>,

    /// Pending outgoing tunnel connections, keyed by target agent ID.
    /// Removed once the tunnel is established.
    pub pending_connects: RwLock<HashMap<String, PendingConnect>>,

    /// Per-stream data channels for TCP ↔ WebSocket relay.
    /// Key format: "{role}-{stream_id}" (e.g., "controller-abc12345").
    /// The sender pushes decoded TCP data to the corresponding relay task.
    pub data_channels: RwLock<HashMap<String, mpsc::UnboundedSender<Vec<u8>>>>,

    /// Agent-side tunnel metadata: session_id → target address.
    /// Used to know where to connect when a StreamOpen arrives.
    pub agent_tunnels: RwLock<HashMap<String, AgentTunnelInfo>>,

    /// Spawned async task handles, grouped by session_id.
    /// Used for cleanup: aborting TCP listeners and relay tasks
    /// when a tunnel is closed.
    pub task_handles: RwLock<HashMap<String, Vec<JoinHandle<()>>>>,
}

impl AgentState {
    /// Creates a new `AgentState` with a freshly generated agent ID
    /// and all registries initialized to empty.
    pub fn new() -> Self {
        Self {
            agent_id: RwLock::new(String::new()),
            server_url: RwLock::new(DEFAULT_SERVER_URL.to_string()),
            connected: RwLock::new(false),
            ws_tx: RwLock::new(None),
            tunnels: RwLock::new(Vec::new()),
            pending_connects: RwLock::new(HashMap::new()),
            data_channels: RwLock::new(HashMap::new()),
            agent_tunnels: RwLock::new(HashMap::new()),
            task_handles: RwLock::new(HashMap::new()),
        }
    }

    /// Aborts all spawned async tasks associated with a specific session.
    /// Called when a tunnel is closed to clean up TCP listeners and relays.
    pub async fn abort_session_tasks(&self, session_id: &str) {
        let mut handles = self.task_handles.write().await;
        if let Some(tasks) = handles.remove(session_id) {
            for handle in tasks {
                handle.abort();
            }
            info!("Aborted tasks for session {}", session_id);
        }
    }

    /// Aborts ALL spawned async tasks across all sessions.
    /// Called on WebSocket disconnect to ensure a clean slate
    /// before reconnecting.
    pub async fn abort_all_tasks(&self) {
        let mut handles = self.task_handles.write().await;
        for (sid, tasks) in handles.drain() {
            for handle in tasks {
                handle.abort();
            }
            info!("Aborted tasks for session {}", sid);
        }
    }
}
