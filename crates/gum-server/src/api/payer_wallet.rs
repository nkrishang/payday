//! The payer's wallet attestation (product plan §4.6, §5.4).
//!
//! ```text
//! POST /v1/payer/deposit-requests/{id}/network           {"chain_id": "8453"}
//! POST /v1/payer/deposit-requests/{id}/wallet/challenge  {"wallet": "0x…", "chain_id": "8453"}
//! POST /v1/payer/deposit-requests/{id}/wallet/attest     {"wallet": "0x…", "signature": "0x…"}
//! ```
//!
//! Once a session satisfies the request's policy (immediately, for a
//! permissionless request), it may ask for a challenge: a one-time nonce
//! wrapped in the EIP-712 document the payer's wallet must sign, under the
//! domain of the network the payer chose to pay on. The signature binds
//! that wallet and that network to the request, and only then does the
//! request have a payment address. The address commits to the attestation,
//! so neither a second wallet nor another network can be bound afterwards.
//! The chain is recorded with the challenge, so the attestation is rebuilt
//! under the same domain whatever the attest call claims.

use std::str::FromStr;
use std::time::Duration;

use alloy_primitives::{Address, Signature, hex};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use gum_core::{
    BindError, ChainDto, ChainId, Invoice, InvoiceStatus, NetworkDto, PayerAttestation,
    PayerAttestationError, PayerAttestationTypedData, PayerDepositRequestResponse,
    payer_wallet_attestation, rfc3339,
};
use gum_ledger::{BindPayerWallet, DbInvoice, DbPayerSession, PAYER_SESSION_TTL};
use serde::{Deserialize, Serialize};

use crate::api::attachments::no_store;
use crate::api::error::ApiError;
use crate::api::json::Json;
use crate::api::payer::{authorized_invoice, parse_invoice_id, payer_response};
use crate::api::payer_verification::session_token;
use crate::state::AppState;

/// How long a challenge may wait for its signature. Short, because the
/// nonce is what proves the signature followed the identity step closely.
pub const WALLET_CHALLENGE_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChallengeRequest {
    pub wallet: String,
    /// The network the payer will pay on: one of the request's `networks`.
    pub chain_id: String,
}

#[derive(Serialize)]
pub struct ChallengeResponse {
    /// The session the challenge belongs to: the caller's, or a fresh one
    /// for a permissionless request that had none.
    pub payer_session: String,
    pub expires_at: String,
    /// The network the challenge is for; the typed data's domain names it.
    pub chain: ChainDto,
    /// Exactly what to pass to `eth_signTypedData_v4`.
    pub typed_data: PayerAttestationTypedData,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttestRequest {
    pub wallet: String,
    /// `0x` hex, 65 bytes.
    pub signature: String,
}

/// The chain the payer named, if the request may be paid there. A chain
/// this deployment knows nothing about and one the request does not offer
/// are the same refusal: the request's own list is what the checkout shows.
fn parse_chain(invoice: &Invoice, value: &str) -> Result<ChainId, ApiError> {
    let chain_id: u64 = value
        .trim()
        .parse()
        .map_err(|_| ApiError::invalid_request("chain_id must be a decimal chain id string"))?;
    invoice
        .network_for(ChainId(chain_id))
        .map(|network| network.chain_id)
        .ok_or_else(ApiError::unsupported_chain)
}

fn parse_wallet(value: &str) -> Result<Address, ApiError> {
    let wallet = Address::from_str(value.trim())
        .map_err(|_| ApiError::invalid_request("wallet must be a 20-byte EVM address"))?;
    if wallet.is_zero() {
        return Err(ApiError::invalid_request(
            "wallet must not be the zero address",
        ));
    }
    Ok(wallet)
}

/// The request behind `id`, still open to a wallet: `created` and before
/// its deadline. Anything else cannot take a binding.
async fn open_invoice(state: &AppState, id: &str) -> Result<(DbInvoice, Invoice), ApiError> {
    let uuid = parse_invoice_id(id)?;
    let row = state
        .repo
        .find_by_id(uuid)
        .await?
        .ok_or_else(ApiError::payer_unauthorized)?;
    let invoice = Invoice::try_from(&row)?;
    if invoice.status != InvoiceStatus::Created
        || Utc::now().timestamp() > invoice.expiration_timestamp as i64
    {
        return Err(ApiError::deposit_request_not_payable());
    }
    Ok((row, invoice))
}

/// The session may take the wallet step once the request's identity add-ons
/// are satisfied. A request with neither identity add-on has no identity
/// step, so any session (or none) will do.
fn policy_satisfied(invoice: &Invoice, session: &DbPayerSession) -> bool {
    let verification = &invoice.issuance_snapshot.payer_verification;
    !verification.is_gated() || session.satisfies(verification)
}

/// The network choice for a request that may still pick one: an address
/// exists only once a network is known, so this is the step that mints it
/// for a request issued across several networks without wallet attestation.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectNetworkRequest {
    /// The network the payer will pay on: one of the request's `networks`.
    pub chain_id: String,
}

