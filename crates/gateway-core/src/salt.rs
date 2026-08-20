use alloy_primitives::B256;
use rand::Rng;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Salt(pub B256);

/// Generate a random 32-byte salt from the OS CSPRNG.
///
/// This is the only way the backend should create a `Salt`. The API never
/// accepts client-supplied salts.
pub fn generate_salt() -> Salt {
    Salt(B256::from(rand::rng().random::<[u8; 32]>()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_salt_is_32_bytes() {
        let salt = generate_salt();
        assert_eq!(salt.0.as_slice().len(), 32);
    }

    #[test]
    fn two_salts_differ() {
        let a = generate_salt();
        let b = generate_salt();
        assert_ne!(a, b);
    }

    #[test]
    fn salt_is_not_all_zero() {
        let salt = generate_salt();
        assert_ne!(salt.0, B256::ZERO);
    }

    #[test]
    fn salt_round_trips_through_b256() {
        let salt = generate_salt();
        let bytes = salt.0.to_string();
        let recovered: B256 = bytes.parse().unwrap();
        assert_eq!(salt.0, recovered);
    }
}
