# TS3 Bot - WebSocket API Reference

**Version**: 1.2.0
**Protocol**: WebSocket (RFC 6455)
**Message Format**: JSON (UTF-8 text frames only)
**Default Endpoint**: `ws://127.0.0.1:8080/ws`
**Authentication**: None (localhost-only deployment)

> **Note**: This document covers the original 5 commands that were stable
> at v1.0.0. The current Rust enum (`src/models/command.rs::WebSocketCommand`)
> exposes additional commands (server admin, channel management, listeners,
> language, etc.) — see [Additional Commands](#additional-commands) for the
> list. The reference for each is the Rust source until this doc catches up.

---

## Table of Contents

1. [Connection](#1-connection)
2. [Message Format Convention](#2-message-format-convention)
3. [Commands (Client to Server)](#3-commands-client--server)
   - [get_status](#31-get_status)
   - [move_channel](#32-move_channel)
   - [send_message](#33-send_message)
   - [speak](#34-speak)
   - [stop_speaking](#35-stop_speaking)
4. [Events (Server to Client)](#4-events-server--client)
   - [welcome](#41-welcome)
   - [command_response](#42-command_response)
   - [message_received](#43-message_received)
   - [transcription](#44-transcription)
   - [connection_status](#45-connection_status)
   - [speak_started](#46-speak_started)
   - [speak_completed](#47-speak_completed)
   - [client_connected](#48-client_connected)
   - [client_disconnected](#49-client_disconnected)
   - [client_moved](#410-client_moved)
5. [Error Handling](#5-error-handling)
6. [Broadcast Model](#6-broadcast-model)
7. [Connection Lifecycle](#7-connection-lifecycle)
8. [Complete Session Example](#8-complete-session-example)
9. [TypeScript Type Definitions](#9-typescript-type-definitions)

---

## 1. Connection

### Endpoint

```
ws://<host>:<port>/ws
```

Default: `ws://127.0.0.1:8080/ws`

The host and port are configured server-side via `WS_HOST` (default `127.0.0.1`) and `WS_PORT` (default `8080`).

### Handshake

Standard WebSocket upgrade handshake (RFC 6455). No custom headers required.

```http
GET /ws HTTP/1.1
Host: 127.0.0.1:8080
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Key: <random-key>
Sec-WebSocket-Version: 13
```

### Authentication

**None.** The API is designed for localhost access only. No API key, token, or credentials are needed.

### After Connection

Upon successful WebSocket connection, the server **immediately** sends a [`welcome`](#41-welcome) event. No handshake message is required from the client.

### Keep-Alive

WebSocket PING/PONG is handled automatically at the protocol level by Axum. No application-level heartbeat is needed. Most WebSocket client libraries handle PONG responses automatically.

---

## 2. Message Format Convention

### All messages follow this pattern:

```json
{
  "type": "<message_type>",
  ...fields specific to this type
}
```

- The `type` field is **always present** and determines the message structure.
- All type values use `snake_case` (e.g., `"get_status"`, `"message_received"`).
- Fields with `null` values **may be omitted** from the JSON (see each message spec).
- Timestamps use **ISO 8601 UTC format**: `"2026-02-08T14:30:00.123Z"`.
- IDs (client, channel) are unsigned 64-bit integers serialized as JSON numbers.

### Commands vs Events

| Direction | Name | Description |
|-----------|------|-------------|
| Client -> Server | **Command** | A request from the client. Always produces a `command_response`. |
| Server -> Client | **Event** | Asynchronous notification from the server. |

### `command_id` Convention

Every command accepts an optional `command_id` field (string). If provided, it is echoed back in the corresponding `command_response`, allowing the client to correlate requests with responses. This is critical because responses are broadcast (see [Broadcast Model](#6-broadcast-model)).

Recommended format: UUID or sequential ID (e.g., `"cmd-001"`, `"a1b2c3d4"`).

---

## 3. Commands (Client -> Server)

---

### 3.1 `get_status`

Returns the full TS3 server state: all channels, all connected clients, and the bot's own client ID.

#### Request

```json
{
  "type": "get_status",
  "command_id": "cmd-001"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | `string` | Yes | Must be `"get_status"` |
| `command_id` | `string` | No | Client-provided correlation ID |

#### Success Response

In the current implementation, `get_status` produces **two** `command_response`
events (immediate ack with `data: null`, then the final response with the
actual payload — see [Multiple Responses](#multiple-responses)).

The payload returned in the final response contains many more fields than
historically documented. Minimal example showing the canonical fields plus
a sample of the additional ones:

```json
{
  "type": "command_response",
  "command_id": "cmd-001",
  "success": true,
  "data": {
    "own_client_id": 10788,
    "channels": [
      {
        "id": 5,
        "name": "Gaming",
        "parent_id": 1,
        "order": 162,
        "codec": "OpusVoice",
        "codec_quality": 5,
        "max_clients": "Unlimited",
        "has_password": false,
        "forced_silence": false,
        "needed_talk_power": 0,
        "subscribed": false,
        "topic": ""
      }
    ],
    "clients": [
      {
        "id": 10788,
        "name": "Marlbot",
        "channel_id": 5,
        "uid": [54, 85, 98, 150, 49, 248, 114, 247, 230, 29, 243, 32, 232, 37, 19, 224, 241, 174, 168, 67],
        "database_id": 231,
        "channel_group": 8,
        "server_groups": [9],
        "talk_power": 75,
        "input_muted": false,
        "output_muted": false,
        "is_recording": false,
        "is_priority_speaker": false,
        "is_channel_commander": false,
        "country_code": "FR"
      }
    ]
  }
}
```

#### Response `data` Schema

| Field | Type | Description |
|-------|------|-------------|
| `own_client_id` | `number` | The bot's TS3 session client ID. |
| `channels` | `array` | All channels on the server. |
| `channels[].id` | `number` | Channel ID. |
| `channels[].name` | `string` | Channel display name. |
| `channels[].parent_id` | `number` | Parent channel ID (`0` = root). |
| `channels[].order` | `number` | Sort order among siblings. |
| `channels[].codec` | `string` | TS3 codec name (e.g. `"OpusVoice"`, `"OpusMusic"`). |
| `channels[].codec_quality` | `number` | Codec quality 0–10. |
| `channels[].max_clients` | `string` | Either `"Unlimited"` or `"Limited(N)"`. |
| `channels[].has_password` | `boolean` | `true` if the channel is password-protected. |
| `channels[].forced_silence` | `boolean` | `true` if talk power requirement silences regular users. |
| `channels[].needed_talk_power` | `number` | Minimum talk power required to speak. |
| `channels[].subscribed` | `boolean` | Whether the bot is subscribed to channel events. |
| `channels[].topic` | `string` | Channel topic. |
| `clients` | `array` | All connected clients. |
| `clients[].id` | `number` | Session client ID (volatile, changes each reconnection). |
| `clients[].name` | `string` | Display name. |
| `clients[].channel_id` | `number` | Current channel ID. |
| `clients[].uid` | `number[]` | **Permanent unique ID as a byte array**, not a base64 string. Encode to base64 client-side if you need a string identifier. |
| `clients[].database_id` | `number` | TS3 server-database ID for the user. |
| `clients[].channel_group` | `number` | Channel group ID. |
| `clients[].server_groups` | `number[]` | Server group IDs. |
| `clients[].talk_power` | `number` | Current talk power. |
| `clients[].input_muted` | `boolean` | Microphone muted. |
| `clients[].output_muted` | `boolean` | Speakers muted. |
| `clients[].is_recording` | `boolean` | Client is currently recording. |
| `clients[].is_priority_speaker` | `boolean` | Priority speaker flag. |
| `clients[].is_channel_commander` | `boolean` | Channel commander flag. |
| `clients[].country_code` | `string` | ISO 3166-1 alpha-2 country code (best-effort). |

#### Error Responses

| Error message | Cause |
|---------------|-------|
| `"TS3 not connected"` | Bot is not connected to the TS3 server |
| `"Failed to read TS3 state"` | Internal error reading server state |
| `"TS3 connection error: ..."` | Connection dropped during query |

#### Notes

- `own_client_id` lets you identify which client in the `clients` list is the bot itself.
- Channel hierarchy can be reconstructed from `parent_id`. Root channels have `parent_id: 0`.
- Client IDs are **session IDs** (change every time a user reconnects). They are not permanent.

---

### 3.2 `move_channel`

Move the bot to a different TS3 channel.

#### Request

```json
{
  "type": "move_channel",
  "command_id": "cmd-002",
  "channel_id": 5,
  "password": null
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | `string` | Yes | Must be `"move_channel"` |
| `command_id` | `string` | No | Client-provided correlation ID |
| `channel_id` | `number` | Yes | Target channel ID (must be > 0) |
| `password` | `string` | No | Channel password (if the channel requires one) |

#### Success Response

```json
{
  "type": "command_response",
  "command_id": "cmd-002",
  "success": true,
  "message": "Moved to channel 5"
}
```

#### Error Responses

| Error message | Cause |
|---------------|-------|
| `"Validation error: channel_id must be > 0"` | `channel_id` is 0 |
| `"TS3 not connected"` | Bot is not connected to TS3 |
| `"Failed to get own client ID"` | Internal error |
| `"Move failed: ..."` | TS3 server rejected the move (bad ID, permissions, password, etc.) |

#### Side Effects

- On success, the bot saves the channel ID to disk (`.last_channel`) and will rejoin this channel automatically on next restart.
- A [`client_moved`](#410-client_moved) event will be broadcast with the bot's own client info.

#### Notes

- Use `get_status` first to obtain valid channel IDs.
- The `channel_id` is a numeric TS3 channel identifier, NOT a channel name.

---

### 3.3 `send_message`

Send a text message to the current TS3 channel or to a specific user (private message).

#### Request

```json
{
  "type": "send_message",
  "command_id": "cmd-003",
  "target": "channel",
  "recipient": null,
  "content": "Hello from a WS client!",
  "tts": false
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | `string` | Yes | Must be `"send_message"` |
| `command_id` | `string` | No | Client-provided correlation ID |
| `target` | `string` | Yes | `"channel"`, `"private"` (alias `"user"`) or `"server"` |
| `recipient` | `string` | Conditional | TS3 session client ID of the target user, **as a string** (e.g. `"11033"`). **Required** if `target` is `"private"`/`"user"`, ignored otherwise. |
| `content` | `string` | Yes | Message text. Must be non-empty. Max 8192 characters. |
| `tts` | `boolean` | No | If `true`, the bot will also speak the content via TTS in addition to sending the chat message. Default: `false`. |

> **Important**: `recipient` is a **string**, not a number. Sending a JSON
> integer (e.g. `"recipient": 11033`) will fail at parse time with
> `"invalid type: integer N, expected a string"`.

#### Success Response

Produces two `command_response` events:

1. Immediate ack: `{ success: true, message: "Sending message..." }`
2. Final result: `{ success: true, message: "Message sent (channel)" }` (or `(private)` / `(server)`)

#### Validation Errors

| Error message | Cause |
|---------------|-------|
| `"Validation error: content is empty"` | `content` is an empty string |
| `"Validation error: content exceeds 8192 characters"` | `content` is too long |
| `"Private message requires recipient client_id"` | `target` is `"private"`/`"user"` but `recipient` is missing or not a numeric string |

---

### 3.4 `speak`

Synthesize text to speech and play it as audio on the current TS3 channel.

Requires TTS to be enabled on the server (`TTS_ENABLED=true` and a running TTS HTTP service).

#### Request

```json
{
  "type": "speak",
  "command_id": "cmd-004",
  "text": "Bonjour tout le monde !",
  "voice": "ff_siwis"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | `string` | Yes | Must be `"speak"` |
| `command_id` | `string` | No | Client-provided correlation ID |
| `text` | `string` | Yes | Text to synthesize. Must be non-empty. Max 2000 characters. |
| `voice` | `string` | No | Voice identifier (e.g., `"ff_siwis"` for French female). Uses server default if omitted. |

#### Success Response

```json
{
  "type": "command_response",
  "command_id": "cmd-004",
  "success": true,
  "message": "Speech queued"
}
```

This response means the request was **accepted and queued**, not that playback has started. Listen for [`speak_started`](#46-speak_started) and [`speak_completed`](#47-speak_completed) events for playback status.

#### Error Responses

| Error message | Cause |
|---------------|-------|
| `"Validation error: text is empty"` | `text` is an empty string |
| `"Validation error: text exceeds 2000 characters"` | `text` is too long |
| `"TTS is disabled"` | TTS is not enabled on the server |
| `"TTS pipeline not available"` | TTS processing task has stopped |

#### Notes

- TTS audio is played on the TS3 channel where the bot currently is.
- If TTS is already playing, the new request replaces the current playback.
- TTS playback is automatically interrupted if a user says the wake word ("marlbot").

---

### 3.5 `stop_speaking`

Immediately stop any ongoing TTS playback.

#### Request

```json
{
  "type": "stop_speaking",
  "command_id": "cmd-005"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | `string` | Yes | Must be `"stop_speaking"` |
| `command_id` | `string` | No | Client-provided correlation ID |

#### Success Response

```json
{
  "type": "command_response",
  "command_id": "cmd-005",
  "success": true,
  "message": "Playback stopped"
}
```

#### Notes

- If no playback is in progress, the command still succeeds (no-op).

---

## 4. Events (Server -> Client)

Events are pushed asynchronously from the server. The client does not request them; they arrive whenever the corresponding action occurs on the TS3 server.

---

### 4.1 `welcome`

Sent **immediately** after WebSocket connection is established.

```json
{
  "type": "welcome",
  "nickname": "Marlbot",
  "server": "ts3.example.com:9987",
  "connection_status": "connected",
  "api_version": "1.0.0"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `type` | `string` | Always `"welcome"` |
| `nickname` | `string` | Bot's display name on TeamSpeak. (Earlier drafts of this doc called this field `bot_nickname` — the wire format is `nickname`.) |
| `server` | `string` | TS3 server address (may include port). (Earlier drafts called this `ts3_server`.) |
| `connection_status` | `string` | Current TS3 connection state: `"connected"`, `"disconnected"`, or `"reconnecting"` |
| `api_version` | `string` | API version string (semver). Currently `"1.0.0"`. |

#### Notes

- This is always the **first message** received after connecting.
- Use `api_version` for client compatibility checks.
- `connection_status` reflects the **live** TS3 connection state at the time of WebSocket connection. It is `"connected"` when the bot has an active TS3 session, `"disconnected"` otherwise (e.g., bot just started and hasn't connected yet).

---

### 4.2 `command_response`

Response to any client command. Always sent after a command is received.

```json
{
  "type": "command_response",
  "command_id": "cmd-001",
  "success": true,
  "message": "Moved to channel 5",
  "data": { ... }
}
```

| Field | Type | Always Present | Description |
|-------|------|----------------|-------------|
| `type` | `string` | Yes | Always `"command_response"` |
| `command_id` | `string \| null` | Yes | Echoed from the command (null if not provided by client) |
| `success` | `boolean` | Yes | `true` if command succeeded, `false` on error |
| `message` | `string` | No | Human-readable result or error description. **Omitted from JSON if null.** |
| `data` | `object` | No | Structured response payload (used by `get_status`). **Omitted from JSON if null.** |

#### Important: Omitted Fields

Fields `message` and `data` are **not present in the JSON** when their value is null. Your client must handle their absence gracefully.

Example (success without message or data):
```json
{
  "type": "command_response",
  "command_id": "cmd-001",
  "success": true
}
```

#### Multiple Responses

Some commands (`get_status`, `move_channel`) produce **two** `command_response` events:
1. An immediate acknowledgment (e.g., `"Moving channel..."`)
2. The actual result after the async operation completes

When correlating responses, use `command_id` and check the `data` or `message` fields to distinguish intermediate from final responses. The final response for `get_status` will have a `data` field. The final response for `move_channel` will have a specific message like `"Moved to channel X"` or an error.

---

### 4.3 `message_received`

A text message was received on TS3 (channel or private message).

```json
{
  "type": "message_received",
  "timestamp": "2026-02-08T14:30:00.123Z",
  "sender_id": 3,
  "sender_uid": "abc123def456=",
  "sender_name": "Alice",
  "message_type": "channel",
  "content": "Hey marlbot, what's up?",
  "channel_id": 5,
  "channel_name": "Gaming"
}
```

| Field | Type | Always Present | Description |
|-------|------|----------------|-------------|
| `type` | `string` | Yes | Always `"message_received"` |
| `timestamp` | `string` | Yes | ISO 8601 UTC timestamp |
| `sender_id` | `number` | Yes | TS3 session client ID of the sender |
| `sender_uid` | `string` | Yes | Permanent unique ID of the sender (base64-encoded, stable across reconnections) |
| `sender_name` | `string` | Yes | Display name of the sender |
| `message_type` | `string` | Yes | `"channel"` or `"private"` |
| `content` | `string` | Yes | Message text content |
| `channel_id` | `number \| null` | Yes | Channel ID where the message was sent (`null` for private messages) |
| `channel_name` | `string \| null` | Yes | Channel name (`null` for private messages) |

#### Notes

- `sender_uid` is a **permanent identifier** (persists across reconnections, unlike `sender_id`). Use it to track users across sessions.
- `sender_id` is a **session identifier** (changes each time the user reconnects).
- Messages from the bot itself are NOT relayed (only messages from other clients).

---

### 4.4 `transcription`

Result of speech-to-text (STT) transcription from voice audio on the TS3 channel.

```json
{
  "type": "transcription",
  "timestamp": "2026-02-08T14:30:05.456Z",
  "speaker_id": 3,
  "speaker_uid": "abc123def456=",
  "speaker_name": "Alice",
  "text": "marlbot dis bonjour a tout le monde",
  "confidence": 0.87,
  "language": "fr",
  "duration_ms": 3200
}
```

| Field | Type | Always Present | Description |
|-------|------|----------------|-------------|
| `type` | `string` | Yes | Always `"transcription"` |
| `timestamp` | `string` | Yes | ISO 8601 UTC timestamp |
| `speaker_id` | `number` | Yes | TS3 session client ID of the speaker |
| `speaker_uid` | `string` | Yes | Permanent unique ID of the speaker |
| `speaker_name` | `string` | Yes | Display name of the speaker |
| `text` | `string` | Yes | Transcribed text (lowercase, normalized) |
| `confidence` | `number \| null` | Yes | Confidence score (0.0 to 1.0). May be `null`. |
| `language` | `string \| null` | Yes | Detected language code (e.g., `"fr"`, `"en"`). May be `null`. |
| `duration_ms` | `number` | Yes | Duration of the audio segment in milliseconds |

#### Notes

- Transcriptions are only generated after the **wake word** ("marlbot") is detected. The bot does not transcribe all audio on the channel.
- The flow is: user says "marlbot" -> bot activates listening -> user speaks -> silence detected -> transcription emitted.
- The `text` field contains the full transcription **after** the wake word activation (the wake word itself is removed from the text).
- Transcription uses Whisper (small model). Accuracy depends on audio quality, accent, and background noise.

---

### 4.5 `connection_status`

TS3 connection state has changed.

```json
{
  "type": "connection_status",
  "status": "connected",
  "server": "ts3.example.com:9987",
  "channel_id": 5,
  "channel_name": "Gaming",
  "error": null
}
```

| Field | Type | Always Present | Description |
|-------|------|----------------|-------------|
| `type` | `string` | Yes | Always `"connection_status"` |
| `status` | `string` | Yes | `"connected"`, `"disconnected"`, or `"reconnecting"` |
| `server` | `string` | Yes | TS3 server address |
| `channel_id` | `number \| null` | Yes | Current channel ID (`null` if disconnected) |
| `channel_name` | `string \| null` | Yes | Current channel name (`null` if disconnected) |
| `error` | `string \| null` | Yes | Error message (`null` when connected normally) |

#### Status Values

| Status | Meaning | `channel_id` | `error` |
|--------|---------|-------------|---------|
| `"connected"` | Bot is connected and operational | Set | `null` |
| `"disconnected"` | Connection was lost | `null` | Contains error details |
| `"reconnecting"` | Attempting to reconnect (auto-retry with exponential backoff) | `null` | Contains attempt info (e.g., `"Reconnection attempt 3/10"`) |

---

### 4.6 `speak_started`

TTS playback has started on the TS3 channel.

```json
{
  "type": "speak_started",
  "text": "Bonjour tout le monde !"
}
```

| Field | Type | Always Present | Description |
|-------|------|----------------|-------------|
| `type` | `string` | Yes | Always `"speak_started"` |
| `text` | `string` | Yes | The text being spoken |

---

### 4.7 `speak_completed`

TTS playback has finished.

```json
{
  "type": "speak_completed",
  "text": "Bonjour tout le monde !",
  "duration_ms": 2400
}
```

| Field | Type | Always Present | Description |
|-------|------|----------------|-------------|
| `type` | `string` | Yes | Always `"speak_completed"` |
| `text` | `string` | Yes | The text that was spoken |
| `duration_ms` | `number` | Yes | Total playback duration in milliseconds |

#### Notes

- If playback is interrupted (by `stop_speaking` or wake word), `speak_completed` may still be emitted with a shorter `duration_ms`.
- The `text` field matches the `text` from the corresponding `speak_started` event.

---

### 4.8 `client_connected`

A new client has connected to the TS3 server.

```json
{
  "type": "client_connected",
  "client_id": 8,
  "client_name": "Charlie",
  "channel_id": 1
}
```

| Field | Type | Always Present | Description |
|-------|------|----------------|-------------|
| `type` | `string` | Yes | Always `"client_connected"` |
| `client_id` | `number` | Yes | TS3 session client ID |
| `client_name` | `string` | Yes | Client display name |
| `channel_id` | `number` | Yes | Channel the client joined into |

---

### 4.9 `client_disconnected`

A client has disconnected from the TS3 server.

```json
{
  "type": "client_disconnected",
  "client_id": 8,
  "client_name": "Charlie"
}
```

| Field | Type | Always Present | Description |
|-------|------|----------------|-------------|
| `type` | `string` | Yes | Always `"client_disconnected"` |
| `client_id` | `number` | Yes | TS3 session client ID |
| `client_name` | `string` | Yes | Client display name |

---

### 4.10 `client_moved`

A client has moved to a different channel. This includes the bot itself.

```json
{
  "type": "client_moved",
  "client_id": 42,
  "client_name": "Marlbot",
  "old_channel_id": 1,
  "new_channel_id": 5
}
```

| Field | Type | Always Present | Description |
|-------|------|----------------|-------------|
| `type` | `string` | Yes | Always `"client_moved"` |
| `client_id` | `number` | Yes | TS3 session client ID |
| `client_name` | `string` | Yes | Client display name |
| `old_channel_id` | `number` | Yes | Previous channel ID |
| `new_channel_id` | `number` | Yes | New channel ID |

#### Notes

- This event fires for **all** clients, including the bot itself.
- To detect the bot's own moves, compare `client_id` with `own_client_id` from `get_status`.

---

## 5. Error Handling

### Invalid JSON

If the client sends a message that cannot be parsed as JSON:

```json
{
  "type": "command_response",
  "command_id": null,
  "success": false,
  "message": "Invalid command: expected value at line 1 column 1"
}
```

### Unknown Command Type

If the `type` field does not match any known command:

```json
{
  "type": "command_response",
  "command_id": null,
  "success": false,
  "message": "Invalid command: unknown variant `foo`, expected one of `send_message`, `move_channel`, `get_status`, `speak`, `stop_speaking`"
}
```

### Validation Errors

Commands are validated before execution. Validation errors are returned immediately:

```json
{
  "type": "command_response",
  "command_id": "cmd-001",
  "success": false,
  "message": "Validation error: content is empty"
}
```

#### All Validation Rules

| Command | Rule | Error message |
|---------|------|---------------|
| `send_message` | `content` must not be empty | `"Validation error: content is empty"` |
| `send_message` | `content` must be <= 8192 chars | `"Validation error: content exceeds 8192 characters"` |
| `send_message` | `recipient` required when `target` = `"user"` | `"Validation error: recipient required for target=user"` |
| `move_channel` | `channel_id` must be > 0 | `"Validation error: channel_id must be > 0"` |
| `speak` | `text` must not be empty | `"Validation error: text is empty"` |
| `speak` | `text` must be <= 2000 chars | `"Validation error: text exceeds 2000 characters"` |

### Runtime Errors

These occur after validation, during execution:

| Error message | Command | Cause |
|---------------|---------|-------|
| `"TS3 not connected"` | `get_status`, `move_channel` | Bot has no active TS3 connection |
| `"Failed to read TS3 state"` | `get_status` | TS3 state query returned no data |
| `"TS3 connection error: ..."` | `get_status` | Connection dropped during query |
| `"Failed to get own client ID"` | `move_channel` | Could not determine bot's own client ID |
| `"Move failed: ..."` | `move_channel` | TS3 server rejected the channel move |
| `"TTS is disabled"` | `speak` | Server started with TTS disabled |
| `"TTS pipeline not available"` | `speak` | TTS processing task crashed or is unavailable |
| `"Not implemented yet"` | `send_message` | Command not yet implemented |

---

## 6. Broadcast Model

**All events and command responses are broadcast to ALL connected WebSocket clients.**

This means:
- If client A sends a command, clients B and C will also receive the `command_response`.
- All real-time events (`message_received`, `transcription`, `client_connected`, etc.) are sent to every connected client.

### Implications for Client Implementation

1. **Always filter by `command_id`**: When sending a command, include a unique `command_id` and only process `command_response` events whose `command_id` matches your request.

2. **Ignore unrelated responses**: You will receive `command_response` events from other clients' commands. If their `command_id` is `null` or doesn't match yours, ignore them.

3. **Events are global**: All events are relevant to all clients (they describe the state of the TS3 server).

### Broadcast Buffer

The broadcast channel has a **capacity of 100 events**. If a client falls behind by more than 100 events, it will miss intermediate events (lagged). This is unlikely under normal usage.

---

## 7. Connection Lifecycle

```
Client connects via WebSocket
    |
    v
Server sends "welcome" event
    |
    v
Client can send commands at any time
    |
    +-- "get_status" --> "command_response" with server state
    +-- "move_channel" --> "command_response" + "client_moved" event
    +-- "speak" --> "command_response" + "speak_started" + "speak_completed"
    +-- "stop_speaking" --> "command_response"
    |
    v
Server pushes events asynchronously:
    +-- "message_received" (when someone types in TS3 chat)
    +-- "transcription" (when voice is transcribed after wake word)
    +-- "client_connected" / "client_disconnected" / "client_moved"
    +-- "connection_status" (TS3 connection changes)
    +-- "speak_started" / "speak_completed" (TTS playback)
    |
    v
Client closes WebSocket connection (normal close)
    OR
Server closes connection (shutdown, timeout)
```

### Server Shutdown

When the bot shuts down, the WebSocket connection is closed with code `1001` (Going Away).

---

## 8. Complete Session Example

```
-- Client connects to ws://127.0.0.1:8080/ws --

<-- Server sends:
{
  "type": "welcome",
  "bot_nickname": "Marlbot",
  "ts3_server": "ts3.example.com:9987",
  "connection_status": "connected",
  "api_version": "1.0.0"
}

--> Client sends:
{
  "type": "get_status",
  "command_id": "init-1"
}

<-- Server sends (immediate ack):
{
  "type": "command_response",
  "command_id": "init-1",
  "success": true
}

<-- Server sends (actual data):
{
  "type": "command_response",
  "command_id": "init-1",
  "success": true,
  "data": {
    "own_client_id": 42,
    "channels": [
      { "id": 1, "name": "Lobby", "parent_id": 0 },
      { "id": 5, "name": "Gaming", "parent_id": 1 }
    ],
    "clients": [
      { "id": 3, "name": "Alice", "channel_id": 5 },
      { "id": 42, "name": "Marlbot", "channel_id": 1 }
    ]
  }
}

--> Client sends:
{
  "type": "move_channel",
  "command_id": "move-1",
  "channel_id": 5
}

<-- Server sends (immediate ack):
{
  "type": "command_response",
  "command_id": "move-1",
  "success": true,
  "message": "Moving channel..."
}

<-- Server sends (result):
{
  "type": "command_response",
  "command_id": "move-1",
  "success": true,
  "message": "Moved to channel 5"
}

<-- Server sends (real-time event):
{
  "type": "client_moved",
  "client_id": 42,
  "client_name": "Marlbot",
  "old_channel_id": 1,
  "new_channel_id": 5
}

-- A user speaks on TS3 and says "marlbot dis bonjour" --

<-- Server sends:
{
  "type": "transcription",
  "timestamp": "2026-02-08T14:31:00.000Z",
  "speaker_id": 3,
  "speaker_uid": "abc123=",
  "speaker_name": "Alice",
  "text": "dis bonjour",
  "confidence": 0.92,
  "language": "fr",
  "duration_ms": 1800
}

--> Client sends:
{
  "type": "speak",
  "command_id": "tts-1",
  "text": "Bonjour Alice !"
}

<-- Server sends:
{
  "type": "command_response",
  "command_id": "tts-1",
  "success": true,
  "message": "Speech queued"
}

<-- Server sends:
{
  "type": "speak_started",
  "text": "Bonjour Alice !"
}

<-- Server sends:
{
  "type": "speak_completed",
  "text": "Bonjour Alice !",
  "duration_ms": 1200
}

-- A new user connects to TS3 --

<-- Server sends:
{
  "type": "client_connected",
  "client_id": 15,
  "client_name": "Bob",
  "channel_id": 1
}

-- Bob moves to Gaming channel --

<-- Server sends:
{
  "type": "client_moved",
  "client_id": 15,
  "client_name": "Bob",
  "old_channel_id": 1,
  "new_channel_id": 5
}

-- Alice types in TS3 channel chat --

<-- Server sends:
{
  "type": "message_received",
  "timestamp": "2026-02-08T14:32:00.000Z",
  "sender_id": 3,
  "sender_uid": "abc123=",
  "sender_name": "Alice",
  "message_type": "channel",
  "content": "Hello Bob!",
  "channel_id": 5,
  "channel_name": "Gaming"
}

-- Client disconnects --
```

---

## 9. Additional Commands {#additional-commands}

The Rust enum `WebSocketCommand` (`src/models/command.rs`) currently
exposes more commands than the five fully documented above. Their wire
format follows the same `{ type, command_id, ...fields }` convention.
Until this reference catches up, treat the Rust source as the
authoritative spec.

| Command type | Purpose (one-liner) |
|--------------|---------------------|
| `poke_client` | Send a poke (popup notification) to a specific client. |
| `kick_client` | Kick a client from the channel or the server. |
| `move_client` | Move another client to a specific channel. |
| `set_nickname` | Change the bot's own nickname. |
| `get_server_info` | Get TS3 virtual server metadata (name, welcome message, etc.). |
| `create_channel` | Create a new channel (optionally temporary, with topic / description / password). |
| `set_channel_description` | Update an existing channel's description. |
| `delete_channel` | Delete a channel. |
| `activate_listener` | Start transcribing voice from a specific client (Whisper STT). |
| `deactivate_listener` | Stop transcribing voice from a specific client. |
| `set_language` | Override the language used for STT for a specific client. |

Validation rules and exact request/response payloads for these commands
are defined in `src/models/command.rs` and handled in
`src/websocket/handlers.rs` and `src/websocket/server.rs`.

---

## 10. TypeScript Type Definitions

For convenience, here are complete TypeScript type definitions for the API:

```typescript
// ============================================================
// Commands (Client -> Server)
// ============================================================

interface GetStatusCommand {
  type: "get_status";
  command_id?: string;
}

interface MoveChannelCommand {
  type: "move_channel";
  command_id?: string;
  channel_id: number;
  password?: string | null;
}

interface SendMessageCommand {
  type: "send_message";
  command_id?: string;
  target: "channel" | "private" | "user" | "server";
  /** Stringified TS3 session client ID. Required if target is "private"/"user". */
  recipient?: string | null;
  content: string;
  tts?: boolean;
}

interface SpeakCommand {
  type: "speak";
  command_id?: string;
  text: string;
  voice?: string | null;
}

interface StopSpeakingCommand {
  type: "stop_speaking";
  command_id?: string;
}

type WebSocketCommand =
  | GetStatusCommand
  | MoveChannelCommand
  | SendMessageCommand
  | SpeakCommand
  | StopSpeakingCommand;

// ============================================================
// Events (Server -> Client)
// ============================================================

interface WelcomeEvent {
  type: "welcome";
  /** Bot's display name on TeamSpeak. (Earlier drafts called this `bot_nickname`.) */
  nickname: string;
  /** TS3 server address. (Earlier drafts called this `ts3_server`.) */
  server: string;
  connection_status: "connected" | "disconnected" | "reconnecting";
  api_version: string;
}

interface CommandResponseEvent {
  type: "command_response";
  command_id: string | null;
  success: boolean;
  message?: string;   // absent from JSON when null
  data?: any;          // absent from JSON when null
}

interface MessageReceivedEvent {
  type: "message_received";
  timestamp: string;   // ISO 8601
  sender_id: number;
  sender_uid: string;
  sender_name: string;
  message_type: "channel" | "private";
  content: string;
  channel_id: number | null;
  channel_name: string | null;
}

interface TranscriptionEvent {
  type: "transcription";
  timestamp: string;   // ISO 8601
  speaker_id: number;
  speaker_uid: string;
  speaker_name: string;
  text: string;
  confidence: number | null;
  language: string | null;
  duration_ms: number;
}

interface ConnectionStatusEvent {
  type: "connection_status";
  status: "connected" | "disconnected" | "reconnecting";
  server: string;
  channel_id: number | null;
  channel_name: string | null;
  error: string | null;
}

interface SpeakStartedEvent {
  type: "speak_started";
  text: string;
}

interface SpeakCompletedEvent {
  type: "speak_completed";
  text: string;
  duration_ms: number;
}

interface ClientConnectedEvent {
  type: "client_connected";
  client_id: number;
  client_name: string;
  channel_id: number;
}

interface ClientDisconnectedEvent {
  type: "client_disconnected";
  client_id: number;
  client_name: string;
}

interface ClientMovedEvent {
  type: "client_moved";
  client_id: number;
  client_name: string;
  old_channel_id: number;
  new_channel_id: number;
}

type WebSocketEvent =
  | WelcomeEvent
  | CommandResponseEvent
  | MessageReceivedEvent
  | TranscriptionEvent
  | ConnectionStatusEvent
  | SpeakStartedEvent
  | SpeakCompletedEvent
  | ClientConnectedEvent
  | ClientDisconnectedEvent
  | ClientMovedEvent;

// ============================================================
// get_status response data
// ============================================================

interface ServerStateData {
  own_client_id: number;
  channels: ChannelInfo[];
  clients: ClientInfo[];
}

interface ChannelInfo {
  id: number;
  name: string;
  parent_id: number;
  order: number;
  codec: string;
  codec_quality: number;
  max_clients: string;       // "Unlimited" | "Limited(N)"
  has_password: boolean;
  forced_silence: boolean;
  needed_talk_power: number;
  subscribed: boolean;
  topic: string;
}

interface ClientInfo {
  id: number;
  name: string;
  channel_id: number;
  /** Permanent unique ID as a byte array, NOT a base64 string. */
  uid: number[];
  database_id: number;
  channel_group: number;
  server_groups: number[];
  talk_power: number;
  input_muted: boolean;
  output_muted: boolean;
  is_recording: boolean;
  is_priority_speaker: boolean;
  is_channel_commander: boolean;
  country_code: string;
}
```
