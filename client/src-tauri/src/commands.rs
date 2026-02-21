//! # Tauri Commands
//!
//! Defines the commands exposed to the React frontend via Tauri's IPC bridge.
//! Each `#[tauri::command]` function can be called from JavaScript using
//! `invoke("command_name", { args })`.

use crate::protocol::WsMessage;
use crate::state::{AgentState, AgentStatus, PendingConnect, TunnelInfo};
use std::sync::Arc;
use tauri::Emitter;
use tracing::info;
use uuid::Uuid;

/// Returns the current agent status (ID, connection state, server URL).
///
/// Called by the frontend on startup to display the agent's identity
/// and connection status.
#[tauri::command]
pub async fn get_agent_info(
    state: tauri::State<'_, Arc<AgentState>>,
) -> Result<AgentStatus, String> {
    let connected = *state.connected.read().await;
    let server_url = state.server_url.read().await.clone();
    let agent_id = state.agent_id.read().await.clone();
    Ok(AgentStatus {
        agent_id,
        connected,
        server_url,
    })
}

/// Updates the relay server URL.
///
/// The new URL takes effect on the next connection attempt.
/// If the agent is currently connected, it will use the new URL
/// after disconnecting and reconnecting.
#[tauri::command]
pub async fn set_server_url(
    url: String,
    state: tauri::State<'_, Arc<AgentState>>,
) -> Result<(), String> {
    info!("Server URL updated to: {}", url);
    *state.server_url.write().await = url;
    Ok(())
}

/// Initiates a tunnel connection to a remote agent.
///
/// ## Parameters
/// - `target_id`: The agent ID to connect to (e.g., "A3F8-B2C1")
/// - `remote_host`: The host on the agent's side to forward to
/// - `remote_port`: The port on the agent's side (e.g., 22 for SSH)
/// - `local_port`: The local port to listen on (e.g., 2222)
///
/// ## Flow
/// 1. Stores the pending connection parameters
/// 2. Sends a `Connect` message to the server via WebSocket
/// 3. Adds a "connecting" tunnel entry to the UI
/// 4. Returns a temporary session ID (updated when the tunnel is ready)
#[tauri::command]
pub async fn connect_to_agent(
    target_id: String,
    remote_host: String,
    remote_port: u16,
    local_port: u16,
    state: tauri::State<'_, Arc<AgentState>>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    // Get the WebSocket sender (fails if not connected)
    let ws_tx = state.ws_tx.read().await;
    let tx = ws_tx.as_ref().ok_or("Not connected to server")?.clone();

    // Store the pending connection info so we can use it when
    // the server responds with TunnelReady
    {
        let mut pending = state.pending_connects.write().await;
        pending.insert(
            target_id.clone(),
            PendingConnect {
                local_port,
                remote_host: remote_host.clone(),
                remote_port,
            },
        );
    }

    // Send the connect request to the relay server
    tx.send(WsMessage::Connect {
        target_id: target_id.clone(),
        remote_host: remote_host.clone(),
        remote_port,
    })
    .map_err(|e| format!("Failed to send: {}", e))?;

    // Add a placeholder tunnel entry for the UI with "connecting" status.
    // The session_id will be updated when we receive TunnelReady.
    let mut tunnels = state.tunnels.write().await;
    let session_id = format!("pending-{}", &Uuid::new_v4().to_string()[..8]);
    tunnels.push(TunnelInfo {
        session_id: session_id.clone(),
        remote_host,
        remote_port,
        local_port,
        direction: "outgoing".to_string(),
        status: "connecting".to_string(),
    });

    // Notify the frontend to refresh the tunnel list
    let _ = app_handle.emit("tunnels-updated", ());

    info!(
        "Connect request → agent {} (local={})",
        target_id, local_port
    );
    Ok(session_id)
}

/// Disconnects an active tunnel by session ID.
///
/// Sends a `TunnelClose` message to the server and removes the
/// tunnel from the local UI list.
#[tauri::command]
pub async fn disconnect_tunnel(
    session_id: String,
    state: tauri::State<'_, Arc<AgentState>>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // Send close message to the server
    let ws_tx = state.ws_tx.read().await;
    if let Some(tx) = ws_tx.as_ref() {
        let _ = tx.send(WsMessage::TunnelClose {
            session_id: session_id.clone(),
        });
    }

    // Remove from local tunnel list
    let mut tunnels = state.tunnels.write().await;
    tunnels.retain(|t| t.session_id != session_id);

    // Notify the frontend
    let _ = app_handle.emit("tunnels-updated", ());
    Ok(())
}

/// Returns the list of all active tunnels.
///
/// Called by the frontend whenever it receives a "tunnels-updated" event.
#[tauri::command]
pub async fn get_tunnels(
    state: tauri::State<'_, Arc<AgentState>>,
) -> Result<Vec<TunnelInfo>, String> {
    Ok(state.tunnels.read().await.clone())
}
