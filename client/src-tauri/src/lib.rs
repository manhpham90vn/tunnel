//! # Tunnel Client — Tauri Application
//!
//! A desktop application built with Tauri v2 that acts as both:
//! - **Agent**: Registers with the relay server and accepts incoming tunnels
//! - **Controller**: Connects to remote agents and forwards local TCP ports
//!
//! ## Module Organization
//!
//! - [`protocol`]  — WebSocket message types (must stay in sync with server)
//! - [`state`]     — Application state (agent ID, tunnels, data channels)
//! - [`commands`]  — Tauri IPC commands exposed to the React frontend
//! - [`agent`]     — WebSocket connection loop and message handling
//! - [`relay`]     — Per-stream TCP ↔ WebSocket bidirectional relay

mod agent;
mod commands;
mod protocol;
mod relay;
mod state;

use state::AgentState;
use std::sync::Arc;

/// Application entry point.
///
/// Sets up logging, creates the shared agent state, registers Tauri commands,
/// and spawns the background WebSocket connection loop.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize structured logging to stderr (visible in the terminal
    // when running `tauri dev`)
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // Create the shared agent state with a fresh agent ID
    let agent_state = Arc::new(AgentState::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // Make the agent state available to all Tauri commands via dependency injection
        .manage(agent_state.clone())
        // Register the commands that the React frontend can call
        .invoke_handler(tauri::generate_handler![
            commands::get_agent_info,
            commands::set_server_url,
            commands::connect_to_agent,
            commands::disconnect_tunnel,
            commands::get_tunnels,
        ])
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let state = agent_state.clone();

            // Spawn the WebSocket connection loop on a dedicated OS thread
            // with its own Tokio runtime. This keeps the agent loop isolated
            // from Tauri's main thread and event loop.
            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new()
                    .expect("Failed to create Tokio runtime");
                rt.block_on(async move {
                    agent::run_agent_loop(state, app_handle).await;
                });
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
