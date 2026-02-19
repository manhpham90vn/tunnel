//! # WebSocket Handlers
//!
//! Contains the core WebSocket logic for the relay server:
//! - Upgrading HTTP connections to WebSocket
//! - Managing the lifecycle of each connection (inbound/outbound tasks, cleanup)
//! - Dispatching incoming messages to the appropriate handler
//! - Relaying data between tunnel endpoints

use crate::protocol::WsMessage;
use crate::state::{generate_agent_id, AgentInfo, AppState, TunnelSession};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};
use uuid::Uuid;

// ─── WebSocket Upgrade Endpoint ─────────────────────────────────

/// `GET /ws` — Upgrades the HTTP connection to a WebSocket connection.
///
/// This is the entry point for all WebSocket clients (both agents and
/// controllers). After the upgrade, the connection is handled by
/// [`handle_connection`].
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_connection(socket, state))
}

// ─── Connection Lifecycle ───────────────────────────────────────

/// Manages the full lifecycle of a single WebSocket connection.
///
/// ## Flow:
/// 1. Assign a unique connection ID
/// 2. Split the socket into a sink (outbound) and stream (inbound)
/// 3. Spawn an outbound task that serializes and sends queued messages
/// 4. Process incoming messages on the current task
/// 5. On disconnect: clean up connection, agent registry, and any sessions
async fn handle_connection(socket: WebSocket, state: AppState) {
    // Generate a unique ID for this connection (used internally for routing)
    let conn_id = Uuid::new_v4().to_string();
    info!("New connection: {}", conn_id);

    // Split the WebSocket into separate read/write halves
    let (ws_sink, mut ws_stream) = socket.split();

    // Create an unbounded channel for queueing outbound messages.
    // Any part of the server can send messages to this client via `tx`.
    let (tx, mut rx) = mpsc::unbounded_channel::<WsMessage>();

    // Register this connection in the global connection registry
    state.connections.insert(conn_id.clone(), tx.clone());

    // Track the agent ID if this connection registers as an agent.
    // Protected by a Mutex because it's shared between the inbound
    // processing loop and the cleanup phase.
    let agent_id: Arc<tokio::sync::Mutex<Option<String>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    // ── Outbound Task ──
    // Spawns a separate task that drains the message queue and sends
    // each message as a JSON text frame over the WebSocket.
    let ws_sink = Arc::new(tokio::sync::Mutex::new(ws_sink));
    let ws_sink_clone = ws_sink.clone();
    let outbound_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let text = match serde_json::to_string(&msg) {
                Ok(t) => t,
                Err(e) => {
                    error!("Serialize error: {}", e);
                    continue;
                }
            };
            let mut sink = ws_sink_clone.lock().await;
            if sink.send(Message::Text(text.into())).await.is_err() {
                break; // WebSocket closed; stop sending
            }
        }
    });

    // ── Inbound Loop ──
    // Processes incoming WebSocket frames. Only text frames containing
    // valid JSON messages are handled; binary frames and pings are ignored.
    while let Some(Ok(msg)) = ws_stream.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                    handle_message(&state, &conn_id, &tx, &agent_id, ws_msg).await;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // ── Cleanup on Disconnect ──
    info!("Disconnecting: {}", conn_id);

    // Stop the outbound sender task
    outbound_task.abort();

    // Remove this connection from the global registry
    state.connections.remove(&conn_id);

    // If this connection was a registered agent, clean up the agent
    // registry and remove any tunnel sessions associated with it.
    let aid = agent_id.lock().await;
    if let Some(ref aid) = *aid {
        info!("Agent {} disconnected", aid);
        state.agents.remove(aid);

        // Find and remove all sessions where this agent was involved,
        // either as the agent or as the controller (via conn_id).
        let sessions_to_remove: Vec<String> = state
            .sessions
            .iter()
            .filter(|s| s.agent_id == *aid || s.controller_id == conn_id)
            .map(|s| s.session_id.clone())
            .collect();

        for sid in sessions_to_remove {
            state.sessions.remove(&sid);
        }
    }
}

// ─── Message Relay Helper ───────────────────────────────────────

/// Routes a message to the "other side" of a tunnel based on who sent it.
///
/// - If `from_role` is `"agent"`, the message is forwarded to the controller.
/// - If `from_role` is `"controller"`, the message is forwarded to the agent.
///
/// This is the core relay function that enables transparent data forwarding
/// between the two endpoints of a tunnel session.
fn relay_message(state: &AppState, session: &TunnelSession, msg: WsMessage, from_role: &str) {
    match from_role {
        "agent" => {
            // Agent sent data → forward to the controller's connection
            if let Some(c) = state.connections.get(&session.controller_id) {
                let _ = c.send(msg);
            }
        }
        "controller" => {
            // Controller sent data → forward to the agent's connection
            if let Some(a) = state.agents.get(&session.agent_id) {
                let _ = a.tx.send(msg);
            }
        }
        _ => {}
    }
}

// ─── Message Dispatcher ─────────────────────────────────────────

