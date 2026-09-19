//! `gum-server`: the API and the only process that touches the ledger.
//!
//! Besides serving the public API it runs every ledger-side loop of the
//! deposit-request and withdrawal lifecycles: the internal RPC the indexer
//! reports into, the sweep scheduler, the withdrawal orchestrator, the
//! consumer of the signers' evidence, and the notification and webhook
//! workers. All of them are restartable at any instant: their state is the
//! database and the bus, never process memory.
//!
//! Subcommands (`gum-server <command>`): `migrate`, `bus dead [limit]`,
//! `bus retry <message-id>`, `chain resume <chain-id>`.

mod api;
mod attachments;
mod attestation;
mod chain_reader;
mod cli;
mod config;
#[cfg(test)]
mod deposit_lifecycle_tests;
mod dispatcher;
mod execution_events;
mod internal_rpc;
mod iris;
mod onboarding_payer;
mod payer_email;
mod payer_identity;
mod pregenerated_wallet;
mod relay_intents;
mod request_pdf;
mod state;
mod sweep_scheduler;
mod webhook_worker;
mod withdrawal_orchestrator;

use std::collections::HashMap;
use std::sync::Arc;

use axum::serve;
use gum_bus::{Consumer, ConsumerOptions, Publisher};
use gum_chain::{AlloyChainClient, ChainReader};
use gum_contracts::ExecutionEvent;
use gum_telemetry::health::{DatabaseReady, ReadinessCheck};
use tokio::net::TcpListener;
use tokio::sync::Notify;

/// The bus consumer name of this service; deliveries are tracked per name.
const CONSUMER_NAME: &str = "gum-server";
/// RPC calls per second the server allows itself per chain; it only reads
/// headers, receipts and balances.
const RPC_MAX_RPS: u64 = 10;

