//! Withdrawals: the merchant's whole balance in one currency, moved to one
//! address they name.
//!
//! Prepare, sign, submit, poll. `POST /v1/withdrawals` reads the Payday
//! wallet's balance in the currency on every chain serving it and returns
//! one leg per non-zero balance, each with the EIP-712 typed data the
//! merchant must sign (an EIP-3009 authorization under that chain's token
//! contract; see
//! `gateway_core::withdrawal_authorization`). `POST …/authorizations` takes
//! the signatures, verified offline against the wallet. From then on
//! `gateway-indexer` relays: a same-chain `transferWithAuthorization`, or
//! the forwarder's CCTP burn and, once Circle attests, the mint on the
//! destination chain. The merchant's signature fixes where every leg's
//! funds may land; Payday pays gas and can redirect nothing.
//!
//! Only USDC bridges: CCTP burns and mints USDC alone, at exactly 1:1. A
//! withdrawal in any other currency moves the destination chain's balance
//! and nothing else; a balance on another chain is withdrawn separately, to
//! an address on that chain, so the merchant is never quoted a rate.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy_primitives::utils::format_units;
use alloy_primitives::{Address, B256, U256, hex};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::{Extension, Json as AxumJson};
use gateway_core::{
    AuthorizationKind, AuthorizationTypedData, ChainDto, ChainRegistry, Currency, TokenDomain,
    TokenDto, WithdrawalAuthorization, WithdrawalId, WithdrawalLegId, bridge_nonce, chain_name,
    native_symbol, rfc3339, verify_authorization,
};
use gateway_db::{
    AccountId, AuthorizeOutcome, CancelOutcome, CreateWithdrawalError, DbWithdrawal,
    DbWithdrawalLeg, LegKind, LegState, NewWithdrawal, NewWithdrawalLeg,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::json::{Json, Query};
use crate::chain_reader::ChainReads;
use crate::state::AppState;

/// How long a leg's authorization stays usable. Long enough for a merchant
/// to sign at leisure and for a Base or Arbitrum burn to be attested; the
/// destination is fixed inside the signature, so a long window costs nothing.
pub const AUTHORIZATION_TTL: Duration = Duration::from_secs(24 * 3600);
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 255;

// --- Requests ---

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateWithdrawalRequest {
    /// `USDC` (the default) or `USDT`: the one currency the withdrawal moves.
    #[serde(default)]
    currency: Option<String>,
    destination: DestinationRequest,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestinationRequest {
    /// Decimal chain id, as `chain.id` reads everywhere else in the API.
    chain_id: String,
    address: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationsRequest {
    authorizations: Vec<LegAuthorization>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegAuthorization {
    leg_id: WithdrawalLegId,
    /// `0x` hex, 65 bytes `r || s || v`.
    signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    limit: Option<u32>,
    starting_after: Option<WithdrawalId>,
}

// --- Responses ---

#[derive(Debug, Serialize)]
pub struct WithdrawalResponse {
    id: WithdrawalId,
    /// `awaiting_signature` while any leg needs the merchant's signature,
    /// `in_progress` while the relayer works, then `completed`, `failed`,
    /// or `cancelled`.
    status: &'static str,
    /// The wallet every leg is signed from.
    wallet_address: String,
    /// The currency every leg moves.
    currency: String,
    destination: DestinationResponse,
    legs: Vec<LegResponse>,
    created_at: String,
    completed_at: Option<String>,
    cancelled_at: Option<String>,
    failed_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DestinationResponse {
    chain: ChainDto,
    address: String,
}

#[derive(Debug, Serialize)]
pub struct LegResponse {
    id: WithdrawalLegId,
    /// `transfer` when the funds already sit on the destination chain,
    /// `bridge` when they cross through CCTP.
    kind: &'static str,
    source_chain: ChainDto,
    /// The contract the leg is signed under: the currency on the source chain.
    token: TokenDto,
    amount: String,
    amount_base_units: String,
    state: &'static str,
    /// What to sign, present while the leg awaits its signature.
    authorization: Option<AuthorizationResponse>,
    transfer_tx_hash: Option<String>,
    burn_tx_hash: Option<String>,
    mint_tx_hash: Option<String>,
    failure_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuthorizationResponse {
    /// `TransferWithAuthorization` or `ReceiveWithAuthorization`.
    primary_type: String,
    /// The EIP-712 document, ready for `eth_signTypedData_v4`.
    typed_data: AuthorizationTypedData,
    /// When the token stops accepting the signature.
    expires_at: String,
    /// Bridge legs: the `WithdrawalForwarder` the authorization pays, and
    /// the destination the nonce commits to, so a signer can verify both.
    forwarder: Option<String>,
    nonce_preimage: Option<NoncePreimage>,
}

#[derive(Debug, Serialize)]
pub struct NoncePreimage {
    destination_domain: u32,
    mint_recipient: String,
    salt: String,
}

#[derive(Debug, Serialize)]
pub struct WithdrawalPage {
    withdrawals: Vec<WithdrawalResponse>,
    next_cursor: Option<WithdrawalId>,
}

// --- Handlers ---

pub async fn create(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    headers: HeaderMap,
    Json(request): Json<CreateWithdrawalRequest>,
) -> Result<(StatusCode, HeaderMap, AxumJson<WithdrawalResponse>), ApiError> {
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(ApiError::missing_idempotency_key)?;
    if idempotency_key.is_empty() || idempotency_key.len() > MAX_IDEMPOTENCY_KEY_BYTES {
        return Err(ApiError::invalid_request(format!(
            "Idempotency-Key must be 1 to {MAX_IDEMPOTENCY_KEY_BYTES} bytes"
        )));
    }
    let currency = match request.currency.as_deref().map(str::trim) {
        None | Some("") => Currency::Usdc,
        Some(code) => code
            .parse::<Currency>()
            .map_err(|_| ApiError::invalid_request("currency must be USDC or USDT"))?,
    };
    if state.networks.networks(currency).is_empty() {
        return Err(ApiError::unsupported_currency(currency));
    }
    let destination_chain_id = request
        .destination
        .chain_id
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|id| state.networks.get(*id).is_some())
        .ok_or_else(|| {
            ApiError::invalid_request(
                "destination.chain_id must be one of this deployment's networks",
            )
        })?;
    if state
        .networks
        .token(destination_chain_id, currency)
        .is_none()
    {
        return Err(ApiError::invalid_request(format!(
            "destination.chain_id must be a network serving {currency}; {} does not",
            chain_name(destination_chain_id)
        )));
    }
    let destination = Address::from_str(request.destination.address.trim()).map_err(|error| {
        ApiError::invalid_request(format!("invalid destination.address: {error}"))
    })?;
    if destination.is_zero() {
        return Err(ApiError::invalid_request(
            "destination.address must not be the zero address",
        ));
    }
    let destination_address = destination.to_checksum(None);

    let wallet = state
        .accounts
        .metadata(account)
        .await?
        .wallet_address
        .and_then(|value| Address::from_str(&value).ok())
        .ok_or_else(ApiError::wallet_not_ready)?;

    // An idempotent replay of the same request returns the same withdrawal;
    // the same key with another destination is a conflict.
    if let Some(existing) = state
        .withdrawals
        .find_by_idempotency_key(account, &idempotency_key)
        .await?
    {
        return replay(
            &state,
            existing,
            currency,
            destination_chain_id,
            &destination_address,
        )
        .await;
    }

    let reader = state.chain_reader()?;
    let balances = balances(
        &state.networks,
        reader.as_ref(),
        currency,
        wallet,
        destination_chain_id,
    )
    .await?;
    let legs = plan_legs(
        &state.networks,
        reader.as_ref(),
        currency,
        &balances,
        destination_chain_id,
        destination,
    )?;
    if legs.is_empty() {
        // A currency without a bridge may still sit on other chains: name
        // them, so the merchant withdraws each to an address there.
        let elsewhere: Vec<&str> = balances
            .iter()
            .filter(|(chain_id, _, balance)| {
                !balance.is_zero() && *chain_id != destination_chain_id
            })
            .map(|(chain_id, _, _)| chain_name(*chain_id))
            .collect();
        return Err(ApiError::nothing_to_withdraw(currency, &elsewhere));
    }
    let input = NewWithdrawal {
        id: Uuid::now_v7(),
        account_id: account,
        idempotency_key: idempotency_key.clone(),
        wallet_address: wallet.to_checksum(None),
        currency: currency.code().into(),
        destination_chain_id,
        destination_address: destination_address.clone(),
        legs,
    };
    match state.withdrawals.create(&input).await {
        Ok(()) => {}
        Err(CreateWithdrawalError::InProgress) => return Err(ApiError::withdrawal_in_progress()),
        Err(CreateWithdrawalError::IdempotencyKeyReused) => {
            // Lost a race with an identical request: answer as it did.
            let existing = state
                .withdrawals
                .find_by_idempotency_key(account, &idempotency_key)
                .await?
                .ok_or_else(|| ApiError::internal("withdrawal vanished after conflict"))?;
            return replay(
                &state,
                existing,
                currency,
                destination_chain_id,
                &destination_address,
            )
            .await;
        }
        Err(CreateWithdrawalError::Database(error)) => return Err(error.into()),
    }
    let row = state
        .withdrawals
        .get(account, input.id)
        .await?
        .ok_or_else(|| ApiError::internal("withdrawal vanished after creation"))?;
    let legs = state.withdrawals.legs(input.id).await?;
    Ok((
        StatusCode::CREATED,
        HeaderMap::new(),
        AxumJson(response(&state.networks, row, legs)),
    ))
}

async fn replay(
    state: &AppState,
    existing: DbWithdrawal,
    currency: Currency,
    destination_chain_id: u64,
    destination_address: &str,
) -> Result<(StatusCode, HeaderMap, AxumJson<WithdrawalResponse>), ApiError> {
    if existing.currency != currency.code()
        || existing.destination_chain_id as u64 != destination_chain_id
        || existing.destination_address != destination_address
    {
        return Err(ApiError::idempotency_conflict());
    }
    let legs = state.withdrawals.legs(existing.id).await?;
    let mut headers = HeaderMap::new();
    headers.insert("idempotency-replayed", HeaderValue::from_static("true"));
    Ok((
        StatusCode::OK,
        headers,
        AxumJson(response(&state.networks, existing, legs)),
    ))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
) -> Result<AxumJson<WithdrawalResponse>, ApiError> {
    let id = withdrawal_id(&id)?;
    let row = state
        .withdrawals
        .get(account, id)
        .await?
        .ok_or_else(ApiError::withdrawal_not_found)?;
    let legs = state.withdrawals.legs(id).await?;
    Ok(AxumJson(response(&state.networks, row, legs)))
}

pub async fn list(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Query(query): Query<ListQuery>,
) -> Result<AxumJson<WithdrawalPage>, ApiError> {
    let limit = query.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::invalid_request("limit must be between 1 and 100"));
    }
    if let Some(cursor) = query.starting_after
        && state.withdrawals.get(account, cursor.0).await?.is_none()
    {
        return Err(ApiError::invalid_request(
            "starting_after does not identify one of your withdrawals",
        ));
    }
    let mut rows = state
        .withdrawals
        .list(account, limit, query.starting_after.map(Uuid::from))
        .await?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = has_more.then(|| WithdrawalId(rows.last().expect("nonzero limit").id));
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut legs_by_withdrawal: HashMap<Uuid, Vec<DbWithdrawalLeg>> = HashMap::new();
    for leg in state.withdrawals.legs_for(&ids).await? {
        legs_by_withdrawal
            .entry(leg.withdrawal_id)
            .or_default()
            .push(leg);
    }
    Ok(AxumJson(WithdrawalPage {
        withdrawals: rows
            .into_iter()
            .map(|row| {
                let legs = legs_by_withdrawal.remove(&row.id).unwrap_or_default();
                response(&state.networks, row, legs)
            })
            .collect(),
        next_cursor,
    }))
}

pub async fn authorize(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
    Json(request): Json<AuthorizationsRequest>,
) -> Result<AxumJson<WithdrawalResponse>, ApiError> {
    let id = withdrawal_id(&id)?;
    if request.authorizations.is_empty() {
        return Err(ApiError::invalid_request(
            "authorizations must name at least one leg",
        ));
    }
    let row = state
        .withdrawals
        .get(account, id)
        .await?
        .ok_or_else(ApiError::withdrawal_not_found)?;
    let wallet = Address::from_str(&row.wallet_address)
        .map_err(|_| ApiError::internal("stored wallet address is malformed"))?;
    let legs = state.withdrawals.legs(id).await?;
    let now = unix_now();
    // Verify every signature before recording any, so a body with one bad
    // signature changes nothing.
    let mut verified = Vec::with_capacity(request.authorizations.len());
    for item in &request.authorizations {
        let leg_name = item.leg_id.to_string();
        let leg = legs
            .iter()
            .find(|leg| leg.id == item.leg_id.0)
            .ok_or_else(|| ApiError::withdrawal_leg_not_found(&leg_name))?;
        let (authorization, domain) = leg_authorization(leg, wallet)?;
        let signature = verify_authorization(&authorization, &domain, item.signature.trim())
            .map_err(|error| {
                ApiError::withdrawal_signature_invalid(&leg_name, &error.to_string())
            })?;
        let signature = signature.as_bytes().to_vec();
        // Resending the signature already on file is a no-op, whatever the
        // withdrawal or the leg has reached since: the relayer may have
        // taken it and moved on, and a lost response — even one that arrives
        // after the whole withdrawal finished — must not read as a conflict.
        if leg.signature.as_deref() == Some(signature.as_slice()) {
            continue;
        }
        verified.push((leg.id, leg_name, signature));
    }
    if verified.is_empty() {
        // Every signature is already on file: an idempotent replay, answered
        // with the withdrawal as it stands now, finished or not.
        return Ok(AxumJson(response(&state.networks, row, legs)));
    }
    if row.cancelled_at.is_some() || row.completed_at.is_some() || row.failed_at.is_some() {
        return Err(ApiError::withdrawal_finished());
    }
    for item in &verified {
        let (_, leg_name, _) = item;
        let leg = legs
            .iter()
            .find(|leg| leg.id == item.0)
            .ok_or_else(|| ApiError::withdrawal_leg_not_found(leg_name))?;
        if !matches!(
            leg.state,
            LegState::AwaitingSignature | LegState::Authorized
        ) {
            return Err(ApiError::withdrawal_leg_not_signable(leg_name));
        }
        if leg.valid_before <= now {
            return Err(ApiError::withdrawal_authorization_expired(leg_name));
        }
        if leg.state == LegState::Authorized {
            // A different valid signature for the same leg is not accepted.
            return Err(ApiError::withdrawal_leg_not_signable(leg_name));
        }
    }
    for (leg_id, leg_name, signature) in verified {
        match state
            .withdrawals
            .authorize(account, id, leg_id, &signature)
            .await?
        {
            AuthorizeOutcome::Authorized | AuthorizeOutcome::Unchanged => {}
            AuthorizeOutcome::NotAwaitingSignature => {
                return Err(ApiError::withdrawal_leg_not_signable(&leg_name));
            }
            AuthorizeOutcome::NotFound => return Err(ApiError::withdrawal_not_found()),
        }
    }
    let row = state
        .withdrawals
        .get(account, id)
        .await?
        .ok_or_else(ApiError::withdrawal_not_found)?;
    let legs = state.withdrawals.legs(id).await?;
    Ok(AxumJson(response(&state.networks, row, legs)))
}

pub async fn cancel(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
) -> Result<AxumJson<WithdrawalResponse>, ApiError> {
    let id = withdrawal_id(&id)?;
    match state.withdrawals.cancel(account, id).await? {
        CancelOutcome::Cancelled | CancelOutcome::AlreadyCancelled => {}
        CancelOutcome::Finished => return Err(ApiError::withdrawal_finished()),
        CancelOutcome::InProgress => return Err(ApiError::withdrawal_not_cancellable()),
        CancelOutcome::NotFound => return Err(ApiError::withdrawal_not_found()),
    }
    let row = state
        .withdrawals
        .get(account, id)
        .await?
        .ok_or_else(ApiError::withdrawal_not_found)?;
    let legs = state.withdrawals.legs(id).await?;
    Ok(AxumJson(response(&state.networks, row, legs)))
}

// --- Planning ---

/// The wallet's balance in `currency` on the chains the withdrawal can move,
/// in registry order. A currency with a 1:1 bridge (CCTP, USDC only) can
/// sweep from any chain serving it, so every balance is needed and one read
/// failure blocks planning: an unknown balance must not be quietly bridged
/// or left behind. A currency without a bridge only ever moves the
/// destination chain's balance, so that one read is required while the
/// others only name themselves in a `nothing_to_withdraw` message — an
/// outage on a chain the withdrawal would not touch must not block it.
/// Returns `(chain_id, token, balance)`.
async fn balances(
    networks: &ChainRegistry,
    reader: &dyn ChainReads,
    currency: Currency,
    wallet: Address,
    destination_chain_id: u64,
) -> Result<Vec<(u64, Address, U256)>, ApiError> {
    let read = |chain: &gateway_core::ChainConfig| {
        let chain_id = chain.chain_id;
        let token = chain.token(currency)?.address;
        Some(async move {
            reader
                .balance(chain_id, token, wallet)
                .await
                .map(|balance| (chain_id, token, balance))
        })
    };
    let unavailable = |error: crate::chain_reader::ChainReadError| {
        tracing::warn!(%error, "withdrawal balance read failed");
        ApiError::withdrawals_unavailable(format!("Could not read the wallet's balance: {error}"))
    };
    if currency.cross_chain_settlement().is_some() {
        let reads = networks.chains().iter().filter_map(read);
        return futures::future::try_join_all(reads)
            .await
            .map_err(unavailable);
    }
    // The destination chain's balance is the withdrawal itself: refuse
    // rather than plan blind.
    let destination = networks
        .get(destination_chain_id)
        .and_then(read)
        .ok_or_else(|| ApiError::internal("destination chain does not serve the currency"))?
        .await
        .map_err(unavailable)?;
    if !destination.2.is_zero() {
        return Ok(vec![destination]);
    }
    // Zero at the destination: other chains only appear in the
    // `nothing_to_withdraw` message, so their reads are best-effort.
    let elsewhere = networks
        .chains()
        .iter()
        .filter(|chain| chain.chain_id != destination_chain_id)
        .filter_map(read);
    let mut balances = vec![destination];
    for (chain_id, token, balance) in futures::future::join_all(elsewhere)
        .await
        .into_iter()
        .flatten()
    {
        if !balance.is_zero() {
            balances.push((chain_id, token, balance));
        }
    }
    Ok(balances)
}

/// One leg per non-zero balance the withdrawal can move, in registry
/// order, with the authorization each will need. A balance on the
/// destination chain is a transfer leg. A balance elsewhere is a bridge leg
/// when the currency has a 1:1 bridge (CCTP, USDC only); otherwise it is
/// left where it is, for a withdrawal to an address on that chain.
fn plan_legs(
    networks: &ChainRegistry,
    reader: &dyn ChainReads,
    currency: Currency,
    balances: &[(u64, Address, U256)],
    destination_chain_id: u64,
    destination: Address,
) -> Result<Vec<NewWithdrawalLeg>, ApiError> {
    let valid_before = unix_now() + AUTHORIZATION_TTL.as_secs();
    let bridges = currency.cross_chain_settlement().is_some();
    let mut legs = Vec::new();
    for &(chain_id, token, balance) in balances {
        if balance.is_zero() || (chain_id != destination_chain_id && !bridges) {
            continue;
        }
        let domain = reader.domain(chain_id, token).ok_or_else(|| {
            ApiError::withdrawals_unavailable(format!(
                "{} is not readable on this deployment",
                chain_name(chain_id)
            ))
        })?;
        let (kind, authorization_to, nonce, salt) = if chain_id == destination_chain_id {
            (
                LegKind::Transfer,
                destination.to_checksum(None),
                B256::from(rand::random::<[u8; 32]>()),
                None,
            )
        } else {
            let source = networks.cctp(chain_id).ok_or_else(|| {
                ApiError::withdrawals_unavailable(format!(
                    "Bridging from {} is not available on this deployment; withdraw to an address on {} instead",
                    chain_name(chain_id),
                    chain_name(chain_id)
                ))
            })?;
            let target = networks.cctp(destination_chain_id).ok_or_else(|| {
                ApiError::withdrawals_unavailable(format!(
                    "Bridging to {} is not available on this deployment",
                    chain_name(destination_chain_id)
                ))
            })?;
            // Circle's TokenMessengerV2 refuses a burn above its per-message
            // limit, and a whole-balance authorization has no smaller piece
            // to send instead: refuse the withdrawal here rather than leave
            // the merchant with a signature that can never execute.
            if balance > gateway_core::MAX_CCTP_BURN_PER_MESSAGE {
                return Err(ApiError::withdrawal_exceeds_bridge_limit(
                    chain_name(chain_id),
                    &format_units(balance, currency.decimals())
                        .unwrap_or_else(|_| balance.to_string()),
                ));
            }
            let salt = B256::from(rand::random::<[u8; 32]>());
            (
                LegKind::Bridge,
                source.forwarder.to_checksum(None),
                bridge_nonce(target.domain, destination, salt),
                Some(salt),
            )
        };
        legs.push(NewWithdrawalLeg {
            id: Uuid::now_v7(),
            position: legs.len() as i16,
            kind,
            source_chain_id: chain_id,
            amount: balance,
            token_address: domain.token.to_checksum(None),
            domain_name: domain.name.clone(),
            domain_version: domain.version.clone(),
            authorization_to,
            nonce,
            salt,
            valid_before,
        });
    }
    Ok(legs)
}

// --- Shaping ---

/// The authorization a stored leg asks the wallet to sign, and the domain
/// it is signed under.
fn leg_authorization(
    leg: &DbWithdrawalLeg,
    wallet: Address,
) -> Result<(WithdrawalAuthorization, TokenDomain), ApiError> {
    let malformed = |field: &str| ApiError::internal(format!("stored leg {field} is malformed"));
    let to = Address::from_str(&leg.authorization_to).map_err(|_| malformed("payee"))?;
    let token = Address::from_str(&leg.token_address).map_err(|_| malformed("token"))?;
    let authorization = WithdrawalAuthorization {
        kind: match leg.kind {
            LegKind::Transfer => AuthorizationKind::Transfer,
            LegKind::Bridge => AuthorizationKind::Receive,
        },
        from: wallet,
        to,
        value: leg.amount,
        valid_before: leg.valid_before,
        nonce: leg.nonce,
    };
    let domain = TokenDomain {
        name: leg.domain_name.clone(),
        version: leg.domain_version.clone(),
        chain_id: leg.source_chain_id,
        token,
    };
    Ok((authorization, domain))
}

pub(crate) fn response(
    networks: &ChainRegistry,
    row: DbWithdrawal,
    legs: Vec<DbWithdrawalLeg>,
) -> WithdrawalResponse {
    let currency = row.currency.parse::<Currency>().unwrap_or(Currency::Usdc);
    let wallet = Address::from_str(&row.wallet_address).unwrap_or_default();
    let destination = Address::from_str(&row.destination_address).unwrap_or_default();
    let destination_domain = networks
        .cctp(row.destination_chain_id as u64)
        .map(|cctp| cctp.domain);
    let status = if row.cancelled_at.is_some() {
        "cancelled"
    } else if row.completed_at.is_some() {
        "completed"
    } else if row.failed_at.is_some() {
        "failed"
    } else if legs
        .iter()
        .any(|leg| leg.state == LegState::AwaitingSignature)
    {
        "awaiting_signature"
    } else {
        "in_progress"
    };
    WithdrawalResponse {
        id: WithdrawalId(row.id),
        status,
        wallet_address: row.wallet_address.clone(),
        currency: currency.code().into(),
        destination: DestinationResponse {
            chain: chain_dto(row.destination_chain_id as u64),
            address: row.destination_address.clone(),
        },
        legs: legs
            .into_iter()
            .map(|leg| leg_response(leg, currency, wallet, destination, destination_domain))
            .collect(),
        created_at: rfc3339(row.created_at),
        completed_at: row.completed_at.map(rfc3339),
        cancelled_at: row.cancelled_at.map(rfc3339),
        failed_at: row.failed_at.map(rfc3339),
    }
}

fn leg_response(
    leg: DbWithdrawalLeg,
    currency: Currency,
    wallet: Address,
    destination: Address,
    destination_domain: Option<u32>,
) -> LegResponse {
    let authorization = (leg.state == LegState::AwaitingSignature)
        .then(|| leg_authorization(&leg, wallet).ok())
        .flatten()
        .map(|(authorization, domain)| AuthorizationResponse {
            primary_type: authorization.kind.primary_type().into(),
            typed_data: authorization.typed_data(&domain),
            expires_at: rfc3339(
                chrono::DateTime::from_timestamp(leg.valid_before as i64, 0).unwrap_or_default(),
            ),
            forwarder: matches!(leg.kind, LegKind::Bridge).then(|| leg.authorization_to.clone()),
            nonce_preimage: match (leg.kind, leg.salt, destination_domain) {
                (LegKind::Bridge, Some(salt), Some(domain)) => Some(NoncePreimage {
                    destination_domain: domain,
                    mint_recipient: destination.to_checksum(None),
                    salt: salt.to_string(),
                }),
                _ => None,
            },
        });
    LegResponse {
        id: WithdrawalLegId(leg.id),
        kind: leg.kind.as_str(),
        source_chain: chain_dto(leg.source_chain_id),
        token: TokenDto {
            symbol: currency.symbol_on(leg.source_chain_id).into(),
            address: leg.token_address.clone(),
            decimals: currency.decimals(),
        },
        amount: format_units(leg.amount, currency.decimals()).unwrap_or_default(),
        amount_base_units: leg.amount.to_string(),
        state: leg.state.as_str(),
        authorization,
        transfer_tx_hash: leg.transfer_tx_hash.map(hex::encode_prefixed),
        burn_tx_hash: leg.burn_tx_hash.map(hex::encode_prefixed),
        mint_tx_hash: leg.mint_tx_hash.map(hex::encode_prefixed),
        failure_reason: leg.failure_reason,
    }
}

fn chain_dto(chain_id: u64) -> ChainDto {
    ChainDto {
        id: chain_id.to_string(),
        name: chain_name(chain_id).into(),
        native_symbol: native_symbol(chain_id).into(),
    }
}

/// Anything but a canonical `wd_` id is a missing withdrawal.
fn withdrawal_id(value: &str) -> Result<Uuid, ApiError> {
    WithdrawalId::parse(value)
        .map(Uuid::from)
        .ok_or_else(ApiError::withdrawal_not_found)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl AppState {
    /// The chain reader, or 503 when the deployment has none.
    pub fn chain_reader(&self) -> Result<Arc<dyn ChainReads>, ApiError> {
        self.chain_reader.clone().ok_or_else(|| {
            ApiError::withdrawals_unavailable("Withdrawals are not available on this deployment")
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;
    use crate::chain_reader::ChainReadError;

    const USDC: &str = "0x754704Bc059F8C67012fEd69BC8A327a5aafb603";
    const USDT: &str = "0xe7cd86e13AC4309349F30B3435a9d337750fC82D";
    const USDT_ARBITRUM: &str = "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9";
    const WALLET: &str = "0x1111111111111111111111111111111111111111";

    fn chain(id: u64, tokens: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "chain_id": id,
            "tokens": tokens,
            "factory": "0x5FbDB2315678afecb367f032d93F642f64180aa3",
            "batch_sweeper": "0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0",
            "factory_code_hash": format!("0x{}", "ab".repeat(32)),
            "batch_sweeper_code_hash": format!("0x{}", "cd".repeat(32)),
            "start_block": 100,
            "finality_source": "finalized",
            "finality_confirmations": 0,
            "block_time_ms": 300,
            "log_range_size": 100
        })
    }

    /// USDT on Monad and Arbitrum One, USDC on Monad — the deployment shape
    /// the pinning rules exist for.
    fn registry() -> ChainRegistry {
        ChainRegistry::parse(
            &serde_json::json!([
                chain(
                    143,
                    serde_json::json!([
                        {"currency": "USDC", "address": USDC},
                        {"currency": "USDT", "address": USDT}
                    ])
                ),
                chain(
                    42161,
                    serde_json::json!([
                        {"currency": "USDT", "address": USDT_ARBITRUM}
                    ])
                )
            ])
            .to_string(),
        )
        .unwrap()
    }

    /// Balances per chain, with the chains whose balance read fails.
    struct MockReader {
        balances: HashMap<u64, U256>,
        failing: HashSet<u64>,
        reads: std::sync::Mutex<HashSet<u64>>,
    }

    impl MockReader {
        fn failing(chain_id: u64) -> Self {
            Self {
                balances: HashMap::new(),
                failing: HashSet::from([chain_id]),
                reads: std::sync::Mutex::new(HashSet::new()),
            }
        }

        /// `failing` chain's balance read errors; `funded` chain holds
        /// `balance`.
        fn funded(funded: u64, balance: u64, failing: u64) -> Self {
            Self {
                balances: HashMap::from([(funded, U256::from(balance))]),
                failing: HashSet::from([failing]),
                reads: std::sync::Mutex::new(HashSet::new()),
            }
        }

        fn read(&self, chain_id: u64) -> bool {
            self.reads.lock().unwrap().contains(&chain_id)
        }
    }

    #[async_trait::async_trait]
    impl ChainReads for MockReader {
        async fn balance(
            &self,
            chain_id: u64,
            _token: Address,
            _wallet: Address,
        ) -> Result<U256, ChainReadError> {
            self.reads.lock().unwrap().insert(chain_id);
            if self.failing.contains(&chain_id) {
                return Err(ChainReadError::Rpc {
                    chain_id,
                    operation: "test",
                    message: "down".into(),
                });
            }
            Ok(self.balances.get(&chain_id).copied().unwrap_or(U256::ZERO))
        }

        fn domain(&self, _chain_id: u64, _token: Address) -> Option<&TokenDomain> {
            None
        }
    }

    fn wallet() -> Address {
        WALLET.parse().unwrap()
    }

    #[tokio::test]
    async fn usdt_withdrawal_ignores_an_unrelated_chain_outage() {
        // The wallet's USDT sits on Monad; Arbitrum's balance read is down.
        // A withdrawal to Monad touches Arbitrum for nothing, so it must
        // still plan — and must never even read Arbitrum's balance.
        let reader = MockReader::funded(143, 5, 42161);
        let balances = balances(&registry(), &reader, Currency::Usdt, wallet(), 143)
            .await
            .unwrap();
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].0, 143);
        assert_eq!(balances[0].1, USDT.parse::<Address>().unwrap());
        assert_eq!(balances[0].2, U256::from(5));
        assert!(!reader.read(42161));
    }

    #[tokio::test]
    async fn usdt_unfunded_destination_is_diagnosed_best_effort() {
        // Nothing on Monad and Arbitrum's read down: the merchant still gets
        // the nothing-to-withdraw answer, built from whatever could be read.
        let reader = MockReader::failing(42161);
        let balances = balances(&registry(), &reader, Currency::Usdt, wallet(), 143)
            .await
            .unwrap();
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].0, 143);
        assert_eq!(balances[0].2, U256::ZERO);
        assert!(reader.read(42161), "the diagnosis looks elsewhere");
    }

    #[tokio::test]
    async fn usdt_destination_balance_blocks_planning_when_unreadable() {
        // The destination chain's balance is the withdrawal itself: an
        // outage there is a 503, not a plan.
        let reader = MockReader::failing(143);
        let error = balances(&registry(), &reader, Currency::Usdt, wallet(), 143)
            .await
            .unwrap_err();
        assert!(
            error
                .message
                .contains("Could not read the wallet's balance")
        );
    }

    #[tokio::test]
    async fn usdt_zero_destination_balance_still_sees_a_funded_elsewhere() {
        // Nothing on Monad, funds on Arbitrum: the zero balance carries
        // through to planning, which reports nothing_to_withdraw naming
        // Arbitrum.
        let reader = MockReader {
            balances: HashMap::from([(42161, U256::from(5u8))]),
            failing: HashSet::new(),
            reads: std::sync::Mutex::new(HashSet::new()),
        };
        let balances = balances(&registry(), &reader, Currency::Usdt, wallet(), 143)
            .await
            .unwrap();
        assert_eq!(balances.len(), 2);
        assert_eq!(balances[0].0, 143);
        assert_eq!(balances[1].0, 42161);
    }

    #[tokio::test]
    async fn usdc_bridge_planning_refuses_an_unreadable_chain() {
        // Every chain holding USDC could become a bridge leg, so a read
        // failure anywhere must not let an unknown balance be bridged or
        // silently dropped.
        let reader = MockReader::failing(42161);
        let networks = ChainRegistry::parse(
            &serde_json::json!([
                chain(
                    143,
                    serde_json::json!([{"currency": "USDC", "address": USDC}])
                ),
                chain(
                    42161,
                    serde_json::json!([{"currency": "USDC", "address": USDC}])
                )
            ])
            .to_string(),
        )
        .unwrap();
        let error = balances(&networks, &reader, Currency::Usdc, wallet(), 143)
            .await
            .unwrap_err();
        assert!(
            error
                .message
                .contains("Could not read the wallet's balance")
        );
    }
}
