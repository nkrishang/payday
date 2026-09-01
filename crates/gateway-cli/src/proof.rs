//! `payday proof verify`: the offline checks from `gateway_core::verify_proof`,
//! itemised so a reader sees which link of the chain held, plus optional live
//! receipt checks over plain JSON-RPC: one for the payer's transfers into the
//! address, one for the settlement transaction that forwarded the funds out.

use std::str::FromStr;
use std::time::Duration;

use alloy_primitives::{Address, B256, U256, b256};
use gateway_core::{ProofError, ProofOfPayment, ProofVerification, verify_proof};
use serde::{Deserialize, Serialize};

/// `keccak256("Transfer(address,address,uint256)")`, the ERC-20 event topic.
const TRANSFER_TOPIC: B256 =
    b256!("0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: CheckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The verifier's verdict: every check in order, and whether none failed.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub checks: Vec<Check>,
    pub ok: bool,
}

// The offline checks in the order `verify_proof` performs them; the receipt
// checks come last because they need the payment address the others establish.
const ATTRIBUTION: usize = 0;
const SALT: usize = 1;
const ADDRESS: usize = 2;
const ATTACHMENT: usize = 3;
const ATTESTATION: usize = 4;
const TRANSFERS: usize = 5;
const RECEIPTS: usize = 6;
const SETTLEMENT: usize = 7;

const NAMES: [&str; 8] = [
    "Canonical invoice hashes to the attribution hash",
    "Salt derives from the nonce and attribution hash",
    "CREATE3 payment address matches the canonical invoice",
    "Attachment matches the committed SHA-256 and length",
    "Verification attestation recovers to its signer and commits to this invoice",
    "Every transfer paid the payment address and together they cover the invoice amount",
    "Transfer receipts on chain succeeded with matching USDC Transfer logs",
    "Settlement receipt on chain forwarded the invoice amount from the payment address to the receiver",
];

impl Check {
    fn passed(index: usize, detail: impl Into<Option<String>>) -> Self {
        Self {
            name: NAMES[index],
            status: CheckStatus::Passed,
            detail: detail.into(),
        }
    }

    fn failed(index: usize, detail: String) -> Self {
        Self {
            name: NAMES[index],
            status: CheckStatus::Failed,
            detail: Some(detail),
        }
    }

    fn skipped(index: usize, detail: &str) -> Self {
        Self {
            name: NAMES[index],
            status: CheckStatus::Skipped,
            detail: Some(detail.into()),
        }
    }
}

/// Run every check. Offline checks always run; the receipt checks run only
/// with an RPC endpoint and only once the offline chain holds, because they
/// depend on the payment address those checks establish.
pub async fn verify(
    proof: &ProofOfPayment,
    attachment: Option<&[u8]>,
    trusted_attestors: &[Address],
    rpc_url: Option<&str>,
) -> Report {
    let (mut checks, verified) = offline(proof, attachment, trusted_attestors);
    let outcome = |index: usize, result: Result<String, String>| match result {
        Ok(detail) => Check::passed(index, detail),
        Err(detail) => Check::failed(index, detail),
    };
    match (rpc_url, verified) {
        (None, _) => {
            checks.push(Check::skipped(RECEIPTS, "no --rpc-url supplied"));
            checks.push(Check::skipped(SETTLEMENT, "no --rpc-url supplied"));
        }
        (Some(_), None) => {
            checks.push(Check::skipped(
                RECEIPTS,
                "not checked after an earlier failure",
            ));
            checks.push(Check::skipped(
                SETTLEMENT,
                "not checked after an earlier failure",
            ));
        }
        (Some(url), Some(verified)) => match Rpc::new(url) {
            Ok(rpc) => {
                // Both checks are reported even when the first fails: they
                // establish different things (money in, money out).
                checks.push(outcome(
                    RECEIPTS,
                    confirm_receipts(&rpc, proof, &verified).await,
                ));
                checks.push(outcome(
                    SETTLEMENT,
                    confirm_settlement(&rpc, proof, &verified).await,
                ));
            }
            Err(detail) => {
                checks.push(Check::failed(RECEIPTS, detail.clone()));
                checks.push(Check::failed(SETTLEMENT, detail));
            }
        },
    }
    let ok = checks
        .iter()
        .all(|check| check.status != CheckStatus::Failed);
    Report { checks, ok }
}

