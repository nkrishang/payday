use chrono::DateTime;
use thiserror::Error;

pub const MIN_EXPIRATION_LEAD_SECS: u64 = 10 * 60;
pub const MAX_EXPIRATION_LEAD_SECS: u64 = 366 * 24 * 60 * 60;
pub const DEFAULT_EXPIRATION_LEAD_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExpirationError {
    #[error("choose only one of expires_in, expires_at, or expiration_timestamp")]
    Conflicting,
    #[error("expires_in must be a duration such as 30m, 24h, or 7d")]
    InvalidDuration,
    #[error("expires_at must be an RFC 3339 time such as 2026-08-28T14:30:00Z")]
    InvalidDateTime,
    #[error("expiration_timestamp must be a Unix timestamp in seconds")]
    InvalidTimestamp,
    #[error("expiry must be at least 10 minutes from now")]
    TooSoon,
    #[error("expiry must be no more than 366 days from now")]
    TooFar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedExpiration {
    pub timestamp: u64,
    /// Stable representation used to compare idempotent retries. Relative
    /// expiries retain their duration rather than the wall clock they resolved against.
    pub intent: String,
}

pub fn resolve_expiration(
    expires_in: Option<&str>,
    expires_at: Option<&str>,
    expiration_timestamp: Option<&str>,
    now: u64,
) -> Result<ResolvedExpiration, ExpirationError> {
    let expiration = parse_expiration(expires_in, expires_at, expiration_timestamp, now)?;
    validate_expiration_window(&expiration, now)?;
    Ok(expiration)
}

/// Parse and canonicalize expiration intent without applying the moving
/// creation window. Servers use this to identify an idempotent replay before
/// deciding whether the request would create a new payment.
pub fn parse_expiration(
    expires_in: Option<&str>,
    expires_at: Option<&str>,
    expiration_timestamp: Option<&str>,
    now: u64,
) -> Result<ResolvedExpiration, ExpirationError> {
    if [
        expires_in.is_some(),
        expires_at.is_some(),
        expiration_timestamp.is_some(),
    ]
    .into_iter()
    .filter(|set| *set)
    .count()
        > 1
    {
        return Err(ExpirationError::Conflicting);
    }

    let (timestamp, intent) = if let Some(value) = expires_in {
        let duration =
            humantime::parse_duration(value).map_err(|_| ExpirationError::InvalidDuration)?;
        let seconds = duration.as_secs();
        (
            now.checked_add(seconds).ok_or(ExpirationError::TooFar)?,
            format!("in:{seconds}"),
        )
    } else if let Some(value) = expires_at {
        let timestamp = DateTime::parse_from_rfc3339(value)
            .map_err(|_| ExpirationError::InvalidDateTime)?
            .timestamp()
            .try_into()
            .map_err(|_| ExpirationError::TooSoon)?;
        (timestamp, format!("at:{timestamp}"))
    } else if let Some(value) = expiration_timestamp {
        let timestamp = value
            .parse()
            .map_err(|_| ExpirationError::InvalidTimestamp)?;
        (timestamp, format!("at:{timestamp}"))
    } else {
        (
            now + DEFAULT_EXPIRATION_LEAD_SECS,
            format!("in:{DEFAULT_EXPIRATION_LEAD_SECS}"),
        )
    };

    Ok(ResolvedExpiration { timestamp, intent })
}

pub fn validate_expiration_window(
    expiration: &ResolvedExpiration,
    now: u64,
) -> Result<(), ExpirationError> {
    if expiration.timestamp < now.saturating_add(MIN_EXPIRATION_LEAD_SECS) {
        Err(ExpirationError::TooSoon)
    } else if expiration.timestamp > now.saturating_add(MAX_EXPIRATION_LEAD_SECS) {
        Err(ExpirationError::TooFar)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_human_rfc3339_and_legacy_expiries() {
        let now = 1_800_000_000;
        assert_eq!(
            resolve_expiration(None, None, None, now).unwrap().timestamp,
            now + 86_400
        );
        assert_eq!(
            resolve_expiration(Some("24h"), None, None, now)
                .unwrap()
                .timestamp,
            now + 86_400
        );
        assert_eq!(
            resolve_expiration(None, None, Some(&(now + 600).to_string()), now)
                .unwrap()
                .timestamp,
            now + 600
        );
        assert!(matches!(
            resolve_expiration(Some("1m"), None, None, now),
            Err(ExpirationError::TooSoon)
        ));
        let parsed = parse_expiration(Some("1m"), None, None, now).unwrap();
        assert_eq!(parsed.intent, "in:60");
        assert!(matches!(
            validate_expiration_window(&parsed, now),
            Err(ExpirationError::TooSoon)
        ));
        assert_eq!(
            resolve_expiration(Some("24h"), None, None, now)
                .unwrap()
                .intent,
            resolve_expiration(Some("24h"), None, None, now + 30)
                .unwrap()
                .intent
        );
        assert!(matches!(
            resolve_expiration(Some("1h"), Some("2027-01-01T00:00:00Z"), None, now),
            Err(ExpirationError::Conflicting)
        ));
    }
}
