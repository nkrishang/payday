//! The indexer-facing side of the internal RPC contract
//! ([`gum_contracts::rpc`]).
//!
//! `gum-indexer` never touches the database: everything it observes on a
//! chain becomes a typed request here, and the server applies it to the
//! ledger in one transaction. The routes live on their own listener
//! (`GUM_INTERNAL_BIND_ADDR`) behind a shared bearer token
//! (`GUM_INTERNAL_TOKEN`), so the public API surface never grows an
//! unauthenticated write path by accident.
//!
//! Every mutation is idempotent or compare-and-set, which is what lets the
//! indexer retry a request whose response it never saw:
//!
//! * `finalized-ranges` is a compare-and-set on the stored cursor
//!   ([`InvoiceRepository::apply_finalized_range`]); a replay of a committed
//!   range is `AlreadyApplied`, a stale one is `CursorMismatch`.
//! * `finalized-head` is an upsert of the chain's clock plus the expiry of
//!   unbound requests, both idempotent.
//! * `faults` opens at most one fault per chain.
//!
//! Errors carry [`RpcError`] bodies; the indexer retries `unavailable` and
//! `internal` after a backoff and treats the rest as permanent.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use gum_contracts::rpc::{
    ChainFaultReport, FinalizedHeadReport, IndexerCursor, RangeApplied, RecordFinalizedRange,
    RpcError, RpcErrorCode, WatchList, WatchListQuery,
};
use gum_core::ChainRegistry;
use gum_ledger::{ChainFaultRepository, CursorRepository, InvoiceRepository};
use tokio::sync::Notify;
use tracing::{Instrument, info, info_span, warn};

#[derive(Clone)]
pub struct InternalState {
    invoices: InvoiceRepository,
    cursors: CursorRepository,
    faults: ChainFaultRepository,
    networks: Arc<ChainRegistry>,
    token: Arc<str>,
    /// Poked when a range funds a request, so the sweep scheduler runs at
    /// once instead of on its next tick.
    sweep_wake: Arc<Notify>,
}

impl InternalState {
    pub fn new(
        invoices: InvoiceRepository,
        faults: ChainFaultRepository,
        networks: Arc<ChainRegistry>,
        token: impl Into<Arc<str>>,
        sweep_wake: Arc<Notify>,
    ) -> Self {
        Self {
            cursors: CursorRepository::new(invoices.pool().clone()),
            invoices,
            faults,
            networks,
            token: token.into(),
            sweep_wake,
        }
    }
}

/// The routes under `/internal/v1/indexer`. Mount on the internal listener
/// only.
pub fn router(state: InternalState) -> Router {
    Router::new()
        .route("/chains/{chain_id}/cursor", get(cursor))
        .route("/chains/{chain_id}/watch-list", post(watch_list))
        .route("/chains/{chain_id}/finalized-head", post(finalized_head))
        .route("/chains/{chain_id}/finalized-ranges", post(finalized_range))
        .route("/chains/{chain_id}/faults", post(fault))
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
}

struct Rejection(RpcError);

impl IntoResponse for Rejection {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self.0)).into_response()
    }
}

impl From<sqlx::Error> for Rejection {
    fn from(error: sqlx::Error) -> Self {
        warn!(error = %error, "internal RPC database failure");
        Self(RpcError::new(
            RpcErrorCode::Unavailable,
            "database unavailable",
        ))
    }
}

impl From<gum_ledger::ChainFaultError> for Rejection {
    fn from(error: gum_ledger::ChainFaultError) -> Self {
        warn!(error = %error, "internal RPC could not record a chain fault");
        Self(RpcError::new(RpcErrorCode::Unavailable, error.to_string()))
    }
}