/// The offline checks, itemised. `verify_proof` stops at the first broken
/// link, so the checks after it are reported as not checked rather than
/// guessed at.
pub fn offline(
    proof: &ProofOfPayment,
    attachment: Option<&[u8]>,
    trusted_attestors: &[Address],
) -> (Vec<Check>, Option<ProofVerification>) {
    let attachment_skipped = || Check::skipped(ATTACHMENT, "no --attachment supplied");
    match verify_proof(proof, attachment, trusted_attestors) {
        Ok(verified) => {
            let attestor = if trusted_attestors.is_empty() {
                format!(
                    "signer {}; no --trusted-attestor supplied, so any self-consistent signer is accepted",
                    verified.attestation_signer.to_checksum(None)
                )
            } else {
                format!(
                    "signer {} is a trusted attestor",
                    verified.attestation_signer.to_checksum(None)
                )
            };
            let checks =
                vec![
                    Check::passed(ATTRIBUTION, verified.attribution_hash.to_string()),
                    Check::passed(SALT, verified.salt.0.to_string()),
                    Check::passed(ADDRESS, verified.payment_address.to_checksum(None)),
                    if verified.attachment_verified {
                        Check::passed(
                            ATTACHMENT,
                            proof.canonical_issuance_snapshot.attachment.as_ref().map(
                                |commitment| {
                                    format!(
                                        "{} over {} bytes",
                                        commitment.sha256, commitment.byte_length
                                    )
                                },
                            ),
                        )
                    } else {
                        attachment_skipped()
                    },
                    Check::passed(ATTESTATION, attestor),
                    Check::passed(
                        TRANSFERS,
                        format!(
                            "{} transfer(s); settlement {}",
                            proof.transfers.len(),
                            proof.settlement_transaction_hash
                        ),
                    ),
                ];
            (checks, Some(verified))
        }
        Err(error) => {
            let failing = failing_check(&error);
            let checks = (ATTRIBUTION..=TRANSFERS)
                .map(|index| {
                    if index == ATTACHMENT && attachment.is_none() {
                        attachment_skipped()
                    } else if index < failing {
                        Check::passed(index, None)
                    } else if index == failing {
                        Check::failed(index, error.to_string())
                    } else {
                        Check::skipped(index, "not checked after an earlier failure")
                    }
                })
                .collect();
            (checks, None)
        }
    }
}

/// Which itemised check a `verify_proof` failure belongs to.
fn failing_check(error: &ProofError) -> usize {
    match error {
        ProofError::Unsupported("attestation version") => ATTESTATION,
        ProofError::Unsupported(_)
        | ProofError::Canonicalization(_)
        | ProofError::AttributionHashMismatch => ATTRIBUTION,
        ProofError::SaltMismatch => SALT,
        ProofError::ChainParametersMismatch | ProofError::PaymentAddressMismatch => ADDRESS,
        ProofError::AttachmentNotCommitted
        | ProofError::AttachmentLengthMismatch
        | ProofError::AttachmentHashMismatch => ATTACHMENT,
        ProofError::AttestationSignatureInvalid
        | ProofError::AttestationSignerMismatch
        | ProofError::UntrustedAttestor
        | ProofError::AttestationPaymentIdMismatch
        | ProofError::AttestationModeMismatch
        | ProofError::AttestationCommitmentMismatch => ATTESTATION,
        ProofError::TransferRecipientMismatch | ProofError::TransfersBelowInvoiceAmount => {
            TRANSFERS
        }
        ProofError::Malformed(field) => match *field {
            "attribution_hash" => ATTRIBUTION,
            "attribution_nonce" | "salt" => SALT,
            "attachment sha256" => ATTACHMENT,
            "verification.signer"
            | "verification.payload.attribution_hash"
            | "verification.payload.payment_address" => ATTESTATION,
            "settlement_transaction_hash"
            | "transfer recipient"
            | "transfer transaction_hash"
            | "transfer amount_base_units" => TRANSFERS,
            _ => ADDRESS,
        },
    }
}

