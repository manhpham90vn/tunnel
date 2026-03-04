# Tunnel

Remote access between computers through a central relay server. The client acts as both an **Agent** (receives incoming tunnel requests) and a **Controller** (connects to remote agents).

```
┌─────────────────┐          ┌─────────────────┐          ┌─────────────────┐
│   Agent (PC A)  │◄──QUIC──►│  Relay Server   │◄──QUIC──►│ Controller (B)  │
│   Tauri App     │          │  Rust / Axum    │          │  Tauri App      │
└────────┬────────┘          └─────────────────┘          └────────┬────────┘
         │                                                         │
    TCP to local                                              TCP listener
    services                                                  on local port
```

## The Problem

When you need to access services on a remote machine (not on the same LAN), you often run into these issues:

| Problem                             | Traditional Solution             | Limitations                                        |
| ----------------------------------- | -------------------------------- | -------------------------------------------------- |
| No public IP                        | VPN, Port forwarding, Reverse proxy | Requires router config, not supported on all networks |
| NAT/Firewall blocking              | DMZ, UPnP                       | Insecure, not supported on all devices            |
| Don't want to expose services       | TeamViewer, AnyDesk             | Resource-heavy, slow, depends on third-party       |
| Need to access multiple machines   | Manual SSH tunnels              | Must configure each machine, hard to manage       |

## Solution

Tunnel creates TCP tunnel connections through a central relay server:

- **Agent** registers with the server and receives a unique ID (e.g., `A3F8-B2C1`)
- **Controller** enters Agent ID + target port → creates tunnel
- Data is relayed: `localhost:local_port` ↔ `QUIC` ↔ `Agent:target_port`

### Advantages

- No router/firewall configuration needed
- No third-party dependency (self-host the relay server)
- QUIC for low-latency with congestion control
- Lightweight — just run the app on both machines

## Use Cases

### Remote SSH

Access remote servers via SSH without public IP or VPN.

```
# Agent: server machine with SSH (port 22)
# Controller: local machine
# → Create tunnel: Target Port 22, Local Port 2222

ssh -p 2222 user@localhost
```

### Remote Desktop (RDP/VNC)

Control Windows/Mac/Linux machines remotely via remote desktop.

```
# Agent: target machine (RDP port 3389 or VNC port 5900)
# Controller: local machine
# → Create tunnel: Target Port 3389, Local Port 3389

# Windows Remote Desktop: connect to localhost:3389
```

### Local Web Server

Share a dev webapp with others without deploying.

```
# Agent: dev machine running web server (port 3000)
# Controller: reviewer's machine
# → Create tunnel: Target Port 3000, Local Port 8080

# Open browser: http://localhost:8080
```

### Database Access

Access databases running on other machines from anywhere.

```
# Agent: machine running PostgreSQL (port 5432)
# Controller: dev laptop
# → Create tunnel: Target Port 5432, Local Port 5432

psql -h localhost -p 5432 -U myuser mydb
```

### IoT / Device Management

Manage IoT devices (Raspberry Pi, smart home hub) behind NAT.

```
# Agent: Raspberry Pi with SSH (port 22) or Home Assistant (port 8123)
# Controller: laptop
# → Create tunnel: Target Port 8123, Local Port 8123

# Open browser: http://localhost:8123
```

### Internal Service Access

Access internal tools (Grafana, Prometheus, admin dashboard) without exposing to the internet.

```
# Agent: machine running Grafana (port 3000)
# Controller: dev machine
# → Create tunnel: Target Port 3000, Local Port 9090

# Open browser: http://localhost:9090
```

## Documentation

- [Usage Guide](docs/USAGE.md) — Installation, configuration, usage examples
- [Development Guide](docs/DEVELOPMENT.md) — Local dev, build, lint, CI/CD
- [Architecture](docs/ARCHITECTURE.md) — Protocol, data flow, components

## Tech Stack

| Component           | Technology                      |
| ------------------- | ------------------------------- |
| **Server**          | Rust, Axum, Tokio, Quinn (QUIC) |
| **Client Backend**  | Rust, Tauri v2, Tokio, Quinn    |
| **Client Frontend** | React, TypeScript, Vite         |
| **Protocol**        | QUIC + bincode binary           |

## License

MIT
