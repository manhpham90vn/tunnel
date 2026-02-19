import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

interface AgentStatus {
  agent_id: string;
  connected: boolean;
  server_url: string;
}

interface TunnelInfo {
  session_id: string;
  remote_host: string;
  remote_port: number;
  local_port: number;
  direction: string;
  status: string;
}

function App() {
  const [agentInfo, setAgentInfo] = useState<AgentStatus | null>(null);
  const [connected, setConnected] = useState(false);
  const [tunnels, setTunnels] = useState<TunnelInfo[]>([]);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Connect form state
  const [targetId, setTargetId] = useState("");
  const [remoteHost, setRemoteHost] = useState("127.0.0.1");
  const [remotePort, setRemotePort] = useState("22");
  const [localPort, setLocalPort] = useState("2222");
  const [connecting, setConnecting] = useState(false);

  // Fetch initial agent info
  useEffect(() => {
    invoke<AgentStatus>("get_agent_info").then((info) => {
      setAgentInfo(info);
      setConnected(info.connected);
    });
  }, []);

  // Listen for events from Rust backend
  useEffect(() => {
    const unlisteners: (() => void)[] = [];

    listen<boolean>("connection-status", (event) => {
      setConnected(event.payload);
    }).then((u) => unlisteners.push(u));

    listen("tunnels-updated", () => {
      invoke<TunnelInfo[]>("get_tunnels").then(setTunnels);
    }).then((u) => unlisteners.push(u));

    listen<string>("server-error", (event) => {
      setError(event.payload);
      setTimeout(() => setError(null), 5000);
    }).then((u) => unlisteners.push(u));

    return () => {
      unlisteners.forEach((u) => u());
    };
  }, []);

  const copyAgentId = useCallback(() => {
    if (agentInfo) {
      navigator.clipboard.writeText(agentInfo.agent_id);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  }, [agentInfo]);

  const handleConnect = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!targetId.trim()) return;

    setConnecting(true);
    try {
      await invoke("connect_to_agent", {
        targetId: targetId.trim(),
        remoteHost,
        remotePort: parseInt(remotePort),
        localPort: parseInt(localPort),
      });
      setTargetId("");
    } catch (err) {
      setError(String(err));
      setTimeout(() => setError(null), 5000);
    }
    setConnecting(false);
  };

  const handleDisconnect = async (sessionId: string) => {
    try {
      await invoke("disconnect_tunnel", { sessionId });
    } catch (err) {
      setError(String(err));
      setTimeout(() => setError(null), 5000);
    }
  };

  return (
    <div className="app">
      {/* Header */}
      <div className="header">
        <div className="header-icon">🚇</div>
        <div className="header-text">
          <h1>Tunnel Agent</h1>
          <p>Secure remote access</p>
        </div>
      </div>

      {/* Agent Info Card */}
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

      {/* Connect Card */}
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
              <label>Remote Host</label>
              <input
                type="text"
                placeholder="127.0.0.1"
                value={remoteHost}
                onChange={(e) => setRemoteHost(e.target.value)}
              />
            </div>
            <div className="input-group">
              <label>Remote Port</label>
              <input
                type="number"
                placeholder="22"
                value={remotePort}
                onChange={(e) => setRemotePort(e.target.value)}
              />
            </div>
          </div>
          <div className="input-group">
            <label>Local Port (listen on)</label>
            <input
              type="number"
              placeholder="2222"
              value={localPort}
              onChange={(e) => setLocalPort(e.target.value)}
            />
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

      {/* Active Tunnels Card */}
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

      {/* Error Toast */}
      {error && <div className="error-toast">⚠ {error}</div>}
    </div>
  );
}

export default App;
