// Integration test for WebSocket basic functionality
// Note: This test requires the server to be running or uses tokio_test for in-process testing

#[tokio::test]
#[ignore] // Ignored by default - run with `cargo test -- --ignored` when server is running
async fn test_websocket_connection() {
    // This would connect to ws://127.0.0.1:8080/ws
    // Verify welcome message is received
    // Verify connection stays alive for 60+ seconds

    // TODO: Implement full integration test with actual WebSocket client
    // For now, this is a placeholder
    assert!(true, "WebSocket integration test placeholder");
}

// Unit test for message serialization
#[test]
fn test_welcome_message_serialization() {
    use ts3_bot::models::WebSocketEvent;

    let welcome = WebSocketEvent::welcome(
        "Test Bot".to_string(),
        "test.server.com:9987".to_string(),
        "connected".to_string(),
    );

    let json = serde_json::to_string(&welcome).expect("Failed to serialize");
    assert!(json.contains("\"type\":\"welcome\""));
    assert!(json.contains("\"bot_nickname\":\"Test Bot\""));
    assert!(json.contains("\"api_version\":\"1.0.0\""));
}
