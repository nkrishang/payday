//! Paying a deposit request from another chain, through Relay.
//!
//! The hosted checkout offers it once the address exists: the payer picks
//! the chain their USDC is on, this service asks Relay for a quote whose
//! recipient is the payment address and whose output is exactly the amount
//! still due, and the page sends the quote's transactions from the attested
//! wallet. The quote is made here, not in the browser, for two reasons:
//! Relay requires an API key on every quote, and the recipient, the
//! destination token, the amount, and the depositing wallet are pinned by
//! the service, so nothing the page does can redirect the funds.
//!
//! Each quote is a `relay_intents` row. Reporting the origin transaction
//! marks it `sent`; the indexer follows it from there (see
//! `gateway-indexer/src/relay_intents.rs`), and the payer view's `relay`
//! block shows where it stands.
//!
//! Routes (all under `/v1/payer/deposit-requests/{id}/relay`):
//!
//! - `GET  /chains`            the chains USDC may be paid from
//! - `POST /quotes`            `{origin_chain_id}` → a quote and its steps
//! - `POST /quotes/{rli}/sent` `{transaction_hash}` → the payer view

use std::str::FromStr;

use alloy_primitives::{B256, U256, hex};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use chrono::{Duration, Utc};
use gateway_core::{
    Invoice, PaymentBinding, RelayIntentId, RelayOriginChainDto, RelayOriginChainsResponse,
    RelayQuoteResponse, RelayQuoteStepDto, RelayTransactionDto, USDC_DECIMALS, rfc3339,
};
use gateway_db::{MarkSent, NewRelayIntent};
use gateway_relay::{QuoteRequest, RelayChain, RelayError};
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::json::Json;
use crate::api::payer::{
    PayerInvoiceAccess, authorized_invoice, payer_response, payment_state, unix_now,
};
use crate::api::payer_verification::session_token;
use crate::state::AppState;

