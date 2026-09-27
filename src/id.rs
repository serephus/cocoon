//! Short, per-user paste identifiers.
//!
//! `id = base62(hash(user_id, content, title, publish_at) mod 62^8)`, giving an
//! 8-character identifier. It is recomputed whenever a paste is edited, so an
//! id is always a pure function of the paste's current fields.

use sha2::{Digest, Sha256};

use crate::user::User;

const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Number of characters in an id.
pub const ID_LEN: usize = 8;

/// `62^ID_LEN`, the size of the id space.
const SPACE: u64 = 62u64.pow(ID_LEN as u32);

/// Compute the id for a paste's current fields.
pub fn new_id(owner: User, content: &str, title: &str, publish_at: i64) -> String {
    let key = owner.storage_key();
    let mut hasher = Sha256::new();
    hasher.update((key.len() as u64).to_be_bytes());
    hasher.update(key.as_bytes());
    hasher.update((content.len() as u64).to_be_bytes());
    hasher.update(content.as_bytes());
    hasher.update((title.len() as u64).to_be_bytes());
    hasher.update(title.as_bytes());
    hasher.update(publish_at.to_be_bytes());

    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    encode(u64::from_be_bytes(bytes) % SPACE)
}

fn encode(mut value: u64) -> String {
    let mut out = [0u8; ID_LEN];
    for slot in out.iter_mut().rev() {
        *slot = ALPHABET[(value % 62) as usize];
        value /= 62;
    }
    String::from_utf8(out.to_vec()).expect("base62 alphabet is ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_eight_url_safe_chars() {
        let id = new_id(User::Telegram(1), "hello", "title", 1_893_456_000);
        assert_eq!(id.len(), ID_LEN);
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn deterministic_and_field_sensitive() {
        let base = new_id(User::Telegram(1), "hello", "title", 100);
        assert_eq!(base, new_id(User::Telegram(1), "hello", "title", 100));
        assert_ne!(base, new_id(User::Telegram(2), "hello", "title", 100));
        assert_ne!(base, new_id(User::Anonymous, "hello", "title", 100));
        assert_ne!(base, new_id(User::Telegram(1), "hellp", "title", 100));
        assert_ne!(base, new_id(User::Telegram(1), "hello", "other", 100));
        assert_ne!(base, new_id(User::Telegram(1), "hello", "title", 101));
    }

    #[test]
    fn editing_changes_the_id() {
        let before = new_id(User::Telegram(1), "draft", "", 100);
        let after = new_id(User::Telegram(1), "draft edited", "", 100);
        assert_ne!(before, after);
    }
}
