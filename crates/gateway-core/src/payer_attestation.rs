//! The payer's wallet attestation: an EIP-712 signature, made in the payer's
//! session once the deposit request's policy is satisfied, from the wallet
//! they will pay from. It is what binds a person who passed the policy to a
//! wallet, and it is committed into the payment address: the CREATE3 salt is
//! derived from the attribution hash and this attestation's signing digest,
//! and the wallet is the address's recovery term (see [`crate::Invoice`]).
//!
//! ```text
//! domain  = EIP712Domain{name: "Payday", version: "1", chainId, verifyingContract: factory}
//! message = PayerAttestation{statement, attributionHash, wallet, nonce, expiresAt}
//! digest  = keccak256(0x1901 || domainSeparator || hashStruct(message))
//! ```
//!
//! Everything a verifier needs to recompute the digest is in the proof; the
//! signature must recover to `wallet`. Only ECDSA (externally owned) wallets
//! are accepted for now; EIP-1271 contract wallets would need a chain read.

use std::str::FromStr;

use alloy_primitives::{Address, B256, Signature, U256, hex};
use alloy_sol_types::{Eip712Domain, SolStruct, eip712_domain, sol};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// EIP-712 domain name and version. A new message shape means a new version.
pub const PAYER_ATTESTATION_DOMAIN_NAME: &str = "Payday";
pub const PAYER_ATTESTATION_DOMAIN_VERSION: &str = "1";
pub const PAYER_ATTESTATION_PRIMARY_TYPE: &str = "PayerAttestation";
/// The sentence the payer signs. It is part of the signed bytes, so a wallet
/// that renders typed data shows the payer exactly what they are agreeing to.
pub const PAYER_ATTESTATION_STATEMENT: &str = "I control this wallet and will pay this Payday deposit request from it. Only transfers from this wallet count toward the request, and any funds Payday returns go back to it.";
/// The one signature method verifiable offline.
pub const PAYER_ATTESTATION_METHOD_ECDSA: &str = "ecdsa";

sol! {
    /// The EIP-712 struct. Field order is the type's canonical order and
    /// must never change; see the version above.
    struct PayerAttestation {
        string statement;
        bytes32 attributionHash;
        address wallet;
        bytes32 nonce;
        uint256 expiresAt;
    }
}

fn domain(chain_id: u64, factory: Address) -> Eip712Domain {
    eip712_domain! {
        name: PAYER_ATTESTATION_DOMAIN_NAME,
        version: PAYER_ATTESTATION_DOMAIN_VERSION,
        chain_id: chain_id,
        verifying_contract: factory,
    }
}

impl PayerAttestation {
    pub fn new(attribution_hash: B256, wallet: Address, nonce: B256, expires_at: u64) -> Self {
        Self {
            statement: PAYER_ATTESTATION_STATEMENT.into(),
            attributionHash: attribution_hash,
            wallet,
            nonce,
            expiresAt: U256::from(expires_at),
        }
    }

    /// The EIP-712 signing hash under the deployment's domain.
    pub fn digest(&self, chain_id: u64, factory: Address) -> B256 {
        self.eip712_signing_hash(&domain(chain_id, factory))
    }

    /// The document a wallet signs (`eth_signTypedData_v4`), exactly as the
    /// checkout hands it to the wallet and as the proof records it.
    pub fn typed_data(&self, chain_id: u64, factory: Address) -> PayerAttestationTypedData {
        PayerAttestationTypedData {
            domain: TypedDataDomain {
                name: PAYER_ATTESTATION_DOMAIN_NAME.into(),
                version: PAYER_ATTESTATION_DOMAIN_VERSION.into(),
                chain_id,
                verifying_contract: factory.to_checksum(None),
            },
            primary_type: PAYER_ATTESTATION_PRIMARY_TYPE.into(),
            types: TypedDataTypes {
                eip712_domain: vec![
                    TypedDataField::new("name", "string"),
                    TypedDataField::new("version", "string"),
                    TypedDataField::new("chainId", "uint256"),
                    TypedDataField::new("verifyingContract", "address"),
                ],
                payer_attestation: vec![
                    TypedDataField::new("statement", "string"),
                    TypedDataField::new("attributionHash", "bytes32"),
                    TypedDataField::new("wallet", "address"),
                    TypedDataField::new("nonce", "bytes32"),
                    TypedDataField::new("expiresAt", "uint256"),
                ],
            },
            message: PayerAttestationMessage {
                statement: self.statement.clone(),
                attribution_hash: self.attributionHash.to_string(),
                wallet: self.wallet.to_checksum(None),
                nonce: self.nonce.to_string(),
                expires_at: self.expiresAt.to::<u64>(),
            },
        }
    }
}

