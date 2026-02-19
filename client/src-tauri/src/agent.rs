//! # Agent WebSocket Connection Loop
//!
//! Manages the persistent WebSocket connection between the client and
//! the relay server. Handles:
//! - Connection establishment and auto-reconnect on failure
//! - Agent registration on connect
//! - Heartbeat (ping/pong) to detect stale connections
//! - Incoming message dispatch to the appropriate handler
//! - Clean state reset on disconnect

use crate::protocol::WsMessage;
use crate::relay::handle_stream_relay;
use crate::state::{AgentState, AgentTunnelInfo, TunnelInfo};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tauri::Emitter;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use uuid::Uuid;



/// How long to wait before attempting to reconnect after a disconnect.
const RECONNECT_DELAY_SECS: u64 = 3;

// ─── Main Connection Loop ───────────────────────────────────────

/// Runs the agent's WebSocket connection loop forever.
///
/// This function never returns — it continuously connects to the relay
/// server, handles messages, and reconnects on failure. It is spawned
/// as a background task during Tauri app setup.
///
/// ## Connection Lifecycle
/// 1. Connect to the server via WebSocket
/// 2. Register this agent with its unique ID
/// 3. Spawn outbound message sender and heartbeat tasks
/// 4. Process incoming messages until disconnect
/// 5. Clean up all state (channels, tunnels, tasks)
/// 6. Wait `RECONNECT_DELAY_SECS` and go to step 1
pub async fn run_agent_loop(state: Arc<AgentState>, app_handle: tauri::AppHandle) {
    loop {
        // Read the server URL from state (may have been updated by the UI)
        let server_url = state.server_url.read().await.clone();
        info!("Connecting to server: {}", server_url);
        let _ = app_handle.emit("connection-status", false);

        match connect_async(&server_url).await {
            Ok((ws_stream, _)) => {
                info!("Connected to server!");
                *state.connected.write().await = true;
                let _ = app_handle.emit("connection-status", true);

                // Split the WebSocket into read and write halves
                let (ws_sink, mut ws_stream_rx) = ws_stream.split();
                let ws_sink = Arc::new(tokio::sync::Mutex::new(ws_sink));

                // Create the outbound message channel
                let (tx, mut rx) = mpsc::unbounded_channel::<WsMessage>();
                *state.ws_tx.write().await = Some(tx.clone());

                // Request registration — the server will assign an Agent ID
                let _ = tx.send(WsMessage::Register);

                // ── Outbound Sender Task ──
                // Drains the message queue, serializes each message to JSON,
                // and sends it over the WebSocket.
                let ws_sink_clone = ws_sink.clone();
                let outbound = tokio::spawn(async move {
                    while let Some(msg) = rx.recv().await {
                        if let Ok(text) = serde_json::to_string(&msg) {
                            let mut sink = ws_sink_clone.lock().await;
                            if sink.send(Message::Text(text.into())).await.is_err() {
                                break; // Connection lost
                            }
                        }
                    }
                });

                // ── Heartbeat Task ──
                // Sends a Ping message every 30 seconds to keep the
                // connection alive and detect stale connections.
                let tx_ping = tx.clone();
                let heartbeat = tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
                        if tx_ping.send(WsMessage::Ping).is_err() {
                            break; // Channel closed; connection lost
                        }
                    }
                });

                // ── Inbound Message Loop ──
                // Processes incoming WebSocket frames. Only JSON text frames
                // are handled; binary and ping frames are ignored.
                while let Some(Ok(msg)) = ws_stream_rx.next().await {
                    match msg {
                        Message::Text(text) => {
                            if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                                handle_server_message(&state, &tx, &app_handle, ws_msg).await;
                            }
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }

                // ── Disconnect Cleanup ──
                // Abort background tasks and reset all state to ensure
                // a clean slate before reconnecting.
                outbound.abort();
                heartbeat.abort();
                *state.connected.write().await = false;
                *state.ws_tx.write().await = None;
                state.data_channels.write().await.clear();
                state.agent_tunnels.write().await.clear();
                state.abort_all_tasks().await;
                state.tunnels.write().await.clear();
                let _ = app_handle.emit("tunnels-updated", ());
                let _ = app_handle.emit("connection-status", false);
                warn!("Disconnected from server");
            }
            Err(e) => {
                error!("Connection failed: {}", e);
            }
        }

        // Wait before attempting to reconnect
        info!("Reconnecting in {}s...", RECONNECT_DELAY_SECS);
        tokio::time::sleep(tokio::time::Duration::from_secs(RECONNECT_DELAY_SECS)).await;
    }
}

// ─── Server Message Handler ─────────────────────────────────────

