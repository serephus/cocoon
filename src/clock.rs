use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Current server time as Unix seconds (UTC).
pub fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

/// Parse an RFC 3339 timestamp into Unix seconds.
pub fn parse_rfc3339(input: &str) -> Result<i64, String> {
    OffsetDateTime::parse(input, &Rfc3339)
        .map(|dt| dt.unix_timestamp())
        .map_err(|e| format!("invalid RFC 3339 timestamp: {e}"))
}

/// Render Unix seconds as an RFC 3339 UTC string.
pub fn format_rfc3339(ts: i64) -> String {
    OffsetDateTime::from_unix_timestamp(ts)
        .ok()
        .and_then(|dt| dt.format(&Rfc3339).ok())
        .unwrap_or_else(|| ts.to_string())
}

/// Compact human-readable duration, e.g. "2d 3h" or "5m 12s".
pub fn humanize_delta(seconds: i64) -> String {
    if seconds <= 0 {
        return "now".to_string();
    }
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let secs = seconds % 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {secs}s")
    } else {
        format!("{secs}s")
    }
}
