//! Typed errors with stable machine-readable codes and safe messages.

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    UnknownTool,
    InvalidParams,
    ProviderUnavailable,
    AuthenticationFailed,
    RateLimited,
    CompatibilityError,
    SecureStorageUnavailable,
    NotConfigured,
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnknownTool => "unknown_tool",
            Self::InvalidParams => "invalid_params",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::AuthenticationFailed => "authentication_failed",
            Self::RateLimited => "rate_limited",
            Self::CompatibilityError => "compatibility_error",
            Self::SecureStorageUnavailable => "secure_storage_unavailable",
            Self::NotConfigured => "not_configured",
            Self::Internal => "internal",
        }
    }
}

/// Error whose `Display` text is safe to show to users and models: it never
/// carries credentials, account identifiers or raw payloads.
#[derive(Debug, Clone, Error)]
#[error("{code}: {message}", code = .code.as_str())]
pub struct T212Error {
    pub code: ErrorCode,
    pub message: String,
}

impl T212Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
