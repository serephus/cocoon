use teloxide::Bot;
use teloxide::prelude::*;
use teloxide::types::{AllowedUpdate, BotCommand};

use crate::config::Config;
use crate::error::AppError;

/// Register the webhook and the command menu with Telegram.
pub async fn register(bot: &Bot, config: &Config) -> Result<(), AppError> {
    let url = config
        .webhook_url()
        .ok_or_else(|| AppError::Internal("no public URL configured".to_string()))?;
    let url = reqwest::Url::parse(&url)
        .map_err(|e| AppError::Internal(format!("invalid webhook URL: {e}")))?;

    bot.set_webhook(url)
        .secret_token(config.webhook_secret.clone())
        .allowed_updates(vec![AllowedUpdate::Message, AllowedUpdate::CallbackQuery])
        .await?;

    bot.set_my_commands(commands()).await?;
    Ok(())
}

fn commands() -> Vec<BotCommand> {
    vec![
        BotCommand::new("new", "Create a time-locked paste"),
        BotCommand::new("list", "List your pastes"),
        BotCommand::new("show", "Show one of your pastes"),
        BotCommand::new("edit", "Edit a paste before it is revealed"),
        BotCommand::new("delete", "Delete a paste before it is revealed"),
        BotCommand::new("share", "Get a shareable link"),
        BotCommand::new("get", "Open a shared paste"),
        BotCommand::new("help", "Show help"),
    ]
}
