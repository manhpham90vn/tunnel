//! # REST API Endpoints
//!
//! Provides HTTP API endpoints for querying server state.
//! Currently only exposes a list of connected agents.

use crate::state::AppState;
use axum::{extract::State, Json};
use serde::Serialize;

/// Response item representing a single connected agent.
#[derive(Serialize)]
pub struct AgentListItem {
    /// The agent's unique identifier (e.g., "A3F8-B2C1").
    pub agent_id: String,
}

/// `GET /api/agents` — Returns a JSON array of all currently connected agents.
///
/// This endpoint can be used by external tools or dashboards to discover
/// which agents are online and available for tunnel connections.
pub async fn list_agents(State(state): State<AppState>) -> Json<Vec<AgentListItem>> {
    let agents: Vec<AgentListItem> = state
        .agents
        .iter()
        .map(|entry| AgentListItem {
            agent_id: entry.key().clone(),
        })
        .collect();
    Json(agents)
}
