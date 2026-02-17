//! Error types for OpenFGA operations.

use std::fmt;

/// Errors that can occur when interacting with OpenFGA.
#[derive(Debug)]
pub enum OpenFgaError {
    /// Failed to connect to the OpenFGA server.
    ConnectionFailed(String),
    /// The OpenFGA server returned an error.
    ServerError(String),
    /// The request timed out.
    Timeout,
    /// The authorization check was denied (used internally, not exposed as error).
    Denied,
    /// Invalid configuration.
    InvalidConfig(String),
    /// Failed to resolve object ID from request.
    ObjectResolutionFailed(String),
}

impl fmt::Display for OpenFgaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenFgaError::ConnectionFailed(msg) => write!(f, "OpenFGA connection failed: {}", msg),
            OpenFgaError::ServerError(msg) => write!(f, "OpenFGA server error: {}", msg),
            OpenFgaError::Timeout => write!(f, "OpenFGA request timed out"),
            OpenFgaError::Denied => write!(f, "Authorization denied"),
            OpenFgaError::InvalidConfig(msg) => write!(f, "Invalid OpenFGA config: {}", msg),
            OpenFgaError::ObjectResolutionFailed(msg) => {
                write!(f, "Failed to resolve object: {}", msg)
            }
        }
    }
}

impl std::error::Error for OpenFgaError {}

impl From<openfga_rs::tonic::transport::Error> for OpenFgaError {
    fn from(err: openfga_rs::tonic::transport::Error) -> Self {
        OpenFgaError::ConnectionFailed(err.to_string())
    }
}

impl From<openfga_rs::tonic::Status> for OpenFgaError {
    fn from(status: openfga_rs::tonic::Status) -> Self {
        match status.code() {
            openfga_rs::tonic::Code::DeadlineExceeded => OpenFgaError::Timeout,
            _ => OpenFgaError::ServerError(status.message().to_string()),
        }
    }
}
