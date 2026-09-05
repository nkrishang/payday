//! Proof of Payment (product plan §5.5, guide §Slice 2.6): everything a
//! holder needs to recompute hash → attestation → salt → CREATE3 address
//! offline and tie the request to the transfers from the attested wallet
//! that paid it and the transaction that settled it, plus a fresh
//! Payday-signed attestation of the verification facts bound to that same
//! commitment.

use alloy_primitives::{Address, B256};
use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use chrono::SecondsFormat;
use gateway_core::{
    ATTESTATION_VERSION, CANONICALIZATION, Invoice, InvoiceStatus, PROOF_VERSION, ProofOfPayment,
    ProofTransfer, VerificationAttestationPayload, VerificationFact,
};
use gateway_db::AccountId;

use crate::api::error::ApiError;
use crate::api::invoices::resolve_payment;
use crate::state::AppState;

pub async fn get_proof(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(reference): Path<String>,
) -> Result<Json<ProofOfPayment>, ApiError> {
    let row = resolve_payment(&state, account, &reference).await?;
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
            _ => return Err(ApiError::payment_not_settled()),
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
    let transfers = settlement_transfers
        .iter()
        .map(|transfer| {
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
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    // The proof's claim is that the attested wallet paid. Money from any
    // other wallet was credited and settled, but it is not that claim, and
    // no proof is issued for it.
    if transfers
        .iter()
        .any(|transfer| transfer.sender != binding.payer_wallet.to_checksum(None))
    {
        return Err(ApiError::payment_sender_mismatch());
    }

    // Permissionless invoices verify no identity. Gated modes report whether
    // the policy was satisfied; the wallet binding is a fact about every
    // invoice, recorded with the attempt that made it.
    let mode = invoice.issuance_snapshot.payer_policy.mode();
    let result = if !mode.is_gated() {
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
    if let Some(mailbox) = attempts
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
    // A merchant-session request's identity fact is the merchant's own
    // sign-in, vouched for by the client secret its server released.
    if let Some(opened) = attempts
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
    facts.push(VerificationFact {
        kind: "wallet".into(),
        provider: "payday".into(),
        at: binding.bound_at.clone(),
    });
    let verification = attestor
        .attest(VerificationAttestationPayload {
            version: ATTESTATION_VERSION.into(),
            payment_id: invoice.id.to_string(),
            // The commitment goes into the signed bytes so the attestation
            // vouches for this invoice and this payer only, not for any
            // proof that reuses its id.
            attribution_hash: invoice.attribution_hash.to_string(),
            chain_id: invoice.chain_id.0.to_string(),
            payment_address: binding.payment_address.0.to_checksum(None),
            payer_wallet: binding.payer_wallet.to_checksum(None),
            wallet_nonce: binding.attestation.typed_data.message.nonce.clone(),
            payer_policy_mode: mode.as_str().into(),
            result: result.into(),
            verified_at: row.verification_completed_at.map(rfc3339),
            wallet_bound_at: binding.bound_at.clone(),
            facts,
        })
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "verification attestation failed");
            ApiError::internal("failed to sign the verification attestation")
        })?;

    let proof = ProofOfPayment {
        version: PROOF_VERSION.into(),
        payment_id: invoice.id.to_string(),
        canonical_issuance_snapshot: invoice.issuance_snapshot.clone(),
        canonicalization: CANONICALIZATION.into(),
        attribution_hash: invoice.attribution_hash.to_string(),
        payer_wallet: binding.attestation.clone(),
        salt: binding.salt.0.to_string(),
        chain_id: invoice.chain_id.0.to_string(),
        factory_address: invoice.factory.0.to_checksum(None),
        payment_address: binding.payment_address.0.to_checksum(None),
        token_address: invoice.token.0.to_checksum(None),
        recovery_address: binding.recovery.0.to_checksum(None),
        settlement_transaction_hash,
        transfers,
        verification,
    };
    state.cache_proof(cache_key.0, cache_key.1, cache_key.2, proof.clone());
    Ok(Json(proof))
}
