# Tasks: TeamSpeak 3 Client Bot with WebSocket API

**Input**: Design documents from `/specs/001-ts3-client-bot/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/websocket-api.md

**Tests**: Integration tests are included for critical API contracts. Unit tests can be added incrementally alongside implementation tasks.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

- **Single Rust project**: `src/`, `tests/` at repository root
- Paths follow structure defined in plan.md

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project initialization and basic Rust structure

- [x] T001 Initialize Cargo project with dependencies (tsclientlib, tokio, axum, serde, tracing, dotenvy) per research.md
- [x] T002 [P] Create module structure: src/ts3/, src/websocket/, src/models/ per plan.md
- [x] T003 [P] Create test directories: tests/integration/, tests/unit/
- [x] T004 [P] Create .env.example with configuration template from data-model.md BotConfig
- [x] T005 [P] Configure Cargo.toml with dev dependencies (tokio-test) and workspace settings
- [x] T006 [P] Setup tracing-subscriber configuration in src/main.rs for structured logging

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core infrastructure that MUST be complete before ANY user story can be implemented

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T007 Create BotConfig struct in src/models/config.rs with env var loading per data-model.md
- [x] T008 Implement BotConfig::from_env() with validation (server, nickname, port, log level)
- [x] T009 [P] Define ConnectionState enum in src/ts3/mod.rs per data-model.md TS3ConnectionState
- [x] T010 [P] Create MessageEvent struct in src/models/message.rs per data-model.md
- [x] T011 [P] Create WebSocketCommand enum in src/models/command.rs with SendMessage, MoveChannel, GetStatus variants per contracts/websocket-api.md
- [x] T012 [P] Create WebSocketEvent enum in src/models/events.rs with MessageReceived, ConnectionStatus, CommandResponse, Welcome variants per contracts/websocket-api.md
- [x] T013 Add serde Serialize/Deserialize derives to all models (T010, T011, T012)
- [x] T014 [P] Create error types in src/models/error.rs using thiserror (TS3Error, WebSocketError, ConfigError)
- [x] T015 Update src/main.rs to load config and initialize logging with configured level

**Checkpoint**: Foundation ready - user story implementation can now begin in parallel

---

## Phase 3: User Story 5 - WebSocket API Connectivity (Priority: P1) 🎯 MVP Foundation

**Goal**: WebSocket server accepts connections, sends welcome messages, handles heartbeat/ping-pong

**Independent Test**: Connect with wscat, receive welcome message, connection stays alive for 60+ seconds

### Implementation for User Story 5

- [x] T016 [US5] Create WebSocket server module in src/websocket/server.rs with Axum router and /ws endpoint
- [x] T017 [US5] Implement ws_handler in src/websocket/server.rs for WebSocket upgrade
- [x] T018 [US5] Implement handle_socket function to send Welcome event on connection per contracts/websocket-api.md
- [x] T019 [US5] Add WebSocket ping/pong mechanism with tokio::time::interval (30s ping, 60s timeout)
- [x] T020 [US5] Create WebSocket client registry in src/websocket/mod.rs using Arc<Mutex<HashMap<ClientId, WebSocketSender>>>
- [x] T021 [US5] Implement broadcast_event function in src/websocket/broadcast.rs to send events to all connected clients
- [x] T022 [US5] Add connection/disconnection logging in handle_socket
- [x] T023 [US5] Update src/main.rs to spawn WebSocket server task concurrently with Tokio
- [x] T024 [US5] Create integration test in tests/integration/websocket_basic.rs: connect, receive welcome, verify ping/pong

**Checkpoint**: WebSocket server running, clients can connect, welcome message sent, heartbeat working

---

## Phase 4: User Story 1 - Client Connection & Visibility (Priority: P1) 🎯 MVP Core

**Goal**: Bot connects to TS3 server as visible client, appears in user list, auto-reconnects on disconnect

**Independent Test**: Run bot, check TS3 client to see bot in user list, kill TS3 connection, verify bot reconnects

### Implementation for User Story 1

- [ ] T025 [P] [US1] Create TS3Client struct in src/ts3/client.rs wrapping tsclientlib client
- [ ] T026 [US1] Implement TS3Client::connect() method using BotConfig (server, nickname, password, channel) per research.md TsClientlib integration
- [ ] T027 [US1] Add ConnectionState tracking in TS3Client struct with Arc<RwLock<ConnectionState>>
- [ ] T028 [US1] Implement event stream handler in src/ts3/events.rs for connection status changes (connected, disconnected)
- [ ] T029 [US1] Broadcast ConnectionStatus event to WebSocket clients when TS3 connection state changes
- [ ] T030 [P] [US1] Create reconnection logic in src/ts3/reconnect.rs with exponential backoff per research.md (1s, 2s, 4s...max 60s)
- [ ] T031 [US1] Implement max retry limit (configurable via env RECONNECT_MAX_ATTEMPTS, default 10)
- [ ] T032 [US1] Add reconnection state notifications via ConnectionStatus events (status: "reconnecting")
- [ ] T033 [US1] Update src/main.rs to spawn TS3 client connection task concurrently
- [ ] T034 [US1] Create integration test in tests/integration/ts3_connection.rs: verify connection state events, simulate disconnect (manual test with real TS3 server)

**Checkpoint**: Bot connects to TS3, visible in client list, reconnects automatically, WebSocket clients notified of state changes

---

## Phase 5: User Story 2 - Message Reception & Broadcasting (Priority: P2)

**Goal**: Bot receives TS3 messages (channel and private) and broadcasts them to WebSocket clients in real-time

**Independent Test**: Send message to bot in TS3, verify MessageReceived event appears in connected WebSocket client within 1 second

### Implementation for User Story 2

- [ ] T035 [P] [US2] Extend event stream handler in src/ts3/events.rs to listen for message events (channel and private)
- [ ] T036 [US2] Implement message parsing in src/ts3/events.rs to create MessageEvent struct from TS3 message data per data-model.md
- [ ] T037 [US2] Add timestamp (chrono::Utc::now()), sender ID, sender name, message type, content to MessageEvent
- [ ] T038 [US2] Add channel_id and channel_name for channel messages, null for private messages
- [ ] T039 [US2] Broadcast MessageReceived events to all WebSocket clients via broadcast_event function
- [ ] T040 [US2] Add message receive logging (INFO level with sender name and content preview)
- [ ] T041 [US2] Create integration test in tests/integration/message_relay.rs: mock TS3 message event, verify WebSocket clients receive it

**Checkpoint**: Messages from TS3 appear in WebSocket clients, both channel and private messages work, metadata included

---

## Phase 6: User Story 3 - Message Sending via API (Priority: P2)

**Goal**: WebSocket clients can send commands to make the bot send messages to TS3 (channel or private)

**Independent Test**: Send send_message command via WebSocket, verify message appears in TS3 chat

### Implementation for User Story 3

- [ ] T042 [P] [US3] Create command handler module in src/websocket/handlers.rs for incoming WebSocket messages
- [ ] T043 [US3] Implement JSON parsing and deserialization to WebSocketCommand enum
- [ ] T044 [US3] Add error handling for invalid JSON and unknown command types, return CommandResponse with error
- [ ] T045 [US3] Implement handle_send_message function in src/websocket/handlers.rs for SendMessage command
- [ ] T046 [US3] Validate SendMessage: content non-empty, max 8192 chars, recipient present if target=user
- [ ] T047 [US3] Add mpsc channel (tokio::sync::mpsc) from WebSocket handler to TS3 client for sending commands
- [ ] T048 [US3] Implement TS3Client::send_message() method wrapping tsclientlib message sending
- [ ] T049 [US3] Route SendMessage commands from WebSocket → mpsc channel → TS3 client
- [ ] T050 [US3] Send CommandResponse back to requesting WebSocket client (success or error)
- [ ] T051 [US3] Add logging for sent messages (INFO level with target and content preview)
- [ ] T052 [US3] Create integration test in tests/integration/send_message.rs: send command via WebSocket, verify CommandResponse (manual verification in TS3 for actual message)

**Checkpoint**: WebSocket clients can send messages through bot to TS3, both channel and private messages work, validation errors returned

---

## Phase 7: User Story 4 - Channel Navigation (Priority: P3)

**Goal**: WebSocket clients can move the bot between channels via move_channel command

**Independent Test**: Send move_channel command with channel ID, verify bot moves in TS3 client and ConnectionStatus event sent

### Implementation for User Story 4

- [ ] T053 [P] [US4] Implement handle_move_channel function in src/websocket/handlers.rs for MoveChannel command
- [ ] T054 [US4] Validate MoveChannel: channel_id > 0
- [ ] T055 [US4] Implement TS3Client::move_channel() method wrapping tsclientlib channel switch
- [ ] T056 [US4] Handle password-protected channels (pass password from command)
- [ ] T057 [US4] Route MoveChannel commands from WebSocket → mpsc channel → TS3 client
- [ ] T058 [US4] Catch TS3 errors (channel not found, insufficient permissions, incorrect password) and return in CommandResponse
- [ ] T059 [US4] Update current_channel_id and current_channel_name in ConnectionState after successful move
- [ ] T060 [US4] Broadcast ConnectionStatus event with new channel info to all WebSocket clients
- [ ] T061 [US4] Send CommandResponse to requesting client
- [ ] T062 [US4] Add logging for channel moves (INFO level)
- [ ] T063 [US4] Create integration test in tests/integration/move_channel.rs: send move_channel command, verify CommandResponse and ConnectionStatus event (manual verification in TS3)

**Checkpoint**: Bot can navigate between channels via API, errors handled gracefully, all clients notified of channel changes

---

## Phase 8: Additional Commands & Status Query

**Goal**: Implement get_status command and finalize command handling

**Independent Test**: Send get_status command, receive current connection state in response

### Implementation

- [ ] T064 [P] Implement handle_get_status function in src/websocket/handlers.rs for GetStatus command
- [ ] T065 Read current ConnectionState from TS3Client
- [ ] T066 Format state data as JSON (connection_state, server, current_channel_id, current_channel_name, reconnect_attempts)
- [ ] T067 Send CommandResponse with data field containing state
- [ ] T068 Add command_id support in CommandResponse (echo from request if provided)
- [ ] T069 Update all command handlers to use command_id from request
- [ ] T070 Create integration test in tests/integration/get_status.rs: send get_status, verify response data

**Checkpoint**: All WebSocket commands implemented (send_message, move_channel, get_status), command_id echoing works

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Improvements that affect multiple user stories

- [ ] T071 [P] Add comprehensive error logging for all TS3 errors (connection failures, message send failures, etc.)
- [ ] T072 [P] Add optional file logging support (check LOG_FILE env var, create file appender if set)
- [ ] T073 [P] Create README.md with setup instructions, configuration reference, and WebSocket API quick reference
- [ ] T074 [P] Add example WebSocket client script (Python or Node.js) demonstrating connection, sending commands, receiving events
- [ ] T075 [P] Verify all success criteria from spec.md (connection <5s, message latency <1s, command execution <500ms, etc.)
- [ ] T076 [P] Run quickstart.md validation: follow setup guide, verify all steps work
- [ ] T077 [P] Add Cargo clippy check and fix any warnings
- [ ] T078 [P] Add unit tests for command validation logic in tests/unit/command_validation.rs
- [ ] T079 [P] Add unit tests for configuration parsing in tests/unit/config_test.rs
- [ ] T080 [P] Document known limitations (no voice, no file transfer, single server) in README.md
- [ ] T081 Code cleanup: remove TODOs, unused imports, debug prints
- [ ] T082 Performance check: verify CPU usage <5% idle (use top/htop while bot running)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies - can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion - BLOCKS all user stories
- **User Stories (Phase 3-7)**: All depend on Foundational phase completion
  - **US5 (Phase 3, P1)**: WebSocket API - Foundation for all API interactions
  - **US1 (Phase 4, P1)**: TS3 Connection - Foundation for TS3 interactions
  - **US2 (Phase 5, P2)**: Message Reception - Depends on US1 (TS3 connection) and US5 (WebSocket)
  - **US3 (Phase 6, P2)**: Message Sending - Depends on US1 (TS3 connection) and US5 (WebSocket)
  - **US4 (Phase 7, P3)**: Channel Navigation - Depends on US1 (TS3 connection) and US5 (WebSocket)
- **Additional Commands (Phase 8)**: Depends on US5 (WebSocket API infrastructure)
- **Polish (Phase 9)**: Depends on all desired user stories being complete

### User Story Dependencies

- **User Story 5 (P1) - WebSocket API**: Can start after Foundational - No dependencies on other stories
- **User Story 1 (P1) - TS3 Connection**: Can start after Foundational - Independent from US5 initially, integrates later
- **User Story 2 (P2) - Message Reception**: Requires US1 (TS3 connection for events) AND US5 (WebSocket for broadcasting)
- **User Story 3 (P2) - Message Sending**: Requires US1 (TS3 client for sending) AND US5 (WebSocket for commands)
- **User Story 4 (P3) - Channel Navigation**: Requires US1 (TS3 client for moving) AND US5 (WebSocket for commands)

### Within Each User Story

- Models/structs before usage in handlers
- Core implementation before integration
- Error handling alongside feature implementation
- Tests can be written in parallel with implementation or after (TDD optional)

### Parallel Opportunities

- **Phase 1**: T002, T003, T004, T005, T006 can all run in parallel
- **Phase 2**: T009, T010, T011, T012, T014 can run in parallel (different model files)
- **Phase 3 (US5)**: T016-T018 sequential, but T024 (test) can be parallel with late implementation tasks
- **Phase 4 (US1)**: T025, T030 can start in parallel, T034 (test) can be parallel with late tasks
- **Phase 5 (US2)**: T035, T036 sequential, T041 (test) can be parallel
- **Phase 6 (US3)**: T042, T045 can start in parallel, T052 (test) can be parallel
- **Phase 7 (US4)**: T053, T063 can be parallel with late implementation
- **Phase 8**: T064, T070 can be parallel
- **Phase 9**: T071-T080 can mostly run in parallel (different files/concerns)

- **User Stories US5 and US1** can be developed in parallel by different developers (both P1)
- **User Stories US2 and US3** can be developed in parallel after US1+US5 complete (both P2)

---

## Parallel Example: User Story 1 (TS3 Connection)

```bash
# Launch model/struct creation in parallel:
Task: "Create TS3Client struct in src/ts3/client.rs"
Task: "Create reconnection logic in src/ts3/reconnect.rs"

