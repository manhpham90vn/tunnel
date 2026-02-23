//! # Tunnel Relay Server
//!
//! A QUIC-based relay server that enables TCP port forwarding between
//! remote machines. It acts as a central hub connecting **agents** (machines
//! exposing services) with **controllers** (machines requesting access).
//!
//! ## Architecture
//!
//! ```text
//! Controller ──QUIC──► Relay Server ──QUIC──► Agent ──TCP──► Local Service
//! ```
//!
//! ## Modules
//!
//! - [`protocol`] — QUIC message types (binary bincode-serialized)
//! - [`state`]    — Shared application state (agent/session registries)
//! - [`handlers`] — WebSocket connection lifecycle and message dispatch
//! - [`api`]      — REST API endpoints

mod api;
mod cert;
mod handlers;
mod state;

use crate::state::AppState;

/// Server entry point.
///
/// Initializes logging, creates the shared state, configures routes,
/// and starts listening for incoming HTTP connections on TCP 7070
/// and QUIC connections on UDP 7070.
#[tokio::main]
async fn main() {
    // Install default crypto provider for rustls
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tunnel_server=info".into()),
        )
        .init();

    let state = AppState::new();

    // ── HTTP API (Axum) ──
    let app = axum::Router::new()
        .route("/api/agents", axum::routing::get(api::list_agents))
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state.clone());

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 7070));
    let tcp_listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    tracing::info!("🚇 Tunnel Server (HTTP API) listening on TCP {}", addr);
    tokio::spawn(async move {
        axum::serve(tcp_listener, app).await.unwrap();
    });

    // ── QUIC Protocol (Quinn) ──
    let (server_config, _cert) =
        cert::generate_self_signed_cert().expect("Failed to generate TLS cert");
    let mut transport_config = quinn::TransportConfig::default();
    transport_config.max_concurrent_bidi_streams(1024u32.into());
    transport_config.max_concurrent_uni_streams(1024u32.into());

    let mut quinn_config = quinn::ServerConfig::with_crypto(std::sync::Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(server_config)
            .expect("Failed to create QUIC config"),
    ));
    quinn_config.transport_config(std::sync::Arc::new(transport_config));
    let endpoint = quinn::Endpoint::server(quinn_config, addr).unwrap();

    tracing::info!(
        "🚇 Tunnel Server (QUIC) listening on UDP {}",
        endpoint.local_addr().unwrap()
    );

    while let Some(incoming) = endpoint.accept().await {
        let state_clone = state.clone();
        tokio::spawn(async move {
            match incoming.await {
                Ok(connection) => {
                    handlers::handle_connection(connection, state_clone).await;
                }
                Err(e) => {
                    tracing::error!("Failed to complete QUIC connection: {}", e);
                }
            }
        });
    }
}
