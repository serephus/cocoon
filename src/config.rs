use std::env;
use std::net::SocketAddr;

use anyhow::{Context, bail};

/// Minimum length of the HMAC secret, in bytes.
const MIN_SECRET_BYTES: usize = 16;

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

        let secret = resolve_secret(
            env::var("COCOON_HMAC_SECRET").ok(),
            env::var("COCOON_HMAC_SECRET_FILE").ok(),
        )?;

        Ok(Self {
            bind,
            db_path,
            secret,
        })
    }
}

/// Resolve the HMAC secret from either a literal value or a file.
fn resolve_secret(env_value: Option<String>, file_path: Option<String>) -> anyhow::Result<Vec<u8>> {
    resolve_secret_with(env_value, file_path, |path| {
        warn_if_world_readable(path);
        std::fs::read(path)
    })
}

/// Testable core of [`resolve_secret`] with an injected file reader.
///
/// Exactly one source must be configured; setting both is an error so the
/// choice is never ambiguous.
fn resolve_secret_with(
    env_value: Option<String>,
    file_path: Option<String>,
    read_file: impl Fn(&str) -> std::io::Result<Vec<u8>>,
) -> anyhow::Result<Vec<u8>> {
    let raw = match (env_value, file_path) {
        (Some(_), Some(_)) => {
            bail!("set only one of COCOON_HMAC_SECRET or COCOON_HMAC_SECRET_FILE")
        }
        (Some(value), None) => value.into_bytes(),
        (None, Some(path)) => {
            read_file(&path).with_context(|| format!("reading COCOON_HMAC_SECRET_FILE '{path}'"))?
        }
        (None, None) => bail!("set COCOON_HMAC_SECRET or COCOON_HMAC_SECRET_FILE"),
    };

    let secret = normalize_secret(raw);
    if secret.len() < MIN_SECRET_BYTES {
        bail!("HMAC secret must be at least {MIN_SECRET_BYTES} bytes");
    }
    Ok(secret)
}

/// Drop a single trailing newline (`\n` or `\r\n`), the usual artifact of
/// writing a secret with `echo` or a text editor. Every other byte, including
/// spaces and tabs, is preserved.
fn normalize_secret(mut raw: Vec<u8>) -> Vec<u8> {
    if raw.last() == Some(&b'\n') {
        raw.pop();
        if raw.last() == Some(&b'\r') {
            raw.pop();
        }
    }
    raw
}

/// Warn (not fail) when a secret file is readable by group or others.
#[cfg(unix)]
fn warn_if_world_readable(path: &str) {
    use std::os::unix::fs::PermissionsExt;

    if let Ok(meta) = std::fs::metadata(path) {
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            tracing::warn!(
                path,
                mode = format!("{:o}", mode & 0o777),
                "HMAC secret file is readable by group or others"
            );
        }
    }
}

#[cfg(not(unix))]
fn warn_if_world_readable(_path: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader that ignores the path and returns fixed bytes.
    fn reader(bytes: &'static [u8]) -> impl Fn(&str) -> std::io::Result<Vec<u8>> {
        move |_| Ok(bytes.to_vec())
    }

    #[test]
    fn env_value_is_used() {
        let secret =
            resolve_secret_with(Some("0123456789abcdef".into()), None, reader(b"")).unwrap();
        assert_eq!(secret, b"0123456789abcdef".to_vec());
    }

    #[test]
    fn file_value_is_used() {
        let secret = resolve_secret_with(
            None,
            Some("/run/secrets/cocoon".into()),
            reader(b"fedcba9876543210"),
        )
        .unwrap();
        assert_eq!(secret, b"fedcba9876543210".to_vec());
    }

    #[test]
    fn both_sources_is_an_error() {
        let err = resolve_secret_with(
            Some("0123456789abcdef".into()),
            Some("/run/secrets/cocoon".into()),
            reader(b""),
        )
        .unwrap_err();
        assert!(err.to_string().contains("only one"), "{err}");
    }

    #[test]
    fn no_source_is_an_error() {
        assert!(resolve_secret_with(None, None, reader(b"")).is_err());
    }

    #[test]
    fn too_short_is_an_error() {
        assert!(resolve_secret_with(Some("short".into()), None, reader(b"")).is_err());
        assert!(
            resolve_secret_with(None, Some("/x".into()), reader(b"short"))
                .unwrap_err()
                .to_string()
                .contains("at least")
        );
    }

    #[test]
    fn reader_error_propagates() {
        let read = |_: &str| {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no such file",
            ))
        };
        let err = resolve_secret_with(None, Some("/missing".into()), read).unwrap_err();
        assert!(err.to_string().contains("/missing"), "{err}");
    }

    #[test]
    fn strips_one_trailing_newline_only() {
        assert_eq!(normalize_secret(b"secret\n".to_vec()), b"secret".to_vec());
        assert_eq!(normalize_secret(b"secret\r\n".to_vec()), b"secret".to_vec());
        assert_eq!(
            normalize_secret(b"secret\n\n".to_vec()),
            b"secret\n".to_vec()
        );
        // interior/other whitespace is preserved
        assert_eq!(normalize_secret(b" a b\t".to_vec()), b" a b\t".to_vec());
    }

    #[test]
    fn trailing_newline_does_not_change_the_secret() {
        let plain =
            resolve_secret_with(None, Some("/x".into()), reader(b"0123456789abcdef")).unwrap();
        let newline =
            resolve_secret_with(None, Some("/x".into()), reader(b"0123456789abcdef\n")).unwrap();
        assert_eq!(plain, newline);
    }
}
