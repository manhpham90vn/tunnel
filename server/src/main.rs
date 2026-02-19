use axum::{
    Router,
    extract::{State, WebSocketUpgrade, ws::{Message, WebSocket}},
    response::IntoResponse,
    routing::get,
    Json,
};
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;
use tracing::{error, info, warn};
use uuid::Uuid;

// ─── Protocol Messages ───────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WsMessage {
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
    // Stream multiplexing within a tunnel
    StreamOpen {
        session_id: String,
        stream_id: String,
    },
    StreamClose {
        session_id: String,
        stream_id: String,
    },
    // Data relay with stream multiplexing
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

// ─── Server State ────────────────────────────────────────────────

type ClientTx = mpsc::UnboundedSender<WsMessage>;

#[derive(Debug, Clone)]
struct AgentInfo {
    tx: ClientTx,
}

#[derive(Debug, Clone)]
struct TunnelSession {
    session_id: String,
    agent_id: String,
    controller_id: String,
    remote_host: String,
    remote_port: u16,
}

#[derive(Clone)]
struct AppState {
    agents: Arc<DashMap<String, AgentInfo>>,
    connections: Arc<DashMap<String, ClientTx>>,
    sessions: Arc<DashMap<String, TunnelSession>>,
}

impl AppState {
    fn new() -> Self {
        Self {
            agents: Arc::new(DashMap::new()),
            connections: Arc::new(DashMap::new()),
            sessions: Arc::new(DashMap::new()),
        }
    }
}

// ─── Agent List API ──────────────────────────────────────────────

#[derive(Serialize)]
struct AgentListItem {
    agent_id: String,
}

async fn list_agents(State(state): State<AppState>) -> Json<Vec<AgentListItem>> {
    let agents: Vec<AgentListItem> = state
        .agents
        .iter()
        .map(|entry| AgentListItem {
            agent_id: entry.key().clone(),
        })
        .collect();
    Json(agents)
}

// ─── WebSocket Handler ──────────────────────────────────────────

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_connection(socket, state))
}

async fn handle_connection(socket: WebSocket, state: AppState) {
    let conn_id = Uuid::new_v4().to_string();
    info!("New connection: {}", conn_id);

    let (ws_sink, mut ws_stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<WsMessage>();

    state.connections.insert(conn_id.clone(), tx.clone());

    let agent_id: Arc<tokio::sync::Mutex<Option<String>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    // Outbound task
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
                break;
            }
        }
    });

    // Inbound
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

    // Cleanup
    info!("Disconnecting: {}", conn_id);
    outbound_task.abort();
    state.connections.remove(&conn_id);

    let aid = agent_id.lock().await;
    if let Some(ref aid) = *aid {
        info!("Agent {} disconnected", aid);
        state.agents.remove(aid);

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

/// Relay a message to the "other side" of the tunnel based on role.
fn relay_message(state: &AppState, session: &TunnelSession, msg: WsMessage, from_role: &str) {
    match from_role {
        "agent" => {
            if let Some(c) = state.connections.get(&session.controller_id) {
                let _ = c.send(msg);
            }
        }
        "controller" => {
            if let Some(a) = state.agents.get(&session.agent_id) {
                let _ = a.tx.send(msg);
            }
        }
        _ => {}
    }
}

async fn handle_message(
    state: &AppState,
    conn_id: &str,
    tx: &ClientTx,
    agent_id: &Arc<tokio::sync::Mutex<Option<String>>>,
    msg: WsMessage,
) {
    match msg {
        WsMessage::Register { agent_id: aid } => {
            info!("Agent registered: {}", aid);
            state.agents.insert(aid.clone(), AgentInfo { tx: tx.clone() });
            *agent_id.lock().await = Some(aid);
            let _ = tx.send(WsMessage::RegisterOk);
        }

        WsMessage::Connect {
            target_id,
            remote_host,
            remote_port,
        } => {
            info!("Connect request: {} → {} ({}:{})", conn_id, target_id, remote_host, remote_port);
            match state.agents.get(&target_id) {
                Some(agent_info) => {
                    let session_id = Uuid::new_v4().to_string()[..8].to_string();
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
                    let _ = agent_info.tx.send(WsMessage::TunnelRequest {
                        session_id,
                        remote_host,
                        remote_port,
                    });
                }
                None => {
                    let _ = tx.send(WsMessage::Error {
                        message: format!("Agent '{}' not found", target_id),
                    });
                }
            }
        }

        WsMessage::TunnelAccept { session_id } => {
            info!("Tunnel accepted: {}", session_id);
            if let Some(session) = state.sessions.get(&session_id) {
                if let Some(c) = state.connections.get(&session.controller_id) {
                    let _ = c.send(WsMessage::TunnelReady {
                        session_id: session_id.clone(),
                    });
                }
            }
        }

        // ── Stream multiplexing: relay to other side ──
        WsMessage::StreamOpen {
            session_id,
            stream_id,
        } => {
            if let Some(session) = state.sessions.get(&session_id) {
                // Determine sender role by checking conn_id
                let role = if conn_id == session.controller_id {
                    "controller"
                } else {
                    "agent"
                };
                relay_message(
                    state,
                    &session,
                    WsMessage::StreamOpen { session_id, stream_id },
                    role,
                );
            }
        }

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
                    WsMessage::StreamClose { session_id, stream_id },
                    role,
                );
            }
        }

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

        WsMessage::TunnelClose { session_id } => {
            info!("Tunnel closing: {}", session_id);
            if let Some((_, session)) = state.sessions.remove(&session_id) {
                let close_msg = WsMessage::TunnelClose {
                    session_id: session.session_id,
                };
                if let Some(c) = state.connections.get(&session.controller_id) {
                    let _ = c.send(close_msg.clone());
                }
                if let Some(a) = state.agents.get(&session.agent_id) {
                    let _ = a.tx.send(close_msg);
                }
            }
        }

        WsMessage::Ping => {
            let _ = tx.send(WsMessage::Pong);
        }
        WsMessage::Pong => {}
        _ => {}
    }
}

// ─── Main ────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tunnel_server=info".into()),
        )
        .init();

    let state = AppState::new();
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/api/agents", get(list_agents))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 7070));
    info!("🚇 Tunnel Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
