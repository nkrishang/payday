mod api;
mod config;
mod state;

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
    let state = state::AppState::new(
        repo,
        accounts,
        identity_verifier,
        config.chain_id(),
        config.factory_address(),
        config.usdc_address(),
    );

    let app = api::router(state);

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