/// Bearer-token authentication. The comparison is constant time in the
/// token length so a wrong token leaks nothing about the right one.
async fn authenticate(
    State(state): State<InternalState>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !constant_time_eq(presented.as_bytes(), state.token.as_bytes()) {
        return Rejection(RpcError::new(
            RpcErrorCode::Unauthorized,
            "missing or invalid bearer token",
        ))
        .into_response();
    }
    let correlation_id = gum_telemetry::correlation::extract(&headers);
    let span = info_span!("internal_rpc", correlation_id = %correlation_id);
    next.run(request).instrument(span).await
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn known_chain(state: &InternalState, chain_id: u64) -> Result<(), Rejection> {
    if state.networks.get(chain_id).is_some() {
        Ok(())
    } else {
        Err(Rejection(RpcError::new(
            RpcErrorCode::UnknownChain,
            format!("chain {chain_id} is not served"),
        )))
    }
}

async fn cursor(
    State(state): State<InternalState>,
    Path(chain_id): Path<u64>,
) -> Result<Json<Option<IndexerCursor>>, Rejection> {
    known_chain(&state, chain_id)?;
    Ok(Json(state.cursors.get(chain_id).await?))
}

async fn watch_list(
    State(state): State<InternalState>,
    Path(chain_id): Path<u64>,
    Json(query): Json<WatchListQuery>,
) -> Result<Json<WatchList>, Rejection> {
    known_chain(&state, chain_id)?;
    let current = state
        .invoices
        .watch_fingerprint(chain_id, query.recent_since)
        .await?;
    let fingerprint = format!(
        "{}:{}",
        current.count,
        current
            .newest
            .map(|newest| newest.timestamp_micros().to_string())
            .unwrap_or_default()
    );
    if query.known_fingerprint.as_deref() == Some(fingerprint.as_str()) {
        return Ok(Json(WatchList {
            fingerprint,
            addresses: None,
        }));
    }
    let addresses = state
        .invoices
        .watch_addresses(chain_id, query.recent_since)
        .await?;
    Ok(Json(WatchList {
        fingerprint,
        addresses: Some(addresses),
    }))
}

async fn finalized_head(
    State(state): State<InternalState>,
    Path(chain_id): Path<u64>,
    Json(report): Json<FinalizedHeadReport>,
) -> Result<Json<()>, Rejection> {
    known_chain(&state, chain_id)?;
    state
        .cursors
        .record_finalized_head(
            chain_id,
            report.block,
            report.block_hash,
            report.block_timestamp,
        )
        .await?;
    let expired = state
        .invoices
        .expire_unbound(report.block_timestamp)
        .await?;
    for deposit_request_id in &expired {
        info!(chain_id, %deposit_request_id, "deposit request expired unpaid");
    }
    Ok(Json(()))
}

async fn finalized_range(
    State(state): State<InternalState>,
    Path(chain_id): Path<u64>,
    Json(range): Json<RecordFinalizedRange>,
) -> Result<Json<RangeApplied>, Rejection> {
    known_chain(&state, chain_id)?;
    let applied = state
        .invoices
        .apply_finalized_range(
            chain_id,
            range.expected_cursor,
            range.end_block,
            range.end_block_hash,
            range.end_block_timestamp,
            &range.observations,
        )
        .await?;
    match &applied {
        RangeApplied::Applied {
            funded,
            expired,
            cursor,
        } => {
            for deposit_request_id in funded {
                info!(chain_id, %deposit_request_id, block = cursor.block, "payment detected: deposit request funded");
            }
            for deposit_request_id in expired {
                info!(chain_id, %deposit_request_id, block = cursor.block, "deposit request expired");
            }
            if !funded.is_empty() || !range.observations.is_empty() {
                state.sweep_wake.notify_one();
            }
        }
        RangeApplied::AlreadyApplied { cursor } => {
            info!(
                chain_id,
                block = cursor.block,
                end_block = range.end_block,
                "finalized range replayed; already applied"
            );
        }
        RangeApplied::CursorMismatch { cursor } => {
            warn!(chain_id, stored = ?cursor.map(|c| c.block), expected = ?range.expected_cursor.map(|c| c.block), "finalized range refused: cursor mismatch");
        }
    }
    Ok(Json(applied))
}

async fn fault(
    State(state): State<InternalState>,
    Path(chain_id): Path<u64>,
    headers: HeaderMap,
    Json(report): Json<ChainFaultReport>,
) -> Result<Json<()>, Rejection> {
    known_chain(&state, chain_id)?;
    let correlation_id = gum_telemetry::correlation::extract(&headers);
    state
        .faults
        .raise(chain_id, &report.reason, report.block, correlation_id)
        .await?;
    Ok(Json(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, B256, U256};
    use axum::body::Body;
    use axum::http::Request;
    use gum_bus::Publisher;
    use gum_contracts::rpc::PaymentObservation;
    use sqlx::PgPool;
    use tower::ServiceExt;

    const CHAIN: u64 = 8453;
    const TOKEN: &str = "test-token";

    fn state(pool: &PgPool) -> InternalState {
        let networks = Arc::new(
            gum_core::ChainRegistry::new(vec![gum_core::ChainConfig {
                chain_id: CHAIN,
                tokens: vec![gum_core::TokenConfig {
                    currency: gum_core::Currency::Usdc,
                    address: Address::repeat_byte(0xaa),
                }],
                factory: Address::repeat_byte(0xfa),
                batch_sweeper: Address::repeat_byte(0x55),
                factory_code_hash: B256::ZERO,
                batch_sweeper_code_hash: B256::ZERO,
                start_block: 0,
                finality_source: gum_core::FinalitySource::Finalized,
                finality_confirmations: 0,
                block_time_ms: 1000,
                log_range_size: 100,
                signer_low_balance_wei: None,
                explorer_base_url: None,
                cctp: None,
                block_gas_limit: None,
                transaction_gas_limit: None,
                sweep_batch_size: None,
            }])
            .unwrap(),
        );
        InternalState::new(
            InvoiceRepository::new(pool.clone()),
            ChainFaultRepository::new(pool.clone(), Publisher::new("gum-server")),
            networks,
            TOKEN,
            Arc::new(Notify::new()),
        )
    }

    async fn call<T: serde::de::DeserializeOwned>(
        app: &Router,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, Result<T, RpcError>) {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        if status.is_success() {
            (status, Ok(serde_json::from_slice(&bytes).unwrap()))
        } else {
            (status, Err(serde_json::from_slice(&bytes).unwrap()))
        }
    }

    #[sqlx::test(migrator = "gum_schema::MIGRATOR")]
    async fn a_wrong_token_is_refused_before_any_work(pool: PgPool) {
        let app = router(state(&pool));
        let (status, result) = call::<Option<IndexerCursor>>(
            &app,
            "GET",
            &format!("/chains/{CHAIN}/cursor"),
            Some("nope"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(result.unwrap_err().code, RpcErrorCode::Unauthorized);
        let (status, _) = call::<Option<IndexerCursor>>(
            &app,
            "GET",
            &format!("/chains/{CHAIN}/cursor"),
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrator = "gum_schema::MIGRATOR")]
    async fn an_unserved_chain_is_a_permanent_error(pool: PgPool) {
        let app = router(state(&pool));
        let (status, result) =
            call::<Option<IndexerCursor>>(&app, "GET", "/chains/1/cursor", Some(TOKEN), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let error = result.unwrap_err();
        assert_eq!(error.code, RpcErrorCode::UnknownChain);
        assert!(!error.is_retryable());
    }

    #[sqlx::test(migrator = "gum_schema::MIGRATOR")]
    async fn ranges_are_compare_and_set_on_the_cursor(pool: PgPool) {
        let app = router(state(&pool));
        let path = format!("/chains/{CHAIN}/finalized-ranges");
        let (_, before) = call::<Option<IndexerCursor>>(
            &app,
            "GET",
            &format!("/chains/{CHAIN}/cursor"),
            Some(TOKEN),
            None,
        )
        .await;
        assert_eq!(before.unwrap(), None);

        let first = RecordFinalizedRange {
            expected_cursor: None,
            end_block: 100,
            end_block_hash: B256::repeat_byte(1),
            end_block_timestamp: 1_700_000_000,
            observations: vec![],
        };
        let (status, applied) = call::<RangeApplied>(
            &app,
            "POST",
            &path,
            Some(TOKEN),
            Some(serde_json::to_value(&first).unwrap()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let cursor = match applied.unwrap() {
            RangeApplied::Applied {
                cursor,
                funded,
                expired,
            } => {
                assert!(funded.is_empty() && expired.is_empty());
                cursor
            }
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(cursor.block, 100);

        // The indexer crashed before reading the response and replays.
        let (_, replay) = call::<RangeApplied>(
            &app,
            "POST",
            &path,
            Some(TOKEN),
            Some(serde_json::to_value(&first).unwrap()),
        )
        .await;
        assert!(
            matches!(replay.unwrap(), RangeApplied::AlreadyApplied { cursor } if cursor.block == 100)
        );

        // A range scanned from a stale cursor is refused and told where to
        // resume.
        let stale = RecordFinalizedRange {
            expected_cursor: Some(IndexerCursor {
                block: 50,
                block_hash: B256::repeat_byte(9),
                block_timestamp: None,
            }),
            end_block: 200,
            end_block_hash: B256::repeat_byte(2),
            end_block_timestamp: 1_700_000_100,
            observations: vec![PaymentObservation {
                token: Address::repeat_byte(0xaa),
                block_number: 150,
                block_hash: B256::repeat_byte(3),
                block_timestamp: 1_700_000_050,
                transaction_hash: B256::repeat_byte(4),
                transaction_index: 0,
                log_index: 0,
                sender: Address::repeat_byte(1),
                recipient: Address::repeat_byte(2),
                amount: U256::from(1),
            }],
        };
        let (status, refused) = call::<RangeApplied>(
            &app,
            "POST",
            &path,
            Some(TOKEN),
            Some(serde_json::to_value(&stale).unwrap()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            matches!(refused.unwrap(), RangeApplied::CursorMismatch { cursor: Some(cursor) } if cursor.block == 100)
        );
    }

    #[sqlx::test(migrator = "gum_schema::MIGRATOR")]
    async fn the_watch_list_is_skipped_when_the_fingerprint_matches(pool: PgPool) {
        let app = router(state(&pool));
        let path = format!("/chains/{CHAIN}/watch-list");
        let query = WatchListQuery {
            recent_since: chrono::Utc::now() - chrono::Duration::hours(1),
            known_fingerprint: None,
        };
        let (_, first) = call::<WatchList>(
            &app,
            "POST",
            &path,
            Some(TOKEN),
            Some(serde_json::to_value(&query).unwrap()),
        )
        .await;
        let first = first.unwrap();
        assert_eq!(first.addresses, Some(vec![]));
        let again = WatchListQuery {
            known_fingerprint: Some(first.fingerprint.clone()),
            ..query
        };
        let (_, second) = call::<WatchList>(
            &app,
            "POST",
            &path,
            Some(TOKEN),
            Some(serde_json::to_value(&again).unwrap()),
        )
        .await;
        let second = second.unwrap();
        assert_eq!(second.fingerprint, first.fingerprint);
        assert_eq!(second.addresses, None);
    }

    #[sqlx::test(migrator = "gum_schema::MIGRATOR")]
    async fn a_fault_halts_the_chain_once(pool: PgPool) {
        let app = router(state(&pool));
        let path = format!("/chains/{CHAIN}/faults");
        let report = ChainFaultReport {
            reason: "finalized block 10 changed hash".into(),
            block: Some(10),
        };
        for _ in 0..2 {
            let (status, _) = call::<()>(
                &app,
                "POST",
                &path,
                Some(TOKEN),
                Some(serde_json::to_value(&report).unwrap()),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }
        let open: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM chain_faults WHERE chain_id = $1 AND cleared_at IS NULL",
        )
        .bind(CHAIN as i64)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open, 1);
        let halts: i64 =
            sqlx::query_scalar("SELECT count(*) FROM bus.messages WHERE topic = 'chain.control'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(halts, 1);
    }
}
