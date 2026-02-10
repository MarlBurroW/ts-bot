use std::sync::Arc;
use tokio::sync::RwLock;
use tsclientlib::{Connection, Identity};
use anyhow::Result;
use tracing::{info, warn, error};
use slog::{Drain, Logger, OwnedKVList, Record};

use ts3_bot::models::BotConfig;
use super::{ConnectionState, TS3ConnectionState};

/// A slog drain that filters out the "Sending audio while muted" warning.
/// All other messages are discarded (tsclientlib logs are noisy and we use tracing).
struct FilteredDrain;

impl Drain for FilteredDrain {
    type Ok = ();
    type Err = slog::Never;

    fn log(&self, _record: &Record, _values: &OwnedKVList) -> Result<(), slog::Never> {
        // Drop all slog messages — we use tracing for our own logging.
        // This silences tsclientlib's "Sending audio while muted" warning
        // which fires on every audio frame (~50x per TTS playback) due to
        // channel talk_power requirements that don't actually block audio.
        Ok(())
    }
}

const IDENTITY_FILE: &str = ".ts3_identity";

pub struct TS3Client {
    pub state: Arc<RwLock<TS3ConnectionState>>,
    config: BotConfig,
}

impl TS3Client {
    pub fn new(config: BotConfig) -> Self {
        let state = TS3ConnectionState::new(config.ts3_server.clone());

        Self {
            state: Arc::new(RwLock::new(state)),
            config,
        }
    }

    /// Connect to TS3 server
    pub async fn connect(&self) -> Result<Connection> {
        info!("Connecting to TS3 server: {}", self.config.ts3_server);

        // Update state to connecting
        {
            let mut state = self.state.write().await;
            state.state = ConnectionState::Connecting;
            state.last_error = None;
        }

        // Parse server address
        let address = if self.config.ts3_server.contains(':') {
            self.config.ts3_server.clone()
        } else {
            format!("{}:9987", self.config.ts3_server)
        };

        // Build connection using the new API
        // Use a silent slog logger to suppress tsclientlib's "Sending audio while muted"
        // warnings that fire on every audio frame due to channel talk_power.
        let silent_logger = Logger::root(FilteredDrain, slog::o!());
        let mut builder = Connection::build(address.clone())
            .logger(silent_logger.clone());

        // Set nickname
        builder = builder.name(self.config.ts3_nickname.clone());

        // Set password if provided
        if let Some(ref password) = self.config.ts3_password {
            if !password.is_empty() {
                builder = builder.password(password.clone());
            }
        }

        // Set channel from config (if provided)
        // Note: .last_channel persistence uses clientmove after connect (in main.rs)
        let has_channel = self.config.ts3_channel.as_ref()
            .map_or(false, |c| !c.is_empty());
        if let Some(ref channel) = self.config.ts3_channel {
            if !channel.is_empty() {
                info!("Joining channel from config: {}", channel);
                builder = builder.channel(channel.clone());
            }
        }

        // Load or create persistent identity
        let identity = match std::fs::read_to_string(IDENTITY_FILE) {
            Ok(json) => {
                match serde_json::from_str::<Identity>(&json) {
                    Ok(id) => {
                        info!("Loaded TS3 identity (level {})", id.level());
                        id
                    }
                    Err(e) => {
                        warn!("Failed to parse {}, creating new identity: {}", IDENTITY_FILE, e);
                        let id = Identity::create();
                        if let Ok(j) = serde_json::to_string(&id) {
                            let _ = std::fs::write(IDENTITY_FILE, j);
                        }
                        id
                    }
                }
            }
            Err(_) => {
                info!("Creating new TS3 identity");
                let id = Identity::create();
                if let Ok(j) = serde_json::to_string(&id) {
                    let _ = std::fs::write(IDENTITY_FILE, &j);
                    info!("Saved identity to {} (level {})", IDENTITY_FILE, id.level());
                }
                id
            }
        };
        builder = builder.identity(identity.clone());

        // Connect (synchronous in tsclientlib 0.2)
        // If connecting with a specific channel fails, retry without channel
        // (e.g. channel full, deleted, password changed)
        let result = builder.connect();
        let con = match result {
            Ok(con) => con,
            Err(e) if has_channel => {
                warn!("Failed to join channel, retrying without channel: {:?}", e);
                let mut retry = Connection::build(address)
                    .logger(silent_logger);
                retry = retry.name(self.config.ts3_nickname.clone());
                retry = retry.identity(identity);
                if let Some(ref password) = self.config.ts3_password {
                    if !password.is_empty() {
                        retry = retry.password(password.clone());
                    }
                }
                retry.connect().map_err(|e2| {
                    let error_msg = format!("{:?}", e2);
                    error!("Failed to connect to TS3: {}", error_msg);
                    anyhow::anyhow!("TS3 connection failed")
                })?
            }
            Err(e) => {
                let error_msg = format!("{:?}", e);
                error!("Failed to connect to TS3: {}", error_msg);

                // Update state to disconnected with error
                {
                    let mut state = self.state.write().await;
                    state.state = ConnectionState::Disconnected;
                    state.last_error = Some(error_msg);
                }

                return Err(anyhow::anyhow!("TS3 connection failed"));
            }
        };

        info!("Successfully connected to TS3 server");

        // Update state to connected
        {
            let mut state = self.state.write().await;
            state.state = ConnectionState::Connected;
            state.reconnect_attempts = 0;
        }

        Ok(con)
    }

}
