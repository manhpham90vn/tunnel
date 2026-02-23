//! Manages the persistent QUIC connection between the client and
//! the relay server. Handles:
//! - Connection establishment and auto-reconnect on failure
//! - Agent registration on connect
//! - Heartbeat (ping/pong) to detect stale connections
//! - Incoming message dispatch to the appropriate handler
//! - Clean state reset on disconnect

use crate::cert::SkipServerVerification;
use crate::relay::handle_stream_relay;
use crate::state::{AgentState, AgentTunnelInfo, TunnelInfo};
use quinn::Endpoint;
use std::sync::Arc;
use tauri::Emitter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use tunnel_protocol::ControlMessage;
use uuid::Uuid;

/// How long to wait before attempting to reconnect after a disconnect.
const RECONNECT_DELAY_SECS: u64 = 3;

// ─── Main Connection Loop ───────────────────────────────────────

pub async fn run_agent_loop(state: Arc<AgentState>, app_handle: tauri::AppHandle) {
    let mut endpoint = Endpoint::client("[::]:0".parse().unwrap()).unwrap();

    // Build the TLS configuration.
    // By default for dev mode, we skip server verification.
    // In prod, if the user specifies a custom CA via environment variable
    // TUNNEL_CA_CERT, we load it and verify against it.
    let ca_path = std::env::var("TUNNEL_CA_CERT").ok();
    let mut use_custom_ca = false;
    let mut roots = rustls::RootCertStore::empty();

    if let Some(path) = &ca_path {
        if let Ok(cert_bytes) = std::fs::read(path) {
            let certs = rustls_pemfile::certs(&mut &cert_bytes[..])
                .filter_map(Result::ok)
                .collect::<Vec<_>>();

            if !certs.is_empty() {
                let (added, ignored) = roots.add_parsable_certificates(certs);
                if added > 0 {
                    use_custom_ca = true;
                    info!(
                        "Loaded {} custom CA certificate(s) from {} (ignored: {})",
                        added, path, ignored
                    );
                }
            }
        } else {
            error!("Failed to read custom CA certificate at {}", path);
        }
    }

    let mut crypto = if use_custom_ca {
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth()
    } else {
        info!("No custom CA provided, skipping server verification (dev mode)");
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(SkipServerVerification::new())
            .with_no_client_auth()
    };

    crypto.alpn_protocols = vec![b"tunnel".to_vec()];
    let quic_client_config = quinn::crypto::rustls::QuicClientConfig::try_from(crypto).unwrap();
    let mut client_config = quinn::ClientConfig::new(std::sync::Arc::new(quic_client_config));

    let mut transport_config = quinn::TransportConfig::default();
    transport_config.max_concurrent_bidi_streams(4096u32.into());
    transport_config.max_concurrent_uni_streams(4096u32.into());
    client_config.transport_config(std::sync::Arc::new(transport_config));

    endpoint.set_default_client_config(client_config);

    loop {
        let server_url = state.server_url.read().await.clone();
        info!("Connecting to server: {}", server_url);
        let _ = app_handle.emit("connection-status", false);

        match server_url.parse() {
            Ok(server_addr) => {
                match endpoint.connect(server_addr, "localhost") {
                    Ok(connecting) => {
                        match connecting.await {
                            Ok(connection) => {
                                info!("Connected to server via QUIC!");
                                *state.connected.write().await = true;
                                let _ = app_handle.emit("connection-status", true);

                                // Open the primary bi-directional stream for ControlMessages
                                match connection.open_bi().await {
                                    Ok((mut control_send, mut control_recv)) => {
                                        let (tx, mut rx) =
                                            mpsc::unbounded_channel::<ControlMessage>();
                                        *state.ctrl_tx.write().await = Some(tx.clone());

                                        // Request registration
                                        let _ = tx.send(ControlMessage::Register);

                                        // ── Outbound Sender Task ──
                                        let outbound = tokio::spawn(async move {
                                            while let Some(msg) = rx.recv().await {
                                                if let Ok(bytes) = msg.serialize() {
                                                    let len = bytes.len() as u32;
                                                    if control_send.write_u32_le(len).await.is_err()
                                                    {
                                                        break;
                                                    }
                                                    if control_send.write_all(&bytes).await.is_err()
                                                    {
                                                        break;
                                                    }
                                                }
                                            }
                                        });

                                        // ── Heartbeat Task ──
                                        let tx_ping = tx.clone();
                                        let heartbeat = tokio::spawn(async move {
                                            loop {
                                                tokio::time::sleep(
                                                    tokio::time::Duration::from_secs(30),
                                                )
                                                .await;
                                                if tx_ping.send(ControlMessage::Ping).is_err() {
                                                    break;
                                                }
                                            }
                                        });

                                        // ── Stream Acceptance Loop ──
                                        // The agent must accept incoming QUIC data streams from the server!
                                        let connection_clone = connection.clone();
                                        let state_clone = state.clone();
                                        let tx_clone = tx.clone();
                                        let inbound_streams = tokio::spawn(async move {
                                            while let Ok((send, mut recv)) =
                                                connection_clone.accept_bi().await
                                            {
                                                tracing::info!(
                                                    "Agent accepted a new bi QUIC stream!"
                                                );
                                                let mut prefix = [0u8; 17];
                                                if let Err(e) = recv.read_exact(&mut prefix).await {
                                                    tracing::error!(
                                                        "Agent failed to read prefix: {}",
                                                        e
                                                    );
                                                    continue;
                                                }
                                                if prefix[0] != 0x0A {
                                                    tracing::warn!(
                                                        "Agent received non-data stream: {}",
                                                        prefix[0]
                                                    );
                                                    continue; // Not a Data stream
                                                }

                                                let sess_bytes = &prefix[1..9];
                                                let strm_bytes = &prefix[9..17];

                                                // Strip trailing null bytes
                                                let sess_str = String::from_utf8(
                                                    sess_bytes
                                                        .iter()
                                                        .filter(|&&c| c != 0)
                                                        .cloned()
                                                        .collect(),
                                                )
                                                .unwrap_or_default();
                                                let strm_str = String::from_utf8(
                                                    strm_bytes
                                                        .iter()
                                                        .filter(|&&c| c != 0)
                                                        .cloned()
                                                        .collect(),
                                                )
                                                .unwrap_or_default();

                                                let at = state_clone.agent_tunnels.read().await;
                                                if let Some(info) = at.get(&sess_str).cloned() {
                                                    drop(at); // Drop before spawning
                                                    tracing::info!("Agent linking stream {} for session {} to {}:{}", strm_str, sess_str, info.remote_host, info.remote_port);
                                                    let addr = format!(
                                                        "{}:{}",
                                                        info.remote_host, info.remote_port
                                                    );
                                                    let tx2 = tx_clone.clone();
                                                    let st3 = state_clone.clone();

                                                    tokio::spawn(async move {
                                                        match tokio::net::TcpStream::connect(&addr)
                                                            .await
                                                        {
                                                            Ok(tcp_stream) => {
                                                                tracing::info!("Agent connected to local target {}", addr);
                                                                handle_stream_relay(
                                                                    tcp_stream,
                                                                    sess_str.clone(),
                                                                    strm_str.clone(),
                                                                    send,
                                                                    recv,
                                                                    tx2,
                                                                    st3,
                                                                )
                                                                .await;
                                                            }
                                                            Err(_) => {
                                                                let _ = tx2.send(
                                                                    ControlMessage::StreamClose {
                                                                        session_id: sess_str,
                                                                        stream_id: strm_str,
                                                                    },
                                                                );
                                                            }
                                                        }
                                                    });
                                                }
                                            }
                                        });

                                        // ── Inbound Message Loop ──
                                        while let Ok(l) = control_recv.read_u32_le().await {
                                            let len = l as usize;

                                            let mut buf = vec![0u8; len];
                                            if control_recv.read_exact(&mut buf).await.is_err() {
                                                break;
                                            }

                                            if let Ok(msg) = ControlMessage::deserialize(&buf) {
                                                handle_server_message(
                                                    &state,
                                                    &tx,
                                                    connection.clone(),
                                                    &app_handle,
                                                    msg,
                                                )
                                                .await;
                                            }
                                        }

                                        // Clean disconnect
                                        outbound.abort();
                                        heartbeat.abort();
                                        inbound_streams.abort();
                                    }
                                    Err(e) => error!("Failed to open control stream: {}", e),
                                }

                                *state.connected.write().await = false;
                                *state.ctrl_tx.write().await = None;
                                state.agent_tunnels.write().await.clear();
                                state.abort_all_tasks().await;
                                state.tunnels.write().await.clear();
                                let _ = app_handle.emit("tunnels-updated", ());
                                let _ = app_handle.emit("connection-status", false);
                                warn!("Disconnected from server");
                            }
                            Err(e) => error!("Connection failed: {}", e),
                        }
                    }
                    Err(e) => error!("QUIC Endpoint connect failed: {}", e),
                }
            }
            Err(e) => error!("Invalid server address {}: {}", server_url, e),
        }

        // Wait before attempting to reconnect
        info!("Reconnecting in {}s...", RECONNECT_DELAY_SECS);
        tokio::time::sleep(tokio::time::Duration::from_secs(RECONNECT_DELAY_SECS)).await;
    }
}

