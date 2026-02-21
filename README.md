# 🚇 Tunnel

Remote access between computers through a central relay server, similar to TeamViewer. The client acts as both an **Agent** (receives incoming tunnel requests) and a **Controller** (connects to remote agents).

## How It Works

```
┌─────────────────┐         ┌─────────────────┐         ┌─────────────────┐
│   Agent (PC A)  │◄──WS───►│  Relay Server   │◄──WS───►│ Controller (B)  │
│   Tauri App     │         │  Rust / Axum    │         │  Tauri App      │
└────────┬────────┘         └─────────────────┘         └────────┬────────┘
         │                                                       │
    TCP to local                                            TCP listener
    services                                                on local port
```

1. **Agent** registers with the relay server and receives a unique Agent ID (e.g., `A3F8-B2C1`)
2. **Controller** enters the Agent ID, specifies target and local ports → creates a tunnel
3. Data is relayed: `Controller local port` ↔ `WebSocket` ↔ `Agent target service`
4. Agent auto-reconnects every 3 seconds if disconnected, with 30-second heartbeat keep-alive

---

## Installation

### Client (Tunnel Agent)

Download the installer for your OS from [GitHub Releases](../../releases/latest):

| OS                        | File                                |
| ------------------------- | ----------------------------------- |
| **macOS** (Universal)     | `Tunnel Agent_x.x.x_universal.dmg`  |
| **Linux** (Debian/Ubuntu) | `tunnel-agent_x.x.x_amd64.deb`      |
| **Linux** (Other)         | `tunnel-agent_x.x.x_amd64.AppImage` |
| **Windows**               | `Tunnel Agent_x.x.x_x64-setup.exe`  |

#### macOS

```bash
open Tunnel\ Agent_*.dmg
# Drag "Tunnel Agent" to Applications
```

#### Linux (Debian/Ubuntu)

```bash
sudo dpkg -i tunnel-agent_*_amd64.deb
```

#### Linux (AppImage)

```bash
chmod +x tunnel-agent_*_amd64.AppImage
./tunnel-agent_*_amd64.AppImage
```

#### Windows

Run `Tunnel Agent_x.x.x_x64-setup.exe` and follow the installer.

---

### Server (Relay Server)

The relay server forwards data between Agents and Controllers. Install it on a machine with a public IP address.

Download `tunnel-server_x.x.x_amd64.deb` from [GitHub Releases](../../releases/latest):

```bash
# Install (systemd service is enabled automatically)
sudo dpkg -i tunnel-server_*_amd64.deb

# Check status
sudo systemctl status tunnel-server

# View logs
sudo journalctl -u tunnel-server -f
```

The server listens on `0.0.0.0:7070` by default. Log level can be configured via the `RUST_LOG` environment variable.

#### Uninstall

```bash
sudo systemctl stop tunnel-server
sudo dpkg -r tunnel-server
```

---

## Usage

### 1. Set Up the Server

Install the relay server on a machine with a public IP. Ensure port **7070** is open in the firewall.

### 2. Connect the Agent

1. Open **Tunnel Agent** on the machine you want to access remotely
2. In **Server Settings**, enter the server IP and port (default: `7070`), then click **Save**
3. The app auto-connects and displays your **Agent ID** — share this ID with the Controller

### 3. Create a Tunnel (Controller)

1. Open **Tunnel Agent** on your local machine
2. In **Connect to Agent**, enter the target's **Agent ID**
3. Set **Target Port** (the port on the agent's machine, e.g., `22` for SSH)
4. Set **Local Port** (the port on your machine to access through, e.g., `2222`)
5. Click **Connect**
6. Access the remote service via `localhost:<local_port>`

### Example: SSH

```bash
# Target Port: 22, Local Port: 2222
ssh -p 2222 user@localhost
```

### Example: Web App

```bash
# Target Port: 3000, Local Port: 8080
# Open http://localhost:8080 in your browser
```

---

## Server API

| Endpoint      | Method | Description                                |
| ------------- | ------ | ------------------------------------------ |
| `/ws`         | GET    | WebSocket upgrade (agents and controllers) |
| `/api/agents` | GET    | List connected agents (JSON array)         |

---

## Development

### Prerequisites

- Rust (stable)
- Node.js 20+
- Linux system libraries: `libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`, `patchelf`

### Run Locally

```bash
# Start the relay server (listens on 0.0.0.0:7070)
cd server && cargo run

# Start the client in dev mode
cd client && npm run tauri dev
```

### Build

```bash
# Server .deb package
cd server && cargo deb

# Client desktop app
cd client && npx tauri build
```

### Lint

```bash
# Server
cd server && cargo fmt --check && cargo clippy -- -D warnings

# Client
cd client/src-tauri && cargo fmt --check && cargo clippy -- -D warnings
```

---

## Tech Stack

| Component           | Technology                   |
| ------------------- | ---------------------------- |
| **Server**          | Rust, Axum, Tokio, WebSocket |
| **Client Backend**  | Rust, Tauri v2, Tokio        |
| **Client Frontend** | React, TypeScript, Vite      |
| **Protocol**        | WebSocket + JSON + base64    |

## License

MIT
