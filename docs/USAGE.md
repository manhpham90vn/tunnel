# Usage Guide

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

Install the relay server on a machine with a public IP. Ensure both **TCP port 7070** (for the REST API) and **UDP port 7070** (for the QUIC protocol) are open in your firewall.

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

### Custom CA Certificates (Production)

To connect securely in a production environment, you can instruct the client to verify the Relay Server's certificate against a custom CA. Set the `TUNNEL_CA_CERT` environment variable to the path of your PEM-encoded CA certificate file before starting the Tunnel Agent.

```bash
export TUNNEL_CA_CERT=/path/to/ca.pem
```

---

## Examples

### SSH

```bash
# Target Port: 22, Local Port: 2222
ssh -p 2222 user@localhost
```

### Web App

```bash
# Target Port: 3000, Local Port: 8080
# Open http://localhost:8080 in your browser
```

---

## Server API

| Endpoint      | Method | Description                        |
| ------------- | ------ | ---------------------------------- |
| `/api/agents` | GET    | List connected agents (JSON array) |