// ─── Server Message Handler ─────────────────────────────────────

/// Handles a single incoming ControlMessage from the relay server.
///
/// This is the central dispatch function for all server messages.
/// Each message type triggers different behavior depending on whether
/// this client is acting as an agent (receiving tunnels) or a controller
/// (initiating tunnels).
async fn handle_server_message(
    state: &Arc<AgentState>,
    tx: &mpsc::UnboundedSender<ControlMessage>,
    connection: quinn::Connection,
    app_handle: &tauri::AppHandle,
    msg: ControlMessage,
) {
    match msg {
        // ── Registration Confirmed with Server-Assigned ID ──
        ControlMessage::RegisterOk { agent_id } => {
            info!("Registered as agent: {}", agent_id);
            // Store the server-assigned agent ID
            *state.agent_id.write().await = agent_id.clone();
            let _ = app_handle.emit("registered", &agent_id);
        }

        // ── Agent Side: Incoming Tunnel Request ──
        // When another client wants to connect to us, the server asks
        // if we accept. We auto-accept all tunnel requests.
        ControlMessage::TunnelRequest {
            session_id,
            remote_host,
            remote_port,
        } => {
            info!(
                "Tunnel request: {} → {}:{}",
                session_id, remote_host, remote_port
            );

            // Auto-accept the tunnel request
            let _ = tx.send(ControlMessage::TunnelAccept {
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
        ControlMessage::TunnelReady { session_id } => {
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
                                        let stream_id = Uuid::new_v4().to_string()[..8].to_string();
                                        info!(
                                            "New stream {} from {} (tunnel {})",
                                            stream_id, peer, sid
                                        );

                                        let _quic_send = match connection.open_bi().await {
                                            Ok((tx, _rx)) => tx,
                                            Err(e) => {
                                                error!("Failed to open QUIC data stream: {}", e);
                                                break;
                                            }
                                        };

                                        let tx2 = tx_clone.clone();
                                        let st2 = state_clone.clone();
                                        let sid2 = sid.clone();

                                        // A new QUIC stream means we need to open it and then send
                                        // the `Data` protocol prefix so the server knows where to route it.
                                        let conn2 = connection.clone();
                                        tokio::spawn(async move {
                                            match conn2.open_bi().await {
                                                Ok((mut q_send, q_recv)) => {
                                                    // Tell the agent to open its TCP connection.
                                                    let _ = tx2.send(ControlMessage::StreamOpen {
                                                        session_id: sid2.clone(),
                                                        stream_id: stream_id.clone(),
                                                    });

                                                    // Send the prefix: 0x0A + 8 bytes session + 8 bytes stream
                                                    let mut prefix = vec![0x0A]; // TAG_DATA
                                                    let mut sess_bytes = [0u8; 8];
                                                    let s_bytes = sid2.as_bytes();
                                                    sess_bytes[..s_bytes.len().min(8)]
                                                        .copy_from_slice(
                                                            &s_bytes[..s_bytes.len().min(8)],
                                                        );

                                                    let mut strm_bytes = [0u8; 8];
                                                    let st_bytes = stream_id.as_bytes();
                                                    strm_bytes[..st_bytes.len().min(8)]
                                                        .copy_from_slice(
                                                            &st_bytes[..st_bytes.len().min(8)],
                                                        );

                                                    prefix.extend_from_slice(&sess_bytes);
                                                    prefix.extend_from_slice(&strm_bytes);
                                                    if q_send.write_all(&prefix).await.is_err() {
                                                        return;
                                                    }

                                                    handle_stream_relay(
                                                        tcp_stream, sid2, stream_id, q_send,
                                                        q_recv, tx2, st2,
                                                    )
                                                    .await;
                                                }
                                                Err(e) => {
                                                    error!("Failed to open QUIC bi-stream: {}", e)
                                                }
                                            }
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
        // The controller has a new TCP connection. The Server will map the stream and just send it to us.
        // We handle this exclusively in the incoming `accept_bi()` loop.
        ControlMessage::StreamOpen {
            session_id,
            stream_id,
        } => {
            info!(
                "StreamOpen: session={}, stream={} (Handled by inbound stream listener)",
                session_id, stream_id
            );
        }

        // ── Stream Closed by the Other Side ──
        // Remove the data channel so the relay task will stop naturally.
        ControlMessage::StreamClose {
            session_id: _,
            stream_id: _, // Keep stream_id in pattern for future use or remove completely if not needed
        } => {}

        // ── Tunnel Closed ──
        // Clean up all resources associated with this tunnel session.
        ControlMessage::TunnelClose { session_id } => {
            info!("Tunnel closed: {}", session_id);
            state.abort_session_tasks(&session_id).await;
            state.agent_tunnels.write().await.remove(&session_id);
            let mut tunnels = state.tunnels.write().await;
            tunnels.retain(|t| t.session_id != session_id);
            let _ = app_handle.emit("tunnels-updated", ());
        }

        // ── Error from Server ──
        ControlMessage::Error { message } => {
            error!("Server error: {}", message);
            let _ = app_handle.emit("server-error", &message);
        }

        // ── Heartbeat ──
        ControlMessage::Pong => {
            // No action needed; confirms the connection is alive
        }
        _ => {}
    }
}
