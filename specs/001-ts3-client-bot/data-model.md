# Data Model: TeamSpeak 3 Client Bot

**Phase**: 1 - Design & Contracts
**Date**: 2026-02-05
**Status**: Complete

## Overview

This document defines the core data structures for the TS3 bot. Since the bot is a real-time relay service with no persistent storage, all models represent transient runtime state and message payloads.

---

## Domain Entities

### 1. BotConfig

**Purpose**: Application configuration loaded from environment variables.

**Fields**:
| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `ts3_server` | String | Yes | - | TS3 server address (e.g., "ts3.example.com:9987") |
| `ts3_nickname` | String | Yes | - | Bot display name in TS3 |
| `ts3_password` | Option\<String\> | No | None | Server password (if required) |
| `ts3_channel` | Option\<String\> | No | None | Initial channel to join (default: server default) |
| `ws_host` | String | No | "127.0.0.1" | WebSocket server bind address |
| `ws_port` | u16 | No | 8080 | WebSocket server port |
| `log_level` | String | No | "INFO" | Logging level (ERROR/WARN/INFO/DEBUG/TRACE) |
| `log_file` | Option\<String\> | No | None | Optional log file path |
| `reconnect_max_attempts` | u32 | No | 10 | Max reconnection attempts |
| `reconnect_initial_delay_ms` | u64 | No | 1000 | Initial reconnect delay (ms) |
| `reconnect_max_delay_ms` | u64 | No | 60000 | Max reconnect delay (ms) |

**Validation Rules**:
- `ts3_server` must be non-empty and valid format (host:port or just host)
- `ts3_nickname` must be non-empty, max 30 characters
- `ws_port` must be in valid range (1-65535)
- `log_level` must be one of: ERROR, WARN, INFO, DEBUG, TRACE (case-insensitive)

**Environment Variables Mapping**:
```
TS3_SERVER → ts3_server
TS3_NICKNAME → ts3_nickname
TS3_PASSWORD → ts3_password
TS3_CHANNEL → ts3_channel
WS_HOST → ws_host
WS_PORT → ws_port
LOG_LEVEL → log_level
LOG_FILE → log_file
RECONNECT_MAX_ATTEMPTS → reconnect_max_attempts
RECONNECT_INITIAL_DELAY_MS → reconnect_initial_delay_ms
RECONNECT_MAX_DELAY_MS → reconnect_max_delay_ms
```

---

### 2. TS3ConnectionState

**Purpose**: Tracks the current state of the TS3 client connection.

**States** (enum):
| State | Description | Transitions To |
|-------|-------------|----------------|
| `Disconnected` | Not connected to TS3 server | `Connecting` |
| `Connecting` | Connection attempt in progress | `Connected`, `Reconnecting` |
| `Connected` | Active connection established | `Disconnected`, `Reconnecting` |
| `Reconnecting` | Auto-reconnection in progress (with backoff) | `Connected`, `Disconnected` |

**Fields**:
| Field | Type | Description |
|-------|------|-------------|
| `state` | ConnectionState (enum) | Current connection state |
| `server_address` | String | Connected/connecting server address |
| `current_channel_id` | Option\<u64\> | Current channel ID (if connected) |
| `current_channel_name` | Option\<String\> | Current channel name (if connected) |
| `reconnect_attempts` | u32 | Number of reconnection attempts (resets on success) |
| `last_error` | Option\<String\> | Last connection error message (if any) |

---

### 3. MessageEvent

**Purpose**: Represents a message received from TeamSpeak (channel or private message).

**Fields**:
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `timestamp` | DateTime\<Utc\> | Yes | Message receipt timestamp (ISO 8601) |
| `sender_id` | u64 | Yes | TS3 client ID of sender |
| `sender_name` | String | Yes | Display name of sender |
| `message_type` | MessageType | Yes | "channel" or "private" |
| `content` | String | Yes | Message text content |
| `channel_id` | Option\<u64\> | Conditional | Channel ID (required if message_type = channel) |
| `channel_name` | Option\<String\> | Conditional | Channel name (required if message_type = channel) |

**MessageType** (enum):
| Value | Description |
|-------|-------------|
| `Channel` | Public message in a channel |
| `Private` | Private/direct message to the bot |

**Validation Rules**:
- `content` must be non-empty
- If `message_type` = Channel, `channel_id` and `channel_name` must be present
- `timestamp` is always UTC

---

### 4. WebSocketCommand

**Purpose**: Represents a command sent by a WebSocket client to control the bot.

**Command Types** (enum):

#### 4a. SendMessage
Send a message to TS3 (channel or private message).

**Fields**:
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `target` | MessageTarget | Yes | "channel" or "user" |
| `recipient` | Option\<u64\> | Conditional | User ID (required if target = user) |
| `content` | String | Yes | Message text to send |

**MessageTarget** (enum):
| Value | Description |
|-------|-------------|
| `Channel` | Send to current channel |
| `User` | Send private message to specific user |

**Validation Rules**:
- `content` must be non-empty, max 8192 characters (TS3 limit)
- If `target` = User, `recipient` must be present and non-zero

---

