mod live;
mod rest;
mod ws;

use anyhow::Result;
use axum::Router;
use axum::routing::get;
use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

#[derive(Parser)]
#[command(about = "Live ingest from CS2 plugins, UI WebSockets and REST")]
struct Cli {
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,
    #[arg(long, env = "LISTEN", default_value = "0.0.0.0:8080")]
    listen: SocketAddr,
    #[arg(long, env = "REPLAY_DIR", default_value = "data/replays")]
    replay_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(8).connect(&cli.database_url).await?;
    let state = Arc::new(live::AppState {
        pool,
        replay_dir: cli.replay_dir,
        workers: Default::default(),
    });

    let app = Router::new()
        .route("/health", get(rest::health))
        .route("/api/matches", get(rest::list_matches))
        .route("/api/matches/{id}", get(rest::get_match))
        .route("/api/matches/{id}/replay", get(rest::replay))
        .route("/api/teams", get(rest::list_teams))
        .route("/api/stats", get(rest::stats))
        .route("/ws/plugin", get(ws::plugin))
        .route("/ws/live/{id}", get(ws::live))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(cli.listen).await?;
    tracing::info!(addr = %cli.listen, "api listening");
    axum::serve(listener, app).with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
