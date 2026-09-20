//! Content-addressed, keyed paste identifiers.
//!
//! `id = truncate_128(HMAC-SHA256(secret, domain || publish_at || len(title) || title || len(content) || content))`
//!
//! The keyed hash makes identifiers deterministic for the server while preventing
//! third parties from guessing the hidden content offline.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Raw identifier length in bytes (128 bits).
pub const ID_BYTES: usize = 16;

const DOMAIN: &[u8] = b"cocoon:v1\0";

/// Compute the raw 128-bit identifier for `(publish_at, title, content)`.
pub fn compute_id(secret: &[u8], publish_at: i64, title: &[u8], content: &[u8]) -> [u8; ID_BYTES] {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts keys of any length");
    mac.update(DOMAIN);
    mac.update(&publish_at.to_be_bytes());
    mac.update(&(title.len() as u64).to_be_bytes());
    mac.update(title);
    mac.update(&(content.len() as u64).to_be_bytes());
    mac.update(content);
    let digest = mac.finalize().into_bytes();

    let mut id = [0u8; ID_BYTES];
    id.copy_from_slice(&digest[..ID_BYTES]);
    id
}

/// Encode a raw identifier for use in URLs (base64url, no padding).
pub fn encode_id(id: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(id)
}

/// Decode a URL identifier, returning `None` if it is malformed or the wrong length.
pub fn decode_id(encoded: &str) -> Option<[u8; ID_BYTES]> {
    let bytes = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    if bytes.len() != ID_BYTES {
        return None;
    }
    let mut id = [0u8; ID_BYTES];
    id.copy_from_slice(&bytes);
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"0123456789abcdef";

    #[test]
    fn deterministic_and_field_sensitive() {
        let a = compute_id(SECRET, 100, b"title", b"hello");
        assert_eq!(a, compute_id(SECRET, 100, b"title", b"hello"));
        assert_ne!(a, compute_id(SECRET, 101, b"title", b"hello"));
        assert_ne!(a, compute_id(SECRET, 100, b"other", b"hello"));
        assert_ne!(a, compute_id(SECRET, 100, b"title", b"hellp"));
    }

    #[test]
    fn no_field_boundary_ambiguity() {
        // Length prefixes keep (title = "ab", content = "") distinct from
        // (title = "a", content = "b").
        assert_ne!(
            compute_id(SECRET, 1, b"ab", b""),
            compute_id(SECRET, 1, b"a", b"b")
        );
    }

    #[test]
    fn roundtrip_encoding() {
        let id = compute_id(SECRET, 42, b"t", b"payload");
        let encoded = encode_id(&id);
        assert_eq!(decode_id(&encoded), Some(id));
        assert_eq!(decode_id("not!valid!"), None);
    }
}
