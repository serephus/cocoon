use std::env;
use std::net::SocketAddr;

use anyhow::{Context, bail};

/// Runtime configuration, sourced from the environment.
#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub db_path: String,
    pub bot_token: String,
    pub webhook_secret: String,
    pub webhook_path: String,
    pub public_url: Option<String>,
    pub register_webhook: bool,
    pub max_subscriptions: i64,
    pub proxy: Option<String>,
    pub api_url: Option<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind: SocketAddr = env::var("COCOON_BIND")
            .unwrap_or_else(|_| "127.0.0.1:3000".to_string())
            .parse()
            .context("COCOON_BIND must be a valid socket address, e.g. 127.0.0.1:3000")?;
        let db_path = env::var("COCOON_DB").unwrap_or_else(|_| "cocoon.db".to_string());

        let bot_token = resolve_secret(
            "COCOON_TELEGRAM_BOT_TOKEN",
            "COCOON_TELEGRAM_BOT_TOKEN_FILE",
            10,
            usize::MAX,
            None,
        )?;
        let webhook_secret = resolve_secret(
            "COCOON_TELEGRAM_WEBHOOK_SECRET",
            "COCOON_TELEGRAM_WEBHOOK_SECRET_FILE",
            16,
            256,
            Some("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-"),
        )?;

        let webhook_path = env::var("COCOON_TELEGRAM_WEBHOOK_PATH")
            .unwrap_or_else(|_| "/telegram/webhook".to_string());
        let public_url = env::var("COCOON_PUBLIC_URL").ok();
        let register_webhook = parse_bool(
            &env::var("COCOON_TELEGRAM_REGISTER_WEBHOOK").unwrap_or_else(|_| "true".to_string()),
        )
        .context("COCOON_TELEGRAM_REGISTER_WEBHOOK")?;
        if register_webhook && public_url.is_none() {
            bail!("COCOON_PUBLIC_URL is required when webhook registration is enabled");
        }

        let max_subscriptions = env::var("COCOON_MAX_SUBSCRIPTIONS")
            .ok()
            .map(|value| {
                value
                    .trim()
                    .parse::<i64>()
                    .with_context(|| format!("invalid COCOON_MAX_SUBSCRIPTIONS '{value}'"))
            })
            .transpose()?
            .unwrap_or(100);
        let proxy = env::var("COCOON_TELEGRAM_PROXY")
            .ok()
            .filter(|value| !value.is_empty());
        let api_url = env::var("COCOON_TELEGRAM_API_URL")
            .ok()
            .filter(|value| !value.is_empty());

        Ok(Self {
            bind,
            db_path,
            bot_token,
            webhook_secret,
            webhook_path,
            public_url,
            register_webhook,
            max_subscriptions,
            proxy,
            api_url,
        })
    }

    /// The full webhook URL, including the secret path segment.
    pub fn webhook_url(&self) -> Option<String> {
        let base = self.public_url.as_ref()?.trim_end_matches('/');
        let path = self.webhook_path.trim_matches('/');
        Some(format!("{base}/{path}/{}", self.webhook_secret))
    }
}

fn resolve_secret(
    env_var: &str,
    file_var: &str,
    min: usize,
    max: usize,
    charset: Option<&str>,
) -> anyhow::Result<String> {
    let raw = match (env::var(env_var).ok(), env::var(file_var).ok()) {
        (Some(_), Some(_)) => bail!("set only one of {env_var} or {file_var}"),
        (Some(value), None) => value,
        (None, Some(path)) => {
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {file_var} '{path}'"))?;
            String::from_utf8(bytes)
                .with_context(|| format!("{file_var} '{path}' is not valid UTF-8"))?
        }
        (None, None) => bail!("set {env_var} or {file_var}"),
    };

    // Strip a single trailing newline, the common `echo`/editor artifact.
    let value = raw.strip_suffix('\n').unwrap_or(&raw);
    let value = value.strip_suffix('\r').unwrap_or(value);

    if value.len() < min {
        bail!("{env_var} must be at least {min} bytes");
    }
    if value.len() > max {
        bail!("{env_var} must be at most {max} bytes");
    }
    if let Some(allowed) = charset
        && !value.chars().all(|c| allowed.contains(c))
    {
        bail!("{env_var} contains characters outside {allowed}");
    }
    Ok(value.to_string())
}

fn parse_bool(value: &str) -> anyhow::Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => bail!("invalid boolean '{other}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boolean_parsing() {
        assert!(parse_bool("true").unwrap());
        assert!(parse_bool("1").unwrap());
        assert!(!parse_bool("off").unwrap());
        assert!(parse_bool("maybe").is_err());
    }
}
