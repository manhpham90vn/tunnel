/**
 * App.tsx — Main Application Component
 *
 * This is the root React component for the Tunnel Agent desktop application.
 * It provides the full UI for:
 * - Configuring the relay server URL
 * - Displaying the agent's identity and connection status
 * - Connecting to remote agents by entering their Agent ID
 * - Viewing and managing active tunnel sessions
 *
 * Communication with the Rust backend happens via:
 * - `invoke()` — calls Tauri commands (get_agent_info, connect_to_agent, etc.)
 * - `listen()` — subscribes to events emitted by the Rust backend
 */

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

// ─── TypeScript Interfaces ──────────────────────────────────────
// These mirror the Rust structs returned by Tauri commands.

/** Agent connection status, returned by the `get_agent_info` command. */
interface AgentStatus {
  agent_id: string;
  connected: boolean;
  server_url: string;
}

/** Information about a single tunnel session, returned by `get_tunnels`. */
interface TunnelInfo {
  session_id: string;
  remote_host: string;
  remote_port: number;
  local_port: number;
  direction: string; // "incoming" or "outgoing"
  status: string;    // "connecting", "active", or "error"
}

// ─── Main Component ─────────────────────────────────────────────

function App() {
  // ── State ──
  const [agentInfo, setAgentInfo] = useState<AgentStatus | null>(null);
  const [connected, setConnected] = useState(false);
  const [tunnels, setTunnels] = useState<TunnelInfo[]>([]);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Server settings (IP + port, constructs ws:// URL automatically)
  const [serverIp, setServerIp] = useState("127.0.0.1");
  const [serverPort, setServerPort] = useState("7070");
  const [serverUrlSaved, setServerUrlSaved] = useState(false);

  // Connect form fields
  const [targetId, setTargetId] = useState("");
  const [remotePort, setRemotePort] = useState("22");
  const [localPort, setLocalPort] = useState("2222");
  const [connecting, setConnecting] = useState(false);

  // ── Fetch initial agent info on mount ──
  useEffect(() => {
    invoke<AgentStatus>("get_agent_info").then((info) => {
      setAgentInfo(info);
      setConnected(info.connected);
      // Parse IP and port from the stored server URL
      try {
        const parts = info.server_url.split(':');
        setServerIp(parts[0] || "127.0.0.1");
        setServerPort(parts[1] || "7070");
      } catch {
        // Keep defaults if parsing fails
      }
    });
  }, []);

  // ── Subscribe to backend events ──
  // The Rust backend emits events when the connection status changes,
  // tunnels are updated, or errors occur.
  useEffect(() => {
    const unlisteners: (() => void)[] = [];

    // Connection status changes (connected/disconnected from server)
    listen<boolean>("connection-status", (event) => {
      setConnected(event.payload);
    }).then((u) => unlisteners.push(u));

    // Server assigned an Agent ID after registration
    listen<string>("registered", (event) => {
      setAgentInfo((prev) =>
        prev ? { ...prev, agent_id: event.payload } : prev
      );
    }).then((u) => unlisteners.push(u));

    // Tunnel list changed — re-fetch the full list from the backend
    listen("tunnels-updated", () => {
      invoke<TunnelInfo[]>("get_tunnels").then(setTunnels);
    }).then((u) => unlisteners.push(u));

    // Error notifications from the backend (displayed as a toast)
    listen<string>("server-error", (event) => {
      setError(event.payload);
      setTimeout(() => setError(null), 5000);
    }).then((u) => unlisteners.push(u));

    // Cleanup all event listeners on unmount
    return () => {
      unlisteners.forEach((u) => u());
    };
  }, []);

  // ── Copy Agent ID to clipboard ──
  const copyAgentId = useCallback(() => {
    if (agentInfo) {
      navigator.clipboard.writeText(agentInfo.agent_id);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  }, [agentInfo]);

  // ── Save server URL to backend ──
  const handleSaveServerUrl = async () => {
    const url = `${serverIp.trim()}:${serverPort.trim()}`;
    try {
      await invoke("set_server_url", { url });
      setServerUrlSaved(true);
      setTimeout(() => setServerUrlSaved(false), 2000);
    } catch (err) {
      setError(String(err));
      setTimeout(() => setError(null), 5000);
    }
  };

  // ── Handle tunnel connection form submission ──
  const handleConnect = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!targetId.trim()) return;

    setConnecting(true);
    try {
      await invoke("connect_to_agent", {
        targetId: targetId.trim(),
        remoteHost: "127.0.0.1",
        remotePort: parseInt(remotePort),
        localPort: parseInt(localPort),
      });
      setTargetId(""); // Clear the input on success
    } catch (err) {
      setError(String(err));
      setTimeout(() => setError(null), 5000);
    }
    setConnecting(false);
  };

  // ── Handle tunnel disconnect ──
  const handleDisconnect = async (sessionId: string) => {
    try {
      await invoke("disconnect_tunnel", { sessionId });
    } catch (err) {
      setError(String(err));
      setTimeout(() => setError(null), 5000);
    }
  };

  // ── Render ──
  return (
    <div className="app">
      {/* Header with app branding */}
      <div className="header">
        <div className="header-icon">🚇</div>
        <div className="header-text">
          <h1>Tunnel Agent</h1>
          <p>Secure remote access</p>
        </div>
      </div>

      {/* Server Settings Card — configure relay server */}
      <div className="card">
        <div className="card-title">Server Settings</div>
        <div className="server-url-row">
          <div className="input-group" style={{ flex: 2 }}>
            <label>Server IP</label>
            <input
              type="text"
              placeholder="1.2.3.4"
              value={serverIp}
              onChange={(e) => setServerIp(e.target.value)}
            />
          </div>
          <div className="input-group" style={{ flex: 1 }}>
            <label>Port</label>
            <input
              type="text"
              placeholder="7070"
              value={serverPort}
              onChange={(e) => setServerPort(e.target.value)}
            />
          </div>
          <button
            className={`save-btn ${serverUrlSaved ? "saved" : ""}`}
            onClick={handleSaveServerUrl}
            style={{ alignSelf: "flex-end", marginBottom: "1px" }}
          >
            {serverUrlSaved ? "✓ Saved" : "Save"}
          </button>
        </div>
        <span className="input-hint">
          Changes take effect on next reconnect (every 3s)
        </span>
      </div>

      {/* Agent Info Card — shows agent ID and connection status */}
      <div className="card">
        <div className="card-title">Your Agent</div>
        <div className="agent-info">
          <div className="agent-id-section">
            <span className="agent-id-label">Agent ID</span>
            <div className="agent-id-row">
              <span className="agent-id">
                {agentInfo?.agent_id || "---"}
              </span>
              <button
                className={`copy-btn ${copied ? "copied" : ""}`}
                onClick={copyAgentId}
              >
                {copied ? "✓ Copied" : "Copy"}
              </button>
            </div>
          </div>
          <div
            className={`status-badge ${connected ? "connected" : "disconnected"}`}
          >
            <span
              className={`status-dot ${connected ? "connected" : "disconnected"}`}
            />
            {connected ? "Online" : "Offline"}
          </div>
        </div>
      </div>

      {/* Connect Card — form to initiate a tunnel to a remote agent */}
      <div className="card">
        <div className="card-title">Connect to Agent</div>
        <form className="connect-form" onSubmit={handleConnect}>
          <div className="input-group">
            <label>Target Agent ID</label>
            <input
              type="text"
              placeholder="XXXX-XXXX"
              value={targetId}
              onChange={(e) => setTargetId(e.target.value)}
            />
          </div>
          <div className="input-row">
            <div className="input-group">
              <label>Target Port (on agent's machine)</label>
              <input
                type="number"
                placeholder="22"
                value={remotePort}
                onChange={(e) => setRemotePort(e.target.value)}
              />
              <span className="input-hint">
                e.g. 22 (SSH), 3000 (web)
              </span>
            </div>
            <div className="input-group">
              <label>Local Port (on your machine)</label>
              <input
                type="number"
                placeholder="2222"
                value={localPort}
                onChange={(e) => setLocalPort(e.target.value)}
              />
            </div>
          </div>
          <button
            type="submit"
            className="connect-btn"
            disabled={!connected || connecting || !targetId.trim()}
          >
            {connecting ? "Connecting..." : "🔗 Connect"}
          </button>
        </form>
      </div>

      {/* Active Tunnels Card — list of all active tunnel sessions */}
      <div className="card">
        <div className="card-title">
          Active Tunnels ({tunnels.length})
        </div>
        {tunnels.length === 0 ? (
          <div className="tunnels-empty">No active tunnels</div>
        ) : (
          tunnels.map((tunnel) => (
            <div className="tunnel-item" key={tunnel.session_id}>
              <div className="tunnel-info">
                <span className="tunnel-session">
                  {tunnel.session_id}
                </span>
                <span className="tunnel-details">
                  {tunnel.direction === "outgoing"
                    ? `localhost:${tunnel.local_port} → ${tunnel.remote_host}:${tunnel.remote_port}`
                    : `${tunnel.remote_host}:${tunnel.remote_port}`}
                </span>
              </div>
              <div className="tunnel-meta">
                <span
                  className={`tunnel-direction ${tunnel.direction}`}
                >
                  {tunnel.direction === "incoming" ? "↓ IN" : "↑ OUT"}
                </span>
                <span className={`tunnel-status ${tunnel.status}`}>
                  {tunnel.status}
                </span>
                <button
                  className="disconnect-btn"
                  onClick={() => handleDisconnect(tunnel.session_id)}
                >
                  Disconnect
                </button>
              </div>
            </div>
          ))
        )}
      </div>

      {/* Error Toast — auto-dismissing error notification */}
      {error && <div className="error-toast">⚠ {error}</div>}
    </div>
  );
}

export default App;
