//! # Connection Handlers
//!
//! Manages the lifecycle of individual QUIC connections connecting to the relay.
//! Each connection represents a single client (either an Agent or a Controller).
//!
//! ## Responsibilities
//!
//! 1. Open the primary bi-directional stream for control messages.
//! 2. Handle initial registration to receive an `agent_id`.
//! 3. Process incoming `ControlMessage` signals (Connect, TunnelRequest, etc.).
//! 4. Clean up active tunnels and notify peers upon disconnection.
//! 5. Handle incoming QUIC streams for data relay natively.

use crate::state::{generate_agent_id, AgentInfo, AppState, ConnectionInfo, TunnelSession};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};
use tunnel_protocol::ControlMessage;
use uuid::Uuid;

// ─── Connection Lifecycle ───────────────────────────────────────

/// Upgrades an incoming QUIC connection and enters the main event loop.
///
/// This function spans a new concurrency task for each client.
pub async fn handle_connection(connection: quinn::Connection, state: AppState) {
    let conn_id = Uuid::new_v4().to_string();
    info!("New QUIC connection: {}", conn_id);

    // Accept the first bi-directional stream as the control stream.
    let (mut send, mut recv) = match connection.accept_bi().await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to accept control stream: {}", e);
            return;
        }
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<ControlMessage>();
    state.connections.insert(
        conn_id.clone(),
        ConnectionInfo {
            tx: tx.clone(),
            conn: connection.clone(),
        },
    );

    let agent_id: Arc<tokio::sync::Mutex<Option<String>>> = Arc::new(tokio::sync::Mutex::new(None));

    // Wait, the send stream needs to safely write bytes. Bincode allows writing directly, but we can also
    // just use ControlMessage::serialize().
    let outbound_task = tokio::spawn(async move {
        // We need to write the length prefix if we want to frame properly over a reliable stream.
        // Bincode doesn't add framing if we serialize to a Vec, it just serializes the enum.
        // Wait, multiple messages sent consecutively on the same stream will need length prefixing,
        // or we use a datagram, but QUIC streams are reliable byte streams.
        // The protocol documentation doesn't specify length framing for control streams,
        // but typically a byte stream needs it (e.g. `[4 byte length][byte payload]`).
        // Actually, if we just use `bincode::serialize` it might be self-describing, but
        // it's safer to read exact frames. Wait, let's keep it simple: bincode serialization
        // is size-prefixed for variable length types natively, but reading stream of bincode is tricky.
        // For Phase 3, we'll write `[4-byte len][tag][bincode_bytes]`.
        while let Some(msg) = rx.recv().await {
            match msg.serialize() {
                Ok(bytes) => {
                    let len = (bytes.len() as u32).to_le_bytes();
                    if send.write_all(&len).await.is_err() {
                        break;
                    }
                    if send.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    error!("Serialize error: {}", e);
                }
            }
        }
    });

    let conn_id_clone = conn_id.clone();
    let cx = connection.clone();
    let state_c = state.clone();
    let inbound_streams_task = tokio::spawn(async move {
        while let Ok((mut q_send, mut q_recv)) = cx.accept_bi().await {
            // Read the 17-byte Data routing prefix
            // [0x0A, 8-byte session_id, 8-byte stream_id]
            let mut prefix = [0u8; 17];
            if q_recv.read_exact(&mut prefix).await.is_err() {
                continue;
            }
            if prefix[0] != 0x0A {
                continue; // Not a Data stream
            }

            let sess_bytes = &prefix[1..9];
            let sess_str =
                String::from_utf8(sess_bytes.iter().filter(|&&c| c != 0).cloned().collect())
                    .unwrap_or_default();
            let strm_bytes = &prefix[9..17];
            let strm_str =
                String::from_utf8(strm_bytes.iter().filter(|&&c| c != 0).cloned().collect())
                    .unwrap_or_default();

            info!(
                "New data stream for session {} / stream {}",
                sess_str, strm_str
            );

            if let Some(session) = state_c.sessions.get(&sess_str) {
                // Determine target connection ID
                let target_conn_id = if conn_id_clone == session.controller_id {
                    let mut agent_conn_id = None;
                    if let Some(agent) = state_c.agents.get(&session.agent_id) {
                        agent_conn_id = Some(agent.conn_id.clone());
                    }
                    agent_conn_id
                } else {
                    Some(session.controller_id.clone())
                };

                tracing::info!(
                    "Finding target_id for session {} -> target {:?}",
                    sess_str,
                    target_conn_id
                );
                if let Some(target_id) = target_conn_id {
                    if let Some(target_info) = state_c.connections.get(&target_id) {
                        // Open stream to target and forward
                        match target_info.conn.open_bi().await {
                            Ok((mut t_send, mut t_recv)) => {
                                // Forward the prefix
                                if t_send.write_all(&prefix).await.is_ok() {
                                    let sid_clone = sess_str.clone();
                                    let target_id_c = target_id.clone();
                                    tokio::spawn(async move {
                                        tracing::info!(
                                            "Starting proxy {} -> {}",
                                            sid_clone,
                                            target_id_c
                                        );
                                        let mut buf = [0u8; 8192];
                                        let mut total = 0;
                                        loop {
                                            match tokio::io::AsyncReadExt::read(
                                                &mut q_recv,
                                                &mut buf,
                                            )
                                            .await
                                            {
                                                Ok(0) => break,
                                                Ok(n) => {
                                                    tracing::info!(
                                                        "Proxy {} -> {}: read {} bytes",
                                                        sid_clone,
                                                        target_id_c,
                                                        n
                                                    );
                                                    if let Err(e) =
                                                        tokio::io::AsyncWriteExt::write_all(
                                                            &mut t_send,
                                                            &buf[..n],
                                                        )
                                                        .await
                                                    {
                                                        tracing::error!(
                                                            "Proxy Error write {} -> {}: {}",
                                                            sid_clone,
                                                            target_id_c,
                                                            e
                                                        );
                                                        break;
                                                    }
                                                    total += n;
                                                }
                                                Err(e) => {
                                                    tracing::error!(
                                                        "Proxy Error read {} -> {}: {}",
                                                        sid_clone,
                                                        target_id_c,
                                                        e
                                                    );
                                                    break;
                                                }
                                            }
                                        }
                                        tracing::info!(
                                            "Proxy {} -> {} finished, {} bytes",
                                            sid_clone,
                                            target_id_c,
                                            total
                                        );
                                        let _ = t_send.finish();
                                    });
                                    let sid_clone2 = sess_str.clone();
                                    let target_id_clone = target_id.clone();
                                    tokio::spawn(async move {
                                        tracing::info!(
                                            "Starting proxy {} -> {}",
                                            target_id_clone,
                                            sid_clone2
                                        );
                                        let mut buf = [0u8; 8192];
                                        let mut total = 0;
                                        loop {
                                            match tokio::io::AsyncReadExt::read(
                                                &mut t_recv,
                                                &mut buf,
                                            )
                                            .await
                                            {
                                                Ok(0) => break,
                                                Ok(n) => {
                                                    tracing::info!(
                                                        "Proxy {} -> {}: read {} bytes",
                                                        target_id_clone,
                                                        sid_clone2,
                                                        n
                                                    );
                                                    if let Err(e) =
                                                        tokio::io::AsyncWriteExt::write_all(
                                                            &mut q_send,
                                                            &buf[..n],
                                                        )
                                                        .await
                                                    {
                                                        tracing::error!(
                                                            "Proxy Error write {} -> {}: {}",
                                                            target_id_clone,
                                                            sid_clone2,
                                                            e
                                                        );
                                                        break;
                                                    }
                                                    total += n;
                                                }
                                                Err(e) => {
                                                    tracing::error!(
                                                        "Proxy Error read {} -> {}: {}",
                                                        target_id_clone,
                                                        sid_clone2,
                                                        e
                                                    );
                                                    break;
                                                }
                                            }
                                        }
                                        tracing::info!(
                                            "Proxy {} -> {} finished, {} bytes",
                                            target_id_clone,
                                            sid_clone2,
                                            total
                                        );
                                        let _ = q_send.finish();
                                    });
                                } else {
                                    tracing::error!(
                                        "Failed to write prefix to target stream for session {}",
                                        sess_str
                                    );
                                }
                            }
                            Err(e) => {
                                error!("Failed to open stream to target {}: {}", target_id, e);
                            }
                        }
                    }
                }
            }
        }
    });

    // Inbound control loop reading framed messages
    loop {
        let mut len_buf = [0u8; 4];
        if recv.read_exact(&mut len_buf).await.is_err() {
            break;
        }
        let len = u32::from_le_bytes(len_buf) as usize;

        // Prevent huge allocations
        if len > 1024 * 1024 {
            error!("Message too large: {}", len);
            break;
        }

        let mut buf = vec![0u8; len];
        if recv.read_exact(&mut buf).await.is_err() {
            break;
        }

        match ControlMessage::deserialize(&buf) {
            Ok(msg) => {
                handle_message(&state, &conn_id, &tx, &agent_id, msg).await;
            }
            Err(e) => {
                error!("Deserialize error: {}", e);
                break;
            }
        }
    }

    info!("Disconnecting: {}", conn_id);
    outbound_task.abort();
    inbound_streams_task.abort();
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

