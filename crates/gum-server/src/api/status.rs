//! `/v1/status`: one picture of the three services per chain, assembled
//! from what each reports into the database. The indexer's cursor and
//! finalized head come from its RPC calls, the signers' heartbeat and
//! balances from their `execution` schema, and the queues from the ledger.

use axum::extract::State;
use gum_ledger::{ChainFaultRepository, CursorRepository, ExecutionStatusView};
use serde::Serialize;

use crate::{
    api::{error::ApiError, json::Json},
    state::AppState,
};

/// A signer heartbeat older than this reports the executor as `stale`.
const STALE_HEARTBEAT_SECS: i64 = 120;

/// One entry per supported network: where its indexer and sweeper stand.
#[derive(Serialize)]
pub struct ServiceStatus {
    chains: Vec<ChainStatus>,
}

#[derive(Serialize)]
struct ChainStatus {
    id: String,
    name: &'static str,
    finalized_block: Option<String>,
    finalized_at: Option<String>,
    /// The open finality fault, when the chain is halted.
    halted: Option<String>,
    indexer: IndexerStatus,
    sweeper: SweeperStatus,
    withdrawals: WithdrawalStatus,
    signers: Vec<SignerStatus>,
}

#[derive(Serialize)]
struct IndexerStatus {
    cursor_block: Option<String>,
    cursor_at: Option<String>,
    lag_blocks: Option<u64>,
}

#[derive(Serialize)]
struct SweeperStatus {
    /// `running`, `halted`, `failing`, `stale` (no recent heartbeat) or
    /// `unknown` (the signers never reported on this chain).
    state: String,
    detail: Option<String>,
    queued: i64,
    in_flight: i64,
    oldest_uncollected_secs: Option<f64>,
}

#[derive(Serialize)]
struct WithdrawalStatus {
    authorized: i64,
    awaiting_attestation: i64,
    attested: i64,
    in_flight: i64,
}

#[derive(Serialize)]
struct SignerStatus {
    address: String,
    balance_wei: Option<String>,
    low_balance: bool,
    error: Option<String>,
    observed_at: String,
}

fn timestamp(value: u64) -> Option<String> {
    sqlx::types::chrono::DateTime::from_timestamp(value as i64, 0).map(gum_core::rfc3339)
}

pub async fn get(State(state): State<AppState>) -> Result<Json<ServiceStatus>, ApiError> {
    let pool = state.repo.pool().clone();
    let cursors = CursorRepository::new(pool.clone());
    let execution = ExecutionStatusView::new(pool.clone());
    let faults = ChainFaultRepository::new(pool, gum_bus::Publisher::new("gum-server"));
    let now = sqlx::types::chrono::Utc::now();
    let mut chains = Vec::with_capacity(state.networks.chains().len());
    for chain in state.networks.chains() {
        let finalized = cursors.finalized_head(chain.chain_id).await?;
        let cursor = cursors.get(chain.chain_id).await?;
        let queue = state.repo.sweep_queue_stats(chain.chain_id).await?;
        let relay = state.withdrawals.relay_stats(chain.chain_id).await?;
        let executor = execution.executor(chain.chain_id).await?;
        let signers = execution.signers(chain.chain_id).await?;
        let fault = faults.open(chain.chain_id).await?;
        let (sweeper_state, detail) = match executor {
            None => ("unknown".to_owned(), None),
            Some(status)
                if now.signed_duration_since(status.heartbeat_at).num_seconds()
                    > STALE_HEARTBEAT_SECS =>
            {
                (
                    "stale".to_owned(),
                    Some(format!(
                        "last heartbeat {}",
                        gum_core::rfc3339(status.heartbeat_at)
                    )),
                )
            }
            Some(status) => (status.state, status.detail),
        };
        chains.push(ChainStatus {
            id: chain.chain_id.to_string(),
            name: gum_core::chain_name(chain.chain_id),
            finalized_block: finalized.map(|value| value.block.to_string()),
            finalized_at: finalized.and_then(|value| value.block_timestamp.and_then(timestamp)),
            halted: fault.map(|fault| fault.reason),
            indexer: IndexerStatus {
                cursor_block: cursor.map(|value| value.block.to_string()),
                cursor_at: cursor.and_then(|value| value.block_timestamp.and_then(timestamp)),
                lag_blocks: finalized
                    .zip(cursor)
                    .map(|(head, indexed)| head.block.saturating_sub(indexed.block)),
            },
            sweeper: SweeperStatus {
                state: sweeper_state,
                detail,
                queued: queue.queued,
                in_flight: queue.in_flight,
                oldest_uncollected_secs: queue.oldest_uncollected_secs,
            },
            withdrawals: WithdrawalStatus {
                authorized: relay.authorized,
                awaiting_attestation: relay.awaiting_attestation,
                attested: relay.attested,
                in_flight: relay.in_flight,
            },
            signers: signers
                .into_iter()
                .map(|signer| SignerStatus {
                    address: signer.signer.to_string(),
                    balance_wei: signer.balance_wei.map(|value| value.to_string()),
                    low_balance: signer.low_balance,
                    error: signer.error,
                    observed_at: gum_core::rfc3339(signer.observed_at),
                })
                .collect(),
        });
    }
    Ok(Json(ServiceStatus { chains }))
}
