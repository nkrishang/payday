//! Proof of Payment (product plan §5.5, guide §Slice 2.6): everything a
//! holder needs to recompute hash → salt → CREATE3 address offline and tie
//! the invoice to the transfers that paid it and the transaction that
//! settled it, plus a fresh Payday-signed attestation of the verification
//! outcome bound to that same issuance commitment.

use alloy_primitives::{Address, B256};
use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use chrono::SecondsFormat;
use gateway_core::{
    ATTESTATION_VERSION, CANONICALIZATION, Invoice, InvoiceStatus, PROOF_VERSION, ProofOfPayment,
    ProofTransfer, VerificationAttestationPayload,
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
    let settlement = state
        .proofs
        .settlement_transfers(row.id)
        .await?
        .ok_or_else(ApiError::payment_not_found)?;
    // Only a fulfilled invoice, whose funds have reached the beneficiary,
    // has anything to prove. The row's settlement_tx_hash is the fulfilment
    // call that forwarded them (our batch sweep or a third party's execute);
    // it is what the proof names as the settlement transaction.
    let settlement_transaction_hash = match (
        settlement.status.parse::<InvoiceStatus>(),
        &settlement.settlement_tx_hash,
    ) {
        (Ok(InvoiceStatus::Fulfilled), Some(hash)) => B256::try_from(hash.as_slice())
            .map_err(|_| ApiError::internal("invalid settlement transaction hash"))?
            .to_string(),
        _ => return Err(ApiError::payment_not_settled()),
    };
    let transfers = settlement
        .transfers
        .iter()
        .map(|transfer| {
            Ok(ProofTransfer {
                transaction_hash: B256::try_from(transfer.transaction_hash.as_slice())
                    .map_err(|_| ApiError::internal("invalid transfer hash"))?
                    .to_string(),
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

    // Permissionless invoices verify nothing. Gated modes report whether the
    // policy was satisfied before settlement; the payer flows of Slices 3
    // and 4 are what set that timestamp.
    let mode = invoice.issuance_snapshot.payer_policy.mode();
    let result = if !mode.is_gated() {
        "not_required"
    } else if row.verification_completed_at.is_some() {
        "approved"
    } else {
        "pending"
    };
    let verification = state
        .attestor()?
        .attest(VerificationAttestationPayload {
            version: ATTESTATION_VERSION.into(),
            payment_id: invoice.id.to_string(),
            // The issuance commitment goes into the signed bytes so the
            // attestation vouches for this invoice only, not for any proof
            // that reuses its id.
            attribution_hash: invoice.attribution_hash.to_string(),
            chain_id: invoice.chain_id.0.to_string(),
            payment_address: invoice.payment_address.0.to_checksum(None),
            payer_policy_mode: mode.as_str().into(),
            result: result.into(),
            verified_at: row
                .verification_completed_at
                .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true)),
        })
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "verification attestation failed");
            ApiError::internal("failed to sign the verification attestation")
        })?;

    Ok(Json(ProofOfPayment {
        version: PROOF_VERSION.into(),
        payment_id: invoice.id.to_string(),
        canonical_issuance_snapshot: invoice.issuance_snapshot,
        canonicalization: CANONICALIZATION.into(),
        attribution_nonce: invoice.attribution_nonce.to_string(),
        attribution_hash: invoice.attribution_hash.to_string(),
        salt: invoice.salt.0.to_string(),
        chain_id: invoice.chain_id.0.to_string(),
        factory_address: invoice.factory.0.to_checksum(None),
        payment_address: invoice.payment_address.0.to_checksum(None),
        token_address: invoice.token.0.to_checksum(None),
        settlement_transaction_hash,
        transfers,
        verification,
    }))
}
