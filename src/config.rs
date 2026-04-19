//! Configuration loaded from environment variables.

use anyhow::{Context, Result};
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub url: String,
    pub username: String,
    pub password: String,
    pub store_id: Option<String>,
    pub allow_self_signed: bool,
}

fn require_env(name: &str) -> Result<String> {
    env::var(name)
        .ok()
        .filter(|s| !s.is_empty())
        .with_context(|| format!("{name} environment variable is required"))
}

pub fn load_config() -> Result<Config> {
    let raw_url = require_env("OMSUPPLY_URL")?;
    let url = raw_url.trim_end_matches('/').to_string();

    let username = require_env("OMSUPPLY_USERNAME")?;
    let password = require_env("OMSUPPLY_PASSWORD")?;

    let store_id = env::var("OMSUPPLY_STORE_ID").ok().filter(|s| !s.is_empty());

    let allow_self_signed = env::var("OMSUPPLY_ALLOW_SELF_SIGNED")
        .map(|v| matches!(v.to_lowercase().as_str(), "true" | "1" | "yes"))
        .unwrap_or(false);

    Ok(Config {
        url,
        username,
        password,
        store_id,
        allow_self_signed,
    })
}
