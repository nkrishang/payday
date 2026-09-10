mod chain;
mod config;
mod deployment;
mod indexer;
mod signal;

use std::sync::Arc;
use std::time::Duration;

use alloy_network::{EthereumWallet, TxSigner};
use alloy_signer_aws::AwsSigner;
use alloy_signer_local::PrivateKeySigner;
use gateway_core::{ChainConfig, ChainId, FinalitySource};
use sqlx::PgPool;
use sqlx::postgres::PgConnection;
use tokio::sync::watch;
use tracing::info;
use tracing_subscriber::EnvFilter;

use chain::ChainClient;
use config::SignerConfig;
use indexer::{IndexerConfig, SWEEP_BACKOFF_BASE_SECS, SWEEP_BACKOFF_CAP_SECS};

/// Base of the per-chain advisory lock ids: one indexer per chain, whatever
/// process it runs in.
const INDEXER_ADVISORY_LOCK_ID: i64 = 0x5041_5944_4159;

/// The advisory lock that says "this process indexes and sweeps `chain_id`".
fn indexer_lock_id(chain_id: u64) -> i64 {
    INDEXER_ADVISORY_LOCK_ID ^ (chain_id as i64)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = config::Config::from_env();

    let pool = gateway_db::connect(config.database_url())
        .await
        .expect("failed to connect to database");

    // One worker per chain, each with its own RPC, signal, cursor, and nonce
    // stream; the signer key is shared, so its address is the same on every
    // chain. Connections are made together: the endpoints are independent.
    let aws = match config.signer() {
        SignerConfig::AwsKms(_) => {
            Some(aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await)
        }
        SignerConfig::Local(_) => None,
    };
    let mut workers = Vec::with_capacity(config.chains().len());
    let mut lock_connections = Vec::with_capacity(config.chains().len());
    for chain in config.chains() {
        let lock_connection = acquire_indexer_lock(&pool, chain.chain_id)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "failed to acquire the indexer database lock for chain {}: {error}",
                    chain.chain_id
                )
            });
        lock_connections.push(lock_connection);
        workers.push(connect_chain(&config, aws.as_ref(), chain, pool.clone()).await);
    }

    // Run until the local interrupt or ECS's termination signal, then allow the
    // current database/chain operation to finish before the task exits.
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut runs = Vec::with_capacity(workers.len());
    for worker in workers {
        // The transfer signal is a latency optimisation with no ledger
        // authority, so it runs as a detached task: it can never fail the
        // process, and without it the worker simply reconciles on its
        // polling cadence.
        match worker.signal {
            Some(signal) => {
                tokio::spawn(signal.run(
                    worker.indexer.watch_list(),
                    worker.indexer.signal(),
                    shutdown_rx.clone(),
                ));
            }
            None => info!(
                chain_id = worker.chain_id,
                poll_interval_ms = config.indexer_poll_interval().as_millis() as u64,
                "transfer signal disabled; reconciling on the polling cadence"
            ),
        }
        let chain_id = worker.chain_id;
        let run = worker.indexer.run(shutdown_rx.clone());
        runs.push(async move { run.await.map_err(|error| (chain_id, error)) });
    }
    let workers = futures::future::try_join_all(runs);
    let lock_monitor = monitor_indexer_locks(&mut lock_connections);
    tokio::pin!(workers, lock_monitor);

    tokio::select! {
        _ = shutdown_signal() => {
            tracing::info!("shutdown signal received");
            let _ = shutdown_tx.send(true);
            if let Err((chain_id, error)) = workers.await {
                tracing::error!(chain_id, %error, "indexer fatal");
                panic!("indexer for chain {chain_id} failed during shutdown: {error}");
            }
        }
        result = &mut workers => {
            match result {
                Ok(_) => {
                    tracing::error!("indexer fatal");
                    panic!("indexer stopped without a shutdown signal");
                }
                Err((chain_id, error)) => {
                    // One chain halting halts the process: ECS restarts it,
                    // and the halted chain's cursor check runs again first.
                    tracing::error!(chain_id, %error, "indexer fatal");
                    panic!("indexer for chain {chain_id} halted: {error}");
                }
            }
        }
        result = &mut lock_monitor => {
            let error = result.expect_err("indexer lock monitor cannot complete successfully");
            tracing::error!(%error, "indexer fatal");
            let _ = shutdown_tx.send(true);
            let _ = workers.await;
            panic!("indexer database lock connection lost: {error}");
        }
    }
}

/// One chain's worker and, if configured, its transfer signal.
struct ChainWorker {
    chain_id: u64,
    indexer: indexer::Indexer,
    signal: Option<signal::TransferSignal>,
}

