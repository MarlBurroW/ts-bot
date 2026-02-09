# ts-bot Development Guidelines

Bot TeamSpeak 3 qui se connecte comme un client réel (pas via API admin) et expose une API WebSocket pour lire/envoyer des messages, transcrire l'audio vocal (STT) et contrôler le bot.

**Branche feature** : `002-trigger-word-refactor`
**Dernière mise à jour** : 2026-02-08

## Technologies

- **Langage** : Rust 1.75+ (edition 2021)
- **Runtime async** : Tokio (features: full)
- **Serveur WebSocket** : Axum 0.7 (endpoint `/ws`)
- **Protocole TS3** : tsclientlib 0.2 + ts-bookkeeping 0.1 + tsproto-packets 0.1.0 (protocole client natif, PAS ServerQuery)
- **STT** : whisper-rs 0.12 avec feature `whisper-cpp-tracing` (modèle ggml-small.bin)
- **Audio** : audiopus 0.2 (décodeur Opus), hound 3.5 (WAV)
- **Logging** : tracing + tracing-subscriber avec env-filter
- **Config** : dotenvy 0.15 + envy 0.4 (variables d'environnement + fichier .env)
- **Erreurs** : anyhow 1.0 + thiserror 1.0
- **Sérialization** : serde + serde_json (JSON pour WebSocket)
- **Dates** : chrono 0.4 avec feature serde

## Build Environment (Windows)

```bash
# Variables requises (déjà set via setx, mais à exporter dans chaque nouveau shell)
export LIBCLANG_PATH="d:\projets\ts-bot\clang+llvm-18.1.8-x86_64-pc-windows-msvc\bin"
export CMAKE_GENERATOR="Visual Studio 17 2022"
```

- `LIBCLANG_PATH` est requis par whisper-rs-sys (compilation whisper.cpp)
- `CMAKE_GENERATOR` corrige l'auto-détection qui choisit la mauvaise version de VS

## Commands

```bash
cargo build                # Compilation
cargo test                 # Tests
cargo clippy               # Linting
cargo run                  # Lancement (nécessite .env configuré)
```

## Project Structure

```text
src/
├── main.rs                # Point d'entrée, boucle événements TS3 + audio
├── lib.rs                 # Exports publics (models, websocket)
├── ts3/                   # Module connexion TeamSpeak
│   ├── mod.rs             # ConnectionState enum + TS3ConnectionState struct
│   ├── client.rs          # TS3Client : wrapper tsclientlib, connect(), identité persistée
│   ├── events.rs          # BookEvents handler (client join/leave/move)
│   └── reconnect.rs       # ReconnectionManager : backoff exponentiel
├── websocket/             # Module serveur WebSocket
│   ├── mod.rs             # Re-export run_server
│   ├── server.rs          # Axum WS server, handle_socket, get_status/move_channel exec
│   ├── handlers.rs        # handle_command() - validation + CommandAction dispatch
│   └── broadcast.rs       # EventBroadcaster (tokio::broadcast, capacité 100)
├── audio/                 # Module traitement audio
│   ├── mod.rs             # Re-exports
│   ├── decoder.rs         # OpusDecoder : Opus 48kHz → PCM i16 → resample 16kHz f32
│   ├── buffer.rs          # AudioBuffer (per-speaker, VecDeque FIFO) + SpeakerBufferManager
│   ├── whisper.rs         # WhisperTranscriber : STT avec anti-hallucination
│   └── wake_word.rs       # WakeWordDetector : détection fuzzy "marlbot" + variantes
└── models/                # Structures de données partagées
    ├── mod.rs             # Re-exports
    ├── config.rs          # BotConfig : chargement + validation env vars
    ├── message.rs         # MessageEvent + MessageType (Channel/Private)
    ├── command.rs         # WebSocketCommand (SendMessage/MoveChannel/GetStatus)
    ├── events.rs          # WebSocketEvent (Welcome/MessageReceived/Transcription/...)
    ├── error.rs           # TS3Error, WebSocketError, ConfigError (thiserror)
    └── transcription.rs   # TranscriptionEvent (résultat STT)

tests/                     # Tests d'intégration
specs/001-ts3-client-bot/  # Spécifications feature
  ├── spec.md              # Spécification fonctionnelle
  ├── plan.md              # Plan d'implémentation
  ├── data-model.md        # Modèle de données
  ├── contracts/
  │   └── websocket-api.md # Contrat API WebSocket v1.0.0
  ├── research.md
  ├── quickstart.md
  ├── checklists/
  │   └── requirements.md
  └── tasks.md
models/                    # Modèles Whisper (ggml-small.bin)
```

## Configuration (.env)

```bash
# TeamSpeak 3
TS3_SERVER=ts3.example.com:9987    # Adresse serveur (port 9987 par défaut)
TS3_NICKNAME=OpenClaw Bot           # Nom affiché (max 30 chars)
TS3_PASSWORD=                       # Mot de passe serveur (optionnel)
TS3_CHANNEL=                        # Channel initial (optionnel)

# WebSocket API
WS_HOST=127.0.0.1                  # Bind address (localhost only, pas d'auth)
WS_PORT=8080                       # Port WebSocket

# Logging
LOG_LEVEL=INFO                     # ERROR | WARN | INFO | DEBUG | TRACE
LOG_FILE=                          # Fichier log (optionnel)

# Reconnexion (optionnel, valeurs par défaut)
RECONNECT_MAX_ATTEMPTS=10
RECONNECT_INITIAL_DELAY_MS=1000    # Backoff exponentiel : delay * 2^(n-1)
RECONNECT_MAX_DELAY_MS=60000       # Plafond backoff
```

## Architecture

### Flux principal (main.rs)

```
tokio::spawn(TS3 task)  ←── SyncConnection (tsclientlib)
    │                           │
    │  tokio::select! {         │
    │    event = sync_con ──────┘ BookEvents (messages) + Audio (paquets Opus)
    │    silence_check ──────────→ Timer 500ms pour détecter fin de parole
    │    whisper_result ─────────→ Résultats STT depuis spawn_blocking
    │  }
    │
    ├── broadcast::channel ────→ WebSocketEvent vers tous les clients WS
    │
    └── mpsc::channel ─────────→ Messages sortants TS3 (via SyncConnection handle)

tokio::spawn(WebSocket server) ←── Axum sur /ws
    └── Par client : split sender/receiver + subscribe broadcast
```

### Pipeline Audio

```
TS3 Audio Packets (Opus 48kHz, par speaker via `from` field u16)
    → OpusDecoder::decode() → PCM i16 48kHz
    → OpusDecoder::resample_to_16khz() → f32 16kHz (décimation ×3)
    → SpeakerBufferManager (buffer FIFO par speaker, max 30s)
    → Wake word check toutes les 3s (si pas busy) via spawn_blocking
    → Si wake word détecté : activate buffer, "J'écoute" en chat
    → Silence timeout 2s → transcription complète via spawn_blocking
    → TranscriptionEvent → broadcast WebSocket
```

### Anti-hallucination Whisper

- **RMS energy check** : `MIN_SPEECH_RMS = 0.005` - ignore le silence
- **Détection de répétition** : filtre "mais mais mais..." / "et et et..."
- **Limite tokens** : `max_tokens = 10` (wake word) / `100` (transcription)
- **Seuil no_speech** : `no_speech_thold = 0.5`
- **Logs C redirigés** : `whisper-cpp-tracing` feature + `install_whisper_tracing_trampoline()`

### Wake Word "marlbot"

Détection fuzzy car Whisper entend mal : malbut, malbot, marlbut, marbot, marbut, melbot, melbut, malbec, aimalbot, + variantes avec espaces. Normalisation du texte (ponctuation supprimée, lowercase, espaces dédupliqués). `extract_command_clean()` retire TOUTES les occurrences du wake word.

## WebSocket API (v1.0.0)

**Endpoint** : `ws://127.0.0.1:8080/ws`
**Auth** : Aucune (localhost only)

### Client → Server (Commands)

| Type | Description | Champs clés |
|------|-------------|-------------|
| `send_message` | Envoyer message TS3 | `target` (channel/user), `content`, `recipient?`, `command_id?` |
| `move_channel` | Changer de channel | `channel_id`, `password?`, `command_id?` |
| `get_status` | État connexion | `command_id?` |

### Server → Client (Events)

| Type | Description |
|------|-------------|
| `welcome` | Envoyé à la connexion WS (nickname, server, api_version) |
| `message_received` | Message TS3 relayé (sender, content, type, timestamp) |
| `transcription` | Résultat STT (speaker, text, language, duration_ms) |
| `connection_status` | Changement état TS3 (connected/disconnected/reconnecting) |
| `command_response` | Réponse à une commande (success, message, data?) |
| `speak_started` | TTS a commencé à jouer (text) |
| `speak_completed` | TTS terminé (text, duration_ms) |
| `client_connected` | Un client rejoint le serveur (client_id, client_name, channel_id) |
| `client_disconnected` | Un client quitte le serveur (client_id, client_name) |
| `client_moved` | Un client change de channel (client_id, client_name, old/new_channel_id) |

## Code Style

- Rust standard conventions (rustfmt, clippy)
- Async/await partout (Tokio runtime)
- `Arc<Mutex<T>>` pour état partagé entre tasks
- `spawn_blocking` pour le code Whisper (CPU-bound)
- `thiserror` pour les types d'erreur, `anyhow` pour la propagation
- Logs structurés via `tracing` (info!, warn!, error!, debug!)

## Pièges connus

- **audiopus** : Utiliser v0.2, PAS v0.3 (n'existe pas en stable)
- **Silence TS3** : Pas de paquets envoyés = détecter via timer périodique, pas dans le handler audio
- **Whisper modèle** : Doit être dans `models/ggml-small.bin`, sinon STT désactivé (graceful degradation)
- **tsclientlib identité** : Persistée dans `.ts3_identity` (JSON). Sans ce fichier, server groups perdus au restart
- **SyncConnection deadlock** : `with_connection()`/`send_command()` bloquent si le stream n'est pas pollé — spawner avec délai si appelé avant la boucle select!
- **builder.channel()** : Attend un NOM de channel, pas un ID numérique — utiliser `clientmove` pour les déplacements programmatiques
- **Channel paths spéciaux** : Les spacers et emojis dans les noms de channel échouent silencieusement
- **base64** : v0.13 utilisée pour encoder les UIDs TeamSpeak

## Status d'implémentation

- [x] Connexion TS3 client natif (SyncConnection)
- [x] Serveur WebSocket Axum avec broadcast
- [x] Réception et relay messages TS3 → WebSocket
- [x] Pipeline audio complet (Opus → Whisper)
- [x] Wake word detection avec fuzzy matching
- [x] Transcription STT et broadcast
- [x] Reconnexion automatique (backoff exponentiel)
- [x] Configuration via .env
- [x] Commande WebSocket `get_status` (channels + clients + own_client_id)
- [x] Commande WebSocket `move_channel` (clientmove via OutCommand)
- [x] Events temps réel (client_connected/disconnected/moved via BookEvents)
- [x] Persistance identité TS3 (.ts3_identity)
- [x] Persistance dernier channel (.last_channel + clientmove au connect)
- [x] TTS via HTTP API (speak/stop_speaking)
- [ ] Exécution commande WebSocket `send_message` - Phase 6-7

<!-- MANUAL ADDITIONS START -->
<!-- MANUAL ADDITIONS END -->
