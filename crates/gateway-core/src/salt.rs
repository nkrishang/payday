use alloy_primitives::B256;
use serde::{Deserialize, Serialize};

/// The 32-byte CREATE3 salt committed into a payment address.
///
/// Issuance salts exist only through [`crate::derive_attribution`], which
/// binds them to the canonical invoice and a fresh nonce. The tuple field is
/// public so stored salts can be decoded from the database and so tests can
/// exercise address derivation with arbitrary values; the API never accepts a
/// client-supplied salt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Salt(pub B256);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salt_round_trips_through_b256() {
        let salt = Salt(B256::repeat_byte(0x5a));
        let bytes = salt.0.to_string();
        let recovered: B256 = bytes.parse().unwrap();
        assert_eq!(Salt(recovered), salt);
    }
}
