mod api;
mod attachments;
mod attestation;
mod config;
mod deployment;
mod dispatcher;
mod onboarding_payer;
mod payer_email;
mod payer_identity;
mod pregenerated_wallet;
mod request_pdf;
mod state;
mod webhook_worker;

use std::sync::Arc;

use axum::serve;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = config::Config::from_env();

    let pool = gateway_db::connect(config.database_url())
        .await
        .expect("failed to connect to database");

    let repo = gateway_db::InvoiceRepository::new(pool.clone());
    let accounts = gateway_db::AccountRepository::new(pool.clone());
    let notifications = gateway_db::NotificationRepository::new(pool.clone());
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
                        "PAYDAY_PRIVY_APP_ID is unset; dashboard sessions are refused and only API keys authenticate"
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
    let onboarding_payer = match config.onboarding_payer() {
        Some(signer_config) => Some(
            onboarding_payer::OnboardingPayerSigner::from_config(
                signer_config,
                &aws,
                &config.settlement().rpc_url,
                config.chain_id().0,
            )
            .await
            .unwrap_or_else(|error| panic!("{error}")),
        ),
        None => None,
    };
    if let Some(onboarding_payer) = &onboarding_payer {
        tracing::info!(address = %onboarding_payer.address(), "configured onboarding payer signer");
    }
    // Absent whenever PAYDAY_PRIVY_APP_SECRET isn't set — a latency
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
        config.explorer_base_url().map(str::to_owned),
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
                "PAYDAY_PAYER_AUTH0_* and PAYDAY_PAYER_REF_MASTER_KEY are unset; gated invoices cannot be verified"
            );
            None
        }
    };
    // Every payment address this service hands out assumes the reviewed
    // contract generation, so refuse to serve against any other deployment.
    let settlement = config.settlement();
    deployment::verify_deployment(
        &settlement.rpc_url,
        &deployment::ExpectedDeployment {
            chain_id: config.chain_id().0,
            factory: config.factory_address(),
            factory_code_hash: settlement.factory_code_hash,
            batch_sweeper: settlement.batch_sweeper_address,
            batch_sweeper_code_hash: settlement.batch_sweeper_code_hash,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("contract deployment verification failed: {error}"));
    let state = state::AppState::new(
        repo,
        accounts,
        merchant_verifier,
        config.chain_id(),
        config.factory_address(),
        config.usdc_address(),
        payer,
        config.api_key_prefix().to_owned(),
        config.webhook_encryption_key(),
        Some(attachment_store),
        Some(attestor),
        payer_verification,
        onboarding_payer,
        pregenerated_wallets,
    );
    if let Some(key) = state.webhook_encryption_key {
        tokio::spawn(webhook_worker::run(state.webhooks.clone(), key));
    } else {
        tracing::warn!(
            "PAYDAY_WEBHOOK_ENCRYPTION_KEY is unset; webhook API and delivery are disabled"
        );
    }

    // The dispatcher renders the payer's link and reads the request it is
    // about through the same repositories the routes use.
    let (invoices, payer_access) = (state.repo.clone(), state.payer.clone());
    let app = api::router(state);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
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
            "PAYDAY_RESEND_API_KEY is unset; payers named on a deposit request are not emailed"
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
