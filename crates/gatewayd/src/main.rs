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
    let state = state::AppState::new(repo, config.chain_id(), config.factory_address());

    let app = api::router(state);

    let listener = TcpListener::bind(config.bind_addr())
        .await
        .expect("failed to bind");

    tracing::info!("listening on {}", config.bind_addr());

    // Serve until Ctrl-C.
    let server = serve(listener, app).with_graceful_shutdown(async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
        tracing::info!("shutdown signal received");
    });

    if let Err(e) = server.await {
        tracing::error!(error = %e, "server error");
    }
}