async fn connect_chain(
    config: &config::Config,
    aws: Option<&aws_config::SdkConfig>,
    chain: &ChainConfig,
    pool: PgPool,
) -> ChainWorker {
    let chain_id = chain.chain_id;
    let wallet = match config.signer() {
        SignerConfig::Local(key) => {
            let signer: PrivateKeySigner = key.parse().expect("invalid PAYDAY_SIGNER_KEY");
            tracing::info!(chain_id, address = %signer.address(), signer = "local", "configured sweep signer");
            EthereumWallet::from(signer)
        }
        SignerConfig::AwsKms(key_id) => {
            let kms = aws_sdk_kms::Client::new(aws.expect("AWS configuration is loaded for KMS"));
            let signer = AwsSigner::new(kms, key_id.clone(), Some(chain_id))
                .await
                .expect("failed to initialize PAYDAY_KMS_KEY_ID");
            tracing::info!(chain_id, address = %signer.address(), signer = "aws-kms", "configured sweep signer");
            EthereumWallet::from(signer)
        }
    };

    // Connect to the chain and assert the RPC endpoint serves the configured
    // chain — a proven-invariant startup check, so a misconfigured node fails
    // fast instead of silently indexing the wrong chain.
    let chain_client =
        chain::AlloyChainClient::connect(config.rpc_url(chain_id), wallet, config.rpc_max_rps())
            .await
            .unwrap_or_else(|error| panic!("failed to connect to chain {chain_id}: {error}"));
    let node_chain_id = chain_client
        .get_chain_id()
        .await
        .expect("failed to query chain id from RPC");
    assert_eq!(
        node_chain_id,
        chain_id,
        "{} does not serve chain {chain_id}",
        chain.rpc_url_var()
    );
    // The factory embeds Payment's creation code and the sweeper is bound to
    // one factory, so a mismatched generation must never index or sweep.
    deployment::verify_deployment(
        &chain_client,
        &deployment::ExpectedDeployment {
            chain_id,
            factory: chain.factory,
            factory_code_hash: chain.factory_code_hash,
            batch_sweeper: chain.batch_sweeper,
            batch_sweeper_code_hash: chain.batch_sweeper_code_hash,
        },
    )
    .await
    .unwrap_or_else(|error| {
        panic!("contract deployment verification failed on chain {chain_id}: {error}")
    });
    // The finality source must be answerable before any range is committed.
    match chain.finality_source {
        FinalitySource::Finalized => {
            chain_client
                .finalized_header()
                .await
                .expect("failed to query the finalized block from RPC");
        }
        FinalitySource::Latest => {
            chain_client
                .latest_block_number()
                .await
                .expect("failed to query the latest block from RPC");
        }
    }

    let repo = gateway_db::InvoiceRepository::new(pool.clone());
    let cursor = gateway_db::CursorRepository::new(pool);
    let client: Arc<dyn ChainClient> = Arc::new(chain_client);
    let indexer = indexer::Indexer::new(
        repo,
        cursor,
        client,
        IndexerConfig {
            chain_id: ChainId(chain_id),
            factory: chain.factory,
            batch_sweeper: chain.batch_sweeper,
            usdc: chain.usdc,
            usdc_start_block: chain.usdc_start_block,
            finality_source: chain.finality_source,
            finality_confirmations: chain.finality_confirmations,
            block_time: Duration::from_millis(chain.block_time_ms),
            scan: chain.scan,
            log_range_size: chain.log_range_size,
            max_ranges_per_tick: config.max_ranges_per_tick(),
            poll_interval: config.indexer_poll_interval(),
            reconcile_interval: config.indexer_reconcile_interval(),
            idle_interval: config.indexer_idle_interval(),
            late_watch_window: config.late_watch_window(),
            sweep_pending_timeout: config.sweep_pending_timeout(),
            sweep_max_submissions: config.sweep_max_submissions(),
            sweep_max_attempts: config.sweep_max_attempts(),
            sweep_backoff_base_secs: SWEEP_BACKOFF_BASE_SECS,
            sweep_backoff_cap_secs: SWEEP_BACKOFF_CAP_SECS,
            signer_low_balance_wei: config.signer_low_balance_wei(),
        },
    );
    let signal = config
        .rpc_ws_url(chain_id)
        .map(|ws_url| signal::TransferSignal::new(ws_url.to_string(), chain_id, chain.usdc));
    ChainWorker {
        chain_id,
        indexer,
        signal,
    }
}

async fn acquire_indexer_lock(pool: &PgPool, chain_id: u64) -> Result<PgConnection, sqlx::Error> {
    // Detaching prevents the session-level advisory lock from returning to the
    // pool, where another caller could unknowingly inherit it.
    let mut connection = pool.acquire().await?.detach();
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(indexer_lock_id(chain_id))
        .fetch_one(&mut connection)
        .await?;

    if !acquired {
        return Err(sqlx::Error::Protocol(format!(
            "another indexer owns the database advisory lock for chain {chain_id}"
        )));
    }

    tracing::info!(chain_id, "exclusive indexer database lock acquired");
    Ok(connection)
}

async fn monitor_indexer_locks(connections: &mut [PgConnection]) -> Result<(), sqlx::Error> {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        interval.tick().await;
        for connection in connections.iter_mut() {
            sqlx::query_scalar::<_, i32>("SELECT 1")
                .fetch_one(&mut *connection)
                .await?;
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Connection;

    #[sqlx::test]
    async fn advisory_lock_allows_only_one_indexer_per_chain(pool: PgPool) {
        let first = acquire_indexer_lock(&pool, 143).await.unwrap();
        let second = acquire_indexer_lock(&pool, 143).await.unwrap_err();
        assert!(second.to_string().contains("another indexer"));
        // Another chain is another lock: one process may hold both.
        let other = acquire_indexer_lock(&pool, 8453).await.unwrap();

        first.close().await.unwrap();
        acquire_indexer_lock(&pool, 143).await.unwrap();
        other.close().await.unwrap();
    }
}
