# QuickStart Guide: TS3 Bot Development

**Audience**: Developers implementing the TS3 Client Bot
**Prerequisites**: Rust 1.75+, TS3 server access (for testing)
**Estimated Setup Time**: 15-30 minutes

---

## Overview

This guide walks through setting up the development environment, running the bot locally, and making your first test connection.

---

## 1. Development Environment Setup

### Install Rust

**macOS/Linux**:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

**Windows**:
Download and run [rustup-init.exe](https://rustup.rs/)

**Verify Installation**:
```bash
rustc --version
# Expected: rustc 1.75.0 or later
```

### Clone Repository

```bash
cd d:/projets
git clone <repo-url> ts-bot
cd ts-bot
git checkout 001-ts3-client-bot
```

---

## 2. Project Initialization

### Create Rust Project

```bash
cargo init --name ts3_bot
```

### Add Dependencies

Edit `Cargo.toml`:

```toml
[package]
name = "ts3_bot"
version = "0.1.0"
edition = "2021"

[dependencies]
# TS3 client protocol
tsclientlib = "0.2"

# Async runtime and WebSocket server
tokio = { version = "1", features = ["full"] }
axum = { version = "0.7", features = ["ws"] }
tower = "0.4"
tower-http = { version = "0.5", features = ["trace"] }

# Serialization
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# Configuration
dotenvy = "0.15"
envy = "0.4"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Utilities
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1.0"
thiserror = "1.0"

[dev-dependencies]
# Testing utilities
tokio-test = "0.4"
```

---

## 3. Configuration Setup

### Create `.env` File

Copy the example configuration:

```bash
cp .env.example .env
```

Edit `.env` with your TS3 server details:

```env
# TeamSpeak 3 Connection
TS3_SERVER=ts3.example.com:9987
TS3_NICKNAME=OpenClaw Bot
TS3_PASSWORD=
TS3_CHANNEL=

# WebSocket API
WS_HOST=127.0.0.1
WS_PORT=8080

# Logging
LOG_LEVEL=INFO
LOG_FILE=

# Reconnection (optional, uses defaults if not set)
RECONNECT_MAX_ATTEMPTS=10
RECONNECT_INITIAL_DELAY_MS=1000
RECONNECT_MAX_DELAY_MS=60000
```

**Test Server Access**:
```bash
# Use the official TS3 client to verify you can connect with these credentials
# Or use a command-line tool like ts3-client-query
```

---

## 4. Initial Code Structure

### Create Module Skeleton

```bash
# Create module directories
mkdir -p src/ts3
mkdir -p src/websocket
mkdir -p src/models

# Create module files
touch src/ts3/mod.rs src/ts3/client.rs src/ts3/events.rs src/ts3/reconnect.rs
touch src/websocket/mod.rs src/websocket/server.rs src/websocket/handlers.rs
touch src/models/mod.rs src/models/config.rs src/models/message.rs src/models/command.rs

# Create lib.rs for shared code
touch src/lib.rs
```

### Minimal `main.rs`

```rust
use anyhow::Result;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into())
        )
        .init();

    info!("TS3 Bot starting...");

    // TODO: Load config
    // TODO: Start TS3 client
    // TODO: Start WebSocket server

    info!("TS3 Bot ready");

    // Keep running
    tokio::signal::ctrl_c().await?;
    info!("Shutdown signal received");

    Ok(())
}
```

### Test Compilation

```bash
cargo build
# Expected: Compiles successfully (warnings OK, no errors)
```

---

## 5. First Feature: Configuration Loading

### Implement `models/config.rs`

```rust
use serde::Deserialize;
use anyhow::Result;

#[derive(Debug, Deserialize)]
pub struct BotConfig {
    pub ts3_server: String,
    pub ts3_nickname: String,
    pub ts3_password: Option<String>,
    pub ts3_channel: Option<String>,

    #[serde(default = "default_ws_host")]
    pub ws_host: String,

    #[serde(default = "default_ws_port")]
    pub ws_port: u16,

    #[serde(default = "default_log_level")]
    pub log_level: String,

    pub log_file: Option<String>,
}

fn default_ws_host() -> String { "127.0.0.1".into() }
fn default_ws_port() -> u16 { 8080 }
fn default_log_level() -> String { "INFO".into() }

impl BotConfig {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok(); // Load .env file (ignore if missing)
        let config = envy::from_env::<BotConfig>()?;
        Ok(config)
    }
}
```

### Test Configuration

Update `main.rs`:

```rust
mod models;
use models::config::BotConfig;

#[tokio::main]
async fn main() -> Result<()> {
    // Load config FIRST (before logging, so we can use LOG_LEVEL)
    let config = BotConfig::from_env()?;

    // Initialize logging with config level
    tracing_subscriber::fmt()
        .with_env_filter(&config.log_level)
        .init();

    info!(?config, "Configuration loaded");

    // ... rest of main
}
```

**Run**:
```bash
cargo run
# Expected output:
# INFO Configuration loaded config=BotConfig { ts3_server: "ts3.example.com:9987", ... }
```

---

## 6. Testing WebSocket Server

### Implement Minimal WebSocket Server

Create `websocket/server.rs`:

```rust
use axum::{
    Router,
    routing::get,
    extract::ws::{WebSocket, WebSocketUpgrade},
    response::Response,
};
use tracing::info;

pub async fn run_server(host: String, port: u16) -> anyhow::Result<()> {
    let addr = format!("{}:{}", host, port);
    let app = Router::new().route("/ws", get(ws_handler));

    info!("WebSocket server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn ws_handler(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    info!("WebSocket client connected");

    // Send welcome message
    let welcome = serde_json::json!({
        "type": "welcome",
        "bot_nickname": "Test Bot",
        "ts3_server": "not connected",
        "connection_status": "disconnected",
        "api_version": "1.0.0"
    });

    if socket.send(axum::extract::ws::Message::Text(welcome.to_string())).await.is_err() {
        return;
    }

    // Echo messages (temporary, for testing)
    while let Some(Ok(msg)) = socket.recv().await {
        if socket.send(msg).await.is_err() {
            break;
        }
    }

    info!("WebSocket client disconnected");
}
```

### Update `main.rs`

```rust
mod websocket;

#[tokio::main]
async fn main() -> Result<()> {
    let config = BotConfig::from_env()?;
    // ... logging setup ...

    info!("Starting WebSocket server...");
    websocket::server::run_server(config.ws_host, config.ws_port).await?;

    Ok(())
}
```

### Test with wscat

**Install wscat**:
```bash
npm install -g wscat
```

**Terminal 1** (Run bot):
```bash
cargo run
```

**Terminal 2** (Connect client):
```bash
wscat -c ws://127.0.0.1:8080/ws

# Expected:
# Connected
# < {"type":"welcome","bot_nickname":"Test Bot",...}

# Send test message:
> {"type":"test"}
< {"type":"test"}
```

---

## 7. Next Steps

Now that you have:
- ✅ Rust environment set up
- ✅ Configuration loading working
- ✅ Basic WebSocket server running

**Phase 1 Implementation Tasks**:
1. Implement TS3 client connection (see `specs/001-ts3-client-bot/research.md` for TsClientlib usage)
2. Implement WebSocket command parsing (see `specs/001-ts3-client-bot/contracts/websocket-api.md`)
3. Implement message relay (TS3 events → WebSocket broadcast)
4. Implement command handlers (WebSocket → TS3 actions)
5. Implement reconnection logic
6. Add tests

**Development Workflow**:
```bash
# Run with auto-reload (install cargo-watch)
cargo install cargo-watch
cargo watch -x run

# Run tests
cargo test

# Check code
cargo clippy
cargo fmt --check
```

---

## 8. Debugging Tips

### Enable Verbose Logging

```env
LOG_LEVEL=DEBUG
```

Or run with:
```bash
RUST_LOG=debug cargo run
```

### Test TS3 Connection Separately

Before integrating, test TsClientlib in isolation:

```bash
# Create example in examples/test_ts3.rs
cargo run --example test_ts3
```

### WebSocket Debugging

Use browser DevTools or `wscat`:

```bash
# Listen mode (for testing client connections)
wscat -l 8080
```

### Common Issues

| Issue | Solution |
|-------|----------|
| "TS3_SERVER not set" | Check `.env` file exists and is loaded |
| "Address already in use" | Another process using port 8080, change WS_PORT |
| "Connection refused (TS3)" | Verify TS3 server address, firewall, credentials |
| Cargo build slow | First build compiles dependencies, subsequent builds faster |

---

## 9. Resources

**Documentation**:
- [Spec](./spec.md) - Feature specification
- [Research](./research.md) - Technology decisions
- [Data Model](./data-model.md) - Data structures
- [API Contract](./contracts/websocket-api.md) - WebSocket API

**External Docs**:
- [TsClientlib Docs](https://docs.rs/tsclientlib/)
- [Axum Book](https://docs.rs/axum/)
- [Tokio Tutorial](https://tokio.rs/tokio/tutorial)
- [Rust Book](https://doc.rust-lang.org/book/)

**Testing**:
- TS3 Public Test Server: (search for "TeamSpeak 3 public servers" for test servers)
- wscat: `npm install -g wscat`

---

**QuickStart Complete** ✅ | Development environment ready | Begin implementation with `/speckit.tasks`
