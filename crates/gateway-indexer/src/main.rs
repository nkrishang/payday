mod chain;
mod config;
mod indexer;

use std::sync::Arc;
use std::time::Duration;

use alloy_network::{EthereumWallet, TxSigner};
use alloy_signer_aws::AwsSigner;
use alloy_signer_local::PrivateKeySigner;
use sqlx::PgPool;
use sqlx::postgres::PgConnection;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

use chain::ChainClient;
use config::SignerConfig;

const INDEXER_ADVISORY_LOCK_ID: i64 = 0x5041_5944_4159;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = config::Config::from_env();

    let pool = gateway_db::connect(config.database_url())
        .await
        .expect("failed to connect to database");
    let mut lock_connection = acquire_indexer_lock(&pool)
        .await
        .expect("failed to acquire exclusive indexer database lock");

    let wallet = match config.signer() {
        SignerConfig::Local(key) => {
            let signer: PrivateKeySigner = key.parse().expect("invalid GATEWAY_SIGNER_KEY");
            tracing::info!(address = %signer.address(), signer = "local", "configured sweep signer");
            EthereumWallet::from(signer)
        }
        SignerConfig::AwsKms(key_id) => {
            let sdk_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
            let kms = aws_sdk_kms::Client::new(&sdk_config);
            let signer = AwsSigner::new(kms, key_id.clone(), Some(config.chain_id().0))
                .await
                .expect("failed to initialize GATEWAY_KMS_KEY_ID");
            tracing::info!(address = %signer.address(), signer = "aws-kms", "configured sweep signer");
            EthereumWallet::from(signer)
        }
    };

    // Connect to the chain and assert the RPC endpoint serves the configured
    // chain — a proven-invariant startup check, so a misconfigured node fails
    // fast instead of silently indexing the wrong chain.
    let chain_client = chain::AlloyChainClient::connect(config.rpc_url(), wallet)
        .await
        .expect("failed to connect to RPC endpoint");
    let node_chain_id = chain_client
        .get_chain_id()
        .await
        .expect("failed to query chain id from RPC");
    assert_eq!(
        node_chain_id,
        config.chain_id().0,
        "configured GATEWAY_CHAIN_ID does not match the RPC node's chain id"
    );

    let repo = gateway_db::InvoiceRepository::new(pool.clone());
    let cursor = gateway_db::CursorRepository::new(pool.clone());

    // Run until the local interrupt or ECS's termination signal, then allow the
    // current database/chain operation to finish before the task exits.
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let chain: Arc<dyn ChainClient> = Arc::new(chain_client);
    let worker = indexer::Indexer::new(
        repo,
        cursor,
        chain,
        config.chain_id(),
        config.factory_address(),
        config.usdc_address(),
        config.usdc_start_block(),
        config.finality_confirmations(),
        config.log_range_size(),
        Duration::from_millis(config.indexer_poll_interval_ms()),
    );
    let worker = worker.run(shutdown_rx);
    let lock_monitor = monitor_indexer_lock(&mut lock_connection);
    tokio::pin!(worker, lock_monitor);

    tokio::select! {
        _ = shutdown_signal() => {
            tracing::info!("shutdown signal received");
            let _ = shutdown_tx.send(true);
            if let Err(error) = worker.await {
                tracing::error!(%error, "indexer fatal");
                panic!("indexer failed during shutdown: {error}");
            }
        }
        result = &mut worker => {
            match result {
                Ok(()) => {
                    tracing::error!("indexer fatal");
                    panic!("indexer stopped without a shutdown signal");
                }
                Err(error) => {
                    tracing::error!(%error, "indexer fatal");
                    panic!("indexer halted: {error}");
                }
            }
        }
        result = &mut lock_monitor => {
            let error = result.expect_err("indexer lock monitor cannot complete successfully");
            tracing::error!(%error, "indexer fatal");
            let _ = shutdown_tx.send(true);
            let _ = worker.await;
            panic!("indexer database lock connection lost: {error}");
        }
    }
}

async fn acquire_indexer_lock(pool: &PgPool) -> Result<PgConnection, sqlx::Error> {
    // Detaching prevents the session-level advisory lock from returning to the
    // pool, where another caller could unknowingly inherit it.
    let mut connection = pool.acquire().await?.detach();
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(INDEXER_ADVISORY_LOCK_ID)
        .fetch_one(&mut connection)
        .await?;

    if !acquired {
        return Err(sqlx::Error::Protocol(
            "another indexer owns the database advisory lock".into(),
        ));
    }

    tracing::info!("exclusive indexer database lock acquired");
    Ok(connection)
}

async fn monitor_indexer_lock(connection: &mut PgConnection) -> Result<(), sqlx::Error> {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        interval.tick().await;
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&mut *connection)
            .await?;
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
    async fn advisory_lock_allows_only_one_indexer(pool: PgPool) {
        let first = acquire_indexer_lock(&pool).await.unwrap();
        let second = acquire_indexer_lock(&pool).await.unwrap_err();
        assert!(second.to_string().contains("another indexer"));

        first.close().await.unwrap();
        acquire_indexer_lock(&pool).await.unwrap();
    }
}
