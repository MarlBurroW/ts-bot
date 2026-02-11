use thiserror::Error;

#[derive(Error, Debug)]
pub enum TS3Error {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Authentication failed: {0}")]
    AuthFailed(String),
}

#[derive(Error, Debug)]
pub enum WebSocketError {
    #[error("Invalid command: {0}")]
    InvalidCommand(String),
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Missing required config: {0}")]
    MissingField(String),
}
