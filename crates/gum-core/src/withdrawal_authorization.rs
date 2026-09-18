//! The merchant's withdrawal authorization: an EIP-3009 signature over their
//! own stablecoin that Gum relays but cannot redirect.
//!
//! A withdrawal moves the merchant's Gum wallet balance in one currency
//! to one destination. Each chain's balance is one *leg*, and each leg is
//! authorized by one EIP-712 signature under that chain's token contract,
//! using the token's own `TransferWithAuthorization` /
//! `ReceiveWithAuthorization` types (EIP-3009). Every contract Gum serves
//! implements them: Circle's FiatToken and Tether's USDT0 alike.
//!
//! ```text
//! domain  = EIP712Domain{name: token.name(), version: token.version(), chainId, verifyingContract: token}
//! transfer leg: TransferWithAuthorization{from: wallet, to: destination, value, validAfter: 0, validBefore, nonce: random}
//! bridge leg:   ReceiveWithAuthorization{from: wallet, to: forwarder,   value, validAfter: 0, validBefore,
//!                                        nonce: keccak256(abi.encode(destinationDomain, bytes32(destination), salt))}
//! digest  = keccak256(0x1901 || domainSeparator || hashStruct(message))
//! ```
//!
//! The signature itself binds where the funds may go. A transfer leg names
//! the destination as the payee. A bridge leg, which only USDC has (CCTP
//! burns and mints USDC alone), names the `WithdrawalForwarder` as the payee
//! and commits to the CCTP destination through the nonce: the forwarder
//! recomputes the nonce from the destination it is asked to burn towards,
//! and USDC rejects the signature unless they agree. The relayer therefore
//! pays gas and nothing else. Only ECDSA (externally owned) wallets are
//! accepted; the Gum wallet is one.
//!
//! The domain's `name` and `version` differ between deployments ("USDC" on
//! Monad, "USD Coin" on Base and Arbitrum, both version "2"; "USDT0" on
//! Monad and "USD₮0" on Arbitrum, version "1"), so they are read from the
//! token and carried with every authorization rather than assumed.

use std::collections::BTreeMap;
use std::str::FromStr;

use alloy_primitives::{Address, B256, Signature, U256, b256, hex, keccak256};
use alloy_sol_types::{Eip712Domain, SolStruct, SolValue, sol};
use serde::de::Error as _;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

pub use crate::payer_attestation::{TypedDataDomain, TypedDataField};

pub const TRANSFER_WITH_AUTHORIZATION_PRIMARY_TYPE: &str = "TransferWithAuthorization";
pub const RECEIVE_WITH_AUTHORIZATION_PRIMARY_TYPE: &str = "ReceiveWithAuthorization";
/// Circle's type hashes, as `FiatTokenV2` exposes them; the tests pin the
/// structs below to these.
pub const TRANSFER_WITH_AUTHORIZATION_TYPEHASH: B256 =
    b256!("0x7c7c6cdb67a18743f49ec6fa9b35f50d52ed05cbed4cc592e13b44501c1a2267");
pub const RECEIVE_WITH_AUTHORIZATION_TYPEHASH: B256 =
    b256!("0xd099cc98ef71107a616c4f0f941f04c322d8e254fe26b3c6668db87aae413de8");

sol! {
    /// EIP-3009's transfer authorization. Field order is the type's
    /// canonical order and is fixed by the token contract.
    struct TransferWithAuthorization {
        address from;
        address to;
        uint256 value;
        uint256 validAfter;
        uint256 validBefore;
        bytes32 nonce;
    }

    /// EIP-3009's payee-submitted authorization: the same fields, a type
    /// only `to` may consume.
    struct ReceiveWithAuthorization {
        address from;
        address to;
        uint256 value;
        uint256 validAfter;
        uint256 validBefore;
        bytes32 nonce;
    }
}

/// One chain's stablecoin contract as an EIP-712 domain: `name()` and
/// `version()` read from the token, its chain, and its address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenDomain {
    pub name: String,
    pub version: String,
    pub chain_id: u64,
    pub token: Address,
}

