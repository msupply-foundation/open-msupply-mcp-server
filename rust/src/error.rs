//! Error types for the Open mSupply MCP server.

use std::error::Error as StdError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error(
        "Cannot connect to Open mSupply server at {url}. Is the server running?{hint} (Original error: {cause})"
    )]
    Connection {
        url: String,
        hint: String,
        cause: String,
    },

    #[error(
        "SSL certificate error connecting to {url}. If using self-signed certificates, set OMSUPPLY_ALLOW_SELF_SIGNED=true. (Original error: {cause})"
    )]
    Certificate { url: String, cause: String },

    #[error("Cannot resolve hostname for {url}. Check that OMSUPPLY_URL is correct. (Original error: {cause})")]
    Dns { url: String, cause: String },

    #[error(
        "Open mSupply server at {url} is still initialising. The server must complete its initialisation and sync before the MCP server can connect. Try again shortly."
    )]
    ServerInitialising { url: String },

    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error(
        "storeId is required. Either set OMSUPPLY_STORE_ID env var or use list_stores to find a store ID and pass it explicitly."
    )]
    StoreIdRequired,

    #[error("GraphQL error: {0}")]
    Graphql(String),

    #[error("Unexpected response from Open mSupply: {0}")]
    UnexpectedResponse(String),

    #[error(transparent)]
    Request(#[from] reqwest::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

// Silence unused-import if only used through the trait method
#[allow(dead_code)]
fn _std_error_trait_in_scope(_e: &dyn StdError) {}

impl AppError {
    /// Classify a reqwest error into a user-friendly AppError variant.
    pub fn from_request(err: reqwest::Error, url: &str) -> Self {
        let msg = err.to_string();
        let cause = err.source().map(|e| e.to_string()).unwrap_or_default();
        let combined = format!("{msg} {cause}");

        if err.is_connect() || combined.contains("ECONNREFUSED") || combined.contains("Connection refused") {
            let hint = if combined.contains("::1") {
                " Try using 127.0.0.1 instead of localhost in OMSUPPLY_URL to force IPv4.".to_string()
            } else {
                String::new()
            };
            return AppError::Connection {
                url: url.to_string(),
                hint,
                cause: if cause.is_empty() { msg } else { cause },
            };
        }

        if combined.to_lowercase().contains("certificate")
            || combined.contains("self-signed")
            || combined.contains("CERT")
        {
            return AppError::Certificate {
                url: url.to_string(),
                cause: if cause.is_empty() { msg } else { cause },
            };
        }

        if combined.contains("dns") || combined.contains("ENOTFOUND") || combined.contains("failed to lookup") {
            return AppError::Dns {
                url: url.to_string(),
                cause: if cause.is_empty() { msg } else { cause },
            };
        }

        AppError::Request(err)
    }
}
