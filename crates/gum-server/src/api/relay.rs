//! Paying a deposit request from another chain, through Relay.
//!
//! The hosted checkout offers it once the address exists: the payer picks
//! the chain and stablecoin they hold, this service asks Relay for a quote whose
//! recipient is the payment address and whose output is exactly the amount
//! still due, and the page sends the quote's transactions from the attested
//! wallet. The quote is made here, not in the browser, for two reasons:
//! Relay requires an API key on every quote, and the recipient, the
//! destination token, the amount, and the depositing wallet are pinned by
//! the service, so nothing the page does can redirect the funds.
//!
//! Each quote is a `relay_intents` row. Reporting the origin transaction
//! marks it `sent`; the indexer follows it from there (see
//! `gum-indexer/src/relay_intents.rs`), and the payer view's `relay`
//! block shows where it stands.
//!
//! Routes (all under `/v1/payer/deposit-requests/{id}/relay`):
//!
//! - `GET  /chains`            the chains, and the stablecoins on them, a payer may pay from
//! - `POST /quotes`            `{origin_chain_id, origin_token}` → a quote and its steps
//! - `POST /quotes/{rli}/sent` `{transaction_hash}` → the payer view

use std::str::FromStr;

use alloy_primitives::{Address, B256, U256, hex};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use chrono::{Duration, Utc};
use gum_core::{
    ChainRegistry, Currency, Invoice, PaymentBinding, RelayIntentId, RelayOriginChainDto,
    RelayOriginChainsResponse, RelayOriginTokenDto, RelayQuoteResponse, RelayQuoteStepDto,
    RelayTransactionDto, rfc3339,
};
use gum_ledger::{MarkSent, NewRelayIntent};
use gum_relay::{QuoteRequest, RelayChain, RelayCurrency, RelayError};
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

