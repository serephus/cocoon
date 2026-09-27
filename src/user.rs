//! Who owns or is viewing a paste.
//!
//! Identity is deliberately small and extensible: new frontends can add a
//! variant without changing the ownership rules. It is used only to tell an
//! owner from a non-owner — there are no roles or permissions beyond that.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum User {
    /// A frontend with no persistent identity (the web).
    Anonymous,
    /// An authenticated Telegram account.
    Telegram(i64),
}

impl User {
    /// Canonical storage form: `"anonymous"` or `"telegram:<id>"`.
    pub fn storage_key(self) -> String {
        match self {
            User::Anonymous => "anonymous".to_string(),
            User::Telegram(id) => format!("telegram:{id}"),
        }
    }

    pub fn telegram_id(self) -> Option<i64> {
        match self {
            User::Telegram(id) => Some(id),
            User::Anonymous => None,
        }
    }

    pub fn is_anonymous(self) -> bool {
        matches!(self, User::Anonymous)
    }
}

impl fmt::Display for User {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.storage_key())
    }
}

impl FromStr for User {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "anonymous" {
            return Ok(User::Anonymous);
        }
        if let Some(id) = value.strip_prefix("telegram:") {
            return id.parse::<i64>().map(User::Telegram).map_err(|_| ());
        }
        Err(())
    }
}

/// Whether `viewer` may read a paste's content.
///
/// A paste is readable once revealed. Before that, only its (non-anonymous)
/// owner may read it.
pub fn can_read(viewer: User, owner: &str, publish_at: i64, now: i64) -> bool {
    now >= publish_at || is_owner(viewer, owner)
}

/// Whether `viewer` may edit or delete a paste.
///
/// Only the (non-anonymous) owner may, and only before it is revealed.
pub fn can_manage(viewer: User, owner: &str, publish_at: i64, now: i64) -> bool {
    now < publish_at && is_owner(viewer, owner)
}

fn is_owner(viewer: User, owner: &str) -> bool {
    !viewer.is_anonymous() && viewer.storage_key() == owner
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANON: &str = "anonymous";
    const TG: &str = "telegram:42";

    #[test]
    fn round_trips() {
        assert_eq!(User::Anonymous.storage_key(), ANON);
        assert_eq!(User::Telegram(42).storage_key(), TG);
        assert_eq!(ANON.parse::<User>().unwrap(), User::Anonymous);
        assert_eq!(TG.parse::<User>().unwrap(), User::Telegram(42));
        assert!("nonsense".parse::<User>().is_err());
    }

    #[test]
    fn anonymous_cannot_read_before_reveal() {
        assert!(!can_read(User::Anonymous, ANON, 100, 50));
        assert!(can_read(User::Anonymous, ANON, 100, 100));
    }

    #[test]
    fn owner_reads_before_reveal_but_others_do_not() {
        assert!(can_read(User::Telegram(42), TG, 100, 50));
        assert!(!can_read(User::Telegram(7), TG, 100, 50));
        assert!(can_read(User::Telegram(7), TG, 100, 100));
    }

    #[test]
    fn only_owner_may_manage_before_reveal() {
        assert!(can_manage(User::Telegram(42), TG, 100, 50));
        assert!(!can_manage(User::Telegram(7), TG, 100, 50));
        assert!(!can_manage(User::Anonymous, ANON, 100, 50));
        // Nothing is manageable after reveal.
        assert!(!can_manage(User::Telegram(42), TG, 100, 100));
    }
}
