# WebSocket API Contract

**Version**: 1.0.0
**Protocol**: WebSocket (RFC 6455)
**Message Format**: JSON (UTF-8 text frames)
**Endpoint**: `ws://<host>:<port>/ws` (default: `ws://127.0.0.1:8080/ws`)

---

## Connection

### Handshake

**Client Request**:
```http
GET /ws HTTP/1.1
Host: 127.0.0.1:8080
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Key: <random-key>
Sec-WebSocket-Version: 13
```

**Server Response**:
```http
HTTP/1.1 101 Switching Protocols
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Accept: <accept-key>
```

### Welcome Event

Upon successful connection, the server immediately sends a `welcome` event:

```json
{
  "type": "welcome",
  "bot_nickname": "OpenClaw Bot",
  "ts3_server": "ts3.example.com:9987",
  "connection_status": "connected",
  "api_version": "1.0.0"
}
```

**Fields**:
- `type`: Always "welcome"
- `bot_nickname`: Bot's display name on TS3
- `ts3_server`: TS3 server address
- `connection_status`: "connected", "disconnected", or "reconnecting"
- `api_version`: WebSocket API version for compatibility

---

## Message Types

All messages are JSON objects with a mandatory `type` field indicating the message type.

### Client → Server (Commands)

Commands are requests from WebSocket clients to control the bot.

---

#### 1. send_message

Send a message to TeamSpeak (channel or private message).

**Request**:
```json
{
  "type": "send_message",
  "command_id": "req-001",  // Optional: client-provided request ID
  "target": "channel",      // "channel" or "user"
  "recipient": null,        // Required if target="user", null otherwise
  "content": "Hello from WebSocket!"
}
```

**Fields**:
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | String | ✅ | Must be "send_message" |
| `command_id` | String | ❌ | Optional request ID (echoed in response) |
| `target` | String | ✅ | "channel" or "user" |
| `recipient` | Number | Conditional | TS3 user ID (required if target="user") |
| `content` | String | ✅ | Message text (max 8192 chars) |

**Success Response**:
```json
{
  "type": "command_response",
  "command_id": "req-001",
  "success": true,
  "message": "Message sent successfully"
}
```

**Error Response**:
```json
{
  "type": "command_response",
  "command_id": "req-001",
  "success": false,
  "message": "Validation error: content exceeds 8192 characters"
}
```

**Possible Errors**:
- "Validation error: content is empty"
- "Validation error: content exceeds 8192 characters"
- "Validation error: recipient required for target=user"
- "Not connected to TS3 server"
- "TS3 error: user not found"

---

#### 2. move_channel

Move the bot to a different channel.

**Request**:
```json
{
  "type": "move_channel",
  "command_id": "req-002",
  "channel_id": 5,
  "password": null  // Optional: channel password if required
}
```

**Fields**:
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | String | ✅ | Must be "move_channel" |
| `command_id` | String | ❌ | Optional request ID |
| `channel_id` | Number | ✅ | Target channel ID (must be > 0) |
| `password` | String | ❌ | Channel password (if required) |

**Success Response**:
```json
{
  "type": "command_response",
  "command_id": "req-002",
  "success": true,
  "message": "Moved to channel 5"
}
```

**Error Response**:
```json
{
  "type": "command_response",
  "command_id": "req-002",
  "success": false,
  "message": "TS3 error: insufficient permissions"
}
```

**Possible Errors**:
- "Validation error: channel_id must be > 0"
- "Not connected to TS3 server"
- "TS3 error: channel not found"
- "TS3 error: insufficient permissions"
- "TS3 error: incorrect password"

---

#### 3. get_status

Query the bot's current connection status and state.

**Request**:
```json
{
  "type": "get_status",
  "command_id": "req-003"
}
```

**Fields**:
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | String | ✅ | Must be "get_status" |
| `command_id` | String | ❌ | Optional request ID |

**Success Response**:
```json
{
  "type": "command_response",
  "command_id": "req-003",
  "success": true,
  "data": {
    "connection_state": "connected",
    "server": "ts3.example.com:9987",
    "current_channel_id": 5,
    "current_channel_name": "General",
    "reconnect_attempts": 0
  }
}
```

**Data Fields**:
| Field | Type | Description |
|-------|------|-------------|
| `connection_state` | String | "connected", "disconnected", "connecting", "reconnecting" |
| `server` | String | TS3 server address |
| `current_channel_id` | Number\|null | Current channel ID (null if not connected) |
| `current_channel_name` | String\|null | Current channel name (null if not connected) |
| `reconnect_attempts` | Number | Number of reconnection attempts since last disconnect |

---

### Server → Client (Events)

Events are messages broadcast from the bot to all connected WebSocket clients.

---

#### 4. message_received

Relays a message received from TeamSpeak.

**Event**:
```json
{
  "type": "message_received",
  "timestamp": "2026-02-05T14:30:00Z",
  "sender_id": 42,
  "sender_name": "Alice",
  "message_type": "channel",
  "content": "Hey bot, how are you?",
  "channel_id": 5,
  "channel_name": "General"
}
```

**Fields**:
| Field | Type | Always Present | Description |
|-------|------|----------------|-------------|
| `type` | String | ✅ | Always "message_received" |
| `timestamp` | String | ✅ | ISO 8601 UTC timestamp |
| `sender_id` | Number | ✅ | TS3 client ID of sender |
| `sender_name` | String | ✅ | Display name of sender |
| `message_type` | String | ✅ | "channel" or "private" |
| `content` | String | ✅ | Message text |
| `channel_id` | Number\|null | ✅ | Channel ID (null if private message) |
| `channel_name` | String\|null | ✅ | Channel name (null if private message) |

