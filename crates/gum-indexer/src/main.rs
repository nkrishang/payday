mod config;
mod indexer;
mod ledger;
mod signal;

use gum_chain::{AlloyChainClient, ChainReader};
use gum_core::ExpectedDeployment;
use std::sync::Arc;
use tokio::sync::watch;

#[tokio::main]
async fn main() {
    gum_telemetry::init("gum-indexer");
    let config = config::Config::from_env();
    let ledger: Arc<dyn ledger::IndexerLedger> = Arc::new(
        ledger::HttpLedger::new(config.server_url(), config.token())
            .expect("failed to build ledger client"),
    );
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    for configured in config.chains() {
        let chain = Arc::new(
            AlloyChainClient::connect(config.rpc_url(configured.chain_id), config.rpc_max_rps())
                .await
                .unwrap_or_else(|e| panic!("failed to connect chain {}: {e}", configured.chain_id)),
        );
        let expected = ExpectedDeployment {
            chain_id: configured.chain_id,
            factory: configured.factory,
            factory_code_hash: configured.factory_code_hash,
            batch_sweeper: configured.batch_sweeper,
            batch_sweeper_code_hash: configured.batch_sweeper_code_hash,
            forwarder: configured
                .cctp
                .as_ref()
                .map(|c| (c.forwarder, c.forwarder_code_hash)),
        };
        gum_chain::deployment::verify_deployment(chain.as_ref(), &expected)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "deployment verification failed for chain {}: {e}",
                    configured.chain_id
                )
            });
        ledger
            .cursor(configured.chain_id)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "server readiness check failed for chain {}: {e}",
                    configured.chain_id
                )
            });
        let signal_state = signal::SignalState::new();
        let (watch_tx, watch_rx) = watch::channel(Arc::new(Vec::new()));
        if let Some(url) = config.ws_url(configured.chain_id) {
            let task = signal::TransferSignal::new(
                url.to_owned(),
                configured.chain_id,
                configured.token_addresses(),
            );
            let state = signal_state.clone();
            let stop = shutdown_rx.clone();
            tokio::spawn(task.run(watch_rx, state, stop));
        }
        let worker = indexer::Indexer::new(
            ledger.clone(),
            chain as Arc<dyn ChainReader>,
            configured.clone(),
            config.late_watch(),
            config.max_ranges(),
            config.poll(),
            config.reconcile(),
            config.idle(),
            signal_state,
            watch_tx,
        );
        tokio::spawn(worker.run(shutdown_rx.clone()));
    }
    let listener = tokio::net::TcpListener::bind(config.listen())
        .await
        .expect("failed to bind health listener");
    let health = axum::serve(listener, gum_telemetry::health::router(Vec::new()))
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            let _ = shutdown_tx.send(true);
        });
    health.await.expect("health server failed");
}
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await.expect("ctrl-c handler");
}