/// Handles a single incoming WebSocket message from the relay server.
///
/// This is the central dispatch function for all server messages.
/// Each message type triggers different behavior depending on whether
/// this client is acting as an agent (receiving tunnels) or a controller
/// (initiating tunnels).
async fn handle_server_message(
    state: &Arc<AgentState>,
    tx: &mpsc::UnboundedSender<WsMessage>,
    app_handle: &tauri::AppHandle,
    msg: WsMessage,
) {
    match msg {
        // ── Registration Confirmed with Server-Assigned ID ──
        WsMessage::RegisterOk { agent_id } => {
            info!("Registered as agent: {}", agent_id);
            // Store the server-assigned agent ID
            *state.agent_id.write().await = agent_id.clone();
            let _ = app_handle.emit("registered", &agent_id);
        }

        // ── Agent Side: Incoming Tunnel Request ──
        // When another client wants to connect to us, the server asks
        // if we accept. We auto-accept all tunnel requests.
        WsMessage::TunnelRequest {
            session_id,
            remote_host,
            remote_port,
        } => {
            info!(
                "Tunnel request: {} → {}:{}",
                session_id, remote_host, remote_port
            );

            // Auto-accept the tunnel request
            let _ = tx.send(WsMessage::TunnelAccept {
                session_id: session_id.clone(),
            });

            // Store the target address so we can connect to it
            // when StreamOpen messages arrive later
            {
                let mut at = state.agent_tunnels.write().await;
                at.insert(
                    session_id.clone(),
                    AgentTunnelInfo {
                        remote_host: remote_host.clone(),
                        remote_port,
                    },
                );
            }

            // Add the tunnel to the UI list
            {
                let mut tunnels = state.tunnels.write().await;
                tunnels.push(TunnelInfo {
                    session_id: session_id.clone(),
                    remote_host,
                    remote_port,
                    local_port: 0, // Agent side doesn't listen on a local port
                    direction: "incoming".to_string(),
                    status: "active".to_string(),
                });
            }
            let _ = app_handle.emit("tunnels-updated", ());
        }

        // ── Controller Side: Tunnel is Ready ──
        // The agent accepted our tunnel request. Now we start a TCP
        // listener on the local port and relay incoming connections.
        WsMessage::TunnelReady { session_id } => {
            info!("Tunnel ready: {}", session_id);

            // Retrieve and remove the pending connection parameters
            let pending = {
                let mut pm = state.pending_connects.write().await;
                let key = pm.keys().next().cloned();
                key.and_then(|k| pm.remove(&k))
            };

            // Update the UI: change status from "connecting" to "active"
            // and replace the placeholder session ID with the real one
            {
                let mut tunnels = state.tunnels.write().await;
                if let Some(t) = tunnels
                    .iter_mut()
                    .find(|t| t.direction == "outgoing" && t.status == "connecting")
                {
                    t.session_id = session_id.clone();
                    t.status = "active".to_string();
                }
            }
            let _ = app_handle.emit("tunnels-updated", ());

            // Start a TCP listener to accept local connections
            if let Some(pending) = pending {
                let local_port = pending.local_port;
                let tx_clone = tx.clone();
                let state_clone = state.clone();
                let app_clone = app_handle.clone();
                let sid = session_id.clone();

                let sid_for_handle = session_id.clone();
                let handle = tokio::spawn(async move {
                    let bind_addr = format!("127.0.0.1:{}", local_port);
                    match TcpListener::bind(&bind_addr).await {
                        Ok(listener) => {
                            info!("Listening on {} for tunnel {}", bind_addr, sid);

                            // Accept loop: each new TCP connection becomes
                            // a new "stream" within the tunnel session
                            loop {
                                match listener.accept().await {
                                    Ok((tcp_stream, peer)) => {
                                        // Generate a unique stream ID for this TCP connection
                                        let stream_id =
                                            Uuid::new_v4().to_string()[..8].to_string();
                                        info!(
                                            "New stream {} from {} (tunnel {})",
                                            stream_id, peer, sid
                                        );

                                        // Tell the agent to open a TCP connection to the target
                                        let _ = tx_clone.send(WsMessage::StreamOpen {
                                            session_id: sid.clone(),
                                            stream_id: stream_id.clone(),
                                        });

                                        // Pre-register the data channel BEFORE spawning the relay.
                                        // This ensures incoming data from the agent is buffered
                                        // while the relay task starts up, preventing a race condition.
                                        let (data_tx, data_rx) =
                                            mpsc::unbounded_channel::<Vec<u8>>();
                                        let channel_key =
                                            format!("controller-{}", stream_id);
                                        {
                                            state_clone
                                                .data_channels
                                                .write()
                                                .await
                                                .insert(channel_key.clone(), data_tx);
                                        }

                                        // Spawn the bidirectional relay task for this stream
                                        let tx2 = tx_clone.clone();
                                        let st2 = state_clone.clone();
                                        let sid2 = sid.clone();
                                        tokio::spawn(async move {
                                            handle_stream_relay(
                                                tcp_stream,
                                                sid2,
                                                stream_id,
                                                channel_key,
                                                "controller".to_string(),
                                                tx2,
                                                st2,
                                                data_rx,
                                            )
                                            .await;
                                        });
                                    }
                                    Err(e) => {
                                        error!("Accept error: {}", e);
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to bind {}: {}", bind_addr, e);
                            let _ = app_clone.emit(
                                "server-error",
                                &format!("Port {} unavailable: {}", local_port, e),
                            );
                        }
                    }
                });

                // Track the task handle for cleanup when the tunnel is closed
                {
                    let mut handles = state.task_handles.write().await;
                    handles.entry(sid_for_handle).or_default().push(handle);
                }
            } else {
                warn!("TunnelReady but no pending connect for {}", session_id);
            }
        }

        // ── Agent Side: Controller Opened a New Stream ──
        // The controller has a new TCP connection. We need to open
        // a TCP connection to the target service and start relaying.
        WsMessage::StreamOpen {
            session_id,
            stream_id,
        } => {
            info!("StreamOpen: session={}, stream={}", session_id, stream_id);

            // Look up the target address for this tunnel
            let tunnel_info = {
                let at = state.agent_tunnels.read().await;
                at.get(&session_id).cloned()
            };

            if let Some(info) = tunnel_info {
                let addr = format!("{}:{}", info.remote_host, info.remote_port);
                let tx_clone = tx.clone();
                let state_clone = state.clone();

                // Pre-register the agent data channel BEFORE connecting to the target.
                // This ensures data from the controller is buffered while the TCP
                // connection is being established.
                let (data_tx, data_rx) = mpsc::unbounded_channel::<Vec<u8>>();
                let channel_key = format!("agent-{}", stream_id);
                {
                    state
                        .data_channels
                        .write()
                        .await
                        .insert(channel_key.clone(), data_tx);
                }

                // Spawn a task to connect to the target and start relaying
                tokio::spawn(async move {
                    match TcpStream::connect(&addr).await {
                        Ok(tcp_stream) => {
                            info!("Connected to {} for stream {}", addr, stream_id);
                            handle_stream_relay(
                                tcp_stream,
                                session_id,
                                stream_id,
                                channel_key,
                                "agent".to_string(),
                                tx_clone,
                                state_clone,
                                data_rx,
                            )
                            .await;
                        }
                        Err(e) => {
                            error!("Failed to connect {} for stream {}: {}", addr, stream_id, e);
                            // Clean up the pre-registered channel
                            state_clone.data_channels.write().await.remove(&channel_key);
                            // Notify the controller that the stream failed
                            let _ = tx_clone.send(WsMessage::StreamClose {
                                session_id,
                                stream_id,
                            });
                        }
                    }
                });
            }
        }

        // ── Stream Closed by the Other Side ──
        // Remove the data channel so the relay task will stop naturally.
        WsMessage::StreamClose {
            session_id: _,
            stream_id,
        } => {
            let mut channels = state.data_channels.write().await;
            // Remove both possible channel keys (we don't know our role here)
            channels.remove(&format!("agent-{}", stream_id));
            channels.remove(&format!("controller-{}", stream_id));
        }

        // ── Tunnel Closed ──
        // Clean up all resources associated with this tunnel session.
        WsMessage::TunnelClose { session_id } => {
            info!("Tunnel closed: {}", session_id);
            state.abort_session_tasks(&session_id).await;
            state.agent_tunnels.write().await.remove(&session_id);
            let mut tunnels = state.tunnels.write().await;
            tunnels.retain(|t| t.session_id != session_id);
            let _ = app_handle.emit("tunnels-updated", ());
        }

        // ── Data Relay ──
        // Route incoming data to the correct stream's TCP handler
        // by looking up the data channel and forwarding the bytes.
        WsMessage::Data {
            session_id: _,
            stream_id,
            role,
            payload,
        } => {
            if let Ok(data) = BASE64.decode(&payload) {
                let channels = state.data_channels.read().await;
                // Data from "agent" → goes to controller's handler
                // Data from "controller" → goes to agent's handler
                let target_key = if role == "agent" {
                    format!("controller-{}", stream_id)
                } else {
                    format!("agent-{}", stream_id)
                };

                if let Some(sender) = channels.get(&target_key) {
                    let _ = sender.send(data);
                }
            }
        }

        // ── Error from Server ──
        WsMessage::Error { message } => {
            error!("Server error: {}", message);
            let _ = app_handle.emit("server-error", &message);
        }

        // ── Heartbeat ──
        WsMessage::Pong => {
            // No action needed; confirms the connection is alive
        }
        _ => {}
    }
}