#[derive(Deserialize)]
struct RpcResponse {
    #[serde(default)]
    result: Option<Receipt>,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Receipt {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    logs: Vec<Log>,
}

#[derive(Deserialize)]
struct Log {
    address: String,
    topics: Vec<String>,
    data: String,
}

impl Receipt {
    /// Whether the receipt carries an ERC-20 `Transfer` of `amount` in `token`
    /// from `from` to `to`.
    fn has_transfer(&self, token: Address, from: Address, to: Address, amount: U256) -> bool {
        self.logs.iter().any(|log| {
            Address::from_str(&log.address).is_ok_and(|address| address == token)
                && log.topics.len() == 3
                && parse_word(&log.topics[0]) == Some(TRANSFER_TOPIC)
                && parse_word(&log.topics[1]) == Some(from.into_word())
                && parse_word(&log.topics[2]) == Some(to.into_word())
                && parse_u256(&log.data) == Some(amount)
        })
    }
}

/// A plain JSON-RPC endpoint for `eth_getTransactionReceipt`.
struct Rpc {
    http: reqwest::Client,
    url: String,
    /// RPC URLs often embed an API key; only the host is named in anything shown.
    host: String,
}

impl Rpc {
    fn new(rpc_url: &str) -> Result<Self, String> {
        let host = reqwest::Url::parse(rpc_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .ok_or_else(|| "--rpc-url is not a valid URL".to_string())?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            http,
            url: rpc_url.to_owned(),
            host,
        })
    }

    /// A successful (`status == 0x1`) receipt for `hash`, or why there is none.
    async fn successful_receipt(&self, hash: &str) -> Result<Receipt, String> {
        let host = &self.host;
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_getTransactionReceipt",
            "params": [hash],
        });
        let response: RpcResponse = self
            .http
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("could not reach {host}: {}", error.without_url()))?
            .error_for_status()
            .map_err(|error| format!("{host} answered {}", error.without_url()))?
            .json()
            .await
            .map_err(|error| {
                format!(
                    "{host} returned an unreadable reply: {}",
                    error.without_url()
                )
            })?;
        if let Some(error) = response.error {
            return Err(format!("{host} returned an RPC error: {error}"));
        }
        let receipt = response.result.ok_or_else(|| {
            format!("no receipt for {hash} at {host}; the transaction is not mined on that chain")
        })?;
        if receipt.status.as_deref() != Some("0x1") {
            return Err(format!(
                "transaction {hash} did not succeed (status {})",
                receipt.status.as_deref().unwrap_or("missing")
            ));
        }
        Ok(receipt)
    }
}

/// Fetch each transfer's receipt and require a successful transaction
/// carrying a USDC `Transfer` log from the proof's sender to the payment
/// address for the proof's amount.
async fn confirm_receipts(
    rpc: &Rpc,
    proof: &ProofOfPayment,
    verified: &ProofVerification,
) -> Result<String, String> {
    let token = Address::from_str(&proof.token_address)
        .map_err(|_| "token_address is malformed".to_string())?;
    let mut hashes: Vec<&str> = Vec::new();
    for transfer in &proof.transfers {
        if !hashes.contains(&transfer.transaction_hash.as_str()) {
            hashes.push(&transfer.transaction_hash);
        }
    }
    for hash in &hashes {
        let receipt = rpc.successful_receipt(hash).await?;
        for transfer in proof
            .transfers
            .iter()
            .filter(|transfer| transfer.transaction_hash == *hash)
        {
            let sender = Address::from_str(&transfer.sender)
                .map_err(|_| format!("transfer sender {} is malformed", transfer.sender))?;
            let amount = U256::from_str_radix(&transfer.amount_base_units, 10).map_err(|_| {
                format!(
                    "transfer amount {} is malformed",
                    transfer.amount_base_units
                )
            })?;
            if !receipt.has_transfer(token, sender, verified.payment_address, amount) {
                return Err(format!(
                    "no USDC Transfer of {} base units from {} to {} in {hash}",
                    transfer.amount_base_units,
                    transfer.sender,
                    verified.payment_address.to_checksum(None)
                ));
            }
        }
    }
    Ok(format!(
        "{} receipt(s) confirmed via {}",
        hashes.len(),
        rpc.host
    ))
}

