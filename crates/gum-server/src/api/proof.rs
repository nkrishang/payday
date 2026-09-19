//! Proof of Payment (product plan §5.5, guide §Slice 2.6): everything a
//! holder needs to recompute hash → attestation → salt → CREATE3 address
//! offline and tie the request to the transfers from the attested wallet
//! that paid it and the transaction that settled it, plus a fresh
//! Gum-signed attestation of the verification facts bound to that same
//! commitment.

use alloy_primitives::{Address, B256};
use axum::Extension;
use axum::extract::{Path, State};
use chrono::SecondsFormat;
use gum_core::{
    ATTESTATION_VERSION, AttestedRelayFill, CANONICALIZATION, Invoice, InvoiceStatus,
    PROOF_VERSION, ProofOfPayment, ProofScope, ProofTransfer, RelayAttribution,
    VerificationAttestationPayload, VerificationFact,
};
use gum_ledger::AccountId;

use crate::api::deposit_requests::resolve_deposit_request;
use crate::api::error::ApiError;
use crate::api::json::Json;
use crate::state::AppState;

/// `0x`-prefixed lowercase hex of a 32-byte nonce, the form the proof and
/// its signed attestation carry.
fn hex_nonce(nonce: &B256) -> String {
    // `B256`'s display is already `0x`-prefixed 32-byte hex.
    nonce.to_string()
}

