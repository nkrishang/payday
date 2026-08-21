mod api;
mod config;
mod db;
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

    let pool = db::connect(config.database_url())
        .await
        .expect("failed to connect to database");

    let repo = db::InvoiceRepository::new(pool);
    let state = state::AppState::new(repo, config.chain_id(), config.factory_address());

    let app = api::router(state);

    let listener = TcpListener::bind(config.bind_addr())
        .await
        .expect("failed to bind");

    tracing::info!("listening on {}", config.bind_addr());

    serve(listener, app).await.expect("server error");
}
