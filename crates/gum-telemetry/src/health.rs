//! Liveness and readiness.
//!
//! `/health/live` answers 200 as long as the process serves HTTP. It is the
//! orchestrator's restart signal: a deadlocked or wedged process fails it.
//!
//! `/health/ready` runs every registered [`ReadinessCheck`] and answers 200
//! only when all pass, with a JSON body naming each check's state. It is
//! the load-balancer's routing signal and the deploy's "the new task is
//! good" signal: a service whose database is unreachable or whose schema is
//! behind stays out of rotation without being restarted.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

#[async_trait]
pub trait ReadinessCheck: Send + Sync {
    fn name(&self) -> &'static str;
    /// `Ok(())` when ready; `Err(reason)` otherwise.
    async fn check(&self) -> Result<(), String>;
}

#[derive(Clone, Default)]
pub struct HealthState {
    checks: Arc<Vec<Arc<dyn ReadinessCheck>>>,
    timeout: Duration,
}

impl HealthState {
    pub fn new(checks: Vec<Arc<dyn ReadinessCheck>>) -> Self {
        Self {
            checks: Arc::new(checks),
            timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Serialize)]
struct CheckReport {
    name: &'static str,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct ReadyReport {
    ready: bool,
    checks: Vec<CheckReport>,
}

async fn live() -> StatusCode {
    StatusCode::OK
}

async fn ready(State(state): State<HealthState>) -> Response {
    let mut checks = Vec::with_capacity(state.checks.len());
    let mut ready = true;
    for check in state.checks.iter() {
        let outcome = match tokio::time::timeout(state.timeout, check.check()).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(reason)) => Err(reason),
            Err(_) => Err(format!("timed out after {:?}", state.timeout)),
        };
        ready &= outcome.is_ok();
        checks.push(CheckReport {
            name: check.name(),
            ok: outcome.is_ok(),
            error: outcome.err(),
        });
    }
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(ReadyReport { ready, checks })).into_response()
}

/// A router serving `/health/live` and `/health/ready`. Merge it into the
/// service's own router or serve it alone on an internal port.
pub fn router(checks: Vec<Arc<dyn ReadinessCheck>>) -> Router {
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .with_state(HealthState::new(checks))
}

/// Readiness of a Postgres pool: a round trip succeeds and, when a
/// migrator is given, the schema is at the version this binary expects.
pub struct DatabaseReady {
    pub pool: sqlx::PgPool,
    pub migrator: Option<&'static sqlx::migrate::Migrator>,
}

#[async_trait]
impl ReadinessCheck for DatabaseReady {
    fn name(&self) -> &'static str {
        "database"
    }

    async fn check(&self) -> Result<(), String> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(migrator) = self.migrator {
            let applied: Vec<i64> =
                sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success")
                    .fetch_all(&self.pool)
                    .await
                    .map_err(|error| format!("migration table unreadable: {error}"))?;
            let missing: Vec<i64> = migrator
                .iter()
                .map(|migration| migration.version)
                .filter(|version| !applied.contains(version))
                .collect();
            if !missing.is_empty() {
                return Err(format!("schema behind: migrations {missing:?} not applied"));
            }
        }
        Ok(())
    }
}
