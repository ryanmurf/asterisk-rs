//! ARI error types.

use crate::models::AriError;

/// Errors that can occur in ARI operations.
#[derive(Debug, thiserror::Error)]
pub enum AriErrorKind {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("unprocessable entity: {0}")]
    UnprocessableEntity(String),

    #[error("authentication required")]
    Unauthorized,

    #[error("internal error: {0}")]
    Internal(String),

    #[error("not implemented: {0}")]
    NotImplemented(String),
}

impl AriErrorKind {
    /// HTTP status code for this error.
    pub fn status_code(&self) -> u16 {
        match self {
            Self::NotFound(_) => 404,
            Self::BadRequest(_) => 400,
            Self::Conflict(_) => 409,
            Self::Forbidden(_) => 403,
            Self::UnprocessableEntity(_) => 422,
            Self::Unauthorized => 401,
            Self::Internal(_) => 500,
            Self::NotImplemented(_) => 501,
        }
    }

    /// Convert to a JSON-serializable error body.
    pub fn to_ari_error(&self) -> AriError {
        AriError {
            message: self.to_string(),
        }
    }
}

/// Result type for ARI operations.
pub type AriResult<T> = Result<T, AriErrorKind>;