# Sequential after structs:
Task: "Implement TS3Client::connect() method"
Task: "Implement event stream handler"
Task: "Broadcast ConnectionStatus events"
```

---

## Parallel Example: User Story 5 (WebSocket API)

```bash
# Launch WebSocket infrastructure tasks in parallel:
Task: "Create WebSocket server module in src/websocket/server.rs"
Task: "Implement broadcast_event function in src/websocket/broadcast.rs"

# Sequential integration:
Task: "Implement handle_socket function"
Task: "Add ping/pong mechanism"
```

---

## Implementation Strategy

### MVP First (US5 + US1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL - blocks all stories)
3. Complete Phase 3: User Story 5 (WebSocket API)
4. Complete Phase 4: User Story 1 (TS3 Connection)
5. **STOP and VALIDATE**:
   - Connect via wscat, verify welcome message
   - Check TS3 client, bot appears in user list
   - Kill connection, verify reconnect
6. Deploy/demo if ready - **This is a working bot presence system**

### Incremental Delivery

1. **Foundation** (Phase 1 + 2) → Config loading and logging work
2. **+ US5** (Phase 3) → WebSocket API functional, can connect and query
3. **+ US1** (Phase 4) → Bot connects to TS3, auto-reconnects → **MVP Deploy!**
4. **+ US2** (Phase 5) → Bot relays TS3 messages to WebSocket → **Message Monitor Deploy!**
5. **+ US3** (Phase 6) → Bot can send messages via API → **Interactive Bot Deploy!**
6. **+ US4** (Phase 7) → Bot can move between channels → **Full Mobility Deploy!**
7. **+ Polish** (Phase 9) → Production-ready

Each increment adds value without breaking previous functionality.

### Parallel Team Strategy

With 2 developers:

1. Both complete Setup + Foundational together
2. Once Foundational is done:
   - **Developer A**: User Story 5 (WebSocket API)
   - **Developer B**: User Story 1 (TS3 Connection)
3. Integrate US5 + US1 (both should work independently, integration is connecting events)
4. **Developer A**: User Story 2 (Message Reception)
5. **Developer B**: User Story 3 (Message Sending) - can work in parallel with US2
6. Integrate US2 + US3
7. Either developer: User Story 4 (Channel Navigation)
8. Both: Polish phase

---

## Notes

- [P] tasks = different files, no dependencies on incomplete tasks
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Integration tests require real TS3 server for full validation (can use public test servers)
- TsClientlib examples in repo are helpful references for TS3 client implementation
- WebSocket testing can be done with wscat (npm install -g wscat) or browser DevTools
- Commit after each task or logical group (e.g., after completing a command handler)
- Stop at any checkpoint to validate story independently
- The two P1 stories (US5 + US1) together form the minimal viable bot

---

## Task Summary

**Total Tasks**: 82
**Setup Tasks**: 6
**Foundational Tasks**: 9
**User Story Tasks**: 59 (spread across 5 stories)
**Polish Tasks**: 12

**Tasks per User Story**:
- US5 (WebSocket API): 9 tasks
- US1 (TS3 Connection): 10 tasks
- US2 (Message Reception): 7 tasks
- US3 (Message Sending): 11 tasks
- US4 (Channel Navigation): 11 tasks
- Additional Commands: 7 tasks
- Polish: 12 tasks

**Parallel Opportunities**: ~30 tasks marked [P] can run in parallel within their phase

**Independent Test Criteria**:
- US5: Connect with wscat, receive welcome, stays alive 60+ seconds
- US1: Bot visible in TS3, reconnects after disconnect
- US2: TS3 message appears in WebSocket client <1s
- US3: WebSocket command results in TS3 message
- US4: Bot moves to new channel, clients notified

**Suggested MVP Scope**: Phase 1 + Phase 2 + Phase 3 (US5) + Phase 4 (US1) = **25 tasks** for minimal working bot

**Format Validation**: ✅ All tasks follow checklist format with ID, [P] where applicable, [Story] labels for user story tasks, and file paths
