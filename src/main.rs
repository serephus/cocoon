#![forbid(unsafe_code)]

mod clock;
mod config;
mod db;
mod error;
mod id;
mod routes;

use std::sync::Arc;

use anyhow::Context;
use diesel::SqliteConnection;
use diesel::r2d2::{ConnectionManager, Pool};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use tokio::net::TcpListener;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// Embedded migrations, applied at startup.
pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

/// Shared application state handed to every route.
#[derive(Clone)]
pub struct AppState {
    pub pool: Pool<ConnectionManager<SqliteConnection>>,
    pub secret: Arc<Vec<u8>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = config::Config::from_env()?;
    let pool = db::build_pool(&config.db_path)?;

    {
        let mut conn = pool
            .get()
            .context("acquiring a database connection for migrations")?;
        conn.run_pending_migrations(MIGRATIONS)
            .map_err(|e| anyhow::anyhow!("running migrations: {e}"))?;
    }

    let state = AppState {
        pool,
        secret: Arc::new(config.secret),
    };

    let app = routes::router(state);
    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;

    tracing::info!("cocoon listening on http://{}", config.bind);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
