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
        let mut buf = [0u8; 8192];
        let mut total = 0;
        loop {
            match tokio::io::AsyncReadExt::read(&mut tcp_read, &mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    tracing::info!("TCP->QUIC [{}]: read {} bytes", stream_id_clone1, n);
                    if let Err(e) =
                        tokio::io::AsyncWriteExt::write_all(&mut quic_send, &buf[..n]).await
                    {
                        tracing::error!("TCP->QUIC [{}] write error: {}", stream_id_clone1, e);
                        break;
                    }
                    total += n;
                }
                Err(e) => {
                    tracing::error!("TCP->QUIC [{}] read error: {}", stream_id_clone1, e);
                    break;
                }
            }
        }
        tracing::info!(
            "Relay TCP->QUIC [{}] finished, {} bytes",
            stream_id_clone1,
            total
        );
        let _ = quic_send.finish();
    });

    let stream_id_clone2 = stream_id.clone();
    // QUIC -> TCP
    let quic_to_tcp = tokio::spawn(async move {
        tracing::info!("Starting relay QUIC->TCP for stream {}", stream_id_clone2);
        let mut buf = [0u8; 8192];
        let mut total = 0;
        loop {
            match tokio::io::AsyncReadExt::read(&mut quic_recv, &mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    tracing::info!("QUIC->TCP [{}]: read {} bytes", stream_id_clone2, n);
                    if let Err(e) =
                        tokio::io::AsyncWriteExt::write_all(&mut tcp_write, &buf[..n]).await
                    {
                        tracing::error!("QUIC->TCP [{}] write error: {}", stream_id_clone2, e);
                        break;
                    }
                    total += n;
                }
                Err(e) => {
                    tracing::error!("QUIC->TCP [{}] read error: {}", stream_id_clone2, e);
                    break;
                }
            }
        }
        tracing::info!(
            "Relay QUIC->TCP [{}] finished, {} bytes",
            stream_id_clone2,
            total
        );
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