#### 4b. MoveChannel
Move the bot to a different channel.

**Fields**:
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `channel_id` | u64 | Yes | Target channel ID |
| `password` | Option\<String\> | No | Channel password (if required) |

**Validation Rules**:
- `channel_id` must be non-zero

---

#### 4c. GetStatus
Query the bot's current connection status.

**Fields**: None (just the command type)

**Response**: Returns TS3ConnectionState

---

### 5. WebSocketEvent

**Purpose**: Events sent from the bot to WebSocket clients (server → client messages).

**Event Types** (enum):

#### 5a. MessageReceived
Relays a TS3 message to WebSocket clients.

**Payload**: MessageEvent (see above)

---

#### 5b. ConnectionStatus
Notifies clients of TS3 connection state changes.

**Fields**:
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `status` | String | Yes | "connected", "disconnected", "reconnecting" |
| `server` | String | Yes | TS3 server address |
| `channel_id` | Option\<u64\> | Conditional | Current channel (if connected) |
| `channel_name` | Option\<String\> | Conditional | Current channel name (if connected) |
| `error` | Option\<String\> | No | Error message (if status = disconnected) |

---

#### 5c. CommandResponse
Response to a WebSocket command.

**Fields**:
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `command_id` | Option\<String\> | No | Request ID (if client provided one) |
| `success` | bool | Yes | Whether command succeeded |
| `message` | String | No | Success confirmation or error details |
| `data` | Option\<serde_json::Value\> | No | Additional response data (e.g., status query result) |

---

#### 5d. Welcome
Sent immediately when a WebSocket client connects.

**Fields**:
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `bot_nickname` | String | Yes | Bot's TS3 nickname |
| `ts3_server` | String | Yes | Connected TS3 server address |
| `connection_status` | String | Yes | Current TS3 connection state |
| `api_version` | String | Yes | WebSocket API version (e.g., "1.0.0") |

---

## Message Flow Diagrams

### TS3 → WebSocket (Message Relay)

```
[TS3 Server] --message--> [Bot TS3 Client]
                               |
                               | (convert to MessageEvent)
                               v
                        [Broadcast Channel]
                               |
                               +---> [WebSocket Client 1]
                               +---> [WebSocket Client 2]
                               +---> [WebSocket Client N]
```

### WebSocket → TS3 (Command Execution)

```
[WebSocket Client] --command--> [Bot WebSocket Server]
                                       |
                                       | (parse & validate)
                                       v
                                [Command Handler]
                                       |
                                       | (execute via TS3 Client)
                                       v
                                [TS3 Server]
                                       |
                                       v
                          [Send CommandResponse back to client]
```

### Connection State Changes

```
[TS3 Connection Lost] ---> [Reconnection Logic]
                                 |
                                 | (exponential backoff)
                                 v
                          [Broadcast ConnectionStatus]
                                 |
                                 +---> All WebSocket clients notified
```

---

## State Transitions

### TS3 Connection Lifecycle

```
[Disconnected] --startup--> [Connecting]
                                |
                                +--success--> [Connected]
                                |
                                +--failure--> [Reconnecting] --success--> [Connected]
                                                     |
                                                     +--max attempts--> [Disconnected]

[Connected] --disconnect--> [Reconnecting] (or [Disconnected] if graceful shutdown)
```

### WebSocket Client Lifecycle

```
[Client Connects] --> [Send Welcome Event]
                           |
                           v
                    [Active Session]
                           |
                           +--receives--> MessageReceived events
                           +--receives--> ConnectionStatus events
                           +--sends----> Commands
                           +--receives--> CommandResponse
                           |
                           v
                    [Client Disconnects]
```

---

## Validation Summary

| Entity | Critical Validations |
|--------|---------------------|
| **BotConfig** | Non-empty server/nickname, valid port, valid log level |
| **MessageEvent** | Non-empty content, channel info present if channel message |
| **SendMessage** | Non-empty content (≤8192 chars), recipient present if target=user |
| **MoveChannel** | Non-zero channel_id |
| **All Commands** | Valid JSON deserialization, enum variant matching |

---

## JSON Schema Examples

### WebSocket Command (SendMessage)
```json
{
  "type": "send_message",
  "target": "channel",
  "recipient": null,
  "content": "Hello from bot!"
}
```

### WebSocket Event (MessageReceived)
```json
{
  "type": "message_received",
  "timestamp": "2026-02-05T14:30:00Z",
  "sender_id": 42,
  "sender_name": "Alice",
  "message_type": "channel",
  "content": "Hey bot!",
  "channel_id": 5,
  "channel_name": "General"
}
```

### WebSocket Event (ConnectionStatus)
```json
{
  "type": "connection_status",
  "status": "connected",
  "server": "ts3.example.com:9987",
  "channel_id": 1,
  "channel_name": "Lobby",
  "error": null
}
```

### WebSocket Event (CommandResponse)
```json
{
  "type": "command_response",
  "command_id": "req-123",
  "success": true,
  "message": "Message sent successfully",
  "data": null
}
```

---

**Data Model Complete** ✅ | All entities defined | Ready for contract specification