impl TokenDomain {
    pub fn eip712(&self) -> Eip712Domain {
        Eip712Domain::new(
            Some(self.name.clone().into()),
            Some(self.version.clone().into()),
            Some(U256::from(self.chain_id)),
            Some(self.token),
            None,
        )
    }

    /// What the token's `DOMAIN_SEPARATOR()` must return for this domain to
    /// be the one it verifies against.
    pub fn separator(&self) -> B256 {
        self.eip712().separator()
    }
}

/// Which EIP-3009 type a leg is authorized with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationKind {
    /// `TransferWithAuthorization`: anyone may relay it to the payee.
    Transfer,
    /// `ReceiveWithAuthorization`: only the payee may consume it.
    Receive,
}

impl AuthorizationKind {
    pub fn primary_type(self) -> &'static str {
        match self {
            Self::Transfer => TRANSFER_WITH_AUTHORIZATION_PRIMARY_TYPE,
            Self::Receive => RECEIVE_WITH_AUTHORIZATION_PRIMARY_TYPE,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transfer => "transfer",
            Self::Receive => "receive",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "transfer" => Some(Self::Transfer),
            "receive" => Some(Self::Receive),
            _ => None,
        }
    }
}

/// The message a merchant signs for one leg. `validAfter` is always 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalAuthorization {
    pub kind: AuthorizationKind,
    pub from: Address,
    pub to: Address,
    pub value: U256,
    /// Unix seconds; the token refuses the authorization at or after this.
    pub valid_before: u64,
    pub nonce: B256,
}

/// The EIP-3009 nonce a bridge leg signs: a commitment to the CCTP
/// destination, exactly as `WithdrawalForwarder.bridgeNonce` computes it.
pub fn bridge_nonce(destination_domain: u32, mint_recipient: Address, salt: B256) -> B256 {
    keccak256((destination_domain, mint_recipient.into_word(), salt).abi_encode())
}

impl WithdrawalAuthorization {
    /// `hashStruct(message)`.
    pub fn struct_hash(&self) -> B256 {
        match self.kind {
            AuthorizationKind::Transfer => TransferWithAuthorization {
                from: self.from,
                to: self.to,
                value: self.value,
                validAfter: U256::ZERO,
                validBefore: U256::from(self.valid_before),
                nonce: self.nonce,
            }
            .eip712_hash_struct(),
            AuthorizationKind::Receive => ReceiveWithAuthorization {
                from: self.from,
                to: self.to,
                value: self.value,
                validAfter: U256::ZERO,
                validBefore: U256::from(self.valid_before),
                nonce: self.nonce,
            }
            .eip712_hash_struct(),
        }
    }

    /// The EIP-712 signing hash under the token's domain.
    pub fn digest(&self, domain: &TokenDomain) -> B256 {
        let separator = domain.separator();
        let mut bytes = [0u8; 66];
        bytes[0] = 0x19;
        bytes[1] = 0x01;
        bytes[2..34].copy_from_slice(separator.as_slice());
        bytes[34..].copy_from_slice(self.struct_hash().as_slice());
        keccak256(bytes)
    }

    /// The document a wallet signs (`eth_signTypedData_v4`), exactly as the
    /// API hands it out.
    pub fn typed_data(&self, domain: &TokenDomain) -> AuthorizationTypedData {
        AuthorizationTypedData {
            domain: TypedDataDomain {
                name: domain.name.clone(),
                version: domain.version.clone(),
                chain_id: domain.chain_id,
                verifying_contract: domain.token.to_checksum(None),
            },
            primary_type: self.kind.primary_type().into(),
            types: AuthorizationTypes {
                primary_type: self.kind.primary_type().into(),
            },
            message: AuthorizationMessage {
                from: self.from.to_checksum(None),
                to: self.to.to_checksum(None),
                value: self.value.to_string(),
                valid_after: "0".into(),
                valid_before: self.valid_before.to_string(),
                nonce: self.nonce.to_string(),
            },
        }
    }
}

