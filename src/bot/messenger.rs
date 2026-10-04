use async_trait::async_trait;
use teloxide::Bot;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, ChatId, InlineKeyboardButton, InlineKeyboardMarkup};

use crate::config::Config;
use crate::error::AppError;

/// One inline keyboard button.
#[derive(Debug, Clone)]
pub struct Button {
    pub label: String,
    pub data: String,
}

impl Button {
    pub fn new(label: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            data: data.into(),
        }
    }
}

/// The subset of Telegram interactions the bot logic needs.
///
/// Kept as a trait so the command/wizard logic can be tested with a fake.
#[async_trait]
pub trait Messenger: Send + Sync {
    /// Send a plain-text message.
    async fn send(&self, chat_id: i64, text: &str) -> Result<(), AppError>;

    /// Send a plain-text message with inline buttons.
    async fn send_with_buttons(
        &self,
        chat_id: i64,
        text: &str,
        buttons: &[Vec<Button>],
    ) -> Result<(), AppError>;

    /// Answer a callback query; `text` is an optional toast.
    async fn answer_callback(&self, callback_id: &str, text: Option<&str>) -> Result<(), AppError>;
}

/// Build a teloxide bot, applying the optional explicit proxy.
pub fn build_bot(config: &Config) -> anyhow::Result<Bot> {
    // Disable ambient `HTTPS_PROXY`/`ALL_PROXY` detection: a proxy is used only
    // when explicitly configured.
    let mut builder = teloxide::net::default_reqwest_settings().no_proxy();
    if let Some(proxy) = &config.proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy)?);
    }
    let client = builder.build()?;
    let mut bot = Bot::with_client(config.bot_token.clone(), client);
    if let Some(api_url) = &config.api_url {
        bot = bot.set_api_url(
            reqwest::Url::parse(api_url)
                .map_err(|e| anyhow::anyhow!("invalid COCOON_TELEGRAM_API_URL: {e}"))?,
        );
    }
    Ok(bot)
}

/// Talks to the real Telegram Bot API.
pub struct TelegramMessenger {
    pub bot: Bot,
}

#[async_trait]
impl Messenger for TelegramMessenger {
    async fn send(&self, chat_id: i64, text: &str) -> Result<(), AppError> {
        self.bot.send_message(ChatId(chat_id), text).await?;
        Ok(())
    }

    async fn send_with_buttons(
        &self,
        chat_id: i64,
        text: &str,
        buttons: &[Vec<Button>],
    ) -> Result<(), AppError> {
        let rows: Vec<Vec<InlineKeyboardButton>> = buttons
            .iter()
            .map(|row| {
                row.iter()
                    .map(|button| {
                        InlineKeyboardButton::callback(button.label.clone(), button.data.clone())
                    })
                    .collect()
            })
            .collect();
        let keyboard = InlineKeyboardMarkup::new(rows);
        self.bot
            .send_message(ChatId(chat_id), text)
            .reply_markup(keyboard)
            .await?;
        Ok(())
    }

    async fn answer_callback(&self, callback_id: &str, text: Option<&str>) -> Result<(), AppError> {
        let request = self
            .bot
            .answer_callback_query(CallbackQueryId(callback_id.to_string()));
        match text {
            Some(text) => request.text(text).await?,
            None => request.await?,
        };
        Ok(())
    }
}
