mod api;
mod config;
mod dispatcher;
mod state;
mod webhook_worker;

use axum::serve;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = config::Config::from_env();

    let pool = if config.status_only() {
        gateway_db::connect_lazy(config.database_url()).expect("invalid database URL")
    } else {
        gateway_db::connect(config.database_url())
            .await
            .expect("failed to connect to database")
    };

    let repo = gateway_db::InvoiceRepository::new(pool.clone());
    let accounts = gateway_db::AccountRepository::new(pool.clone());
    let cursor = gateway_db::CursorRepository::new(pool.clone());
    let health_cursor = cursor.clone();
    let notifications = gateway_db::NotificationRepository::new(pool.clone());
    let identity_verifier = match config.auth0() {
        Some(auth0) => Some(
            api::Auth0Verifier::new(
                auth0.issuer.clone(),
                auth0.audience.clone(),
                auth0.client_id.clone(),
            )
            .await
            .expect("failed to initialize Auth0 JWT verification"),
        ),
        None => None,
    };
    let payer = api::payer::PayerAccess::new(
        config.public_base_url(),
        config.explorer_base_url().map(str::to_owned),
        config.payer_token_secret(),
    )
    .expect("invalid payer link configuration");
    let state = state::AppState::new(
        repo,
        accounts,
        identity_verifier,
        config.chain_id(),
        config.factory_address(),
        config.usdc_address(),
        payer,
        config.api_key_prefix().to_owned(),
        config.webhook_encryption_key(),
        config.status_stale_seconds(),
    );
    if !config.status_only()
        && let Some(key) = state.webhook_encryption_key
    {
        tokio::spawn(webhook_worker::run(state.webhooks.clone(), key));
    } else if !config.status_only() {
        tracing::warn!(
            "PAYDAY_WEBHOOK_ENCRYPTION_KEY is unset; webhook API and delivery are disabled"
        );
    }

    let app = if config.status_only() {
        api::status_router(state)
    } else {
        api::router(state)
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let api_health = if !config.status_only() {
        let mut shutdown = shutdown_rx.clone();
        let chain_id = config.chain_id().0;
        Some(tokio::spawn(async move {
            loop {
                if let Err(error) = health_cursor.record_api_health(chain_id).await {
                    tracing::error!(%error, "API health heartbeat failed");
                }
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {},
                    _ = shutdown.changed() => break,
                }
            }
        }))
    } else {
        None
    };
    let dispatcher = if !config.status_only() {
        if let Some(from) = config.notification_from_address() {
            let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
            Some(tokio::spawn(dispatcher::run(
                notifications,
                aws_sdk_sesv2::Client::new(&aws),
                from.to_owned(),
                shutdown_rx,
            )))
        } else {
            None
        }
    } else {
        None
    };

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
    if let Some(worker) = api_health {
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
