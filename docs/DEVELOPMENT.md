# Development Guide

## Requirements

- Rust (stable)
- Node.js 20+
- Linux system libraries: `libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`, `patchelf`

## Project Structure

```
tunnel/
├── server/              # Relay server (Rust/Axum/Quinn)
├── client/              # Desktop app (Tauri v2)
│   ├── src/             # Frontend (React/TypeScript/Vite)
│   └── src-tauri/       # Backend (Rust/Tauri)
├── tunnel-protocol/     # Shared protocol library (Rust)
└── .github/workflows/   # CI/CD
```

## Run Locally

```bash
# Start the relay server (HTTP 0.0.0.0:7070 + UDP 0.0.0.0:7070)
cd server && cargo run

# Start the client in dev mode
cd client && npm run tauri dev
```

The server auto-generates a self-signed certificate for local dev. The client skips certificate validation in dev mode by default.

## Build

```bash
# Server .deb package
cd server && cargo deb

# Client desktop app
cd client && npx tauri build
```

## Lint

```bash
# Server
cd server && cargo fmt --check && cargo clippy -- -D warnings

# Client backend
cd client/src-tauri && cargo fmt --check && cargo clippy -- -D warnings

# Client frontend
cd client && npm run build
```

## CI/CD

### Pull Request (`check.yml`)

Runs automatically on PRs to `main`:
- Server: `cargo fmt` → `cargo clippy` → `cargo check`
- Protocol: `cargo fmt` → `cargo clippy`
- Client: `cargo fmt` → `cargo clippy` → `npm run build`

### Release (`release.yml`)

Triggered when pushing `v*` tags:

| Platform | Artifacts                    |
| -------- | ---------------------------- |
| macOS    | `.dmg` (universal binary)    |
| Linux    | `.deb`, `.AppImage`          |
| Windows  | `.exe` (NSIS installer)      |
| Server   | `.deb` (Ubuntu, cargo-deb)   |

All artifacts are attached to the auto-generated GitHub Release.

## Tech Stack

| Component           | Technology                      |
| ------------------- | ------------------------------- |
| **Server**          | Rust, Axum, Tokio, Quinn (QUIC) |
| **Client Backend**  | Rust, Tauri v2, Tokio, Quinn    |
| **Client Frontend** | React, TypeScript, Vite         |
| **Protocol**        | QUIC + bincode binary           |
