use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::Emitter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use uuid::Uuid;

// ─── Configuration ───────────────────────────────────────────────

const SERVER_URL: &str = "ws://127.0.0.1:7070/ws";
const RECONNECT_DELAY_SECS: u64 = 3;

// ─── Protocol Messages (must match server) ───────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    Register { agent_id: String },
    RegisterOk,
    Connect {
        target_id: String,
        remote_host: String,
        remote_port: u16,
    },
    TunnelRequest {
        session_id: String,
        remote_host: String,
        remote_port: u16,
    },
    TunnelAccept { session_id: String },
    TunnelReady { session_id: String },
    TunnelClose { session_id: String },
    StreamOpen {
        session_id: String,
        stream_id: String,
    },
    StreamClose {
        session_id: String,
        stream_id: String,
    },
    Data {
        session_id: String,
        stream_id: String,
        role: String,
        payload: String,
    },
    Ping,
    Pong,
    Error { message: String },
}

// ─── Agent State ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct TunnelInfo {
    pub session_id: String,
    pub remote_host: String,
    pub remote_port: u16,
    pub local_port: u16,
    pub direction: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentStatus {
    pub agent_id: String,
    pub connected: bool,
    pub server_url: String,
}

#[derive(Debug, Clone)]
pub struct PendingConnect {
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
}

/// Info stored for agent-side tunnels so we can open new TCP connections
/// when StreamOpen arrives from the controller.
#[derive(Debug, Clone)]
pub struct AgentTunnelInfo {
    pub remote_host: String,
    pub remote_port: u16,
}

pub struct AgentState {
    pub agent_id: String,
    pub connected: RwLock<bool>,
    pub ws_tx: RwLock<Option<mpsc::UnboundedSender<WsMessage>>>,
    pub tunnels: RwLock<Vec<TunnelInfo>>,
    pub pending_connects: RwLock<HashMap<String, PendingConnect>>,
    /// Per-stream data channels: "{role}-{stream_id}" → sender
    pub data_channels: RwLock<HashMap<String, mpsc::UnboundedSender<Vec<u8>>>>,
    /// Agent-side tunnel info: session_id → target address
    pub agent_tunnels: RwLock<HashMap<String, AgentTunnelInfo>>,
    /// Spawned task handles: session_id → JoinHandle (for cleanup)
    pub task_handles: RwLock<HashMap<String, Vec<JoinHandle<()>>>>,
}

impl AgentState {
    fn new() -> Self {
        let agent_id = generate_agent_id();
        Self {
            agent_id,
            connected: RwLock::new(false),
            ws_tx: RwLock::new(None),
            tunnels: RwLock::new(Vec::new()),
            pending_connects: RwLock::new(HashMap::new()),
            data_channels: RwLock::new(HashMap::new()),
            agent_tunnels: RwLock::new(HashMap::new()),
            task_handles: RwLock::new(HashMap::new()),
        }
    }

    /// Abort all spawned tasks for a session
    async fn abort_session_tasks(&self, session_id: &str) {
        let mut handles = self.task_handles.write().await;
        if let Some(tasks) = handles.remove(session_id) {
            for handle in tasks {
                handle.abort();
            }
            info!("Aborted tasks for session {}", session_id);
        }
    }

    /// Abort ALL spawned tasks (used on WS disconnect)
    async fn abort_all_tasks(&self) {
        let mut handles = self.task_handles.write().await;
        for (sid, tasks) in handles.drain() {
            for handle in tasks {
                handle.abort();
            }
            info!("Aborted tasks for session {}", sid);
        }
    }
}

fn generate_agent_id() -> String {
    let uuid = Uuid::new_v4().to_string();
    let short = &uuid[..8];
    format!(
        "{}-{}",
        short[..4].to_uppercase(),
        short[4..8].to_uppercase()
    )
}

// ─── Tauri Commands ──────────────────────────────────────────────

#[tauri::command]
async fn get_agent_info(state: tauri::State<'_, Arc<AgentState>>) -> Result<AgentStatus, String> {
    let connected = *state.connected.read().await;
    Ok(AgentStatus {
        agent_id: state.agent_id.clone(),
        connected,
        server_url: SERVER_URL.to_string(),
    })
}

#[tauri::command]
async fn connect_to_agent(
    target_id: String,
    remote_host: String,
    remote_port: u16,
    local_port: u16,
    state: tauri::State<'_, Arc<AgentState>>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    let ws_tx = state.ws_tx.read().await;
    let tx = ws_tx.as_ref().ok_or("Not connected to server")?.clone();

    // Store pending info
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

    tx.send(WsMessage::Connect {
        target_id: target_id.clone(),
        remote_host: remote_host.clone(),
        remote_port,
    })
    .map_err(|e| format!("Failed to send: {}", e))?;

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

    let _ = app_handle.emit("tunnels-updated", ());
    info!("Connect request → agent {} (local={})", target_id, local_port);
    Ok(session_id)
}

