use axum::extract::State;
use gateway_db::CursorRepository;
use serde::Serialize;

use crate::{
    api::{error::ApiError, json::Json},
    state::AppState,
};

#[derive(Serialize)]
pub struct ServiceStatus {
    chain: ChainStatus,
    indexer: IndexerStatus,
    sweeper: SweeperStatus,
}

#[derive(Serialize)]
struct ChainStatus {
    id: String,
    name: &'static str,
    finalized_block: Option<String>,
    finalized_at: Option<String>,
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
    let finalized = cursors
        .finalized_head(state.chain_id.0, state.usdc_address)
        .await?;
    let cursor = cursors.get(state.chain_id.0, state.usdc_address).await?;
    let queue = state.repo.sweep_queue_stats(state.chain_id.0).await?;
    let worker = state.repo.sweeper_status(state.chain_id.0).await?;
    Ok(Json(ServiceStatus {
        chain: ChainStatus {
            id: state.chain_id.0.to_string(),
            name: match state.chain_id.0 {
                1 => "Ethereum",
                143 => "Monad",
                10_143 => "Monad Testnet",
                31_337 => "Local",
                _ => "Unknown",
            },
            finalized_block: finalized.map(|value| value.block.to_string()),
            finalized_at: finalized.and_then(|value| value.block_timestamp.and_then(timestamp)),
        },
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
    }))
}