/// EIP-712 typed data in the JSON shape wallets accept (`eth_signTypedData_v4`,
/// viem's `signTypedData`): camelCase keys, a numeric chain id, and the
/// message's `expiresAt` as a JSON number (a unix timestamp, well within
/// double precision).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PayerAttestationTypedData {
    pub domain: TypedDataDomain,
    pub primary_type: String,
    pub types: TypedDataTypes,
    pub message: PayerAttestationMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TypedDataDomain {
    pub name: String,
    pub version: String,
    pub chain_id: u64,
    pub verifying_contract: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TypedDataTypes {
    #[serde(rename = "EIP712Domain")]
    pub eip712_domain: Vec<TypedDataField>,
    #[serde(rename = "PayerAttestation")]
    pub payer_attestation: Vec<TypedDataField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TypedDataField {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
}

impl TypedDataField {
    fn new(name: &str, kind: &str) -> Self {
        Self {
            name: name.into(),
            kind: kind.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PayerAttestationMessage {
    pub statement: String,
    /// `0x` hex, 32 bytes.
    pub attribution_hash: String,
    /// EIP-55 checksummed.
    pub wallet: String,
    /// `0x` hex, 32 bytes: the one-time challenge the session was issued
    /// after its policy passed.
    pub nonce: String,
    /// Unix seconds after which the challenge is void.
    pub expires_at: u64,
}

/// The attestation as the proof carries it and as the request stores it:
/// the wallet, the exact typed data it signed, the digest that data hashes
/// to, and the signature. `digest` is redundant with `typed_data` and is
/// kept so the salt derivation is legible without an EIP-712 implementation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PayerWalletAttestation {
    /// EIP-55 checksummed wallet the signature recovers to.
    pub address: String,
    pub typed_data: PayerAttestationTypedData,
    /// `0x` hex, 32 bytes: the EIP-712 signing hash of `typed_data`.
    pub digest: String,
    /// `0x` hex, 65 bytes `r || s || v`.
    pub signature: String,
    /// `ecdsa`.
    pub method: String,
}

/// What the attestation must be for, so a signature for one request or one
/// deployment never binds another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayerAttestationScope {
    pub chain_id: u64,
    pub factory: Address,
    pub attribution_hash: B256,
}

/// What a verified attestation established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedPayerAttestation {
    pub wallet: Address,
    pub digest: B256,
    pub nonce: B256,
    pub expires_at: u64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PayerAttestationError {
    #[error("payer attestation {0} is malformed")]
    Malformed(&'static str),
    #[error("payer attestation method is not supported")]
    UnsupportedMethod,
    #[error("payer attestation typed data is not the Payday attestation type")]
    UnexpectedTypedData,
    #[error("payer attestation domain does not match this deployment")]
    DomainMismatch,
    #[error("payer attestation does not name this deposit request")]
    AttributionHashMismatch,
    #[error("payer attestation digest does not match its typed data")]
    DigestMismatch,
    #[error("payer attestation signature is invalid")]
    SignatureInvalid,
    #[error("payer attestation signature was not made by the stated wallet")]
    SignerMismatch,
}

/// Verify an attestation offline: the typed data must be the Payday
/// attestation type under `scope`'s domain and name `scope`'s attribution
/// hash; its digest must be what the typed data hashes to; and the signature
/// must recover to the wallet the message names, which must be `address`.
pub fn verify_payer_attestation(
    attestation: &PayerWalletAttestation,
    scope: PayerAttestationScope,
) -> Result<VerifiedPayerAttestation, PayerAttestationError> {
    if attestation.method != PAYER_ATTESTATION_METHOD_ECDSA {
        return Err(PayerAttestationError::UnsupportedMethod);
    }
    let typed = &attestation.typed_data;
    if typed.primary_type != PAYER_ATTESTATION_PRIMARY_TYPE
        || typed.message.statement != PAYER_ATTESTATION_STATEMENT
    {
        return Err(PayerAttestationError::UnexpectedTypedData);
    }
    let verifying_contract = Address::from_str(&typed.domain.verifying_contract)
        .map_err(|_| PayerAttestationError::Malformed("domain.verifyingContract"))?;
    if typed.domain.name != PAYER_ATTESTATION_DOMAIN_NAME
        || typed.domain.version != PAYER_ATTESTATION_DOMAIN_VERSION
        || typed.domain.chain_id != scope.chain_id
        || verifying_contract != scope.factory
    {
        return Err(PayerAttestationError::DomainMismatch);
    }
    let attribution_hash = B256::from_str(&typed.message.attribution_hash)
        .map_err(|_| PayerAttestationError::Malformed("message.attributionHash"))?;
    if attribution_hash != scope.attribution_hash {
        return Err(PayerAttestationError::AttributionHashMismatch);
    }
    let wallet = Address::from_str(&typed.message.wallet)
        .map_err(|_| PayerAttestationError::Malformed("message.wallet"))?;
    let stated = Address::from_str(&attestation.address)
        .map_err(|_| PayerAttestationError::Malformed("address"))?;
    if wallet.is_zero() || wallet != stated {
        return Err(PayerAttestationError::SignerMismatch);
    }
    let nonce = B256::from_str(&typed.message.nonce)
        .map_err(|_| PayerAttestationError::Malformed("message.nonce"))?;
    let message = PayerAttestation::new(attribution_hash, wallet, nonce, typed.message.expires_at);
    let digest = message.digest(scope.chain_id, scope.factory);
    let stated_digest = B256::from_str(&attestation.digest)
        .map_err(|_| PayerAttestationError::Malformed("digest"))?;
    if digest != stated_digest {
        return Err(PayerAttestationError::DigestMismatch);
    }
    let signature = hex::decode(&attestation.signature)
        .ok()
        .and_then(|bytes| Signature::from_raw(&bytes).ok())
        .ok_or(PayerAttestationError::SignatureInvalid)?;
    let recovered = signature
        .recover_address_from_prehash(&digest)
        .map_err(|_| PayerAttestationError::SignatureInvalid)?;
    if recovered != wallet {
        return Err(PayerAttestationError::SignerMismatch);
    }
    Ok(VerifiedPayerAttestation {
        wallet,
        digest,
        nonce,
        expires_at: typed.message.expires_at,
    })
}

/// Assemble the stored/proof form from a message and its raw signature. The
/// signature is not checked here; [`verify_payer_attestation`] is.
pub fn payer_wallet_attestation(
    message: &PayerAttestation,
    chain_id: u64,
    factory: Address,
    signature: &Signature,
) -> PayerWalletAttestation {
    PayerWalletAttestation {
        address: message.wallet.to_checksum(None),
        typed_data: message.typed_data(chain_id, factory),
        digest: message.digest(chain_id, factory).to_string(),
        signature: hex::encode_prefixed(signature.as_bytes()),
        method: PAYER_ATTESTATION_METHOD_ECDSA.into(),
    }
}

/// Sign an attestation with a raw secp256k1 key. For tests and local tooling
/// only: production signatures come from the payer's wallet.
#[cfg(any(test, feature = "test-helpers"))]
pub fn sign_payer_attestation(
    secret: &[u8; 32],
    message: &PayerAttestation,
    chain_id: u64,
    factory: Address,
) -> PayerWalletAttestation {
    let key = k256::ecdsa::SigningKey::from_slice(secret).expect("valid secp256k1 key");
    let digest = message.digest(chain_id, factory);
    let (signature, recovery) = key
        .sign_prehash_recoverable(digest.as_slice())
        .expect("signing cannot fail");
    let signature = Signature::from_signature_and_parity(signature, recovery.is_y_odd());
    payer_wallet_attestation(message, chain_id, factory, &signature)
}

/// The address a raw secp256k1 key signs as. Test helper.
#[cfg(any(test, feature = "test-helpers"))]
pub fn wallet_of(secret: &[u8; 32]) -> Address {
    let key = k256::ecdsa::SigningKey::from_slice(secret).expect("valid secp256k1 key");
    Address::from_public_key(key.verifying_key())
}

#[cfg(test)]
mod tests {
    use alloy_primitives::address;

    use super::*;

    const CHAIN: u64 = 143;
    const FACTORY: Address = address!("0x5FbDB2315678afecb367f032d93F642f64180aa3");
    const SECRET: [u8; 32] = [7u8; 32];

    fn message() -> PayerAttestation {
        PayerAttestation::new(
            B256::repeat_byte(0x8a),
            wallet_of(&SECRET),
            B256::repeat_byte(0x11),
            1_900_000_000,
        )
    }

    fn scope() -> PayerAttestationScope {
        PayerAttestationScope {
            chain_id: CHAIN,
            factory: FACTORY,
            attribution_hash: B256::repeat_byte(0x8a),
        }
    }

    /// Pinned against viem's `hashTypedData` for the same document
    /// (`web/lib/payer-attestation.test.ts`); the two implementations must
    /// agree or a wallet's signature would never verify.
    #[test]
    fn digest_is_pinned_and_typed_data_is_the_wallet_shape() {
        let message = message();
        assert_eq!(
            message.digest(CHAIN, FACTORY).to_string(),
            "0x23f81e489d7192b8735c0d0c7866fbd8cd502c345b7dca546fc0bd29c0585392"
        );
        let typed = message.typed_data(CHAIN, FACTORY);
        let json = serde_json::to_value(&typed).unwrap();
        assert_eq!(json["primaryType"], "PayerAttestation");
        assert_eq!(json["domain"]["chainId"], 143);
        assert_eq!(
            json["domain"]["verifyingContract"],
            FACTORY.to_checksum(None)
        );
        assert_eq!(json["message"]["expiresAt"], 1_900_000_000u64);
        assert_eq!(
            json["message"]["wallet"],
            wallet_of(&SECRET).to_checksum(None)
        );
        assert_eq!(
            json["types"]["PayerAttestation"][1]["name"],
            "attributionHash"
        );
        let parsed: PayerAttestationTypedData = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, typed);
    }

    #[test]
    fn a_signed_attestation_verifies_and_every_tamper_is_caught() {
        let good = sign_payer_attestation(&SECRET, &message(), CHAIN, FACTORY);
        let verified = verify_payer_attestation(&good, scope()).unwrap();
        assert_eq!(verified.wallet, wallet_of(&SECRET));
        assert_eq!(verified.nonce, B256::repeat_byte(0x11));
        assert_eq!(verified.expires_at, 1_900_000_000);
        assert_eq!(verified.digest.to_string(), good.digest);

        let check = |mutate: fn(&mut PayerWalletAttestation)| {
            let mut attestation = good.clone();
            mutate(&mut attestation);
            verify_payer_attestation(&attestation, scope()).unwrap_err()
        };
        assert_eq!(
            check(|a| a.method = "eip1271".into()),
            PayerAttestationError::UnsupportedMethod
        );
        assert_eq!(
            check(|a| a.typed_data.message.statement = "I agree".into()),
            PayerAttestationError::UnexpectedTypedData
        );
        assert_eq!(
            check(|a| a.typed_data.domain.chain_id = 1),
            PayerAttestationError::DomainMismatch
        );
        assert_eq!(
            check(|a| a.typed_data.message.attribution_hash = B256::repeat_byte(0x8b).to_string()),
            PayerAttestationError::AttributionHashMismatch
        );
        assert_eq!(
            check(|a| a.typed_data.message.nonce = B256::repeat_byte(0x12).to_string()),
            PayerAttestationError::DigestMismatch
        );
        assert_eq!(
            check(|a| a.typed_data.message.expires_at += 1),
            PayerAttestationError::DigestMismatch
        );
        assert_eq!(
            check(|a| a.digest = B256::repeat_byte(0x01).to_string()),
            PayerAttestationError::DigestMismatch
        );
        assert_eq!(
            check(|a| a.signature = "0x1234".into()),
            PayerAttestationError::SignatureInvalid
        );
        // Another key's signature over the same message recovers elsewhere.
        assert_eq!(
            check(|a| {
                a.signature =
                    sign_payer_attestation(&[9u8; 32], &message(), CHAIN, FACTORY).signature
            }),
            PayerAttestationError::SignerMismatch
        );
        // Restating the wallet without re-signing moves the digest first.
        assert_eq!(
            check(|a| {
                let other = wallet_of(&[9u8; 32]).to_checksum(None);
                a.address = other.clone();
                a.typed_data.message.wallet = other;
            }),
            PayerAttestationError::DigestMismatch
        );
        assert_eq!(
            check(|a| a.address = wallet_of(&[9u8; 32]).to_checksum(None)),
            PayerAttestationError::SignerMismatch
        );
        // The scope decides what the attestation is for.
        assert_eq!(
            verify_payer_attestation(
                &good,
                PayerAttestationScope {
                    attribution_hash: B256::repeat_byte(0x8b),
                    ..scope()
                }
            )
            .unwrap_err(),
            PayerAttestationError::AttributionHashMismatch
        );
        assert_eq!(
            verify_payer_attestation(
                &good,
                PayerAttestationScope {
                    factory: Address::repeat_byte(0x33),
                    ..scope()
                }
            )
            .unwrap_err(),
            PayerAttestationError::DomainMismatch
        );
    }
}
