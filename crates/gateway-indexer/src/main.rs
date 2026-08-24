mod chain;
mod config;
mod indexer;

use std::sync::Arc;
use std::time::Duration;

use alloy_signer_local::PrivateKeySigner;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

use chain::ChainClient;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = config::Config::from_env();

    let pool = gateway_db::connect(config.database_url())
        .await
        .expect("failed to connect to database");

    // The backend key that signs sweep (`execute`) transactions.
    let signer: PrivateKeySigner = config
        .signer_key()
        .parse()
        .expect("invalid GATEWAY_SIGNER_KEY");

    // Connect to the chain and assert the RPC endpoint serves the configured
    // chain — a proven-invariant startup check, so a misconfigured node fails
    // fast instead of silently indexing the wrong chain.
    let chain_client = chain::AlloyChainClient::connect(config.rpc_url(), signer)
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

    // Run the payment indexer until Ctrl-C, then drive a graceful shutdown.
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let chain: Arc<dyn ChainClient> = Arc::new(chain_client);
    let worker = indexer::Indexer::new(
        repo,
        cursor,
        chain,
        config.chain_id(),
        config.factory_address(),
        Duration::from_millis(config.indexer_poll_interval_ms()),
    );
    let indexer_handle = tokio::spawn(worker.run(shutdown_rx));

    tokio::signal::ctrl_c()
        .await
        .expect("failed to install Ctrl-C handler");
    tracing::info!("shutdown signal received");

    // Signal the indexer and wait for it to finish.
    let _ = shutdown_tx.send(true);
    if let Err(e) = indexer_handle.await {
        tracing::error!(error = %e, "indexer task did not shut down cleanly");
    }
}
