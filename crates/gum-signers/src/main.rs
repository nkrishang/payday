use std::collections::HashMap;
use std::sync::Arc;

use alloy_network::{EthereumWallet, TxSigner};
use alloy_primitives::Address;
use alloy_signer_aws::AwsSigner;
use alloy_signer_local::PrivateKeySigner;
use gum_bus::{Consumer, ConsumerOptions};
use gum_chain::{AlloyChainClient, ChainExecutor};
use gum_contracts::{ChainControl, ExecutionCommand};
use gum_core::{ChainConfig, ExpectedDeployment};
use gum_signers::{ChainControlHandler, ChainWorker, CommandHandler, Config, SignerConfig};
use gum_telemetry::health::{DatabaseReady, ReadinessCheck};
use tokio::sync::{Notify, watch};
use tracing::info;

const CONSUMER_NAME: &str = "gum-signers";

#[tokio::main]
async fn main() {
    gum_telemetry::init("gum-signers");
    let config = Config::from_env();
    let pool = gum_schema::connect(config.database_url(), 16)
        .await
        .expect("failed to connect to database");

    let aws = match config.signer() {
        SignerConfig::AwsKms(_) => {
            Some(aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await)
        }
        SignerConfig::Local(_) => None,
    };

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut wakers: HashMap<u64, Arc<Notify>> = HashMap::new();
    let mut workers = Vec::new();
    for chain in config.chains() {
        let client = connect(&config, aws.as_ref(), chain).await;
        let wake = Arc::new(Notify::new());
        wakers.insert(chain.chain_id, wake.clone());
        workers.push(ChainWorker::new(
            client,
            chain.clone(),
            pool.clone(),
            config.policy(chain),
            wake,
        ));
    }
    let wakers = Arc::new(wakers);
    for worker in workers {
        tokio::spawn(worker.run(config.poll_interval(), shutdown_rx.clone()));
    }

    // Handlers only write rows, so a short lease is plenty.
    let options = ConsumerOptions::default();
    tokio::spawn(gum_bus::run_consumer::<ExecutionCommand, _>(
        Consumer::new(CONSUMER_NAME, pool.clone(), options),
        CommandHandler::new(wakers),
        shutdown_rx.clone(),
    ));
    tokio::spawn(gum_bus::run_consumer::<ChainControl, _>(
        Consumer::new(CONSUMER_NAME, pool.clone(), options),
        ChainControlHandler,
        shutdown_rx.clone(),
    ));

    let checks: Vec<Arc<dyn ReadinessCheck>> = vec![Arc::new(DatabaseReady {
        pool: pool.clone(),
        migrator: Some(&gum_schema::MIGRATOR),
    })];
    let listener = tokio::net::TcpListener::bind(config.listen())
        .await
        .expect("failed to bind health listener");
    info!(listen = %config.listen(), chains = config.chains().len(), "gum-signers ready");
    axum::serve(listener, gum_telemetry::health::router(checks))
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            let _ = shutdown_tx.send(true);
        })
        .await
        .expect("health server failed");
}

/// Connect to one chain with the whole key pool registered, and refuse to
/// start against a node that serves another chain or another contract
/// generation.
async fn connect(
    config: &Config,
    aws: Option<&aws_config::SdkConfig>,
    chain: &ChainConfig,
) -> Arc<dyn ChainExecutor> {
    let chain_id = chain.chain_id;
    let (wallet, signers) = signer_pool(config, aws, chain_id).await;
    let client = AlloyChainClient::connect_signing(
        config.rpc_url(chain_id),
        wallet,
        signers,
        config.rpc_max_rps(),
    )
    .await
    .unwrap_or_else(|error| panic!("failed to connect to chain {chain_id}: {error}"));
    let expected = ExpectedDeployment {
        chain_id,
        factory: chain.factory,
        factory_code_hash: chain.factory_code_hash,
        batch_sweeper: chain.batch_sweeper,
        batch_sweeper_code_hash: chain.batch_sweeper_code_hash,
        forwarder: chain
            .cctp
            .as_ref()
            .map(|cctp| (cctp.forwarder, cctp.forwarder_code_hash)),
    };
    gum_chain::deployment::verify_deployment(&client, &expected)
        .await
        .unwrap_or_else(|error| {
            panic!("deployment verification failed for chain {chain_id}: {error}")
        });
    Arc::new(client)
}

/// Every configured key as one wallet for `chain_id`, and the pool's
/// addresses in configuration order. Two keys with one address would be
/// two lanes on one nonce stream, so duplicates refuse to start.
async fn signer_pool(
    config: &Config,
    aws: Option<&aws_config::SdkConfig>,
    chain_id: u64,
) -> (EthereumWallet, Vec<Address>) {
    let mut wallet: Option<EthereumWallet> = None;
    let mut signers: Vec<Address> = Vec::new();
    let mut register = |signer: Box<dyn TxSigner<alloy_primitives::Signature> + Send + Sync>,
                        kind: &str| {
        let address = signer.address();
        assert!(
            !signers.contains(&address),
            "signer {address} is configured twice"
        );
        info!(chain_id, signer = %address, kind, "configured signer");
        match wallet.as_mut() {
            None => wallet = Some(EthereumWallet::new(signer)),
            Some(wallet) => wallet.register_signer(signer),
        }
        signers.push(address);
    };
    match config.signer() {
        SignerConfig::Local(keys) => {
            for key in keys {
                let signer: PrivateKeySigner =
                    key.parse().expect("invalid PAYDAY_SIGNER_KEYS entry");
                register(Box::new(signer), "local");
            }
        }
        SignerConfig::AwsKms(key_ids) => {
            let kms = aws_sdk_kms::Client::new(aws.expect("AWS configuration is loaded for KMS"));
            for key_id in key_ids {
                let signer = AwsSigner::new(kms.clone(), key_id.clone(), Some(chain_id))
                    .await
                    .unwrap_or_else(|error| {
                        panic!("failed to initialize KMS key {key_id}: {error}")
                    });
                register(Box::new(signer), "aws-kms");
            }
        }
    }
    (
        wallet.expect("the signer pool has at least one key"),
        signers,
    )
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await.expect("ctrl-c handler");
}
