//! # Tunnel Relay Server
//!
//! A WebSocket-based relay server that enables TCP port forwarding between
//! remote machines. It acts as a central hub connecting **agents** (machines
//! exposing services) with **controllers** (machines requesting access).
//!
//! ## Architecture
//!
//! ```text
//! Controller ──WS──► Relay Server ──WS──► Agent ──TCP──► Local Service
//! ```
//!
//! ## Modules
//!
//! - [`protocol`] — WebSocket message types (JSON-serialized)
//! - [`state`]    — Shared application state (agent/session registries)
//! - [`handlers`] — WebSocket connection lifecycle and message dispatch
//! - [`api`]      — REST API endpoints

mod api;
mod handlers;
mod protocol;
mod state;

use axum::{routing::get, Router};
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;
use tracing::info;

use crate::state::AppState;

/// Server entry point.
///
/// Initializes logging, creates the shared state, configures routes,
/// and starts listening for incoming connections on port 7070.
#[tokio::main]
async fn main() {
    // Initialize structured logging with env-filter support.
    // Default log level is `info` for the tunnel_server crate.
    // Override with the `RUST_LOG` environment variable.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tunnel_server=info".into()),
        )
        .init();

    // Create the shared application state (agent/connection/session registries)
    let state = AppState::new();

    // Build the Axum router with WebSocket and REST endpoints
    let app = Router::new()
        .route("/ws", get(handlers::ws_handler))         // WebSocket upgrade
        .route("/api/agents", get(api::list_agents))      // REST: list agents
        .layer(CorsLayer::permissive())                   // Allow all CORS origins
        .with_state(state);

    // Bind to all interfaces on port 7070
    let addr = SocketAddr::from(([0, 0, 0, 0], 7070));
    info!("🚇 Tunnel Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