/// The payer's origin wallet: a 20-byte EVM address, never the zero one.
fn payer_wallet_value(value: &str) -> Result<Address, ApiError> {
    let wallet = Address::from_str(value.trim())
        .map_err(|_| ApiError::invalid_request("payer_wallet must be a 20-byte EVM address"))?;
    if wallet.is_zero() {
        return Err(ApiError::invalid_request(
            "payer_wallet must not be the zero address",
        ));
    }
    Ok(wallet)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuoteBody {
    /// Decimal chain id of the chain the payer pays from.
    pub origin_chain_id: String,
    /// The contract the payer sends there: one of the chain's offered
    /// `tokens`. Absent, the chain's USDC.
    #[serde(default)]
    pub origin_token: Option<String>,
    /// The wallet that will send the origin transactions: the payer's own,
    /// connected one. Relay builds its steps for it; the intent records it
    /// so only that wallet's report of the send is accepted. Relay is
    /// offered only on requests without the wallet-attestation add-on, so
    /// this wallet is a fact about the payment, not a verified identity.
    pub payer_wallet: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SentBody {
    /// `0x` hex, 32 bytes: the deposit transaction the wallet sent.
    pub transaction_hash: String,
}

/// The request as the relay routes need it: unlocked and bound.
struct UnlockedBound {
    access: PayerInvoiceAccess,
}

impl UnlockedBound {
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

async fn unlocked_bound(
    state: &AppState,
    id: &str,
    headers: &HeaderMap,
) -> Result<UnlockedBound, ApiError> {
    let access = authorized_invoice(state, id, session_token(headers)).await?;
    if !access.content_unlocked {
        return Err(ApiError::verification_required());
    }
    if access.invoice.binding.is_none() {
        return Err(ApiError::wallet_required());
    }
    Ok(UnlockedBound { access })
}

/// The quoting routes additionally need the request payable: money is only
/// moved for a request that can still take it.
async fn payable(
    state: &AppState,
    id: &str,
    headers: &HeaderMap,
) -> Result<(UnlockedBound, U256), ApiError> {
    let request = unlocked_bound(state, id, headers).await?;
    let (remaining, is_payable) = payment_state(request.invoice(), unix_now());
    if !is_payable || remaining.is_zero() {
        return Err(ApiError::deposit_request_not_payable());
    }
    Ok((request, remaining))
}

/// The stablecoins a payer may send from a Relay chain: each currency this
/// build knows that Relay's solvers take there. Where this deployment serves
/// the currency on that chain too, the addresses must agree, since the
/// registry names the issuer's contract and Relay must not be quoting a
/// look-alike; a currency this deployment does not serve there (USDT on Base)
/// is offered on Relay's word, which the quote and the receipt both check.
fn origin_tokens(chain: &RelayChain, registry: &ChainRegistry) -> Vec<(Currency, RelayCurrency)> {
    Currency::ALL
        .into_iter()
        .filter_map(|currency| {
            let listed = chain.currency(currency.code())?;
            match registry.token(chain.id, currency) {
                Some(ours) if ours.address != listed.address => None,
                _ => Some((currency, listed.clone())),
            }
        })
        .collect()
}

fn origin_token_dto(currency: Currency, listed: &RelayCurrency) -> RelayOriginTokenDto {
    RelayOriginTokenDto {
        currency: currency.code().into(),
        symbol: listed.symbol.clone(),
        address: listed.address.to_checksum(None),
        decimals: listed.decimals,
    }
}

/// A Relay chain is offered as an origin only when this deployment serves it
/// too: attribution needs the origin chain's own receipt. A chain with no
/// stablecoin a payer could send is never offered — and the quote route
/// rejects it even if a caller bypasses the list.
fn origin_dto(chain: &RelayChain, registry: &ChainRegistry) -> Option<RelayOriginChainDto> {
    registry.get(chain.id)?;
    let tokens: Vec<RelayOriginTokenDto> = origin_tokens(chain, registry)
        .iter()
        .map(|(currency, listed)| origin_token_dto(*currency, listed))
        .collect();
    if tokens.is_empty() {
        return None;
    }
    Some(RelayOriginChainDto {
        chain_id: chain.id.to_string(),
        name: chain.display_name.clone(),
        native_symbol: chain.native_symbol.clone(),
        tokens,
        explorer_url: chain.explorer_url.clone(),
        icon_url: chain.icon_url.clone(),
        rpc_url: chain.http_rpc_url.clone(),
    })
}

/// The chains a payer may pay this request from: every chain Relay takes a
/// known stablecoin on that this deployment also serves, except the
/// request's own.
pub async fn chains(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<RelayOriginChainsResponse>, ApiError> {
    let relay = state.relay()?;
    let request = unlocked_bound(&state, &id, &headers).await?;
    let destination = request.binding().network.chain_id.0;
    let chains = relay.chains().await.map_err(relay_error)?;
    let chains = chains
        .iter()
        .filter(|chain| chain.id != destination)
        .filter_map(|chain| origin_dto(chain, &state.networks))
        .collect();
    Ok(Json(RelayOriginChainsResponse { chains }))
}

/// A quote for the amount still due, from the chosen stablecoin on the chosen
/// chain, delivered to the payment address in the request's currency by
/// Relay; recorded as a `quoted` intent.
pub async fn quote(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<QuoteBody>,
) -> Result<Json<RelayQuoteResponse>, ApiError> {
    let relay = state.relay()?;
    let (request, remaining) = payable(&state, &id, &headers).await?;
    // Relay is offered only where no wallet was attested: an attested
    // request's deposit must come from the attested wallet itself.
    if request.invoice().wallet_attestation_required() {
        return Err(ApiError::wallet_attestation_required());
    }
    let origin_chain_id: u64 = body.origin_chain_id.trim().parse().map_err(|_| {
        ApiError::invalid_request("origin_chain_id must be a decimal chain id string")
    })?;
    let payer_wallet = payer_wallet_value(&body.payer_wallet)?;
    let binding = request.binding();
    let destination = binding.network;
    if origin_chain_id == destination.chain_id.0 {
        return Err(ApiError::relay_unsupported_origin());
    }
    let chains = relay.chains().await.map_err(relay_error)?;
    let origin = chains
        .iter()
        .find(|chain| chain.id == origin_chain_id)
        .and_then(|chain| origin_dto(chain, &state.networks).map(|dto| (chain, dto)))
        .ok_or_else(ApiError::relay_unsupported_origin)?;
    let (origin_chain, origin) = origin;
    let offered = origin_tokens(origin_chain, &state.networks);
    let (origin_currency, origin_token) = match body.origin_token.as_deref().map(str::trim) {
        None | Some("") => offered
            .iter()
            .find(|(currency, _)| *currency == Currency::Usdc)
            .cloned()
            .ok_or_else(ApiError::relay_unsupported_origin)?,
        Some(text) => {
            let wanted = alloy_primitives::Address::from_str(text)
                .map_err(|_| ApiError::invalid_request("origin_token must be an address"))?;
            offered
                .iter()
                .find(|(_, listed)| listed.address == wanted)
                .cloned()
                .ok_or_else(ApiError::relay_unsupported_origin)?
        }
    };
    let origin_usdc = origin_token.address;

    let quote = relay
        .api()
        .quote(&QuoteRequest {
            user: payer_wallet,
            recipient: binding.payment_address.0,
            origin_chain_id,
            destination_chain_id: destination.chain_id.0,
            origin_currency: origin_usdc,
            destination_currency: destination.token.0,
            amount: remaining,
        })
        .await
        .map_err(relay_error)?;
    // The quote must be exactly what was asked: the amount due lands, and
    // every step is a transaction on the origin chain. Anything else is a
    // route we do not offer.
    if quote.amount_out != remaining {
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
            if transaction.chain_id != origin_chain_id {
                return Err(ApiError::relay_quote_failed(
                    "Relay's route leaves the chosen origin chain",
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
            payer_wallet,
            quoted_in_amount: quote.amount_in,
            quoted_out_amount: quote.amount_out,
            expires_at,
        })
        .await?;
    let human = |units: U256, decimals: u8| {
        alloy_primitives::utils::format_units(units, decimals).unwrap_or_default()
    };
    let destination_decimals = request.invoice().currency().decimals();
    Ok(Json(RelayQuoteResponse {
        id: RelayIntentId(intent.id).to_string(),
        request_id: quote.request_id.to_string(),
        origin,
        origin_token: origin_token_dto(origin_currency, &origin_token),
        amount_in: human(quote.amount_in, origin_token.decimals),
        amount_in_base_units: quote.amount_in.to_string(),
        amount_out: human(quote.amount_out, destination_decimals),
        amount_out_base_units: quote.amount_out.to_string(),
        relayer_fee_usd: quote.relayer_fee_usd,
        time_estimate_seconds: quote.time_estimate_secs,
        expires_at: rfc3339(expires_at),
        steps,
    }))
}

/// The wallet sent the deposit: the intent is `sent` and the indexer follows
/// it. Answers with the payer view, whose `relay` block now shows it.
///
/// Reporting is authorized like any payer view of this request — unlocked
/// and wallet-bound — but not gated on payability or on Relay being
/// configured: the report records what already happened, and it may arrive
/// after the invoice funded, after the quote expired, or after the key was
/// removed. It is idempotent in identity and hash, so a retry over a flaky
/// connection is safe.
pub async fn sent(
    State(state): State<AppState>,
    Path((id, intent)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<SentBody>,
) -> Result<Json<gum_core::PayerDepositRequestResponse>, ApiError> {
    let request = unlocked_bound(&state, &id, &headers).await?;
    let intent_id = RelayIntentId::parse(&intent).ok_or_else(ApiError::relay_intent_not_found)?;
    let transaction_hash = B256::from_str(body.transaction_hash.trim())
        .map_err(|_| ApiError::invalid_request("transaction_hash must be a 32-byte hex hash"))?;
    // Only the wallet the quote was made for may report its send; the
    // intent's own record says which one that was.
    let intent_row = state
        .relay_intents
        .find(intent_id.0)
        .await?
        .filter(|row| row.invoice_id == request.invoice().id.0)
        .ok_or_else(ApiError::relay_intent_not_found)?;
    let intent_wallet = intent_row
        .payer_wallet()
        .ok_or_else(|| ApiError::internal("relay intent has an invalid payer wallet"))?;
    match state
        .relay_intents
        .mark_sent(
            intent_id.0,
            request.invoice().id.0,
            intent_wallet,
            transaction_hash,
        )
        .await?
    {
        MarkSent::Sent => {}
        MarkSent::HashConflict => return Err(ApiError::relay_report_conflict()),
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