/// Fetch the settlement transaction's receipt and require the exact
/// settlement leg: a successful transaction carrying a USDC `Transfer` of the
/// invoice amount from the payment address to the snapshot's receiver. This
/// is the one thing the offline checks cannot establish about the settlement
/// hash, which is otherwise unsigned data in the proof.
async fn confirm_settlement(
    rpc: &Rpc,
    proof: &ProofOfPayment,
    verified: &ProofVerification,
) -> Result<String, String> {
    let snapshot = &proof.canonical_issuance_snapshot;
    let token = Address::from_str(&proof.token_address)
        .map_err(|_| "token_address is malformed".to_string())?;
    let receiver = Address::from_str(&snapshot.receiver_address)
        .map_err(|_| "receiver_address is malformed".to_string())?;
    let amount = U256::from_str_radix(&snapshot.amount_base_units, 10)
        .map_err(|_| "amount_base_units is malformed".to_string())?;
    let hash = &proof.settlement_transaction_hash;
    let receipt = rpc.successful_receipt(hash).await?;
    if !receipt.has_transfer(token, verified.payment_address, receiver, amount) {
        return Err(format!(
            "no USDC Transfer of {} base units from {} to {} in {hash}",
            snapshot.amount_base_units,
            verified.payment_address.to_checksum(None),
            snapshot.receiver_address
        ));
    }
    Ok(format!(
        "{} base units forwarded to {} in {hash} via {}",
        snapshot.amount_base_units, snapshot.receiver_address, rpc.host
    ))
}

fn parse_word(value: &str) -> Option<B256> {
    B256::from_str(value).ok()
}

fn parse_u256(value: &str) -> Option<U256> {
    U256::from_str_radix(value.trim_start_matches("0x"), 16).ok()
}

#[cfg(test)]
pub(crate) mod fixture {
    //! A proof issued and attested in-test with a throwaway key, so the CLI
    //! verifier is exercised against real `gateway-core` material.

    use alloy_primitives::{Address, B256, Signature, U256, address, hex};
    use gateway_core::{
        ATTESTATION_VERSION, Amount, AttachmentCommitment, BeneficiaryAddress, CANONICALIZATION,
        CanonicalIssuanceSnapshot, ChainId, FactoryAddress, Invoice, PROOF_VERSION, Party,
        PayerPolicy, PayerPolicyMode, ProofOfPayment, ProofTransfer, RecoveryAddress,
        SignedVerificationAttestation, TokenAddress, VerificationAttestationPayload,
        attestation_digest,
    };
    use k256::ecdsa::SigningKey;
    use sha2::{Digest, Sha256};

    pub(crate) const ATTACHMENT_BYTES: &[u8] = b"%PDF-1.7 minimal test document";
    pub(crate) const SENDER: Address = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
    pub(crate) const TRANSFER_HASH: B256 = B256::repeat_byte(0xA1);
    pub(crate) const SETTLEMENT_HASH: B256 = B256::repeat_byte(0xB2);

    fn signing_key() -> SigningKey {
        SigningKey::from_slice(&[7u8; 32]).unwrap()
    }

    pub(crate) fn attestor() -> Address {
        Address::from_public_key(signing_key().verifying_key())
    }

    fn party(name: &str) -> Party {
        Party {
            name: name.into(),
            email: None,
            details: None,
        }
    }

