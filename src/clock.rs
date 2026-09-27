use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Current server time as Unix seconds (UTC).
pub fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

/// Render Unix seconds as an RFC 3339 UTC string.
pub fn format_rfc3339(ts: i64) -> String {
    OffsetDateTime::from_unix_timestamp(ts)
        .ok()
        .and_then(|dt| dt.format(&Rfc3339).ok())
        .unwrap_or_else(|| ts.to_string())
}

/// Parse a human-entered publish time.
///
/// Accepts RFC 3339 (`2030-01-01T00:00:00Z`), `YYYY-MM-DD HH:MM`, and
/// `YYYY-MM-DD`; all zoneless forms are interpreted as UTC.
pub fn parse_publish_at(input: &str) -> Result<i64, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err("the time is empty".to_string());
    }

    // Already a full RFC 3339 timestamp (with `Z` or an offset).
    if let Ok(dt) = OffsetDateTime::parse(raw, &Rfc3339) {
        return Ok(dt.unix_timestamp());
    }

    let normalized = raw.replace(' ', "T");
    let candidate = match normalized.len() {
        10 => format!("{normalized}T00:00:00Z"),
        16 => format!("{normalized}:00Z"),
        19 => format!("{normalized}Z"),
        _ => normalized,
    };
    OffsetDateTime::parse(&candidate, &Rfc3339)
        .map(|dt| dt.unix_timestamp())
        .map_err(|_| {
            "could not parse the time; use `2030-01-01 00:00` or `2030-01-01T00:00:00Z` (UTC)"
                .to_string()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_rfc3339() {
        assert_eq!(
            parse_publish_at("2030-01-01T00:00:00Z").unwrap(),
            1_893_456_000
        );
    }

    #[test]
    fn accepts_zoneless_utc() {
        let a = parse_publish_at("2030-01-01 00:00").unwrap();
        let b = parse_publish_at("2030-01-01T00:00").unwrap();
        let c = parse_publish_at("2030-01-01").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_publish_at("tomorrow").is_err());
        assert!(parse_publish_at("").is_err());
    }
}
