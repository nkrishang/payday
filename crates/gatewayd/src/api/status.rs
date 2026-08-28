use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use gateway_db::CursorRepository;
use serde::Serialize;

use crate::{api::error::ApiError, state::AppState};

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
    sqlx::types::chrono::DateTime::from_timestamp(value as i64, 0).map(|value| value.to_rfc3339())
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

const MAX_LAG_BLOCKS: u64 = 1_000;
const MAX_BACKLOG_SECONDS: f64 = 15.0 * 60.0;

#[derive(Serialize)]
pub struct PublicStatus {
    status: &'static str,
    components: PublicComponents,
}
#[derive(Serialize)]
struct PublicComponents {
    api_and_database: PublicComponent,
    payment_indexing_and_settlement: PublicComponent,
}
#[derive(Serialize)]
struct PublicComponent {
    status: &'static str,
}

async fn public(state: &AppState) -> PublicStatus {
    let database_ok = state.repo.ping().await.is_ok();
    let cursors = CursorRepository::new(state.repo.pool().clone());
    let api_ok = database_ok
        && cursors
            .api_is_fresh(state.chain_id.0, state.status_stale_seconds)
            .await
            .unwrap_or(false);
    let indexing_ok = database_ok
        && cursors
            .indexer_is_healthy(
                state.chain_id.0,
                state.usdc_address,
                state.status_stale_seconds,
                MAX_LAG_BLOCKS,
            )
            .await
            .unwrap_or(false);
    let settlement_ok = if database_ok {
        state
            .repo
            .sweeper_status(state.chain_id.0)
            .await
            .ok()
            .flatten()
            .is_some_and(|worker| worker.state == "running")
            && state
                .repo
                .sweep_queue_stats(state.chain_id.0)
                .await
                .is_ok_and(|queue| {
                    queue
                        .oldest_uncollected_secs
                        .is_none_or(|age| age <= MAX_BACKLOG_SECONDS)
                })
    } else {
        false
    };
    let payment_ok = indexing_ok && settlement_ok;
    PublicStatus {
        status: if api_ok && payment_ok {
            "operational"
        } else {
            "degraded"
        },
        components: PublicComponents {
            api_and_database: PublicComponent {
                status: if api_ok { "operational" } else { "outage" },
            },
            payment_indexing_and_settlement: PublicComponent {
                status: if payment_ok {
                    "operational"
                } else {
                    "degraded"
                },
            },
        },
    }
}

pub async fn public_json(State(state): State<AppState>) -> Json<PublicStatus> {
    Json(public(&state).await)
}

pub async fn html(State(state): State<AppState>) -> Response {
    let status = public(&state).await;
    let ok = status.status == "operational";
    let (color, headline, detail) = if ok {
        (
            "#16a34a",
            "All systems operational",
            "Payments are being detected and settled normally.",
        )
    } else {
        (
            "#d97706",
            "Some systems are degraded",
            "Payment detection or settlement may be delayed. Funds remain safe; no action is needed from payers.",
        )
    };
    let api = if status.components.api_and_database.status == "operational" {
        "Operational"
    } else {
        "Outage"
    };
    let payment = if status.components.payment_indexing_and_settlement.status == "operational" {
        "Operational"
    } else {
        "Degraded"
    };
    let body = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="refresh" content="60"><title>Payday Status</title><style>body{{margin:0;background:#f8fafc;color:#172033;font:16px system-ui,sans-serif}}main{{max-width:680px;margin:10vh auto;padding:24px}}h1{{font-size:30px}}.banner,.component{{background:white;border:1px solid #e2e8f0;border-radius:12px;padding:20px;margin:16px 0;box-shadow:0 2px 8px #0f172a0d}}.banner{{border-left:6px solid {color}}}.banner p{{color:#526078;line-height:1.5;margin:8px 0 0}}.row{{display:flex;justify-content:space-between;gap:20px}}.state{{color:{color};font-weight:650}}footer{{color:#64748b;margin-top:28px;font-size:14px}}</style></head><body><main><h1>Payday service status</h1><div class="banner" role="status" aria-live="polite"><strong>{headline}</strong><p>{detail}</p></div><div class="component row"><span>API and database</span><span class="state">{api}</span></div><div class="component row"><span>Payment indexing and settlement</span><span class="state">{payment}</span></div><footer>Automatically refreshed every 60 seconds.</footer></main></body></html>"#
    );
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Html(body),
    )
        .into_response()
}
