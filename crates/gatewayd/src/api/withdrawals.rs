//! Withdrawals: the merchant's whole USDC balance across every supported
//! network, moved to one address they name.
//!
//! Prepare, sign, submit, poll. `POST /v1/withdrawals` reads the Payday
//! wallet's balance on every chain and returns one leg per non-zero balance,
//! each with the EIP-712 typed data the merchant must sign (an EIP-3009
//! authorization under that chain's USDC; see
//! `gateway_core::withdrawal_authorization`). `POST …/authorizations` takes
//! the signatures, verified offline against the wallet. From then on
//! `gateway-indexer` relays: a same-chain `transferWithAuthorization`, or
//! the forwarder's CCTP burn and, once Circle attests, the mint on the
//! destination chain. The merchant's signature fixes where every leg's
//! funds may land; Payday pays gas and can redirect nothing.

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
    AuthorizationKind, AuthorizationTypedData, ChainDto, ChainRegistry, USDC_DECIMALS, UsdcDomain,
    WithdrawalAuthorization, WithdrawalId, WithdrawalLegId, bridge_nonce, chain_name,
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
        return replay(&state, existing, destination_chain_id, &destination_address).await;
    }

    let reader = state.chain_reader()?;
    let balances = balances(&state.networks, reader.as_ref(), wallet).await?;
    let legs = plan_legs(
        &state.networks,
        reader.as_ref(),
        &balances,
        destination_chain_id,
        destination,
    )?;
    if legs.is_empty() {
        return Err(ApiError::nothing_to_withdraw());
    }
    let input = NewWithdrawal {
        id: Uuid::now_v7(),
        account_id: account,
        idempotency_key: idempotency_key.clone(),
        wallet_address: wallet.to_checksum(None),
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
            return replay(&state, existing, destination_chain_id, &destination_address).await;
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
    destination_chain_id: u64,
    destination_address: &str,
) -> Result<(StatusCode, HeaderMap, AxumJson<WithdrawalResponse>), ApiError> {
    if existing.destination_chain_id as u64 != destination_chain_id
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
    if row.cancelled_at.is_some() || row.completed_at.is_some() || row.failed_at.is_some() {
        return Err(ApiError::withdrawal_finished());
    }
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
        if !matches!(
            leg.state,
            LegState::AwaitingSignature | LegState::Authorized
        ) {
            return Err(ApiError::withdrawal_leg_not_signable(&leg_name));
        }
        if leg.valid_before <= now {
            return Err(ApiError::withdrawal_authorization_expired(&leg_name));
        }
        let (authorization, domain) = leg_authorization(leg, wallet)?;
        let signature = verify_authorization(&authorization, &domain, item.signature.trim())
            .map_err(|error| {
                ApiError::withdrawal_signature_invalid(&leg_name, &error.to_string())
            })?;
        let signature = signature.as_bytes().to_vec();
        if leg.state == LegState::Authorized {
            // Resending the signature already on file is a no-op; a
            // different valid one for the same leg is not accepted.
            if leg.signature.as_deref() == Some(signature.as_slice()) {
                continue;
            }
            return Err(ApiError::withdrawal_leg_not_signable(&leg_name));
        }
        verified.push((leg.id, leg_name, signature));
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

/// The wallet's USDC on every registered chain, read together.
async fn balances(
    networks: &ChainRegistry,
    reader: &dyn ChainReads,
    wallet: Address,
) -> Result<Vec<(u64, U256)>, ApiError> {
    let reads = networks.chains().iter().map(|chain| async move {
        reader
            .usdc_balance(chain.chain_id, wallet)
            .await
            .map(|balance| (chain.chain_id, balance))
    });
    futures::future::try_join_all(reads).await.map_err(|error| {
        tracing::warn!(%error, "withdrawal balance read failed");
        ApiError::withdrawals_unavailable(format!("Could not read the wallet's balance: {error}"))
    })
}

/// One leg per non-zero balance, in registry order, with the authorization
/// each will need.
fn plan_legs(
    networks: &ChainRegistry,
    reader: &dyn ChainReads,
    balances: &[(u64, U256)],
    destination_chain_id: u64,
    destination: Address,
) -> Result<Vec<NewWithdrawalLeg>, ApiError> {
    let valid_before = unix_now() + AUTHORIZATION_TTL.as_secs();
    let mut legs = Vec::new();
    for (position, &(chain_id, balance)) in balances.iter().enumerate() {
        if balance.is_zero() {
            continue;
        }
        let domain = reader.usdc_domain(chain_id).ok_or_else(|| {
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
            position: position as i16,
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
) -> Result<(WithdrawalAuthorization, UsdcDomain), ApiError> {
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
    let domain = UsdcDomain {
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
        destination: DestinationResponse {
            chain: chain_dto(row.destination_chain_id as u64),
            address: row.destination_address.clone(),
        },
        legs: legs
            .into_iter()
            .map(|leg| leg_response(leg, wallet, destination, destination_domain))
            .collect(),
        created_at: rfc3339(row.created_at),
        completed_at: row.completed_at.map(rfc3339),
        cancelled_at: row.cancelled_at.map(rfc3339),
        failed_at: row.failed_at.map(rfc3339),
    }
}

fn leg_response(
    leg: DbWithdrawalLeg,
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
        amount: format_units(leg.amount, USDC_DECIMALS).unwrap_or_default(),
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
