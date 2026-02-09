use crate::models::WebSocketEvent;
use tokio::sync::broadcast;
use tracing::warn;

/// Broadcast channel for sending events to all WebSocket clients
pub type EventBroadcaster = broadcast::Sender<WebSocketEvent>;
pub type EventReceiver = broadcast::Receiver<WebSocketEvent>;

/// Create a new broadcast channel for WebSocket events
/// Capacity of 100 events (old events will be dropped if channel is full)
pub fn create_broadcaster() -> EventBroadcaster {
    let (tx, _rx) = broadcast::channel(100);
    tx
}

/// Broadcast an event to all connected WebSocket clients
pub fn broadcast_event(broadcaster: &EventBroadcaster, event: WebSocketEvent) {
    match broadcaster.send(event) {
        Ok(receiver_count) => {
            if receiver_count > 0 {
                tracing::debug!("Event broadcast to {} clients", receiver_count);
            }
        }
        Err(e) => {
            warn!("Failed to broadcast event (no receivers): {:?}", e);
        }
    }
}
