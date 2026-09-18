use async_trait::async_trait;
use gum_contracts::rpc::{
    ChainFaultReport, FinalizedHeadReport, IndexerCursor, RangeApplied, RecordFinalizedRange,
    RpcError, WatchList, WatchListQuery,
};
use rand::Rng;
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("ledger transport failure: {0}")]
    Transport(String),
    #[error(transparent)]
    Rpc(#[from] RpcError),
}
impl LedgerError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Transport(_)) || matches!(self, Self::Rpc(e) if e.is_retryable())
    }
}

#[async_trait]
pub trait IndexerLedger: Send + Sync {
    async fn cursor(&self, chain_id: u64) -> Result<Option<IndexerCursor>, LedgerError>;
    async fn watch_list(
        &self,
        chain_id: u64,
        query: WatchListQuery,
    ) -> Result<WatchList, LedgerError>;
    async fn report_finalized_head(
        &self,
        chain_id: u64,
        report: FinalizedHeadReport,
    ) -> Result<(), LedgerError>;
    async fn record_finalized_range(
        &self,
        chain_id: u64,
        range: RecordFinalizedRange,
    ) -> Result<RangeApplied, LedgerError>;
    async fn report_fault(
        &self,
        chain_id: u64,
        report: ChainFaultReport,
    ) -> Result<(), LedgerError>;
}

pub struct HttpLedger {
    client: reqwest::Client,
    base: String,
    token: String,
}
impl HttpLedger {
    pub fn new(base: &str, token: &str) -> Result<Self, LedgerError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|e| LedgerError::Transport(e.to_string()))?,
            base: format!("{}/internal/v1/indexer", base.trim_end_matches('/')),
            token: token.into(),
        })
    }
    async fn call<T: serde::de::DeserializeOwned, B: serde::Serialize + ?Sized>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, LedgerError> {
        let url = format!("{}{path}", self.base);
        let mut delay = Duration::from_millis(100);
        for attempt in 1..=4 {
            let started = Instant::now();
            let mut headers = reqwest::header::HeaderMap::new();
            gum_telemetry::correlation::inject(&mut headers, Default::default(), None);
            let mut request = self
                .client
                .request(method.clone(), &url)
                .bearer_auth(&self.token)
                .headers(headers);
            if let Some(value) = body {
                request = request.json(value);
            }
            let result = match request.send().await {
                Err(e) => Err(LedgerError::Transport(e.to_string())),
                Ok(response) if response.status().is_success() => response
                    .json()
                    .await
                    .map_err(|e| LedgerError::Transport(e.to_string())),
                Ok(response) => match response.json::<RpcError>().await {
                    Ok(e) => Err(LedgerError::Rpc(e)),
                    Err(e) => Err(LedgerError::Transport(e.to_string())),
                },
            };
            match result {
                Ok(value) => return Ok(value),
                Err(error) if attempt < 4 && error.is_retryable() => {
                    tracing::warn!(attempt, latency_ms = started.elapsed().as_millis() as u64, error = %error, "indexer ledger RPC failed; retrying");
                    let jitter = rand::rng().random_range(0..=delay.as_millis() as u64 / 2);
                    tokio::time::sleep(delay + Duration::from_millis(jitter)).await;
                    delay = (delay * 2).min(Duration::from_secs(2));
                }
                Err(error) => {
                    tracing::error!(attempt, latency_ms = started.elapsed().as_millis() as u64, error = %error, "indexer ledger RPC failed");
                    return Err(error);
                }
            }
        }
        unreachable!()
    }
}
#[async_trait]
impl IndexerLedger for HttpLedger {
    async fn cursor(&self, id: u64) -> Result<Option<IndexerCursor>, LedgerError> {
        self.call::<_, ()>(reqwest::Method::GET, &format!("/chains/{id}/cursor"), None)
            .await
    }
    async fn watch_list(&self, id: u64, q: WatchListQuery) -> Result<WatchList, LedgerError> {
        self.call(
            reqwest::Method::POST,
            &format!("/chains/{id}/watch-list"),
            Some(&q),
        )
        .await
    }
    async fn report_finalized_head(
        &self,
        id: u64,
        r: FinalizedHeadReport,
    ) -> Result<(), LedgerError> {
        self.call(
            reqwest::Method::POST,
            &format!("/chains/{id}/finalized-head"),
            Some(&r),
        )
        .await
    }
    async fn record_finalized_range(
        &self,
        id: u64,
        r: RecordFinalizedRange,
    ) -> Result<RangeApplied, LedgerError> {
        self.call(
            reqwest::Method::POST,
            &format!("/chains/{id}/finalized-ranges"),
            Some(&r),
        )
        .await
    }
    async fn report_fault(&self, id: u64, r: ChainFaultReport) -> Result<(), LedgerError> {
        self.call(
            reqwest::Method::POST,
            &format!("/chains/{id}/faults"),
            Some(&r),
        )
        .await
    }
}