/// How long a quote is worth sending. Relay's own quotes are good for a
/// minute or so; the page re-quotes when it is about to send an old one.
pub const QUOTE_VALIDITY: Duration = Duration::minutes(15);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuoteBody {
    /// Decimal chain id of the chain the payer's USDC is on.
    pub origin_chain_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SentBody {
    /// `0x` hex, 32 bytes: the deposit transaction the wallet sent.
    pub transaction_hash: String,
}

/// The request as the relay routes need it: unlocked, bound, and payable.
struct Payable {
    access: PayerInvoiceAccess,
    remaining: U256,
}

impl Payable {
    fn invoice(&self) -> &Invoice {
        &self.access.invoice
    }

    fn binding(&self) -> &PaymentBinding {
        self.access
            .invoice
            .binding
            .as_ref()
            .expect("checked when the access was resolved")
    }
}

async fn payable(state: &AppState, id: &str, headers: &HeaderMap) -> Result<Payable, ApiError> {
    let access = authorized_invoice(state, id, session_token(headers)).await?;
    if !access.content_unlocked {
        return Err(ApiError::verification_required());
    }
    if access.invoice.binding.is_none() {
        return Err(ApiError::wallet_required());
    }
    let (remaining, payable) = payment_state(&access.invoice, unix_now());
    if !payable || remaining.is_zero() {
        return Err(ApiError::deposit_request_not_payable());
    }
    Ok(Payable { access, remaining })
}

fn origin_dto(chain: &RelayChain) -> Option<RelayOriginChainDto> {
    let usdc = chain.usdc()?;
    Some(RelayOriginChainDto {
        chain_id: chain.id.to_string(),
        name: chain.display_name.clone(),
        native_symbol: chain.native_symbol.clone(),
        usdc_address: usdc.address.to_checksum(None),
        explorer_url: chain.explorer_url.clone(),
        icon_url: chain.icon_url.clone(),
        rpc_url: chain.http_rpc_url.clone(),
    })
}

/// The chains a payer may pay this request from: every chain Relay takes
/// USDC deposits on, except the request's own.
pub async fn chains(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<RelayOriginChainsResponse>, ApiError> {
    let relay = state.relay()?;
    let request = payable(&state, &id, &headers).await?;
    let destination = request.binding().network.chain_id.0;
    let chains = relay.chains().await.map_err(relay_error)?;
    let chains = chains
        .iter()
        .filter(|chain| chain.id != destination)
        .filter_map(origin_dto)
        .collect();
    Ok(Json(RelayOriginChainsResponse { chains }))
}

/// A quote for the amount still due, from the chosen chain's USDC, delivered
/// to the payment address by Relay; recorded as a `quoted` intent.
pub async fn quote(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<QuoteBody>,
) -> Result<Json<RelayQuoteResponse>, ApiError> {
    let relay = state.relay()?;
    let request = payable(&state, &id, &headers).await?;
    let origin_chain_id: u64 = body.origin_chain_id.trim().parse().map_err(|_| {
        ApiError::invalid_request("origin_chain_id must be a decimal chain id string")
    })?;
    let binding = request.binding();
    let destination = binding.network;
    if origin_chain_id == destination.chain_id.0 {
        return Err(ApiError::relay_unsupported_origin());
    }
    let chains = relay.chains().await.map_err(relay_error)?;
    let origin = chains
        .iter()
        .find(|chain| chain.id == origin_chain_id)
        .and_then(|chain| origin_dto(chain).map(|dto| (chain, dto)))
        .ok_or_else(ApiError::relay_unsupported_origin)?;
    let (origin_chain, origin) = origin;
    let origin_usdc = origin_chain.usdc().expect("origin_dto checked it").address;

    let quote = relay
        .api()
        .quote(&QuoteRequest {
            user: binding.payer_wallet,
            recipient: binding.payment_address.0,
            origin_chain_id,
            destination_chain_id: destination.chain_id.0,
            origin_currency: origin_usdc,
            destination_currency: destination.token.0,
            amount: request.remaining,
        })
        .await
        .map_err(relay_error)?;
    // The quote must be exactly what was asked: the amount due lands, and
    // every step is a transaction the attested wallet sends on the origin
    // chain. Anything else is a route we do not offer.
    if quote.amount_out != request.remaining {
        return Err(ApiError::relay_quote_failed(
            "Relay did not quote the exact amount due",
        ));
    }
    let mut steps = Vec::with_capacity(quote.steps.len());
    for step in &quote.steps {
        if step.kind != "transaction" {
            return Err(ApiError::relay_quote_failed(format!(
                "Relay's route needs a {} step this checkout cannot perform",
                step.kind
            )));
        }
        for transaction in &step.transactions {
            if transaction.chain_id != origin_chain_id || transaction.from != binding.payer_wallet {
                return Err(ApiError::relay_quote_failed(
                    "Relay's route is not from the attested wallet on the chosen chain",
                ));
            }
            steps.push(RelayQuoteStepDto {
                id: step.id.clone(),
                transaction: RelayTransactionDto {
                    chain_id: transaction.chain_id.to_string(),
                    to: transaction.to.to_checksum(None),
                    data: hex::encode_prefixed(&transaction.data),
                    value: transaction.value.to_string(),
                    gas: transaction.gas.map(|gas| gas.to_string()),
                },
            });
        }
    }
    if steps.is_empty() {
        return Err(ApiError::relay_quote_failed(
            "Relay's route has no transaction",
        ));
    }

    let expires_at = Utc::now() + QUOTE_VALIDITY;
    let intent = state
        .relay_intents
        .insert(&NewRelayIntent {
            id: RelayIntentId::generate().0,
            invoice_id: request.invoice().id.0,
            request_id: quote.request_id,
            origin_chain_id,
            destination_chain_id: destination.chain_id.0,
            origin_currency: origin_usdc,
            payer_wallet: binding.payer_wallet,
            quoted_in_amount: quote.amount_in,
            quoted_out_amount: quote.amount_out,
            expires_at,
        })
        .await?;
    let human = |units: U256| {
        alloy_primitives::utils::format_units(units, USDC_DECIMALS).unwrap_or_default()
    };
    Ok(Json(RelayQuoteResponse {
        id: RelayIntentId(intent.id).to_string(),
        request_id: quote.request_id.to_string(),
        origin,
        amount_in: human(quote.amount_in),
        amount_in_base_units: quote.amount_in.to_string(),
        amount_out: human(quote.amount_out),
        amount_out_base_units: quote.amount_out.to_string(),
        relayer_fee_usd: quote.relayer_fee_usd,
        time_estimate_seconds: quote.time_estimate_secs,
        expires_at: rfc3339(expires_at),
        steps,
    }))
}

/// The wallet sent the deposit: the intent is `sent` and the indexer follows
/// it. Answers with the payer view, whose `relay` block now shows it.
pub async fn sent(
    State(state): State<AppState>,
    Path((id, intent)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<SentBody>,
) -> Result<Json<gateway_core::PayerDepositRequestResponse>, ApiError> {
    state.relay()?;
    let request = payable(&state, &id, &headers).await?;
    let intent_id = RelayIntentId::parse(&intent).ok_or_else(ApiError::relay_intent_not_found)?;
    let transaction_hash = B256::from_str(body.transaction_hash.trim())
        .map_err(|_| ApiError::invalid_request("transaction_hash must be a 32-byte hex hash"))?;
    match state
        .relay_intents
        .mark_sent(intent_id.0, request.invoice().id.0, transaction_hash)
        .await?
    {
        MarkSent::Sent => {}
        MarkSent::NotQuoted => return Err(ApiError::relay_intent_not_quoted()),
        MarkSent::TransactionClaimed => return Err(ApiError::relay_transaction_claimed()),
        MarkSent::NotFound => return Err(ApiError::relay_intent_not_found()),
    }
    let access = authorized_invoice(&state, &id, session_token(&headers)).await?;
    Ok(Json(payer_response(&state, access)))
}

fn relay_error(error: RelayError) -> ApiError {
    match error {
        RelayError::Status { message, .. } => {
            tracing::warn!(%message, "relay refused the request");
            ApiError::relay_quote_failed(format!("Relay could not quote this payment: {message}"))
        }
        RelayError::Transport(message) | RelayError::Malformed(message) => {
            tracing::error!(%message, "relay unreachable or malformed");
            ApiError::relay_quote_failed("Relay is not answering; try again shortly")
        }
    }
}
