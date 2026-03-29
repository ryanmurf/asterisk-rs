use thiserror::Error;

/// Common error types used throughout the Asterisk Rust implementation.
#[derive(Error, Debug)]
pub enum AsteriskError {
    /// Generic internal error
    #[error("internal error: {0}")]
    Internal(String),

    /// Resource not found
    #[error("not found: {0}")]
    NotFound(String),

    /// Invalid argument
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// Operation not supported
    #[error("not supported: {0}")]
    NotSupported(String),

    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Already exists
    #[error("already exists: {0}")]
    AlreadyExists(String),

    /// Timeout
    #[error("timeout: {0}")]
    Timeout(String),

    /// Hangup
    #[error("hangup: {0}")]
    Hangup(String),

    /// Parse error
    #[error("parse error: {0}")]
    Parse(String),
}

/// Convenience Result type using AsteriskError.
pub type AsteriskResult<T> = Result<T, AsteriskError>;

/// Errors specific to channel operations.
#[derive(Error, Debug)]
pub enum ChannelError {
    /// Channel not found
    #[error("channel not found: {0}")]
    NotFound(String),

    /// Channel already hung up
    #[error("channel already hung up: {0}")]
    AlreadyHungUp(String),

    /// Invalid channel state for the requested operation
    #[error("invalid state for operation: channel is {state}, expected {expected}")]
    InvalidState {
        state: String,
        expected: String,
    },

    /// Channel technology driver error
    #[error("channel driver error: {0}")]
    DriverError(String),

    /// Frame could not be written
    #[error("frame write error: {0}")]
    WriteError(String),

    /// General channel error
    #[error("channel error: {0}")]
    Other(String),
}

/// Errors specific to bridge operations.
#[derive(Error, Debug)]
pub enum BridgeError {
    /// Bridge not found
    #[error("bridge not found: {0}")]
    NotFound(String),

    /// Channel already in bridge
    #[error("channel already in bridge: {0}")]
    ChannelAlreadyInBridge(String),

    /// Channel not in bridge
    #[error("channel not in bridge: {0}")]
    ChannelNotInBridge(String),

    /// Incompatible bridge technology
    #[error("incompatible bridge technology: {0}")]
    IncompatibleTechnology(String),

    /// General bridge error
    #[error("bridge error: {0}")]
    Other(String),
}

/// Errors specific to PBX/dialplan operations.
#[derive(Error, Debug)]
pub enum PbxError {
    /// Context not found
    #[error("context not found: {0}")]
    ContextNotFound(String),

    /// Extension not found
    #[error("extension not found: {exten}@{context}")]
    ExtensionNotFound {
        context: String,
        exten: String,
    },

    /// Priority not found
    #[error("priority not found: {exten}@{context} priority {priority}")]
    PriorityNotFound {
        context: String,
        exten: String,
        priority: i32,
    },

    /// Application not found
    #[error("application not found: {0}")]
    AppNotFound(String),

    /// Application execution failed
    #[error("application execution failed: {0}")]
    AppFailed(String),

    /// General PBX error
    #[error("PBX error: {0}")]
    Other(String),
}
