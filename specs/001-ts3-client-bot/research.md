# Research: TeamSpeak 3 Client Bot with WebSocket API

**Phase**: 0 - Outline & Research
**Date**: 2026-02-05
**Status**: Complete

## Overview

This document consolidates research findings for implementing a TeamSpeak 3 bot using the native client protocol with a WebSocket control API. Key research areas include TS3 client protocol implementation, async WebSocket servers in Rust, configuration management, and reliability patterns.

---

## R1: TeamSpeak 3 Client Protocol Implementation

### Decision
Use **TsClientlib** (ReSpeak/tsclientlib) as the TS3 native client protocol library.

### Rationale
- **Native client protocol**: TsClientlib implements the full TS3 client protocol (not ServerQuery), allowing the bot to appear as a visible client in the server
- **Rust-native**: Written in Rust with Tokio async runtime, perfect fit for our stack
- **Active maintenance**: Last updated August 2024, reasonably maintained
- **Event-driven API**: Supports async event handling for messages, connections, disconnections
- **Community validation**: Used by ReSpeak client project, proven in production

### Alternatives Considered
| Alternative | Reason Rejected |
|-------------|-----------------|
| **TS3j** (Java) | Would require JVM, adds complexity with inter-process communication to Rust |
| **PyTS3Bot** (Python) | Same issue - Python runtime + IPC overhead, not as performant |
| **Custom implementation** | Reverse-engineering TS3 protocol from scratch is extremely complex and time-consuming |
| **ServerQuery API** | Admin-only API, bot wouldn't appear as visible client (fails core requirement) |