#[tauri::command]
async fn disconnect_tunnel(
    session_id: String,
    state: tauri::State<'_, Arc<AgentState>>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let ws_tx = state.ws_tx.read().await;
    if let Some(tx) = ws_tx.as_ref() {
        let _ = tx.send(WsMessage::TunnelClose {
            session_id: session_id.clone(),
        });
    }
    let mut tunnels = state.tunnels.write().await;
    tunnels.retain(|t| t.session_id != session_id);
    let _ = app_handle.emit("tunnels-updated", ());
    Ok(())
}

#[tauri::command]
async fn get_tunnels(state: tauri::State<'_, Arc<AgentState>>) -> Result<Vec<TunnelInfo>, String> {
    Ok(state.tunnels.read().await.clone())
}

// ─── WebSocket Connection Manager ────────────────────────────────

async fn run_agent_loop(state: Arc<AgentState>, app_handle: tauri::AppHandle) {
    loop {
        info!("Connecting to server: {}", SERVER_URL);
        let _ = app_handle.emit("connection-status", false);

        match connect_async(SERVER_URL).await {
            Ok((ws_stream, _)) => {
                info!("Connected to server!");
                *state.connected.write().await = true;
                let _ = app_handle.emit("connection-status", true);

                let (ws_sink, mut ws_stream_rx) = ws_stream.split();
                let ws_sink = Arc::new(tokio::sync::Mutex::new(ws_sink));

                let (tx, mut rx) = mpsc::unbounded_channel::<WsMessage>();
                *state.ws_tx.write().await = Some(tx.clone());

                let _ = tx.send(WsMessage::Register {
                    agent_id: state.agent_id.clone(),
                });

                // Outbound
                let ws_sink_clone = ws_sink.clone();
                let outbound = tokio::spawn(async move {
                    while let Some(msg) = rx.recv().await {
                        if let Ok(text) = serde_json::to_string(&msg) {
                            let mut sink = ws_sink_clone.lock().await;
                            if sink.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                });

                // Heartbeat
                let tx_ping = tx.clone();
                let heartbeat = tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
                        if tx_ping.send(WsMessage::Ping).is_err() {
                            break;
                        }
                    }
                });

                // Inbound
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

        info!("Reconnecting in {}s...", RECONNECT_DELAY_SECS);
        tokio::time::sleep(tokio::time::Duration::from_secs(RECONNECT_DELAY_SECS)).await;
    }
}

async fn handle_server_message(
    state: &Arc<AgentState>,
    tx: &mpsc::UnboundedSender<WsMessage>,
    app_handle: &tauri::AppHandle,
    msg: WsMessage,
) {
    match msg {
        WsMessage::RegisterOk => {
            info!("Registered as agent: {}", state.agent_id);
            let _ = app_handle.emit("registered", &state.agent_id);
        }

        // ── Agent side: incoming tunnel request ──
        WsMessage::TunnelRequest {
            session_id,
            remote_host,
            remote_port,
        } => {
            info!("Tunnel request: {} → {}:{}", session_id, remote_host, remote_port);

            // Auto-accept
            let _ = tx.send(WsMessage::TunnelAccept {
                session_id: session_id.clone(),
            });

            // Store target info for later StreamOpen requests
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

            // Add to UI
            {
                let mut tunnels = state.tunnels.write().await;
                tunnels.push(TunnelInfo {
                    session_id: session_id.clone(),
                    remote_host,
                    remote_port,
                    local_port: 0,
                    direction: "incoming".to_string(),
                    status: "active".to_string(),
                });
            }
            let _ = app_handle.emit("tunnels-updated", ());
        }

        // ── Controller side: tunnel is ready, start TCP listener ──
        WsMessage::TunnelReady { session_id } => {
            info!("Tunnel ready: {}", session_id);

            let pending = {
                let mut pm = state.pending_connects.write().await;
                let key = pm.keys().next().cloned();
                key.and_then(|k| pm.remove(&k))
            };

            // Update UI
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

            // Start TCP listener
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
                            loop {
                                match listener.accept().await {
                                    Ok((tcp_stream, peer)) => {
                                        // Each TCP connection = new stream
                                        let stream_id =
                                            Uuid::new_v4().to_string()[..8].to_string();
                                        info!(
                                            "New stream {} from {} (tunnel {})",
                                            stream_id, peer, sid
                                        );

                                        // Tell agent to open TCP to target
                                        let _ = tx_clone.send(WsMessage::StreamOpen {
                                            session_id: sid.clone(),
                                            stream_id: stream_id.clone(),
                                        });

                                        // Pre-register controller data channel BEFORE spawning relay
                                        let (data_tx, data_rx) = mpsc::unbounded_channel::<Vec<u8>>();
                                        let channel_key = format!("controller-{}", stream_id);
                                        {
                                            state_clone.data_channels.write().await
                                                .insert(channel_key.clone(), data_tx);
                                        }

                                        // Spawn relay for this stream
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
                // Track handle for cleanup
                {
                    let mut handles = state.task_handles.write().await;
                    handles.entry(sid_for_handle).or_default().push(handle);
                }
            } else {
                warn!("TunnelReady but no pending connect for {}", session_id);
            }
        }

        // ── Agent side: controller opened a new stream, open TCP to target ──
        WsMessage::StreamOpen {
            session_id,
            stream_id,
        } => {
            info!("StreamOpen: session={}, stream={}", session_id, stream_id);

            let tunnel_info = {
                let at = state.agent_tunnels.read().await;
                at.get(&session_id).cloned()
            };

            if let Some(info) = tunnel_info {
                let addr = format!("{}:{}", info.remote_host, info.remote_port);
                let tx_clone = tx.clone();
                let state_clone = state.clone();

                // Pre-register agent data channel BEFORE connecting TCP
                // This ensures data from controller is buffered while TCP connects
                let (data_tx, data_rx) = mpsc::unbounded_channel::<Vec<u8>>();
                let channel_key = format!("agent-{}", stream_id);
                {
                    state.data_channels.write().await
                        .insert(channel_key.clone(), data_tx);
                }

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
                            state_clone.data_channels.write().await.remove(&channel_key);
                            let _ = tx_clone.send(WsMessage::StreamClose {
                                session_id,
                                stream_id,
                            });
                        }
                    }
                });
            }
        }

        // ── Stream closed by other side ──
        WsMessage::StreamClose {
            session_id: _,
            stream_id,
        } => {
            // Remove data channel to signal the relay to stop
            let mut channels = state.data_channels.write().await;
            // Remove both possible keys
            channels.remove(&format!("agent-{}", stream_id));
            channels.remove(&format!("controller-{}", stream_id));
        }

        WsMessage::TunnelClose { session_id } => {
            info!("Tunnel closed: {}", session_id);
            state.abort_session_tasks(&session_id).await;
            state.agent_tunnels.write().await.remove(&session_id);
            let mut tunnels = state.tunnels.write().await;
            tunnels.retain(|t| t.session_id != session_id);
            let _ = app_handle.emit("tunnels-updated", ());
        }

        // ── Data → route to correct stream's TCP handler ──
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

        WsMessage::Error { message } => {
            error!("Server error: {}", message);
            let _ = app_handle.emit("server-error", &message);
        }

        WsMessage::Pong => {}
        _ => {}
    }
}