#[tokio::main]
async fn main() {
    gum_telemetry::init("gum-server");

    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = gum_ledger::connect(&database_url, 4)
            .await
            .expect("failed to connect to database");
        match cli::run(&pool, &args).await {
            Ok(()) => return,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
    }

    let config = config::Config::from_env();

    let pool = gum_ledger::connect(config.database_url(), 16)
        .await
        .expect("failed to connect to database");

    let repo = gum_ledger::InvoiceRepository::new(pool.clone());
    let accounts = gum_ledger::AccountRepository::new(pool.clone());
    let notifications = gum_ledger::NotificationRepository::new(pool.clone());
    // Privy key discovery and AWS provider-chain loading are independent
    // network work. Start them together, and retain one AWS configuration for
    // S3, KMS, and SES.
    let privy_app_id = config.privy().map(|privy| privy.app_id.clone());
    let (merchant_verifier, aws) = tokio::join!(
        async move {
            match privy_app_id {
                Some(app_id) => Some(
                    api::PrivyVerifier::new(app_id)
                        .await
                        .expect("failed to initialize Privy identity token verification"),
                ),
                None => {
                    tracing::warn!(
                        "GUM_PRIVY_APP_ID is unset; dashboard sessions are refused and only API keys authenticate"
                    );
                    None
                }
            }
        },
        aws_config::load_defaults(aws_config::BehaviorVersion::latest())
    );
    let attachments = config.attachments();
    let storage = attachments::S3ObjectStorage::new(
        &aws,
        attachments.bucket.clone(),
        attachments.s3_endpoint.as_deref(),
        attachments.force_path_style,
    );
    let attachment_store =
        attachments::AttachmentStore::new(Arc::new(storage), attachments.download_ttl);
    let attestor = attestation::VerificationAttestor::from_config(config.attestation(), &aws)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    tracing::info!(address = %attestor.address(), "configured attestation signer");
    // Absent whenever a deployment hasn't deliberately funded and configured
    // this — production may never set it.
    let onboarding_chain = config
        .networks()
        .get(config.onboarding_chain_id())
        .expect("the onboarding chain is registered");
    let onboarding_payer = match config.onboarding_payer() {
        Some(signer_config) => Some(
            onboarding_payer::OnboardingPayerSigner::from_config(
                signer_config,
                &aws,
                config.rpc_url(onboarding_chain.chain_id),
                onboarding_chain.chain_id,
                onboarding_chain
                    .token(gum_core::Currency::Usdc)
                    .unwrap_or_else(|| {
                        panic!(
                            "the onboarding chain {} does not serve USDC; the demo pays USDC",
                            onboarding_chain.chain_id
                        )
                    })
                    .address,
            )
            .await
            .unwrap_or_else(|error| panic!("{error}")),
        ),
        None => None,
    };
    if let Some(onboarding_payer) = &onboarding_payer {
        tracing::info!(address = %onboarding_payer.address(), chain_id = onboarding_chain.chain_id, "configured onboarding payer signer");
    }
    // Absent whenever GUM_PRIVY_APP_SECRET isn't set — a latency
    // optimization, not a dependency: sign-in still creates a merchant's
    // wallet itself either way (config.rs's PrivyConfig doc comment).
    let pregenerated_wallets: Option<Arc<dyn pregenerated_wallet::WalletPregenerator>> =
        match config.privy().and_then(|privy| privy.app_secret.clone()) {
            Some(app_secret) => {
                let app_id = config
                    .privy()
                    .expect("app_secret is only set alongside app_id")
                    .app_id
                    .clone();
                Some(Arc::new(
                    pregenerated_wallet::PrivyPregeneration::new(app_id, app_secret)
                        .unwrap_or_else(|error| panic!("{error}")),
                )
                    as Arc<dyn pregenerated_wallet::WalletPregenerator>)
            }
            None => None,
        };
    if pregenerated_wallets.is_some() {
        tracing::info!("configured Privy wallet pregeneration");
    }
    let payer = api::payer::PayerAccess::new(
        config.public_base_url(),
        config
            .networks()
            .chains()
            .iter()
            .filter_map(|chain| Some((chain.chain_id, chain.explorer_base_url.clone()?)))
            .collect(),
        config.hosted_checkout_origin().map(str::to_owned),
    )
    .expect("invalid payer link configuration");
    // The payer audience is its own Auth0 API and application (product plan
    // §6.1).
    let payer_verification = match config.payer_verification() {
        Some(payer_auth) => {
            let verifier = api::Auth0Verifier::new(
                payer_auth.issuer.clone(),
                payer_auth.audience.clone(),
                payer_auth.client_id.clone(),
                config.dev_identity(),
            )
            .await
            .expect("failed to initialize payer Auth0 JWT verification");
            let otp = payer_identity::Auth0Passwordless::new(
                &payer_auth.issuer,
                payer_auth.client_id.clone(),
                payer_auth.audience.clone(),
            )
            .expect("failed to initialize payer Auth0 passwordless client");
            Some(payer_identity::PayerVerification::new(
                Arc::new(otp),
                verifier,
                payer_auth.payer_ref_master_key,
            ))
        }
        None => {
            tracing::warn!(
                "GUM_PAYER_AUTH0_* and GUM_PAYER_REF_MASTER_KEY are unset; gated invoices cannot be verified"
            );
            None
        }
    };
    // Every payment address this service hands out assumes the reviewed
    // contract generation on every chain it offers, so refuse to serve
    // against any other deployment anywhere. The chains are independent
    // endpoints, so they are checked together.
    let chain_clients: HashMap<u64, Arc<dyn ChainReader>> =
        futures::future::try_join_all(config.networks().chains().iter().map(|chain| async {
            let client = AlloyChainClient::connect(config.rpc_url(chain.chain_id), RPC_MAX_RPS)
                .await
                .map_err(|error| format!("chain {}: {error}", chain.chain_id))?;
            let expected = gum_core::ExpectedDeployment {
                chain_id: chain.chain_id,
                factory: chain.factory,
                factory_code_hash: chain.factory_code_hash,
                batch_sweeper: chain.batch_sweeper,
                batch_sweeper_code_hash: chain.batch_sweeper_code_hash,
                forwarder: chain
                    .cctp
                    .as_ref()
                    .map(|cctp| (cctp.forwarder, cctp.forwarder_code_hash)),
            };
            gum_chain::verify_deployment(&client, &expected)
                .await
                .map_err(|error| format!("chain {}: {error}", chain.chain_id))?;
            Ok::<_, String>((chain.chain_id, Arc::new(client) as Arc<dyn ChainReader>))
        }))
        .await
        .unwrap_or_else(|error| panic!("contract deployment verification failed: {error}"))
        .into_iter()
        .collect();
    // Withdrawals read balances and each token's EIP-712 domain over the
    // same RPC endpoints; a domain the token disowns is a startup failure,
    // never a signature the relayer discovers is worthless.
    let chain_reader = chain_reader::AlloyChainReader::connect(config.networks(), |chain_id| {
        config.rpc_url(chain_id).to_owned()
    })
    .await
    .unwrap_or_else(|error| panic!("token domain read failed: {error}"));
    // Cross-chain payments through Relay need its API key on every quote;
    // without one the hosted checkout simply does not offer them.
    let relay: Option<Arc<dyn gum_relay::RelayApi>> = match config.relay_api_key() {
        Some(key) => Some(Arc::new(
            gum_relay::RelayClient::new(config.relay_url(), key)
                .unwrap_or_else(|error| panic!("failed to build the Relay client: {error}")),
        )),
        None => {
            tracing::warn!(
                "GUM_RELAY_API_KEY is unset; paying from another network is unavailable"
            );
            None
        }
    };
    let state = state::AppState::new(
        repo,
        accounts,
        merchant_verifier,
        Arc::new(config.networks().clone()),
        config.recovery_address(),
        payer,
        config.api_key_prefix().to_owned(),
        config.webhook_encryption_key(),
        Some(attachment_store),
        Some(attestor),
        payer_verification,
        onboarding_payer,
        pregenerated_wallets,
        Some(Arc::new(chain_reader)),
        relay,
    );
    if let Some(key) = state.webhook_encryption_key {
        tokio::spawn(webhook_worker::run(state.webhooks.clone(), key));
    } else {
        tracing::warn!(
            "GUM_WEBHOOK_ENCRYPTION_KEY is unset; webhook API and delivery are disabled"
        );
    }

    // The dispatcher renders the payer's link and reads the request it is
    // about through the same repositories the routes use.
    let (invoices, payer_access) = (state.repo.clone(), state.payer.clone());
    let withdrawals = state.withdrawals.clone();
    let relay_intents = state.relay_intents.clone();
    let relay_api = state.relay.as_ref().map(|service| service.shared_api());
    let networks = state.networks.clone();
    let app = api::router(state);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // --- The ledger-side lifecycle loops. Each is a pure function of the
    // database and the bus; killing and restarting the process at any point
    // resumes every one of them where the last commit left it.
    let publisher = Publisher::new(CONSUMER_NAME);
    let sweep_wake = Arc::new(Notify::new());
    let sweep_policy = gum_ledger::SweepPolicy::default();
    let step_policy = gum_ledger::StepPolicy::default();

    let internal = internal_rpc::router(internal_rpc::InternalState::new(
        invoices.clone(),
        gum_ledger::ChainFaultRepository::new(pool.clone(), publisher),
        networks.clone(),
        config.internal_token(),
        sweep_wake.clone(),
    ));
    let checks: Vec<Arc<dyn ReadinessCheck>> = vec![Arc::new(DatabaseReady {
        pool: pool.clone(),
        migrator: Some(&gum_schema::MIGRATOR),
    })];
    let internal_app = axum::Router::new()
        .nest("/internal/v1/indexer", internal)
        .merge(gum_telemetry::health::router(checks));
    let internal_listener = TcpListener::bind(config.internal_bind_addr())
        .await
        .expect("failed to bind the internal listener");
    tracing::info!(
        addr = config.internal_bind_addr(),
        "internal RPC and health listening"
    );
    let mut internal_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        if let Err(error) = serve(internal_listener, internal_app)
            .with_graceful_shutdown(async move {
                let _ = internal_shutdown.wait_for(|stop| *stop).await;
            })
            .await
        {
            tracing::error!(error = %error, "internal listener failed");
        }
    });

    tokio::spawn(gum_bus::run_consumer::<ExecutionEvent, _>(
        Consumer::new(CONSUMER_NAME, pool.clone(), ConsumerOptions::default()),
        execution_events::ExecutionEventHandler::new(
            invoices.clone(),
            withdrawals.clone(),
            sweep_policy,
            step_policy,
        ),
        shutdown_rx.clone(),
    ));
    tokio::spawn(
        sweep_scheduler::SweepScheduler::new(
            invoices.clone(),
            publisher,
            networks.clone(),
            sweep_policy,
            config.sweep_scheduler_interval(),
            sweep_wake,
        )
        .run(shutdown_rx.clone()),
    );
    let attestations: Arc<dyn iris::AttestationSource> = Arc::new(
        iris::IrisClient::new(config.iris_url()).unwrap_or_else(|error| panic!("{error}")),
    );
    tokio::spawn(
        withdrawal_orchestrator::WithdrawalOrchestrator::new(
            withdrawals,
            pool.clone(),
            networks.clone(),
            publisher,
            attestations,
            step_policy,
            chain_clients.clone(),
        )
        .run(shutdown_rx.clone()),
    );
    tokio::spawn(
        relay_intents::RelayIntentPoller::new(relay_intents, relay_api, chain_clients, networks)
            .run(shutdown_rx.clone()),
    );
    let senders = dispatcher::Senders {
        merchant: config
            .notification_from_address()
            .map(|from| (aws_sdk_sesv2::Client::new(&aws), from.to_owned())),
        payer: config.resend().map(|resend| {
            payer_email::ResendClient::new(resend.api_key.clone(), resend.from.clone())
        }),
    };
    if senders.payer.is_none() {
        tracing::warn!(
            "GUM_RESEND_API_KEY is unset; payers named on a deposit request are not emailed"
        );
    }
    let dispatcher = (!senders.recipients().is_empty()).then(|| {
        tokio::spawn(dispatcher::run(
            notifications,
            invoices,
            payer_access,
            senders,
            shutdown_rx,
        ))
    });

    let listener = TcpListener::bind(config.bind_addr())
        .await
        .expect("failed to bind");

    tracing::info!("listening on {}", config.bind_addr());

    // ECS stops tasks with SIGTERM; local runs use Ctrl-C.
    let server = serve(listener, app).with_graceful_shutdown(async {
        shutdown_signal().await;
        tracing::info!("shutdown signal received");
    });

    if let Err(e) = server.await {
        tracing::error!(error = %e, "server error");
    }
    let _ = shutdown_tx.send(true);
    if let Some(worker) = dispatcher {
        let _ = worker.await;
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
