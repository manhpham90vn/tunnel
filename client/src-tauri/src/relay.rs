//! # TCP ↔ QUIC Stream Relay
//!
//! Handles the bidirectional relay of data between a local TCP connection
//! and a QUIC tunnel stream. Each TCP connection within a tunnel session
//! is represented as a stream with its own `stream_id`.
//!
//! ## Data Flow
//!
//! ```text
//! TCP App ←──TCP──→ [Relay Task] ←──QUIC Data Stream──→ Server ←──→ Other Side
//! ```
//!
//! The relay task manually copies data back and forth
//! between the TCP socket and the QUIC stream.

use crate::state::AgentState;
use quinn::{RecvStream, SendStream};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tunnel_protocol::ControlMessage;

/// Runs a bidirectional relay between a TCP stream and a QUIC stream.
pub async fn handle_stream_relay(
    tcp_stream: TcpStream,
    session_id: String,
    stream_id: String,
    mut quic_send: SendStream,
    mut quic_recv: RecvStream,
    ctrl_tx: mpsc::UnboundedSender<ControlMessage>,
    _state: Arc<AgentState>,
) {
    // We use tokio::io::copy_bidirectional to easily pipe data
    // between the TCP socket and the QUIC stream natively.

    // Note: copy_bidirectional requires AsyncRead + AsyncWrite
    // We can map SendStream and RecvStream into a unified Read/Write type
    // or just run two manual tokio::spawn loops. Let's do the loops
    // since SendStream and RecvStream are split types in Quinn.

    let (mut tcp_read, mut tcp_write) = tcp_stream.into_split();

    let stream_id_clone1 = stream_id.clone();
    // TCP -> QUIC
    let tcp_to_quic = tokio::spawn(async move {
        tracing::info!("Starting relay TCP->QUIC for stream {}", stream_id_clone1);
        match tokio::io::copy(&mut tcp_read, &mut quic_send).await {
            Ok(total) => {
                tracing::info!(
                    "Relay TCP->QUIC [{}] finished, {} bytes",
                    stream_id_clone1,
                    total
                );
            }
            Err(e) => {
                tracing::error!("TCP->QUIC [{}] error: {}", stream_id_clone1, e);
            }
        }
        let _ = quic_send.finish();
    });

    let stream_id_clone2 = stream_id.clone();
    // QUIC -> TCP
    let quic_to_tcp = tokio::spawn(async move {
        tracing::info!("Starting relay QUIC->TCP for stream {}", stream_id_clone2);
        match tokio::io::copy(&mut quic_recv, &mut tcp_write).await {
            Ok(total) => {
                tracing::info!(
                    "Relay QUIC->TCP [{}] finished, {} bytes",
                    stream_id_clone2,
                    total
                );
            }
            Err(e) => {
                tracing::error!("QUIC->TCP [{}] error: {}", stream_id_clone2, e);
            }
        }
        // Optionally shutdown TCP write half
    });

    // Wait for both to finish
    let _ = tokio::join!(tcp_to_quic, quic_to_tcp);

    // Notify the other side that this stream is closed
    let _ = ctrl_tx.send(ControlMessage::StreamClose {
        session_id,
        stream_id,
    });
}