/// POST /v1/payer/deposit-requests/{id}/network {"chain_id": "8453"}.
///
/// Binds the request to the chosen network and mints its payment address.
/// Idempotent for the network already chosen; any other network is a
/// conflict, because the address commits to the first one. A request with
/// the wallet-attestation add-on never comes here: its address waits for
/// the signature instead.
pub async fn select_network(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<SelectNetworkRequest>,
) -> Result<Response, ApiError> {
    let (row, invoice) = open_invoice(&state, &id).await?;
    if invoice.wallet_attestation_required() {
        return Err(ApiError::wallet_attestation_required());
    }
    let chain = parse_chain(&invoice, &request.chain_id)?;
    // A gated request answers only a session that satisfies its identity
    // add-ons, exactly like the wallet step; an open one needs no session.
    let verification = &invoice.issuance_snapshot.payer_verification;
    if verification.is_gated() {
        let token = session_token(&headers).ok_or_else(ApiError::payer_session_invalid)?;
        let session = state
            .payer_sessions
            .find_active(token, row.id)
            .await?
            .ok_or_else(ApiError::payer_session_invalid)?;
        if !session.satisfies(verification) {
            return Err(ApiError::verification_required());
        }
    }
    if let Some(binding) = &invoice.binding {
        if binding.network.chain_id == chain {
            let access = authorized_invoice(&state, &id, session_token(&headers)).await?;
            return Ok((no_store(), Json(payer_response(&state, access))).into_response());
        }
        return Err(ApiError::network_already_chosen());
    }
    let now = Utc::now();
    let binding = invoice.bind_network(chain, rfc3339(now)).map_err(|error| {
        match error {
            // Unreachable in practice: parse_chain already checked
            // membership, but the salt derivation is the last word.
            BindError::UnsupportedChain(_) => ApiError::network_conflict(),
            error => ApiError::internal(error.to_string()),
        }
    })?;
    match state.repo.bind_network(row.id, &binding, now).await? {
        BindPayerWallet::Bound(_) => {}
        // Another caller chose first while this call was in flight; the
        // same answer as if the row had carried it all along.
        BindPayerWallet::AlreadyBound(_) => return Err(ApiError::network_already_chosen()),
        BindPayerWallet::NotBindable(_) => return Err(ApiError::deposit_request_not_payable()),
    }
    let access = authorized_invoice(&state, &id, session_token(&headers)).await?;
    Ok((no_store(), Json(payer_response(&state, access))).into_response())
}

pub async fn challenge(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ChallengeRequest>,
) -> Result<Response, ApiError> {
    let wallet = parse_wallet(&request.wallet)?;
    let (row, invoice) = open_invoice(&state, &id).await?;
    // The attestation step exists only for a request that attached the
    // wallet-attestation add-on: without it there is no wallet to bind, and
    // the address exists (or waits on a network choice) without a signature.
    if !invoice.wallet_attestation_required() {
        return Err(ApiError::wallet_attestation_not_required());
    }
    if let Some(binding) = &invoice.binding {
        return Err(ApiError::wallet_already_bound(
            &binding
                .wallet
                .as_ref()
                .map(|wallet| wallet.payer_wallet.to_checksum(None))
                .unwrap_or_default(),
        ));
    }
    let chain = parse_chain(&invoice, &request.chain_id)?;
    let network = *invoice.network_for(chain).expect("parse_chain checked");
    let gated = invoice.issuance_snapshot.payer_verification.is_gated();
    let (session, token) = match session_token(&headers) {
        Some(token) => {
            let session = state
                .payer_sessions
                .find_active(token, row.id)
                .await?
                .ok_or_else(ApiError::payer_session_invalid)?;
            (session, token.to_owned())
        }
        // A gated request's session comes from its verification flow; a
        // permissionless one starts here.
        None if gated => return Err(ApiError::payer_session_invalid()),
        None => {
            let created = state
                .payer_sessions
                .create(row.id, PAYER_SESSION_TTL)
                .await?;
            let session = state
                .payer_sessions
                .find_active(&created.token, row.id)
                .await?
                .ok_or_else(|| ApiError::internal("fresh payer session not found"))?;
            (session, created.token)
        }
    };
    if !policy_satisfied(&invoice, &session) {
        return Err(ApiError::verification_required());
    }
    let challenge = state
        .payer_sessions
        .issue_wallet_challenge(session.id, chain.0, WALLET_CHALLENGE_TTL)
        .await?;
    let message = PayerAttestation::new(
        invoice.attribution_hash,
        wallet,
        challenge.nonce,
        challenge.expires_at.timestamp().max(0) as u64,
    );
    Ok((
        no_store(),
        Json(ChallengeResponse {
            payer_session: token,
            expires_at: rfc3339(challenge.expires_at),
            chain: NetworkDto::from_terms(&network, invoice.currency()).chain,
            typed_data: message.typed_data(network.chain_id.0, network.factory.0),
        }),
    )
        .into_response())
}