/// Handles a single incoming WebSocket message from a client.
///
/// This is the central dispatch function that routes each message type
/// to the appropriate logic:
/// - **Register**: Adds the client to the agent registry
/// - **Connect**: Creates a tunnel session and forwards the request to the target agent
/// - **TunnelAccept**: Notifies the controller that the tunnel is ready
/// - **StreamOpen/StreamClose**: Relayed to the other side of the tunnel
/// - **Data**: Relayed to the other side based on the sender's role
/// - **TunnelClose**: Tears down the session and notifies both sides
/// - **Ping/Pong**: Heartbeat handling
async fn handle_message(
    state: &AppState,
    conn_id: &str,
    tx: &mpsc::UnboundedSender<WsMessage>,
    agent_id: &Arc<tokio::sync::Mutex<Option<String>>>,
    msg: WsMessage,
) {
    match msg {
        // ── Agent Registration ──
        WsMessage::Register => {
            // Generate a unique, human-readable agent ID on the server
            let aid = generate_agent_id();
            info!("Agent registered: {} (conn={})", aid, conn_id);
            // Store the agent in the registry with its message sender
            state.agents.insert(aid.clone(), AgentInfo { tx: tx.clone() });
            // Remember this connection's agent ID for cleanup on disconnect
            *agent_id.lock().await = Some(aid.clone());
            // Send the assigned ID back to the client
            let _ = tx.send(WsMessage::RegisterOk { agent_id: aid });
        }

        // ── Controller Requests a Tunnel ──
        WsMessage::Connect {
            target_id,
            remote_host,
            remote_port,
        } => {
            info!(
                "Connect request: {} → {} ({}:{})",
                conn_id, target_id, remote_host, remote_port
            );

            match state.agents.get(&target_id) {
                Some(agent_info) => {
                    // Generate a short session ID from a UUID
                    let session_id = Uuid::new_v4().to_string()[..8].to_string();

                    // Create and store the tunnel session metadata
                    state.sessions.insert(
                        session_id.clone(),
                        TunnelSession {
                            session_id: session_id.clone(),
                            agent_id: target_id.clone(),
                            controller_id: conn_id.to_string(),
                            remote_host: remote_host.clone(),
                            remote_port,
                        },
                    );

                    // Forward the tunnel request to the target agent
                    let _ = agent_info.tx.send(WsMessage::TunnelRequest {
                        session_id,
                        remote_host,
                        remote_port,
                    });
                }
                None => {
                    // Target agent not found; notify the controller
                    let _ = tx.send(WsMessage::Error {
                        message: format!("Agent '{}' not found", target_id),
                    });
                }
            }
        }

        // ── Agent Accepts the Tunnel ──
        WsMessage::TunnelAccept { session_id } => {
            info!("Tunnel accepted: {}", session_id);
            // Look up the session and notify the controller that the tunnel is ready
            if let Some(session) = state.sessions.get(&session_id) {
                if let Some(c) = state.connections.get(&session.controller_id) {
                    let _ = c.send(WsMessage::TunnelReady {
                        session_id: session_id.clone(),
                    });
                }
            }
        }

        // ── Stream Multiplexing: Relay StreamOpen to the other side ──
        WsMessage::StreamOpen {
            session_id,
            stream_id,
        } => {
            if let Some(session) = state.sessions.get(&session_id) {
                // Determine who sent this message by comparing conn_id
                let role = if conn_id == session.controller_id {
                    "controller"
                } else {
                    "agent"
                };
                relay_message(
                    state,
                    &session,
                    WsMessage::StreamOpen {
                        session_id,
                        stream_id,
                    },
                    role,
                );
            }
        }

        // ── Stream Multiplexing: Relay StreamClose to the other side ──
        WsMessage::StreamClose {
            session_id,
            stream_id,
        } => {
            if let Some(session) = state.sessions.get(&session_id) {
                let role = if conn_id == session.controller_id {
                    "controller"
                } else {
                    "agent"
                };
                relay_message(
                    state,
                    &session,
                    WsMessage::StreamClose {
                        session_id,
                        stream_id,
                    },
                    role,
                );
            }
        }

        // ── Data Relay: Forward data to the other side of the tunnel ──
        WsMessage::Data {
            session_id,
            stream_id,
            role,
            payload,
        } => {
            if let Some(session) = state.sessions.get(&session_id) {
                relay_message(
                    state,
                    &session,
                    WsMessage::Data {
                        session_id,
                        stream_id,
                        role: role.clone(),
                        payload,
                    },
                    &role,
                );
            }
        }

        // ── Tunnel Teardown ──
        WsMessage::TunnelClose { session_id } => {
            info!("Tunnel closing: {}", session_id);
            // Remove the session and notify both sides
            if let Some((_, session)) = state.sessions.remove(&session_id) {
                let close_msg = WsMessage::TunnelClose {
                    session_id: session.session_id,
                };
                // Notify the controller
                if let Some(c) = state.connections.get(&session.controller_id) {
                    let _ = c.send(close_msg.clone());
                }
                // Notify the agent
                if let Some(a) = state.agents.get(&session.agent_id) {
                    let _ = a.tx.send(close_msg);
                }
            }
        }

        // ── Heartbeat ──
        WsMessage::Ping => {
            let _ = tx.send(WsMessage::Pong);
        }
        WsMessage::Pong => {
            // No action needed; the pong confirms the connection is alive
        }
        _ => {}
    }
}
