use axum::extract::State;
use gateway_db::CursorRepository;
use serde::Serialize;

use crate::{
    api::{error::ApiError, json::Json},
    state::AppState,
};

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
    indexer: IndexerStatus,
    sweeper: SweeperStatus,
}

#[derive(Serialize)]
struct IndexerStatus {
    cursor_block: Option<String>,
    cursor_at: Option<String>,
    lag_blocks: Option<u64>,
}

#[derive(Serialize)]
struct SweeperStatus {
    state: String,
    queued: i64,
}

fn timestamp(value: u64) -> Option<String> {
    sqlx::types::chrono::DateTime::from_timestamp(value as i64, 0).map(gateway_core::rfc3339)
}

pub async fn get(State(state): State<AppState>) -> Result<Json<ServiceStatus>, ApiError> {
    let cursors = CursorRepository::new(state.repo.pool().clone());
    let mut chains = Vec::with_capacity(state.networks.chains().len());
    for chain in state.networks.chains() {
        let finalized = cursors.finalized_head(chain.chain_id, chain.usdc).await?;
        let cursor = cursors.get(chain.chain_id, chain.usdc).await?;
        let queue = state.repo.sweep_queue_stats(chain.chain_id).await?;
        let worker = state.repo.sweeper_status(chain.chain_id).await?;
        chains.push(ChainStatus {
            id: chain.chain_id.to_string(),
            name: gateway_core::chain_name(chain.chain_id),
            finalized_block: finalized.map(|value| value.block.to_string()),
            finalized_at: finalized.and_then(|value| value.block_timestamp.and_then(timestamp)),
            indexer: IndexerStatus {
                cursor_block: cursor.map(|value| value.block.to_string()),
                cursor_at: cursor.and_then(|value| value.block_timestamp.and_then(timestamp)),
                lag_blocks: finalized
                    .zip(cursor)
                    .map(|(head, indexed)| head.block.saturating_sub(indexed.block)),
            },
            sweeper: SweeperStatus {
                state: worker.map_or_else(|| "degraded".into(), |value| value.state),
                queued: queue.queued,
            },
        });
    }
    Ok(Json(ServiceStatus { chains }))
}
