//! Entry point for the Open mSupply MCP server (Rust rewrite).

mod client;
mod config;
mod error;
mod format;
mod tools;

use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;

use crate::client::OmSupplyClient;
use crate::tools::OmSupplyServer;

#[tokio::main]
async fn main() -> Result<()> {
    // Log to stderr so it does not corrupt the stdio MCP transport on stdout.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let cfg = config::load_config().context("failed to load configuration")?;
    tracing::info!(url = %cfg.url, "starting omsupply-mcp-server");

    let client = Arc::new(OmSupplyClient::new(cfg).context("failed to build HTTP client")?);
    let server = OmSupplyServer::new(client);

    let service = server.serve(stdio()).await.context("failed to start MCP service")?;
    service.waiting().await?;
    Ok(())
}