pub async fn get_proof(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(reference): Path<String>,
) -> Result<Json<ProofOfPayment>, ApiError> {
    let row = resolve_deposit_request(&state, account, &reference).await?;
    let invoice = Invoice::try_from(&row)?;
    // Only a fulfilled invoice, whose funds have reached the beneficiary,
    // has anything to prove. The row's settlement_tx_hash is the fulfilment
    // call that forwarded them (our batch sweep or a third party's execute);
    // it is what the proof names as the settlement transaction.
    let settlement_transaction_hash =
        match (row.status.parse::<InvoiceStatus>(), &row.settlement_tx_hash) {
            (Ok(InvoiceStatus::Fulfilled), Some(hash)) => B256::try_from(hash.as_slice())
                .map_err(|_| ApiError::internal("invalid settlement transaction hash"))?
                .to_string(),
            _ => return Err(ApiError::deposit_request_not_settled()),
        };
    // A fulfilled invoice was funded, and funds only reach a bound address.
    let binding = invoice
        .binding
        .as_ref()
        .ok_or_else(|| ApiError::internal("fulfilled invoice has no payer wallet binding"))?;
    let attestor = state.attestor()?;
    // A fulfilled invoice and its proof inputs are immutable. Account and
    // signer are in the key so cached material can never cross ownership or
    // attestation-key boundaries; authorization is still checked above.
    let cache_key = (account.0, row.id, attestor.address());
    if let Some(proof) = state.cached_proof(cache_key.0, cache_key.1, cache_key.2) {
        return Ok(Json(proof));
    }
    let settlement_transfers = state.proofs.settlement_transfers(row.id).await?;
    // The scope is the request's, not the holder's: an attested request
    // proves its wallet's payment, a permissionless one proves settlement.
    let scope = if invoice.wallet_attestation_required() {
        ProofScope::WalletAttributed
    } else {
        ProofScope::Settlement
    };
    let attested_wallet = binding.wallet.as_ref().map(|wallet| &wallet.payer_wallet);
    let payer_wallet = attested_wallet.map(|wallet| wallet.to_checksum(None));
    let transfers = settlement_transfers
        .iter()
        .map(|transfer| {
            // A transfer Relay's solver made for a cross-chain payment the
            // wallet sent carries its origin; the intent's payer wallet is
            // the depositor the indexer verified.
            let relay = match (
                &transfer.relay_request_id,
                transfer.relay_origin_chain_id,
                &transfer.relay_origin_tx_hash,
                &transfer.relay_payer_wallet,
                &transfer.relay_attribution_source,
            ) {
                (
                    Some(request_id),
                    Some(origin_chain_id),
                    Some(origin_tx),
                    Some(wallet),
                    Some(source),
                ) => Some(RelayAttribution {
                    request_id: B256::try_from(request_id.as_slice())
                        .map_err(|_| ApiError::internal("invalid relay request id"))?
                        .to_string(),
                    origin_chain_id: origin_chain_id.to_string(),
                    origin_transaction_hash: B256::try_from(origin_tx.as_slice())
                        .map_err(|_| ApiError::internal("invalid relay origin hash"))?
                        .to_string(),
                    origin_sender: Address::try_from(wallet.as_slice())
                        .map_err(|_| ApiError::internal("invalid relay depositor"))?
                        .to_checksum(None),
                    attribution_source: source.clone(),
                }),
                _ => None,
            };
            Ok(ProofTransfer {
                transaction_hash: B256::try_from(transfer.transaction_hash.as_slice())
                    .map_err(|_| ApiError::internal("invalid transfer hash"))?
                    .to_string(),
                log_index: transfer.log_index.to_string(),
                sender: Address::try_from(transfer.sender_address.as_slice())
                    .map_err(|_| ApiError::internal("invalid transfer sender"))?
                    .to_checksum(None),
                recipient: Address::try_from(transfer.recipient_address.as_slice())
                    .map_err(|_| ApiError::internal("invalid transfer recipient"))?
                    .to_checksum(None),
                amount_base_units: transfer.amount.clone(),
                block_number: transfer.block_number.to_string(),
                relay,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    // The proof's claim is scope-dependent. A wallet-attributed proof says
    // the attested wallet paid: from its own address, or from another chain
    // through Relay with the origin verified as that wallet. Money from any
    // other wallet was credited and settled, but it is not that claim, and
    // no such proof is issued for it. A settlement-scope proof makes no
    // claim about whose wallet paid, so no sender is checked.
    if scope == ProofScope::WalletAttributed {
        let payer_wallet = payer_wallet
            .as_deref()
            .ok_or_else(|| ApiError::internal("attested request has no attested wallet"))?;
        if transfers.iter().any(|transfer| {
            transfer.sender != payer_wallet
                && transfer
                    .relay
                    .as_ref()
                    .is_none_or(|relay| relay.origin_sender != payer_wallet)
        }) {
            return Err(ApiError::deposit_sender_mismatch());
        }
    }
    let relay_fills = match payer_wallet.as_deref() {
        Some(payer_wallet) => transfers
            .iter()
            .filter(|transfer| transfer.sender != payer_wallet)
            .filter_map(|transfer| {
                transfer.relay.clone().map(|relay| AttestedRelayFill {
                    transaction_hash: transfer.transaction_hash.clone(),
                    log_index: transfer.log_index.clone(),
                    relay,
                })
            })
            .collect(),
        None => Vec::new(),
    };

    // Identity facts: the attached add-ons only. A permissionless request
    // has none, and the attestation signs `not_required`.
    let verification_addons = &invoice.issuance_snapshot.payer_verification;
    let result = if !verification_addons.is_gated() {
        "not_required"
    } else if row.verification_completed_at.is_some() {
        "approved"
    } else {
        "pending"
    };
    let rfc3339 = |at: chrono::DateTime<chrono::Utc>| at.to_rfc3339_opts(SecondsFormat::Secs, true);
    let attempts = state
        .payer_sessions
        .attempts_for_invoice(account.0, row.id)
        .await?;
    let mut facts = Vec::new();
    if verification_addons.email.is_some()
        && let Some(mailbox) = attempts
            .iter()
            .filter(|attempt| attempt.kind == "email" && attempt.status == "approved")
            .filter_map(|attempt| attempt.verified_at)
            .min()
    {
        facts.push(VerificationFact {
            kind: "mailbox".into(),
            provider: "auth0".into(),
            at: rfc3339(mailbox),
        });
    }
    // A merchant-auth request's identity fact is the merchant's own
    // sign-in, vouched for by the client secret its server released.
    if verification_addons.merchant_auth.is_some()
        && let Some(opened) = attempts
            .iter()
            .filter(|attempt| attempt.kind == "merchant_session" && attempt.status == "approved")
            .filter_map(|attempt| attempt.verified_at)
            .min()
    {
        facts.push(VerificationFact {
            kind: "merchant_session".into(),
            provider: "merchant".into(),
            at: rfc3339(opened),
        });
    }
    let wallet_evidence = binding.wallet.as_ref();
    if let Some(wallet) = wallet_evidence {
        facts.push(VerificationFact {
            kind: "wallet".into(),
            provider: "gum".into(),
            at: wallet.bound_at.clone(),
        });
    }
    let verification = attestor
        .attest(VerificationAttestationPayload {
            version: ATTESTATION_VERSION.into(),
            payment_id: invoice.id.to_string(),
            // The commitment goes into the signed bytes so the attestation
            // vouches for this invoice and this payer only, not for any
            // proof that reuses its id.
            scope,
            attribution_hash: invoice.attribution_hash.to_string(),
            issuance_nonce: hex_nonce(&invoice.issuance_nonce),
            chain_id: binding.network.chain_id.to_string(),
            payment_address: binding.payment_address.0.to_checksum(None),
            payer_wallet: payer_wallet.clone(),
            wallet_nonce: wallet_evidence
                .map(|wallet| wallet.attestation.typed_data.message.nonce.clone()),
            result: result.into(),
            verified_at: row.verification_completed_at.map(rfc3339),
            wallet_bound_at: wallet_evidence.map(|wallet| wallet.bound_at.clone()),
            facts,
            relay_fills,
        })
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "verification attestation failed");
            ApiError::internal("failed to sign the verification attestation")
        })?;

    let proof = ProofOfPayment {
        version: PROOF_VERSION.into(),
        payment_id: invoice.id.to_string(),
        scope,
        canonical_issuance_snapshot: invoice.issuance_snapshot.clone(),
        canonicalization: CANONICALIZATION.into(),
        attribution_hash: invoice.attribution_hash.to_string(),
        issuance_nonce: hex_nonce(&invoice.issuance_nonce),
        payer_wallet: wallet_evidence.map(|wallet| wallet.attestation.clone()),
        salt: binding.salt.0.to_string(),
        chain_id: binding.network.chain_id.to_string(),
        factory_address: binding.network.factory.0.to_checksum(None),
        payment_address: binding.payment_address.0.to_checksum(None),
        token_address: binding.network.token.0.to_checksum(None),
        recovery_address: binding.recovery.0.to_checksum(None),
        settlement_transaction_hash,
        transfers,
        verification,
    };
    state.cache_proof(cache_key.0, cache_key.1, cache_key.2, proof.clone());
    Ok(Json(proof))
}