### Integration Notes
- TsClientlib uses Tokio async runtime (same as our WebSocket server)
- Provides event streams for messages, user joins/leaves, channel changes
- Supports connection parameters: server address, credentials, nickname, channel
- Auto-reconnection must be implemented manually (library doesn't provide built-in reconnect)

### Key Resources
- GitHub: https://github.com/ReSpeak/tsclientlib
- Docs: https://docs.rs/tsclientlib/latest/tsclientlib/
- Examples in repo: `tsclientlib/examples/audio.rs` shows connection pattern

---

## R2: WebSocket Server Framework

### Decision
Use **Axum** with Tokio for WebSocket server implementation.

### Rationale
- **Tokio integration**: Built on Tokio, shares async runtime with TsClientlib
- **WebSocket support**: First-class WebSocket support via `axum::extract::ws`
- **Lightweight**: Minimal overhead, focuses on async handlers
- **Type-safe**: Leverages Rust type system for request/response handling
- **Active ecosystem**: Maintained by Tokio team, excellent documentation
- **Broadcasting**: Easy to implement broadcast pattern with `tokio::sync::broadcast` channel

### Alternatives Considered
| Alternative | Reason Rejected |
|-------------|-----------------|
| **Actix-web** | More complex, different async runtime, heavier than needed |
| **Warp** | Less maintained, smaller community, similar capabilities to Axum |
| **Rocket** | Sync-first framework, doesn't fit async-heavy workload |
| **tokio-tungstenite** (raw) | Too low-level, would need to build routing/handler logic manually |

### Integration Pattern
```rust
// Conceptual structure (not implementation code)
// 1. Axum server with WebSocket upgrade endpoint
// 2. Broadcast channel (tokio::sync::broadcast) for TS3 → WebSocket relay
// 3. mpsc channel for WebSocket → TS3 commands
// 4. Concurrent tasks: TS3 client, WebSocket server, message router
```

### Key Resources
- Axum WebSocket example: https://github.com/tokio-rs/axum/blob/main/examples/websockets/src/main.rs
- Tokio broadcast docs: https://docs.rs/tokio/latest/tokio/sync/broadcast/

---

## R3: Configuration Management

### Decision
Use **dotenvy** (formerly dotenv) + **serde** for environment-based configuration.

### Rationale
- **12-factor compliance**: Environment variables are standard for config
- **Flexibility**: Supports both .env files (dev) and system env vars (prod/docker)
- **Type-safe parsing**: Combine with serde for structured config validation
- **Rust standard**: dotenvy is the maintained fork of the original dotenv crate
- **Simple**: No complex config file parsing, easy to document

### Configuration Structure
```rust
// Conceptual config structure
struct BotConfig {
    // TS3 connection
    ts3_server: String,      // TS3_SERVER
    ts3_nickname: String,    // TS3_NICKNAME
    ts3_password: Option<String>, // TS3_PASSWORD
    ts3_channel: Option<String>,  // TS3_CHANNEL

    // WebSocket API
    ws_host: String,         // WS_HOST (default: 127.0.0.1)
    ws_port: u16,            // WS_PORT (default: 8080)

    // Logging
    log_level: String,       // LOG_LEVEL (default: INFO)
    log_file: Option<String>, // LOG_FILE (optional)
}
```

### Alternatives Considered
| Alternative | Reason Rejected |
|-------------|-----------------|
| **TOML/YAML files** | Adds parsing complexity, user requested env vars only |
| **figment** | Over-engineered for simple env var needs |
| **config-rs** | Supports multiple formats but adds unnecessary complexity |

### Key Resources
- dotenvy crate: https://docs.rs/dotenvy/
- envy crate (serde deserializer for env vars): https://docs.rs/envy/

---

## R4: Logging Strategy

### Decision
Use **tracing** + **tracing-subscriber** for structured logging to stdout, with optional file output.

### Rationale
- **Rust standard**: tracing is the de-facto logging framework for async Rust
- **Structured logging**: Spans and events with structured fields, not just strings
- **Async-aware**: Integrates with Tokio, tracks async context automatically
- **Flexible output**: Easy to configure stdout, file, or both
- **Filter by level**: Runtime log level filtering (INFO default, configurable)
- **Zero-cost abstractions**: Minimal overhead when logs disabled

### Log Levels Strategy
- **ERROR**: Connection failures, critical errors that stop the bot
- **WARN**: Reconnection attempts, recoverable errors, degraded state
- **INFO**: Connection established, messages sent/received, commands executed (default)
- **DEBUG**: Detailed event processing, state transitions (dev only)
- **TRACE**: Raw protocol messages, verbose internals (deep debugging)

### Alternatives Considered
| Alternative | Reason Rejected |
|-------------|-----------------|
| **log + env_logger** | Less structured, no async context awareness |
| **slog** | More complex API, tracing is more idiomatic for Tokio |
| **fern** | Older, less async-aware than tracing |

### Key Resources
- tracing docs: https://docs.rs/tracing/
- tracing-subscriber guide: https://docs.rs/tracing-subscriber/latest/tracing_subscriber/

---

## R5: Auto-Reconnection Strategy

### Decision
Implement **exponential backoff reconnection** with max retry limit using Tokio timers.

### Rationale
- **Resilience**: TS3 servers can restart, network can be unstable
- **Exponential backoff**: Prevents hammering server during outages (1s, 2s, 4s, 8s, max 60s)
- **Max attempts**: Give up after N failures to avoid infinite loops
- **Event notification**: Notify WebSocket clients of connection state changes
- **Tokio-native**: Use `tokio::time::sleep` for async delays

### Reconnection Flow
```
1. TS3 connection lost (detected via event or timeout)
2. Log warning, notify WebSocket clients (connection_lost event)
3. Wait (backoff delay)
4. Attempt reconnection
5. If success: reset backoff, notify clients (connection_restored)
6. If fail: increase backoff, retry (up to max attempts)
7. If max attempts reached: log error, notify clients, exit or wait for manual restart
```

### Configuration
- `RECONNECT_MAX_ATTEMPTS`: Default 10
- `RECONNECT_INITIAL_DELAY`: Default 1s
- `RECONNECT_MAX_DELAY`: Default 60s

### Alternatives Considered
| Alternative | Reason Rejected |
|-------------|-----------------|
| **Fixed delay retry** | Can overload server during outages |
| **Infinite retry** | Can mask persistent config issues |
| **No auto-reconnect** | Fails resilience requirement (FR-010) |

### Key Resources
- Tokio time module: https://docs.rs/tokio/latest/tokio/time/
- Backoff crate (if needed): https://docs.rs/backoff/

---

## R6: WebSocket Message Protocol

### Decision
Use **JSON-based text messages** with structured command/event format.

### Rationale
- **Human-readable**: Easy to debug with browser devtools or wscat
- **Language-agnostic**: OpenClaw (or any client) can easily parse JSON
- **Structured validation**: serde deserializes directly to Rust structs with validation
- **Extensible**: Easy to add new command types or event fields
- **Standard**: Universally supported, no custom binary protocol needed

### Message Format
```json
// Client → Server (Commands)
{
  "type": "send_message",
  "target": "channel",  // or "user"
  "recipient": null,    // or user_id for private messages
  "content": "Hello from bot"
}

{
  "type": "move_channel",
  "channel_id": 5,
  "password": null
}

// Server → Client (Events)
{
  "type": "message_received",
  "timestamp": "2026-02-05T12:34:56Z",
  "sender_id": 123,
  "sender_name": "Alice",
  "message_type": "channel",  // or "private"
  "content": "Hello bot!"
}

{
  "type": "connection_status",
  "status": "connected",  // or "disconnected", "reconnecting"
  "server": "ts3.example.com"
}
```

### Alternatives Considered
| Alternative | Reason Rejected |
|-------------|-----------------|
| **Binary protocol (MessagePack)** | Harder to debug, overkill for low message volume |
| **Plain text (IRC-style)** | Less structured, harder to parse reliably |
| **Protocol Buffers** | Requires schema compilation, too heavy for simple use case |

### Key Resources
- serde_json docs: https://docs.rs/serde_json/

---

## R7: Testing Strategy

### Decision
**Integration tests** for WebSocket API + **unit tests** for business logic + **manual testing** with real TS3 server.

### Rationale
- **Integration tests**: Critical for WebSocket command flow (send command → verify response)
- **Unit tests**: For command parsing, validation, message formatting
- **Manual testing**: Real TS3 connection cannot be fully mocked (protocol complexity)
- **Test harness**: Use `axum-test` or similar for WebSocket client in tests

### Test Coverage Priorities
1. **WebSocket command parsing** (unit): Valid/invalid JSON, missing fields
2. **WebSocket API flow** (integration): Connect, send command, receive response
3. **Message relay** (integration): Mock TS3 message → WebSocket broadcast
4. **Configuration loading** (unit): Parse env vars, validate required fields
5. **Reconnection logic** (unit): Backoff timing, max attempts

### Test Data
- Use `.env.test` for test configuration (mock server addresses)
- Mock TS3 events using in-memory event streams
- Test WebSocket connections using localhost client

### Alternatives Considered
| Alternative | Reason Rejected |
|-------------|-----------------|
| **Full TS3 mock server** | Too complex, TsClientlib internals hard to replicate |
| **E2E tests only** | Slow, brittle, not suitable for CI |
| **No integration tests** | Misses critical API contract validation |

### Key Resources
- Rust testing guide: https://doc.rust-lang.org/book/ch11-00-testing.html
- Tokio test utilities: https://docs.rs/tokio/latest/tokio/attr.test.html

---

## Summary of Decisions

| Area | Decision | Key Benefit |
|------|----------|-------------|
| **TS3 Protocol** | TsClientlib | Native client protocol, Rust-native, proven |
| **WebSocket Server** | Axum + Tokio | Lightweight, type-safe, async-first |
| **Configuration** | dotenvy + env vars | Simple, 12-factor compliant, type-safe |
| **Logging** | tracing + tracing-subscriber | Structured, async-aware, standard |
| **Reconnection** | Exponential backoff | Resilient, server-friendly |
| **Message Protocol** | JSON (serde) | Human-readable, language-agnostic |
| **Testing** | Integration + unit tests | Balances coverage and maintainability |

---

## Open Questions / Future Work

1. **Voice handling**: Out of scope for v1, but TsClientlib supports audio if needed later
2. **Multi-server support**: Current design is single-server; future feature could spawn multiple bot instances
3. **Authentication**: Currently localhost-only; if remote access needed, add JWT/API key validation
4. **Metrics/monitoring**: Consider adding Prometheus metrics for production deployments
5. **Rate limiting**: TS3 servers may have message rate limits; add throttling if needed

---

**Research Complete** ✅ | All technical unknowns resolved | Ready for Phase 1 (Design)
