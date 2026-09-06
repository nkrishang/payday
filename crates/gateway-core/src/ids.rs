//! Prefixed resource identifiers.
//!
//! Every id the API emits is a UUID behind a short prefix that says what it
//! names: `dr_` for a deposit request, `cus_` for a customer, `att_` for an
//! attachment, and so on. The prefix is presentation: the database stores
//! the UUID, and the API layer adds the prefix on the way out and strips it
//! on the way in. Only the canonical form the API emits is accepted back —
//! the exact prefix, then a lowercase hyphenated UUID — so a lookup can never
//! resolve to more than one row and a customer id handed to an attachment
//! route reads as a missing attachment rather than a lucky hit.
//!
//! The one place a raw UUID remains visible is the Proof of Payment's
//! `canonical_issuance_snapshot.attachment.id`: that document is hashed into
//! the deposit address and its schema is frozen, so it carries the UUID the
//! API's `att_` id wraps.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

/// The example UUID error messages show, so a caller sees the shape wanted.
const EXAMPLE: &str = "0198f80c-8d2f-7dc1-a369-90556a64f700";

/// Parse `<prefix><uuid>` in the canonical lowercase hyphenated form only.
pub fn parse_prefixed(prefix: &str, value: &str) -> Option<Uuid> {
    let suffix = value.strip_prefix(prefix)?;
    let uuid = Uuid::try_parse(suffix).ok()?;
    let mut canonical = [0u8; uuid::fmt::Hyphenated::LENGTH];
    (uuid.hyphenated().encode_lower(&mut canonical) == suffix).then_some(uuid)
}

/// A value that was not a `<prefix><uuid>` id of the expected resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidId {
    pub noun: &'static str,
    pub prefix: &'static str,
}

impl fmt::Display for InvalidId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "expected a {} id like {}{EXAMPLE}",
            self.noun, self.prefix
        )
    }
}

impl std::error::Error for InvalidId {}

macro_rules! prefixed_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal, $noun:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub Uuid);

        impl $name {
            pub const PREFIX: &'static str = $prefix;
            pub const NOUN: &'static str = $noun;

            /// A fresh, time-ordered id.
            pub fn generate() -> Self {
                Self(Uuid::now_v7())
            }

            /// The canonical `<prefix><uuid>` form, or `None`.
            pub fn parse(value: &str) -> Option<Self> {
                parse_prefixed(Self::PREFIX, value).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", Self::PREFIX, self.0)
            }
        }

        impl FromStr for $name {
            type Err = InvalidId;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse(value).ok_or(InvalidId {
                    noun: Self::NOUN,
                    prefix: Self::PREFIX,
                })
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Uuid {
                id.0
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                value.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

prefixed_id!(
    /// A merchant account, as `GET /v1/account` reports it.
    AccountId, "acct_", "account"
);
prefixed_id!(
    /// A saved counterparty record.
    CustomerId, "cus_", "customer"
);
prefixed_id!(
    /// A saved issuer identity.
    IssuerId, "iss_", "issuer identity"
);
prefixed_id!(
    /// A saved payout wallet.
    PayoutAddressId, "pa_", "payout address"
);
prefixed_id!(
    /// A PDF upload, before and after it is attached to a deposit request.
    AttachmentId, "att_", "attachment"
);
prefixed_id!(
    /// A webhook endpoint.
    WebhookId, "wh_", "webhook endpoint"
);
prefixed_id!(
    /// One delivery of one event to one endpoint.
    WebhookDeliveryId, "whd_", "webhook delivery"
);
prefixed_id!(
    /// One event, as the webhook envelope's `id` and the `Payday-Event-Id`
    /// header carry it; the idempotency key for a handler.
    WebhookEventId, "evt_", "webhook event"
);
prefixed_id!(
    /// One verification attempt on a deposit request.
    VerificationAttemptId, "va_", "verification attempt"
);
prefixed_id!(
    /// One recovery ledger entry, as `deposit_request.recovered_funds` names it.
    RecoveryId, "rec_", "recovery"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_in_their_canonical_form_only() {
        let id = CustomerId(Uuid::parse_str(EXAMPLE).unwrap());
        assert_eq!(id.to_string(), format!("cus_{EXAMPLE}"));
        assert_eq!(CustomerId::parse(&id.to_string()), Some(id));
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            format!("\"cus_{EXAMPLE}\"")
        );
        assert_eq!(
            serde_json::from_str::<CustomerId>(&format!("\"cus_{EXAMPLE}\"")).unwrap(),
            id
        );
        for rejected in [
            EXAMPLE.to_owned(),
            format!("iss_{EXAMPLE}"),
            format!("cus_{}", EXAMPLE.to_uppercase()),
            format!("cus_{}", EXAMPLE.replace('-', "")),
            "cus_".to_owned(),
            format!("cus_{EXAMPLE}x"),
        ] {
            assert_eq!(CustomerId::parse(&rejected), None, "{rejected}");
        }
        let error = serde_json::from_str::<CustomerId>(&format!("\"{EXAMPLE}\"")).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("expected a customer id like cus_0198f80c"),
            "{error}"
        );
    }

    #[test]
    fn every_prefix_is_distinct() {
        let prefixes = [
            AccountId::PREFIX,
            CustomerId::PREFIX,
            IssuerId::PREFIX,
            PayoutAddressId::PREFIX,
            AttachmentId::PREFIX,
            WebhookId::PREFIX,
            WebhookDeliveryId::PREFIX,
            WebhookEventId::PREFIX,
            VerificationAttemptId::PREFIX,
            RecoveryId::PREFIX,
            "dr_",
        ];
        let unique: std::collections::BTreeSet<_> = prefixes.iter().collect();
        assert_eq!(unique.len(), prefixes.len());
        for prefix in prefixes {
            assert!(prefix.ends_with('_') && prefix.len() > 1, "{prefix}");
        }
    }
}
