use std::env;
use std::net::SocketAddr;

use anyhow::{Context, bail};

/// Runtime configuration, sourced entirely from the environment.
pub struct Config {
    pub bind: SocketAddr,
    pub db_path: String,
    pub secret: Vec<u8>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind: SocketAddr = env::var("COCOON_BIND")
            .unwrap_or_else(|_| "127.0.0.1:3000".to_string())
            .parse()
            .context("COCOON_BIND must be a valid socket address, e.g. 127.0.0.1:3000")?;

        let db_path = env::var("COCOON_DB").unwrap_or_else(|_| "cocoon.db".to_string());

        let secret = env::var("COCOON_HMAC_SECRET")
            .context("COCOON_HMAC_SECRET must be set (>= 16 bytes)")?;
        if secret.len() < 16 {
            bail!("COCOON_HMAC_SECRET must be at least 16 bytes");
        }

        Ok(Self {
            bind,
            db_path,
            secret: secret.into_bytes(),
        })
    }
}
