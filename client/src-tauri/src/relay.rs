//! # TCP ↔ WebSocket Stream Relay
//!
//! Handles the bidirectional relay of data between a local TCP connection
//! and a WebSocket tunnel. Each TCP connection within a tunnel session
//! is represented as a "stream" with its own `stream_id`.
//!
//! ## Data Flow
//!
//! ```text
//! TCP App ←──TCP──→ [Relay Task] ←──WS (base64 JSON)──→ Server ←──→ Other Side
//! ```
//!
//! The relay task has two concurrent sub-tasks:
//! 1. **TCP → WebSocket**: Reads bytes from TCP, base64-encodes them,
//!    and sends them as `Data` messages over WebSocket.
//! 2. **WebSocket → TCP**: Receives decoded bytes from a data channel
//!    and writes them to the TCP socket.

use crate::protocol::WsMessage;
use crate::state::AgentState;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

/// Runs a bidirectional relay between a TCP stream and a WebSocket tunnel.
///
/// ## Parameters
/// - `tcp_stream`: The local TCP connection to relay data for
/// - `session_id`: The tunnel session this stream belongs to
/// - `stream_id`: Unique identifier for this specific TCP connection
/// - `channel_key`: Key in the data_channels map for receiving WebSocket data
/// - `role`: "agent" or "controller" — identifies who is sending data
/// - `ws_tx`: Channel to send outbound WebSocket messages
/// - `state`: Shared agent state (for cleanup)
/// - `data_rx`: Receiver for incoming data from the WebSocket side
///
/// ## Lifecycle
/// - Runs until either the TCP connection closes or the WebSocket
///   data channel is dropped
/// - On exit: removes the data channel entry and sends a `StreamClose`
///   message to notify the other side
#[allow(clippy::too_many_arguments)]
pub async fn handle_stream_relay(
    tcp_stream: TcpStream,
    session_id: String,
    stream_id: String,
    channel_key: String,
    role: String,
    ws_tx: mpsc::UnboundedSender<WsMessage>,
    state: Arc<AgentState>,
    mut data_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    // Split the TCP stream into independent read and write halves
    let (mut tcp_read, mut tcp_write) = tcp_stream.into_split();

    // Clone identifiers for use in the spawned sub-tasks
    let sid = session_id.clone();
    let stid = stream_id.clone();
    let my_role = role.clone();

    // ── Sub-task 1: TCP → WebSocket ──
    // Reads raw bytes from TCP, encodes them as base64, and sends
    // them as Data messages through the WebSocket tunnel.
    let ws_tx_clone = ws_tx.clone();
    let tcp_to_ws = tokio::spawn(async move {
        let mut buf = vec![0u8; 8192]; // 8KB read buffer
        loop {
            match tcp_read.read(&mut buf).await {
                Ok(0) => break, // TCP connection closed gracefully
                Ok(n) => {
                    // Base64-encode the raw bytes for JSON transport
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
                        break; // WebSocket channel closed
                    }
                }
                Err(_) => break, // TCP read error
            }
        }
    });

    // ── Sub-task 2: WebSocket → TCP ──
    // Receives already-decoded bytes from the data channel (populated
    // by `handle_server_message` when Data messages arrive) and writes
    // them to the TCP socket.
    let ws_to_tcp = tokio::spawn(async move {
        while let Some(data) = data_rx.recv().await {
            if tcp_write.write_all(&data).await.is_err() {
                break; // TCP write error
            }
        }
    });

    // Wait for either sub-task to finish (the other will be dropped)
    tokio::select! {
        _ = tcp_to_ws => {},
        _ = ws_to_tcp => {},
    }

    // ── Cleanup ──
    // Remove the data channel entry so no more data is buffered
    state.data_channels.write().await.remove(&channel_key);

    // Notify the other side that this stream has closed
    let _ = ws_tx.send(WsMessage::StreamClose {
        session_id,
        stream_id,
    });
}