**Notes**:
- `channel_id` and `channel_name` are `null` for private messages
- All timestamps are UTC in ISO 8601 format
- Broadcast to all connected WebSocket clients

---

#### 5. connection_status

Notifies clients of TS3 connection state changes.

**Event (Connected)**:
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

**Event (Disconnected)**:
```json
{
  "type": "connection_status",
  "status": "disconnected",
  "server": "ts3.example.com:9987",
  "channel_id": null,
  "channel_name": null,
  "error": "Connection lost: timeout"
}
```

**Event (Reconnecting)**:
```json
{
  "type": "connection_status",
  "status": "reconnecting",
  "server": "ts3.example.com:9987",
  "channel_id": null,
  "channel_name": null,
  "error": "Reconnection attempt 3/10"
}
```

**Fields**:
| Field | Type | Always Present | Description |
|-------|------|----------------|-------------|
| `type` | String | ✅ | Always "connection_status" |
| `status` | String | ✅ | "connected", "disconnected", "reconnecting" |
| `server` | String | ✅ | TS3 server address |
| `channel_id` | Number\|null | ✅ | Current channel (null if not connected) |
| `channel_name` | String\|null | ✅ | Current channel name (null if not connected) |
| `error` | String\|null | ✅ | Error message or reconnection info (null if no error) |

**Trigger Conditions**:
- Sent when TS3 connection is established (status = "connected")
- Sent when TS3 connection is lost (status = "disconnected")
- Sent during reconnection attempts (status = "reconnecting")
- Sent after successful channel move (status = "connected" with new channel)

---

#### 6. command_response

Response to a client command (see command sections above).

**Fields**:
| Field | Type | Always Present | Description |
|-------|------|----------------|-------------|
| `type` | String | ✅ | Always "command_response" |
| `command_id` | String\|null | ✅ | Echoed request ID (null if not provided) |
| `success` | Boolean | ✅ | true if command succeeded, false otherwise |
| `message` | String | ❌ | Success confirmation or error details |
| `data` | Object\|null | ❌ | Additional response data (e.g., for get_status) |

---

## Error Handling

### Invalid JSON

If the client sends malformed JSON, the server responds with:

```json
{
  "type": "command_response",
  "command_id": null,
  "success": false,
  "message": "Invalid JSON: expected '}' at line 1 column 42"
}
```

### Unknown Command Type

If the `type` field is unrecognized:

```json
{
  "type": "command_response",
  "command_id": null,
  "success": false,
  "message": "Unknown command type: invalid_command"
}
```

### Missing Required Fields

If a required field is missing:

```json
{
  "type": "command_response",
  "command_id": "req-001",
  "success": false,
  "message": "Validation error: missing required field 'content'"
}
```

---

## Heartbeat / Keep-Alive

**WebSocket Ping/Pong**: The server sends WebSocket PING frames every 30 seconds. Clients must respond with PONG frames (handled automatically by most WebSocket libraries).

**No application-level heartbeat required** - WebSocket protocol handles this natively.

---

## Disconnection

### Client-Initiated

Clients can close the connection by sending a WebSocket CLOSE frame. No special message required.

### Server-Initiated

The server may close the connection if:
- Bot service is shutting down (CLOSE frame with code 1001 - Going Away)
- Client fails to respond to PING frames (timeout after 60s without PONG)

---

## Rate Limiting

**Not implemented in v1.0.0**: No rate limiting on commands. Clients should implement their own throttling to avoid overwhelming the TS3 server (recommended: max 10 messages/second).

---

## Example Session

```
→ Client connects to ws://127.0.0.1:8080/ws
← Server sends: {"type": "welcome", ...}

→ Client sends: {"type": "send_message", "target": "channel", "content": "Hello!"}
← Server sends: {"type": "command_response", "success": true, "message": "Message sent"}

← Server broadcasts: {"type": "message_received", "sender_name": "Alice", "content": "Hi bot!", ...}

→ Client sends: {"type": "move_channel", "channel_id": 5}
← Server sends: {"type": "command_response", "success": true, "message": "Moved to channel 5"}
← Server broadcasts: {"type": "connection_status", "status": "connected", "channel_id": 5, ...}

← Server broadcasts: {"type": "connection_status", "status": "disconnected", "error": "Connection lost"}
← Server broadcasts: {"type": "connection_status", "status": "reconnecting", ...}
← Server broadcasts: {"type": "connection_status", "status": "connected", ...}

→ Client sends: {"type": "get_status"}
← Server sends: {"type": "command_response", "success": true, "data": {...}}

→ Client closes connection
```

---

## Versioning

**Current Version**: 1.0.0

**Version Compatibility**: Clients should check the `api_version` field in the `welcome` message. Breaking changes will increment the major version (e.g., 2.0.0).

**Future Extensions**: New event types or optional command fields will be added with minor version bumps (e.g., 1.1.0).

---

## Security

**Authentication**: None (localhost-only deployment assumed in v1.0.0)

**Future Considerations**:
- If remote access is needed, add token-based authentication (JWT or API key in initial handshake)
- TLS/WSS support for encrypted connections

---

**WebSocket API Contract Complete** ✅ | Ready for implementation