/// EIP-712 typed data in the JSON shape wallets accept: camelCase keys, a
/// numeric chain id, and every `uint256` as a decimal string (the only
/// encoding every signer library reads back losslessly).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorizationTypedData {
    pub domain: TypedDataDomain,
    pub primary_type: String,
    pub types: AuthorizationTypes,
    pub message: AuthorizationMessage,
}

/// `types`: `EIP712Domain` plus the one EIP-3009 struct named by
/// `primaryType`. Serialized as the map wallets expect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationTypes {
    pub primary_type: String,
}

impl AuthorizationTypes {
    pub fn domain_fields() -> Vec<TypedDataField> {
        vec![
            TypedDataField::new("name", "string"),
            TypedDataField::new("version", "string"),
            TypedDataField::new("chainId", "uint256"),
            TypedDataField::new("verifyingContract", "address"),
        ]
    }

    /// The EIP-3009 struct's fields, in the token's canonical order.
    pub fn message_fields() -> Vec<TypedDataField> {
        vec![
            TypedDataField::new("from", "address"),
            TypedDataField::new("to", "address"),
            TypedDataField::new("value", "uint256"),
            TypedDataField::new("validAfter", "uint256"),
            TypedDataField::new("validBefore", "uint256"),
            TypedDataField::new("nonce", "bytes32"),
        ]
    }
}