// ─── Per-Stream TCP ↔ WebSocket Relay ────────────────────────────

async fn handle_stream_relay(
    tcp_stream: TcpStream,
    session_id: String,
    stream_id: String,
    channel_key: String,
    role: String,
    ws_tx: mpsc::UnboundedSender<WsMessage>,
    state: Arc<AgentState>,
    mut data_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let (mut tcp_read, mut tcp_write) = tcp_stream.into_split();

    let sid = session_id.clone();
    let stid = stream_id.clone();
    let my_role = role.clone();

    // TCP → WebSocket
    let ws_tx_clone = ws_tx.clone();
    let tcp_to_ws = tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            match tcp_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let payload = BASE64.encode(&buf[..n]);
                    if ws_tx_clone
                        .send(WsMessage::Data {
                            session_id: sid.clone(),
                            stream_id: stid.clone(),
                            role: my_role.clone(),
                            payload,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // WebSocket → TCP (via pre-registered data channel)
    let ws_to_tcp = tokio::spawn(async move {
        while let Some(data) = data_rx.recv().await {
            if tcp_write.write_all(&data).await.is_err() {
                break;
            }
        }
    });

    tokio::select! {
        _ = tcp_to_ws => {},
        _ = ws_to_tcp => {},
    }

    // Cleanup
    state.data_channels.write().await.remove(&channel_key);
    let _ = ws_tx.send(WsMessage::StreamClose {
        session_id,
        stream_id,
    });
}

// ─── Tauri App Setup ─────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let agent_state = Arc::new(AgentState::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(agent_state.clone())
        .invoke_handler(tauri::generate_handler![
            get_agent_info,
            connect_to_agent,
            disconnect_tunnel,
            get_tunnels,
        ])
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let state = agent_state.clone();

            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new()
                    .expect("Failed to create Tokio runtime");
                rt.block_on(async move {
                    run_agent_loop(state, app_handle).await;
                });
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
