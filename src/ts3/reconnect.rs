use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn, error};

use ts3_bot::models::BotConfig;
use super::{client::TS3Client, ConnectionState};

/// Reconnection manager with exponential backoff
pub struct ReconnectionManager {
    client: TS3Client,
    config: BotConfig,
}

impl ReconnectionManager {
    pub fn new(client: TS3Client, config: BotConfig) -> Self {
        Self { client, config }
    }

    /// Attempt to reconnect with exponential backoff
    pub async fn reconnect_loop(&self) -> bool {
        let max_attempts = self.config.reconnect_max_attempts;
        let initial_delay = self.config.reconnect_initial_delay_ms;
        let max_delay = self.config.reconnect_max_delay_ms;

        info!("Starting reconnection attempts (max: {})", max_attempts);

        for attempt in 1..=max_attempts {
            // Calculate delay with exponential backoff
            let delay_ms = std::cmp::min(
                initial_delay * 2u64.pow(attempt - 1),
                max_delay
            );
            let delay = Duration::from_millis(delay_ms);

            warn!(
                "Reconnection attempt {}/{} - waiting {}ms",
                attempt, max_attempts, delay_ms
            );

            // Set state to reconnecting
            self.client.set_state(ConnectionState::Reconnecting).await;
            self.client.increment_reconnect_attempts().await;

            // Wait before attempting
            sleep(delay).await;

            // Attempt connection
            match self.client.connect().await {
                Ok(_) => {
                    info!("Reconnection successful after {} attempts", attempt);
                    self.client.reset_reconnect_attempts().await;
                    return true;
                }
                Err(e) => {
                    warn!("Reconnection attempt {} failed: {}", attempt, e);
                }
            }
        }

        error!("Max reconnection attempts ({}) reached, giving up", max_attempts);
        self.client.set_state(ConnectionState::Disconnected).await;
        false
    }
}
