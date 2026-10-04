#![forbid(unsafe_code)]

mod api;
mod bot;
mod clock;
mod config;
mod error;
mod id;
mod listing;
mod server;
mod state;
mod store;
mod user;
mod web;

use std::sync::Arc;

use anyhow::Context;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use teloxide::prelude::Requester;
use tokio::net::TcpListener;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = config::Config::from_env()?;
    let pool = store::build_pool(&config.db_path)?;
    {
        let mut conn = pool.get().context("database connection")?;
        conn.run_pending_migrations(MIGRATIONS)
            .map_err(|e| anyhow::anyhow!("running migrations: {e}"))?;
    }

    let bot = bot::messenger::build_bot(&config)?;
    // Only ask Telegram who we are when we will actually need the handle
    // (sharing links / registering the webhook). Keeps offline runs quiet.
    let bot_username = if config.register_webhook || config.public_url.is_some() {
        match bot.get_me().await {
            Ok(me) => me.username.clone().unwrap_or_default(),
            Err(e) => {
                tracing::warn!("get_me failed: {e}");
                String::new()
            }
        }
    } else {
        String::new()
    };

    if config.register_webhook {
        match bot::registration::register(&bot, &config).await {
            Ok(()) => tracing::info!(
                "webhook registered at {}",
                config.webhook_url().unwrap_or_default()
            ),
            Err(e) => tracing::error!("failed to register webhook: {e}"),
        }
    }

    let messenger: Arc<dyn bot::Messenger> = Arc::new(bot::messenger::TelegramMessenger { bot });
    let state = state::AppState {
        pool,
        config: Arc::new(config.clone()),
        messenger,
        bot_username: Arc::from(bot_username.as_str()),
    };

    tokio::spawn(bot::reveal::run(state.clone()));

    let bind = config.bind;
    let app = server::app(state);
    let listener = TcpListener::bind(bind).await.context("binding listener")?;
    tracing::info!("cocoon listening on http://{bind}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
