use std::sync::Arc;

use crate::bot::Messenger;
use crate::config::Config;
use crate::store::DbPool;

/// Shared application state handed to the webhook handler and background tasks.
#[derive(Clone)]
pub struct AppState {
    pub pool: DbPool,
    pub config: Arc<Config>,
    pub messenger: Arc<dyn Messenger>,
    /// The bot's @username, used to build deep links (empty if unknown).
    pub bot_username: Arc<str>,
}
