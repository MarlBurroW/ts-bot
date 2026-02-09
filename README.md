# TS3 Bot — TeamSpeak WebSocket Bridge

Bot Rust qui se connecte à un serveur TeamSpeak 3 et expose une API WebSocket pour l'intégration avec OpenClaw.

## Fonctionnalités

- **Connexion TS3** : se connecte comme client au serveur TeamSpeak
- **WebSocket API** : expose un serveur WS pour contrôler le bot (parler, bouger, status)
- **STT** : détection de wake word ("Marlbot") + transcription vocale via Whisper
- **TTS** : synthèse vocale via OpenAI TTS API (ou compatible)
- **Bridge OpenClaw** : plugin channel OpenClaw pour intégration complète (le bot devient un channel comme Discord/Telegram)

## Architecture

```
TeamSpeak Server ←→ [TS3 Bot Rust] ←→ WebSocket ←→ [Plugin OpenClaw] ←→ Agent IA
                     Port 8080              Channel "teamspeak"
```

## Configuration

Fichier `.env` à la racine du projet :

```env
# TeamSpeak 3 Connection
TS3_SERVER=13.39.63.90:9987
TS3_NICKNAME=Marlbot
TS3_PASSWORD=
TS3_CHANNEL=

# WebSocket API
WS_HOST=0.0.0.0
WS_PORT=8080

# Logging
LOG_LEVEL=INFO
LOG_FILE=

# Reconnection
RECONNECT_MAX_ATTEMPTS=10
RECONNECT_INITIAL_DELAY_MS=1000
RECONNECT_MAX_DELAY_MS=60000

# TTS (Text-to-Speech) via OpenAI API
TTS_ENABLED=true
TTS_API_URL=https://api.openai.com/v1/audio/speech
TTS_API_KEY=<clé OpenAI>
TTS_MODEL=tts-1
TTS_VOICE=nova
```

## Build

```bash
# Prérequis système
sudo apt install build-essential cmake libclang-dev libopus-dev libssl-dev pkg-config

# Build
source ~/.cargo/env  # si Rust installé via rustup
cargo build --release
```

Le binaire est dans `target/release/ts3_bot`.

## Modèle Whisper

Le bot utilise Whisper (small) pour la transcription vocale. Le modèle est dans `models/ggml-small.bin` (465MB). S'il manque :

```bash
mkdir -p models
wget -O models/ggml-small.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin
```

## Service systemd

Le bot tourne comme service systemd `ts3-bot` :

```bash
# Status
sudo systemctl status ts3-bot

# Logs
sudo journalctl -u ts3-bot -f

# Restart
sudo systemctl restart ts3-bot

# Stop
sudo systemctl stop ts3-bot
```

Fichier service : `/etc/systemd/system/ts3-bot.service`

## Plugin OpenClaw

Le plugin channel est dans `~/.openclaw/extensions/teamspeak/`. Config dans `openclaw.json` :

```json
{
  "channels": {
    "teamspeak": {
      "enabled": true,
      "wsUrl": "ws://127.0.0.1:8080/ws",
      "outputMode": "voice",
      "voice": "nova"
    }
  }
}
```

### Output modes

- `voice` (défaut) : réponses en TTS sur le channel TS
- `text` : messages texte dans le chat TS (pas encore implémenté côté Rust)
- `both` : les deux

### Tools agent

Le plugin expose 3 tools :
- `teamspeak_status` — état du serveur (qui est en ligne, channels)
- `teamspeak_move` — déplacer le bot vers un channel (par nom ou ID)
- `teamspeak_stop_speaking` — interrompre le TTS en cours

## API WebSocket

Documentation complète : `docs/ts3_websocket_api.md`

### Commandes principales

| Commande | Description |
|----------|-------------|
| `get_status` | État du serveur (channels, clients, bot ID) |
| `move_channel` | Déplacer le bot vers un channel |
| `speak` | Synthèse vocale TTS sur le channel courant |
| `stop_speaking` | Interrompre le TTS |

### Événements

| Événement | Description |
|-----------|-------------|
| `welcome` | Envoyé à la connexion WS |
| `message_received` | Message texte reçu sur TS |
| `transcription` | Transcription vocale après wake word |
| `client_connected/disconnected/moved` | Événements clients |
| `speak_started/completed` | État du TTS |

## Identité TS3

Le bot crée une identité TS3 dans `.ts3_identity` au premier lancement. Si le serveur a des restrictions de sécurité ou des groupes liés à une identité spécifique, il faut importer l'ancienne identité.

## Stack technique

- **Rust** + Tokio (async runtime)
- **tsclientlib** — client TS3 natif
- **axum** — serveur WebSocket
- **whisper-rs** — STT via whisper.cpp
- **audiopus** — décodage audio Opus
- **ureq** — client HTTP pour TTS API