fn relay_message(state: &AppState, session: &TunnelSession, msg: ControlMessage, from_role: &str) {
    match from_role {
        "agent" => {
            if let Some(c) = state.connections.get(&session.controller_id) {
                let _ = c.tx.send(msg);
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
    tx: &mpsc::UnboundedSender<ControlMessage>,
    agent_id: &Arc<tokio::sync::Mutex<Option<String>>>,
    msg: ControlMessage,
) {
    match msg {
        ControlMessage::Register => {
            let aid = generate_agent_id();
            info!("Agent registered: {} (conn={})", aid, conn_id);
            state.agents.insert(
                aid.clone(),
                AgentInfo {
                    tx: tx.clone(),
                    conn_id: conn_id.to_string(),
                },
            );
            *agent_id.lock().await = Some(aid.clone());
            let _ = tx.send(ControlMessage::RegisterOk { agent_id: aid });
        }
        ControlMessage::Connect {
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

                    let _ = agent_info.tx.send(ControlMessage::TunnelRequest {
                        session_id,
                        remote_host,
                        remote_port,
                    });
                }
                None => {
                    let _ = tx.send(ControlMessage::Error {
                        message: format!("Agent '{}' not found", target_id),
                    });
                }
            }
        }
        ControlMessage::TunnelAccept { session_id } => {
            info!("Tunnel accepted: {}", session_id);
            if let Some(session) = state.sessions.get(&session_id) {
                if let Some(c) = state.connections.get(&session.controller_id) {
                    let _ = c.tx.send(ControlMessage::TunnelReady {
                        session_id: session_id.clone(),
                    });
                }
            }
        }
        ControlMessage::StreamOpen {
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
                    ControlMessage::StreamOpen {
                        session_id,
                        stream_id,
                    },
                    role,
                );
            }
        }
        ControlMessage::StreamClose {
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
                    ControlMessage::StreamClose {
                        session_id,
                        stream_id,
                    },
                    role,
                );
            }
        }
        ControlMessage::TunnelClose { session_id } => {
            info!("Tunnel closing: {}", session_id);
            if let Some((_, session)) = state.sessions.remove(&session_id) {
                let close_msg = ControlMessage::TunnelClose {
                    session_id: session.session_id,
                };
                if let Some(c) = state.connections.get(&session.controller_id) {
                    let _ = c.tx.send(close_msg.clone());
                }
                if let Some(a) = state.agents.get(&session.agent_id) {
                    let _ = a.tx.send(close_msg);
                }
            }
        }
        ControlMessage::Ping => {
            let _ = tx.send(ControlMessage::Pong);
        }
        ControlMessage::Pong
        | ControlMessage::RegisterOk { .. }
        | ControlMessage::Error { .. }
        | ControlMessage::TunnelReady { .. }
        | ControlMessage::TunnelRequest { .. } => {}
    }
}
