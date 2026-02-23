//! # Server State
//!
//! Holds the shared application state for the relay server, including:
//! - **Agent registry**: maps agent IDs to their message senders
//! - **Connection registry**: maps connection IDs to their message senders
//! - **Session registry**: maps session IDs to tunnel session metadata
//!
//! All registries use [`DashMap`] for lock-free concurrent access,
//! since multiple QUIC connections are handled concurrently.

use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tunnel_protocol::ControlMessage;
use uuid::Uuid;

/// Type alias for the unbounded sender used to push messages to a client's
/// outbound QUIC control stream. Each connected client gets one of these.
pub type ClientTx = mpsc::UnboundedSender<ControlMessage>;

/// Generates a short, human-readable agent ID from a UUID.
///
/// Format: "XXXX-XXXX" (8 uppercase hex characters split by a hyphen).
/// Example: "A3F8-B2C1"
pub fn generate_agent_id() -> String {
    let uuid = Uuid::new_v4().to_string();
    let short = &uuid[..8];
    format!(
        "{}-{}",
        short[..4].to_uppercase(),
        short[4..8].to_uppercase()
    )
}

/// Information stored for each registered agent.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    /// Channel to send messages to this agent's QUIC connection.
    pub tx: ClientTx,
    pub conn_id: String,
}

#[derive(Clone)]
pub struct ConnectionInfo {
    pub tx: ClientTx,
    pub conn: quinn::Connection,
}

/// Metadata for an active tunnel session between a controller and an agent.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TunnelSession {
    /// Unique identifier for this tunnel session (short UUID).
    pub session_id: String,

    /// The agent ID that this tunnel connects to.
    pub agent_id: String,

    /// The connection ID of the controller that initiated this tunnel.
    pub controller_id: String,

    /// The remote host the agent should connect to (e.g., "127.0.0.1").
    pub remote_host: String,

    /// The remote port on the agent side (e.g., 22 for SSH).
    pub remote_port: u16,
}

/// Shared application state, cloned and passed to each request handler.
///
/// Uses `Arc<DashMap<...>>` for thread-safe, lock-free concurrent access
/// across all QUIC handler tasks.
#[derive(Clone)]
pub struct AppState {
    /// Registry of currently connected agents, keyed by agent ID.
    pub agents: Arc<DashMap<String, AgentInfo>>,

    /// Registry of all active QUIC connections, keyed by connection ID.
    /// This includes both agents and controllers.
    pub connections: Arc<DashMap<String, ConnectionInfo>>,

    /// Registry of active tunnel sessions, keyed by session ID.
    pub sessions: Arc<DashMap<String, TunnelSession>>,
}

impl AppState {
    /// Creates a new empty application state with all registries initialized.
    pub fn new() -> Self {
        Self {
            agents: Arc::new(DashMap::new()),
            connections: Arc::new(DashMap::new()),
            sessions: Arc::new(DashMap::new()),
        }
    }
}