pub async fn attest(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<AttestRequest>,
) -> Result<Response, ApiError> {
    let wallet = parse_wallet(&request.wallet)?;
    let signature = hex::decode(request.signature.trim())
        .ok()
        .filter(|bytes| bytes.len() == 65)
        .and_then(|bytes| Signature::from_raw(&bytes).ok())
        .ok_or_else(ApiError::wallet_signature_invalid)?;
    let (row, invoice) = open_invoice(&state, &id).await?;
    let token = session_token(&headers).ok_or_else(ApiError::payer_session_invalid)?;
    let session = state
        .payer_sessions
        .find_active(token, row.id)
        .await?
        .ok_or_else(ApiError::payer_session_invalid)?;
    if !policy_satisfied(&invoice, &session) {
        return Err(ApiError::verification_required());
    }
    let now = Utc::now();
    let challenge = session
        .wallet_challenge(now)
        .ok_or_else(ApiError::wallet_challenge_required)?;
    // Rebuild the document the wallet was asked to sign from what this
    // service issued, never from the request body: only the wallet and the
    // signature come from the payer. The chain is the challenge's: a
    // request whose networks no longer include it cannot be bound there.
    let chain = ChainId(challenge.chain_id);
    let network = *invoice
        .network_for(chain)
        .ok_or_else(ApiError::unsupported_chain)?;
    let message = PayerAttestation::new(
        invoice.attribution_hash,
        wallet,
        challenge.nonce,
        challenge.expires_at.timestamp().max(0) as u64,
    );
    let attestation =
        payer_wallet_attestation(&message, network.chain_id.0, network.factory.0, &signature);
    let binding = invoice
        .bind_payer_wallet(chain, attestation, rfc3339(now))
        .map_err(|error| match error {
            BindError::Attestation(
                PayerAttestationError::SignatureInvalid | PayerAttestationError::SignerMismatch,
            ) => ApiError::wallet_signature_invalid(),
            other => {
                tracing::error!(payment_id = %row.id, error = %other, "attestation built by this service failed to verify");
                ApiError::internal("failed to verify the wallet attestation")
            }
        })?;
    match state
        .repo
        .bind_payer_wallet(row.id, session.id, &binding, now)
        .await?
    {
        BindPayerWallet::Bound(_) => {
            tracing::info!(payment_id = %row.id, wallet = %wallet, chain_id = chain.0, "payer wallet bound; payment address derived");
        }
        // The same wallet signing again (another tab, a retry) finds its own
        // binding; a different one learns which wallet holds the request.
        BindPayerWallet::AlreadyBound(existing) => {
            if existing.payer_wallet.as_deref() != Some(wallet.as_slice()) {
                let bound = existing
                    .payer_wallet
                    .as_deref()
                    .and_then(|bytes| Address::try_from(bytes).ok())
                    .map(|address| address.to_checksum(None))
                    .unwrap_or_default();
                return Err(ApiError::wallet_already_bound(&bound));
            }
        }
        BindPayerWallet::NotBindable(_) => return Err(ApiError::deposit_request_not_payable()),
    }
    let access = authorized_invoice(&state, &id, Some(token)).await?;
    let response: PayerDepositRequestResponse = payer_response(&state, access);
    Ok((no_store(), Json(response)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallets_must_be_real_addresses() {
        assert!(parse_wallet("0x0000000000000000000000000000000000000000").is_err());
        assert!(parse_wallet("not-an-address").is_err());
        assert_eq!(
            parse_wallet(" 0x70997970c51812dc3a010c7d01b50e0d17dc79c8 ")
                .unwrap()
                .to_checksum(None),
            "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
        );
    }
}