impl Serialize for AuthorizationTypes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("EIP712Domain", &Self::domain_fields())?;
        map.serialize_entry(&self.primary_type, &Self::message_fields())?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for AuthorizationTypes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut map: BTreeMap<String, Vec<TypedDataField>> = BTreeMap::deserialize(deserializer)?;
        if map.remove("EIP712Domain") != Some(Self::domain_fields()) {
            return Err(D::Error::custom(
                "types.EIP712Domain is not the standard domain",
            ));
        }
        let (primary_type, fields) = match map.len() {
            1 => map.into_iter().next().expect("one entry"),
            _ => return Err(D::Error::custom("types must name exactly one message type")),
        };
        if fields != Self::message_fields() {
            return Err(D::Error::custom(format!(
                "types.{primary_type} is not the EIP-3009 authorization type"
            )));
        }
        Ok(Self { primary_type })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorizationMessage {
    /// EIP-55 checksummed: the merchant's Gum wallet.
    pub from: String,
    /// EIP-55 checksummed: the destination (transfer) or the forwarder (receive).
    pub to: String,
    /// Decimal base units.
    pub value: String,
    /// Always `"0"`.
    pub valid_after: String,
    /// Decimal unix seconds.
    pub valid_before: String,
    /// `0x` hex, 32 bytes.
    pub nonce: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthorizationError {
    #[error("signature must be 0x-prefixed hex of 65 bytes (r || s || v)")]
    SignatureMalformed,
    #[error("signature is invalid")]
    SignatureInvalid,
    #[error("signature was not made by the wallet the authorization names")]
    SignerMismatch,
}

/// Verify a merchant's signature over `authorization` under `domain`: it
/// must recover to `authorization.from`. Returns the parsed signature.
pub fn verify_authorization(
    authorization: &WithdrawalAuthorization,
    domain: &TokenDomain,
    signature: &str,
) -> Result<Signature, AuthorizationError> {
    let bytes = hex::decode(signature).map_err(|_| AuthorizationError::SignatureMalformed)?;
    if bytes.len() != 65 {
        return Err(AuthorizationError::SignatureMalformed);
    }
    let signature =
        Signature::from_raw(&bytes).map_err(|_| AuthorizationError::SignatureMalformed)?;
    let recovered = signature
        .recover_address_from_prehash(&authorization.digest(domain))
        .map_err(|_| AuthorizationError::SignatureInvalid)?;
    if recovered.is_zero() || recovered != authorization.from {
        return Err(AuthorizationError::SignerMismatch);
    }
    Ok(signature)
}

/// Parse a `0x` hex address the API accepted, checksum-insensitively.
pub fn parse_address(value: &str) -> Option<Address> {
    Address::from_str(value.trim()).ok()
}

/// Sign an authorization with a raw secp256k1 key. For tests and local
/// tooling only: production signatures come from the merchant's wallet.
#[cfg(any(test, feature = "test-helpers"))]
pub fn sign_authorization(
    secret: &[u8; 32],
    authorization: &WithdrawalAuthorization,
    domain: &TokenDomain,
) -> String {
    let key = k256::ecdsa::SigningKey::from_slice(secret).expect("valid secp256k1 key");
    let digest = authorization.digest(domain);
    let (signature, recovery) = key
        .sign_prehash_recoverable(digest.as_slice())
        .expect("signing cannot fail");
    let signature = Signature::from_signature_and_parity(signature, recovery.is_y_odd());
    hex::encode_prefixed(signature.as_bytes())
}

#[cfg(test)]
mod tests {
    use alloy_primitives::address;

    use super::*;
    use crate::wallet_of;

    fn monad_usdc() -> TokenDomain {
        TokenDomain {
            name: "USDC".into(),
            version: "2".into(),
            chain_id: 143,
            token: address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603"),
        }
    }

    fn secret() -> [u8; 32] {
        let mut secret = [0u8; 32];
        secret[31] = 7;
        secret
    }

    fn receive(from: Address) -> WithdrawalAuthorization {
        WithdrawalAuthorization {
            kind: AuthorizationKind::Receive,
            from,
            to: address!("0x2222222222222222222222222222222222222222"),
            value: U256::from(1_234_567u64),
            valid_before: 1_800_000_000,
            nonce: bridge_nonce(
                6,
                address!("0x000000000000000000000000000000000000d00d"),
                B256::from(U256::from(7)),
            ),
        }
    }

    #[test]
    fn structs_hash_to_circles_type_hashes() {
        assert_eq!(
            keccak256(TransferWithAuthorization::eip712_encode_type().as_bytes()),
            TRANSFER_WITH_AUTHORIZATION_TYPEHASH
        );
        assert_eq!(
            keccak256(ReceiveWithAuthorization::eip712_encode_type().as_bytes()),
            RECEIVE_WITH_AUTHORIZATION_TYPEHASH
        );
    }

    /// The live separators read from each chain's token on 2026-09-11;
    /// the fork tests assert the same values against the contracts.
    #[test]
    fn domain_separators_match_the_live_tokens() {
        assert_eq!(
            monad_usdc().separator(),
            b256!("0xfe22123edc0dd4aeb912eb7948c5f0e531592c2053b3067612f427db342c93c6")
        );
        let base = TokenDomain {
            name: "USD Coin".into(),
            version: "2".into(),
            chain_id: 8453,
            token: address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
        };
        assert_eq!(
            base.separator(),
            b256!("0x02fa7265e7c5d81118673727957699e4d68f74cd74b7db77da710fe8a2c7834f")
        );
        let arbitrum = TokenDomain {
            name: "USD Coin".into(),
            version: "2".into(),
            chain_id: 42_161,
            token: address!("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
        };
        assert_eq!(
            arbitrum.separator(),
            b256!("0x08d11903f8419e68b1b8721bcbe2e9fc68569122a77ef18c216f10b3b5112c78")
        );
    }

    /// Tether's USDT0: `DOMAIN_SEPARATOR()` read from each contract on
    /// 2026-09-15. Neither exposes `version()`; "1" is what the separator
    /// hashes under, which is how the chain reader settles it.
    #[test]
    fn usdt0_domain_separators_match_the_live_tokens() {
        let monad = TokenDomain {
            name: "USDT0".into(),
            version: "1".into(),
            chain_id: 143,
            token: address!("0xe7cd86e13AC4309349F30B3435a9d337750fC82D"),
        };
        assert_eq!(
            monad.separator(),
            b256!("0x101a90213d0b0d9d607fe3b94dcb41e91f20ab1432d4cd23ae9f820c0968452a")
        );
        let arbitrum = TokenDomain {
            name: "USD₮0".into(),
            version: "1".into(),
            chain_id: 42_161,
            token: address!("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"),
        };
        assert_eq!(
            arbitrum.separator(),
            b256!("0x566af68fb471b22d6421762f84aa7bd761c670a2e4d5c8a47d4085d5957b127c")
        );
    }

    /// Pinned across languages: `WithdrawalForwarder.t.sol` asserts the
    /// nonce and the digest, and the SDK and web tests the digest.
    #[test]
    fn bridge_nonce_and_digest_vectors() {
        let nonce = bridge_nonce(
            6,
            address!("0x000000000000000000000000000000000000d00d"),
            B256::from(U256::from(7)),
        );
        assert_eq!(
            nonce,
            b256!("0x18b79105e486e10f626b71939a0226c47316949b7c24ff1c397661ea861fafaa"),
            "nonce vector"
        );
        let authorization = receive(address!("0x1111111111111111111111111111111111111111"));
        assert_eq!(
            authorization.digest(&monad_usdc()),
            b256!("0xfe0bcc7d9e69ee02881011f29a4156caa15e02b8e94c2e0c9f40e66711651993"),
            "digest vector"
        );
    }

    #[test]
    fn typed_data_is_the_wallet_shape() {
        let authorization = receive(address!("0x1111111111111111111111111111111111111111"));
        let typed = authorization.typed_data(&monad_usdc());
        let json = serde_json::to_value(&typed).unwrap();
        assert_eq!(json["primaryType"], "ReceiveWithAuthorization");
        assert_eq!(json["domain"]["name"], "USDC");
        assert_eq!(json["domain"]["chainId"], 143);
        assert_eq!(
            json["domain"]["verifyingContract"],
            "0x754704Bc059F8C67012fEd69BC8A327a5aafb603"
        );
        assert_eq!(
            json["types"]["ReceiveWithAuthorization"][5],
            serde_json::json!({"name": "nonce", "type": "bytes32"})
        );
        assert_eq!(json["types"]["EIP712Domain"][2]["name"], "chainId");
        assert_eq!(json["message"]["value"], "1234567");
        assert_eq!(json["message"]["validAfter"], "0");
        assert_eq!(json["message"]["validBefore"], "1800000000");
        assert_eq!(
            json["message"]["from"],
            "0x1111111111111111111111111111111111111111"
        );
        let back: AuthorizationTypedData = serde_json::from_value(json).unwrap();
        assert_eq!(back, typed);
    }

    #[test]
    fn signatures_verify_only_for_the_named_wallet() {
        let wallet = wallet_of(&secret());
        let authorization = receive(wallet);
        let domain = monad_usdc();
        let signature = sign_authorization(&secret(), &authorization, &domain);
        verify_authorization(&authorization, &domain, &signature).unwrap();

        let other = WithdrawalAuthorization {
            from: address!("0x1111111111111111111111111111111111111111"),
            ..authorization.clone()
        };
        assert_eq!(
            verify_authorization(&other, &domain, &signature),
            Err(AuthorizationError::SignerMismatch)
        );
        let redirected = WithdrawalAuthorization {
            to: address!("0x3333333333333333333333333333333333333333"),
            ..authorization.clone()
        };
        assert_eq!(
            verify_authorization(&redirected, &domain, &signature),
            Err(AuthorizationError::SignerMismatch)
        );
        let base = TokenDomain {
            chain_id: 8453,
            ..domain.clone()
        };
        assert_eq!(
            verify_authorization(&authorization, &base, &signature),
            Err(AuthorizationError::SignerMismatch)
        );
        assert_eq!(
            verify_authorization(&authorization, &domain, "0x1234"),
            Err(AuthorizationError::SignatureMalformed)
        );
    }
}