    pub(crate) fn proof() -> ProofOfPayment {
        let factory = FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3"));
        let chain_id = ChainId(143);
        let token = TokenAddress(address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603"));
        let beneficiary =
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"));
        let amount = Amount(U256::from(2_500_000));
        let recovery = RecoveryAddress(address!("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"));
        let mut snapshot = CanonicalIssuanceSnapshot::new(
            party("Acme"),
            party("Globex"),
            PayerPolicy::VerifiedEmail {
                expected_email: "alice@example.com".into(),
            },
            factory,
            chain_id,
            token,
            beneficiary,
            amount,
            1_900_000_000,
            recovery,
        );
        let digest: [u8; 32] = Sha256::digest(ATTACHMENT_BYTES).into();
        snapshot.attachment = Some(AttachmentCommitment {
            id: uuid::Uuid::from_u128(9),
            byte_length: ATTACHMENT_BYTES.len().to_string(),
            sha256: hex::encode_prefixed(digest),
        });
        let invoice = Invoice::issue(
            factory,
            chain_id,
            token,
            beneficiary,
            amount,
            1_900_000_000,
            recovery,
            snapshot,
        )
        .unwrap();

        let payload = VerificationAttestationPayload {
            version: ATTESTATION_VERSION.into(),
            payment_id: invoice.id.to_string(),
            attribution_hash: invoice.attribution_hash.to_string(),
            chain_id: invoice.chain_id.0.to_string(),
            payment_address: invoice.payment_address.0.to_checksum(None),
            payer_policy_mode: PayerPolicyMode::VerifiedEmail.as_str().into(),
            result: "approved".into(),
            verified_at: Some("2026-09-01T12:00:00Z".into()),
        };
        let digest = attestation_digest(&payload).unwrap();
        let (signature, recovery_id) = signing_key()
            .sign_prehash_recoverable(digest.as_slice())
            .unwrap();
        let signature = Signature::from_signature_and_parity(signature, recovery_id.is_y_odd());

        // The payer's transfer and the fulfilment transaction are distinct.
        ProofOfPayment {
            version: PROOF_VERSION.into(),
            payment_id: invoice.id.to_string(),
            canonical_issuance_snapshot: invoice.issuance_snapshot.clone(),
            canonicalization: CANONICALIZATION.into(),
            attribution_nonce: invoice.attribution_nonce.to_string(),
            attribution_hash: invoice.attribution_hash.to_string(),
            salt: invoice.salt.0.to_string(),
            chain_id: invoice.chain_id.0.to_string(),
            factory_address: invoice.factory.0.to_checksum(None),
            payment_address: invoice.payment_address.0.to_checksum(None),
            token_address: invoice.token.0.to_checksum(None),
            settlement_transaction_hash: SETTLEMENT_HASH.to_string(),
            transfers: vec![ProofTransfer {
                transaction_hash: TRANSFER_HASH.to_string(),
                sender: SENDER.to_checksum(None),
                recipient: invoice.payment_address.0.to_checksum(None),
                amount_base_units: "2500000".into(),
                block_number: "7".into(),
            }],
            verification: SignedVerificationAttestation {
                payload,
                signer: attestor().to_checksum(None),
                signature: hex::encode_prefixed(signature.as_bytes()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Address, keccak256};

    use super::fixture::{
        ATTACHMENT_BYTES, SENDER, SETTLEMENT_HASH, TRANSFER_HASH, attestor, proof,
    };
    use super::*;
    use crate::client::test_support::{capture_server, response};

    fn statuses(report: &Report) -> Vec<CheckStatus> {
        report.checks.iter().map(|check| check.status).collect()
    }

    fn detail(report: &Report, index: usize) -> &str {
        report.checks[index].detail.as_deref().unwrap()
    }

    #[test]
    fn transfer_topic_is_the_erc20_event_signature() {
        assert_eq!(
            TRANSFER_TOPIC,
            keccak256(b"Transfer(address,address,uint256)")
        );
    }

    #[tokio::test]
    async fn a_genuine_proof_passes_every_offline_check() {
        let proof = proof();
        let report = verify(&proof, Some(ATTACHMENT_BYTES), &[attestor()], None).await;
        assert!(report.ok, "{report:?}");
        assert_eq!(
            statuses(&report),
            [
                CheckStatus::Passed,
                CheckStatus::Passed,
                CheckStatus::Passed,
                CheckStatus::Passed,
                CheckStatus::Passed,
                CheckStatus::Passed,
                CheckStatus::Skipped,
                CheckStatus::Skipped,
            ]
        );
        assert_eq!(
            report.checks[ADDRESS].detail.as_deref(),
            Some(proof.payment_address.as_str())
        );
        assert!(detail(&report, ATTESTATION).contains("trusted attestor"));

        // Without the file the attachment check is reported as skipped, not passed.
        let report = verify(&proof, None, &[], None).await;
        assert!(report.ok);
        assert_eq!(report.checks[ATTACHMENT].status, CheckStatus::Skipped);
        assert!(detail(&report, ATTESTATION).contains("no --trusted-attestor"));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["checks"][RECEIPTS]["status"], "skipped");
        assert_eq!(json["checks"][SETTLEMENT]["status"], "skipped");
    }

    #[tokio::test]
    async fn a_tampered_snapshot_fails_at_the_attribution_hash() {
        let mut proof = proof();
        proof.canonical_issuance_snapshot.amount_base_units = "2500001".into();
        let report = verify(
            &proof,
            Some(ATTACHMENT_BYTES),
            &[attestor()],
            Some("http://127.0.0.1:9"),
        )
        .await;
        assert!(!report.ok);
        assert_eq!(report.checks[ATTRIBUTION].status, CheckStatus::Failed);
        assert!(detail(&report, ATTRIBUTION).contains("attribution hash"));
        // Nothing after the broken link is claimed, including the receipt checks.
        assert!(
            report.checks[SALT..]
                .iter()
                .all(|check| check.status == CheckStatus::Skipped)
        );
    }

    #[tokio::test]
    async fn an_untrusted_attestor_fails_only_the_attestation_check() {
        let proof = proof();
        let report = verify(&proof, None, &[Address::repeat_byte(0x05)], None).await;
        assert!(!report.ok);
        assert_eq!(
            statuses(&report),
            [
                CheckStatus::Passed,
                CheckStatus::Passed,
                CheckStatus::Passed,
                CheckStatus::Skipped,
                CheckStatus::Failed,
                CheckStatus::Skipped,
                CheckStatus::Skipped,
                CheckStatus::Skipped,
            ]
        );
    }

    #[tokio::test]
    async fn transfers_short_of_the_invoice_amount_fail_the_transfers_check() {
        let mut proof = proof();
        proof.transfers[0].amount_base_units = "2499999".into();
        let report = verify(&proof, None, &[attestor()], None).await;
        assert!(!report.ok);
        assert_eq!(report.checks[TRANSFERS].status, CheckStatus::Failed);
        assert!(detail(&report, TRANSFERS).contains("less than the invoice amount"));
        assert_eq!(report.checks[ATTESTATION].status, CheckStatus::Passed);
    }

    #[tokio::test]
    async fn a_different_attachment_fails_the_attachment_check() {
        let proof = proof();
        let mut altered = ATTACHMENT_BYTES.to_vec();
        altered[0] ^= 0xFF;
        let report = verify(&proof, Some(&altered), &[attestor()], None).await;
        assert!(!report.ok);
        assert_eq!(report.checks[ATTACHMENT].status, CheckStatus::Failed);
        assert!(detail(&report, ATTACHMENT).contains("SHA-256"));
        assert_eq!(report.checks[ATTESTATION].status, CheckStatus::Skipped);
    }

    fn receipt(status: &str, logs: serde_json::Value) -> String {
        response(
            "200 OK",
            "Content-Type: application/json\r\n",
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": { "status": status, "logs": logs }
            })
            .to_string(),
        )
    }

    fn no_receipt() -> String {
        response(
            "200 OK",
            "Content-Type: application/json\r\n",
            r#"{"jsonrpc":"2.0","id":1,"result":null}"#,
        )
    }

    fn transfer_log(
        proof: &ProofOfPayment,
        from: Address,
        to: Address,
        amount: u64,
    ) -> serde_json::Value {
        serde_json::json!({
            "address": proof.token_address.to_lowercase(),
            "topics": [
                TRANSFER_TOPIC.to_string(),
                from.into_word().to_string(),
                to.into_word().to_string(),
            ],
            "data": format!("0x{amount:064x}"),
        })
    }

    /// The payer's transfer into the address and the sweep out of it, as the
    /// chain would report them for `proof`. (Every `fixture::proof()` issues
    /// a fresh nonce, so receipts must be built for the proof under test.)
    fn genuine_receipts(proof: &ProofOfPayment) -> (String, String) {
        let payment_address: Address = proof.payment_address.parse().unwrap();
        let receiver: Address = proof
            .canonical_issuance_snapshot
            .receiver_address
            .parse()
            .unwrap();
        (
            receipt(
                "0x1",
                serde_json::json!([transfer_log(proof, SENDER, payment_address, 2_500_000)]),
            ),
            receipt(
                "0x1",
                serde_json::json!([transfer_log(proof, payment_address, receiver, 2_500_000)]),
            ),
        )
    }

    #[tokio::test]
    async fn the_rpc_check_requires_a_successful_receipt_with_the_transfer_log() {
        let proof = proof();
        let payment_address: Address = proof.payment_address.parse().unwrap();
        let (transfer_receipt, settlement_receipt) = genuine_receipts(&proof);
        // Each verification fetches the transfer receipt, then the settlement's.
        let (rpc_url, requests) = capture_server(vec![
            transfer_receipt.clone(),
            settlement_receipt.clone(),
            receipt(
                "0x0",
                serde_json::json!([transfer_log(&proof, SENDER, payment_address, 2_500_000)]),
            ),
            settlement_receipt.clone(),
            receipt("0x1", serde_json::json!([])),
            settlement_receipt.clone(),
            no_receipt(),
            settlement_receipt,
        ])
        .await;

        let report = verify(&proof, None, &[], Some(&rpc_url)).await;
        assert!(report.ok, "{report:?}");
        assert_eq!(report.checks[RECEIPTS].status, CheckStatus::Passed);
        assert_eq!(report.checks[SETTLEMENT].status, CheckStatus::Passed);

        let report = verify(&proof, None, &[], Some(&rpc_url)).await;
        assert!(!report.ok);
        assert!(detail(&report, RECEIPTS).contains("did not succeed"));
        assert_eq!(report.checks[SETTLEMENT].status, CheckStatus::Passed);

        let report = verify(&proof, None, &[], Some(&rpc_url)).await;
        assert!(!report.ok);
        assert!(detail(&report, RECEIPTS).contains("no USDC Transfer"));

        let report = verify(&proof, None, &[], Some(&rpc_url)).await;
        assert!(!report.ok);
        assert!(detail(&report, RECEIPTS).contains("no receipt"));

        let requests = requests.await.unwrap();
        assert_eq!(requests.len(), 8);
        assert!(requests[0].starts_with("POST / HTTP/1.1"));
        assert!(requests[0].contains(r#""method":"eth_getTransactionReceipt""#));
        assert!(requests[0].contains(&TRANSFER_HASH.to_string()));
        assert!(requests[1].contains(&SETTLEMENT_HASH.to_string()));
    }

    #[tokio::test]
    async fn the_settlement_check_requires_the_sweep_from_the_address_to_the_receiver() {
        let proof = proof();
        let payment_address: Address = proof.payment_address.parse().unwrap();
        let receiver: Address = proof
            .canonical_issuance_snapshot
            .receiver_address
            .parse()
            .unwrap();
        assert_ne!(
            proof.settlement_transaction_hash, proof.transfers[0].transaction_hash,
            "the fulfilment transaction is not a payer transfer"
        );
        let (transfer_receipt, _) = genuine_receipts(&proof);
        let (rpc_url, requests) = capture_server(vec![
            transfer_receipt.clone(),
            receipt(
                "0x0",
                serde_json::json!([transfer_log(&proof, payment_address, receiver, 2_500_000)]),
            ),
            transfer_receipt.clone(),
            // A sweep to somebody else, and one short of the invoice amount.
            receipt(
                "0x1",
                serde_json::json!([
                    transfer_log(
                        &proof,
                        payment_address,
                        Address::repeat_byte(0x09),
                        2_500_000
                    ),
                    transfer_log(&proof, payment_address, receiver, 2_499_999),
                ]),
            ),
            transfer_receipt,
            no_receipt(),
        ])
        .await;

        let report = verify(&proof, None, &[], Some(&rpc_url)).await;
        assert!(!report.ok);
        assert_eq!(report.checks[RECEIPTS].status, CheckStatus::Passed);
        assert_eq!(report.checks[SETTLEMENT].status, CheckStatus::Failed);
        assert!(detail(&report, SETTLEMENT).contains("did not succeed"));

        let report = verify(&proof, None, &[], Some(&rpc_url)).await;
        assert!(!report.ok);
        assert_eq!(report.checks[SETTLEMENT].status, CheckStatus::Failed);
        let reason = detail(&report, SETTLEMENT);
        assert!(reason.contains("no USDC Transfer of 2500000 base units"));
        assert!(reason.contains(&proof.payment_address));
        assert!(reason.contains(&proof.canonical_issuance_snapshot.receiver_address));

        let report = verify(&proof, None, &[], Some(&rpc_url)).await;
        assert!(!report.ok);
        assert!(detail(&report, SETTLEMENT).contains("no receipt"));

        let requests = requests.await.unwrap();
        assert_eq!(requests.len(), 6);
        assert!(requests[1].contains(&SETTLEMENT_HASH.to_string()));
        assert!(!requests[1].contains(&TRANSFER_HASH.to_string()));
    }
}
