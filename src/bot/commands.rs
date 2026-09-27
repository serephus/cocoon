use teloxide::types::{CallbackQuery, Update, UpdateKind};

use crate::bot::messenger::Button;
use crate::clock::{format_rfc3339, now_unix, parse_publish_at};
use crate::error::AppError;
use crate::state::AppState;
use crate::store::{
    self,
    models::{Conversation, NewConversation, Paste},
};
use crate::user::{User, can_manage, can_read};

/// Telegram's per-message limit.
pub const MAX_CONTENT: usize = crate::store::MAX_CONTENT;
pub const MAX_TITLE: usize = 256;
const PAGE_SIZE: usize = 20;

const HELP: &str = "\
Commands:
/new - create a paste
/list - list every paste
/mine - list your pastes
/show <id> - show a paste
/edit <id> - edit one of your pastes before it is revealed
/delete <id> - delete one of your pastes before it is revealed
/share <id> - get a shareable link
/subscribe <id> - get notified when a paste is revealed
/unsubscribe <id> - stop notifications
/subscriptions - list your subscriptions
/cancel - cancel the current wizard";

pub async fn handle_update(state: &AppState, update: Update) -> Result<(), AppError> {
    match update.kind {
        UpdateKind::Message(message) => {
            let Some(from) = message.from.as_ref() else {
                return Ok(());
            };
            let Some(text) = message.text() else {
                return Ok(());
            };
            handle_text(state, from.id.0 as i64, message.chat.id.0, text).await
        }
        UpdateKind::CallbackQuery(query) => handle_callback(state, query).await,
        _ => Ok(()),
    }
}

/// Handle a text message. Public so tests can drive the bot without Telegram.
pub async fn handle_text(
    state: &AppState,
    user_id: i64,
    chat_id: i64,
    text: &str,
) -> Result<(), AppError> {
    // Continue an active wizard unless the user typed a command.
    let conversation = store::run(&state.pool, move |conn| {
        Ok(store::get_conversation(conn, user_id)?)
    })
    .await?;
    if let Some(conversation) = conversation {
        if text.trim() == "/cancel" {
            store::run(&state.pool, move |conn| {
                store::delete_conversation(conn, user_id)?;
                Ok(())
            })
            .await?;
            return send(state, chat_id, "Cancelled.").await;
        }
        if !text.starts_with('/') || text.trim() == "/skip" {
            return handle_wizard(state, user_id, chat_id, conversation, text).await;
        }
    }

    let (command, args) = parse_command(text);
    match command {
        "start" => start(state, chat_id, user_id, args).await,
        "help" => send(state, chat_id, HELP).await,
        "new" => start_new(state, user_id, chat_id).await,
        "cancel" => send(state, chat_id, "Nothing to cancel.").await,
        "list" => {
            list_page(
                state,
                chat_id,
                first_arg(args).and_then(|p| p.parse().ok()).unwrap_or(1),
            )
            .await
        }
        "mine" => mine(state, chat_id, user_id).await,
        "show" => show(state, chat_id, user_id, args).await,
        "edit" => start_edit(state, chat_id, user_id, args).await,
        "delete" => ask_delete(state, chat_id, user_id, args).await,
        "share" => share(state, chat_id, user_id, args).await,
        "subscribe" => subscribe_cmd(state, chat_id, user_id, args).await,
        "unsubscribe" => unsubscribe_cmd(state, chat_id, user_id, args).await,
        "subscriptions" => subscriptions_cmd(state, chat_id, user_id).await,
        _ => send(state, chat_id, HELP).await,
    }
}

async fn handle_callback(state: &AppState, query: CallbackQuery) -> Result<(), AppError> {
    let user_id = query.from.id.0 as i64;
    let callback_id = query.id.0.clone();
    let chat_id = query.message.as_ref().map(|message| message.chat().id.0);
    let data = query.data.clone().unwrap_or_default();

    if let Some(id) = data.strip_prefix("delete:") {
        return confirm_delete(state, user_id, id, &callback_id).await;
    }
    if let Some(page) = data.strip_prefix("list:") {
        if let (Some(chat_id), Ok(page)) = (chat_id, page.parse::<i64>()) {
            list_page(state, chat_id, page).await?;
        }
        return state.messenger.answer_callback(&callback_id, None).await;
    }
    state
        .messenger
        .answer_callback(&callback_id, Some("Unknown action."))
        .await
}

// ---------------------------------------------------------------------------
// Wizard
// ---------------------------------------------------------------------------

async fn handle_wizard(
    state: &AppState,
    user_id: i64,
    chat_id: i64,
    conversation: Conversation,
    text: &str,
) -> Result<(), AppError> {
    match conversation.step.as_str() {
        "content" => {
            let content = if text.trim() == "/skip" {
                if conversation.kind == "edit" {
                    conversation.content.clone()
                } else {
                    send(state, chat_id, "Content cannot be skipped.").await?;
                    return Ok(());
                }
            } else {
                text.to_string()
            };
            if content.chars().count() > MAX_CONTENT {
                return send(
                    state,
                    chat_id,
                    &format!("That is too long; the limit is {MAX_CONTENT} characters."),
                )
                .await;
            }
            save_step(
                state,
                user_id,
                &conversation,
                "publish_at",
                &content,
                None,
                None,
            )
            .await?;
            send(
                state,
                chat_id,
                "Send the publish time in UTC (e.g. `2030-01-01 00:00`), or /skip to publish now.",
            )
            .await
        }
        "publish_at" => {
            let publish_at = if text.trim() == "/skip" {
                match conversation.kind.as_str() {
                    "edit" => conversation.publish_at.unwrap_or_else(now_unix),
                    _ => now_unix(),
                }
            } else {
                match parse_publish_at(text) {
                    Ok(value) => value,
                    Err(message) => return send(state, chat_id, &message).await,
                }
            };
            save_step(
                state,
                user_id,
                &conversation,
                "title",
                &conversation.content.clone(),
                Some(publish_at),
                None,
            )
            .await?;
            send(state, chat_id, "Send a title (optional), or /skip.").await
        }
        "title" => {
            let title = if text.trim() == "/skip" {
                conversation.title.clone()
            } else {
                text.trim().to_string()
            };
            if let Err(message) = validate_title(&title) {
                return send(state, chat_id, &message).await;
            }

            let owner = User::Telegram(user_id);
            let content = conversation.content.clone();
            let publish_at = conversation.publish_at.unwrap_or_else(now_unix);
            let result = if conversation.kind == "edit" {
                let old_id = conversation.paste_id.clone();
                store::run(&state.pool, move |conn| {
                    store::update_paste(conn, &old_id, owner, &content, &title, publish_at)
                })
                .await
                .map(|id| format!("Updated. The paste id is now {id}."))
            } else {
                store::run(&state.pool, move |conn| {
                    store::create_paste(conn, owner, &content, &title, publish_at)
                })
                .await
                .map(|id| {
                    format!(
                        "Created paste {id}.\npublishes: {}",
                        format_rfc3339(publish_at)
                    )
                })
            };

            match result {
                Ok(message) => {
                    store::run(&state.pool, move |conn| {
                        store::delete_conversation(conn, user_id)?;
                        Ok(())
                    })
                    .await?;
                    send(state, chat_id, &message).await
                }
                Err(AppError::Duplicate(id)) => {
                    send(
                        state,
                        chat_id,
                        &format!("You already have an identical paste: {id}"),
                    )
                    .await
                }
                Err(e) => Err(e),
            }
        }
        _ => {
            store::run(&state.pool, move |conn| {
                store::delete_conversation(conn, user_id)?;
                Ok(())
            })
            .await?;
            send(
                state,
                chat_id,
                "Something went wrong; the wizard was cancelled.",
            )
            .await
        }
    }
}

async fn save_step(
    state: &AppState,
    user_id: i64,
    conversation: &Conversation,
    step: &'static str,
    content: &str,
    publish_at: Option<i64>,
    title: Option<&str>,
) -> Result<(), AppError> {
    let kind = conversation.kind.clone();
    let paste_id = conversation.paste_id.clone();
    let content = content.to_string();
    let title = title
        .map(str::to_string)
        .unwrap_or_else(|| conversation.title.clone());
    let publish_at = publish_at.or(conversation.publish_at);
    store::run(&state.pool, move |conn| {
        let next = NewConversation {
            user_id,
            kind: &kind,
            step,
            paste_id: &paste_id,
            content: &content,
            title: &title,
            publish_at,
            updated_at: now_unix(),
        };
        store::set_conversation(conn, &next)?;
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

async fn start(state: &AppState, chat_id: i64, user_id: i64, args: &str) -> Result<(), AppError> {
    if let Some(payload) = first_arg(args) {
        if let Some(id) = payload.strip_prefix("subscribe_") {
            return subscribe_cmd(state, chat_id, user_id, id).await;
        }
        return show(state, chat_id, user_id, payload).await;
    }
    send(
        state,
        chat_id,
        &format!("Welcome to cocoon, a time-locked pastebin.\n\n{HELP}"),
    )
    .await
}

async fn start_new(state: &AppState, user_id: i64, chat_id: i64) -> Result<(), AppError> {
    store::run(&state.pool, move |conn| {
        let next = NewConversation {
            user_id,
            kind: "new",
            step: "content",
            paste_id: "",
            content: "",
            title: "",
            publish_at: None,
            updated_at: now_unix(),
        };
        store::set_conversation(conn, &next)?;
        Ok(())
    })
    .await?;
    send(state, chat_id, "Send the content for your paste.").await
}

async fn start_edit(
    state: &AppState,
    chat_id: i64,
    user_id: i64,
    args: &str,
) -> Result<(), AppError> {
    let Some(id) = first_arg(args) else {
        return send(state, chat_id, "Usage: /edit <id>").await;
    };
    let Some(paste) = fetch(state, id).await? else {
        return send(state, chat_id, "No paste with that id.").await;
    };
    if !can_manage(
        User::Telegram(user_id),
        &paste.owner,
        paste.publish_at,
        now_unix(),
    ) {
        return send(
            state,
            chat_id,
            "You can only edit your own pastes, and only before they are revealed.",
        )
        .await;
    }

    let paste_id = paste.id.clone();
    let content = paste.content.clone();
    let title = paste.title.clone();
    let publish_at = paste.publish_at;
    store::run(&state.pool, move |conn| {
        let next = NewConversation {
            user_id,
            kind: "edit",
            step: "content",
            paste_id: &paste_id,
            content: &content,
            title: &title,
            publish_at: Some(publish_at),
            updated_at: now_unix(),
        };
        store::set_conversation(conn, &next)?;
        Ok(())
    })
    .await?;
    send(
        state,
        chat_id,
        "Send the new content, or /skip to keep the current content.",
    )
    .await
}

async fn list_page(state: &AppState, chat_id: i64, page: i64) -> Result<(), AppError> {
    let page = page.max(1);
    let per = PAGE_SIZE as i64;
    let offset = (page - 1) * per;
    let (pastes, has_next) = store::run(&state.pool, move |conn| {
        let mut rows = store::list_pastes(conn, per + 1, offset)?;
        let has_next = rows.len() as i64 > per;
        if has_next {
            rows.truncate(PAGE_SIZE);
        }
        Ok((rows, has_next))
    })
    .await?;

    if pastes.is_empty() {
        return send(state, chat_id, "No pastes yet. Use /new.").await;
    }

    let now = now_unix();
    let mut lines = vec![format!("Pastes (page {page}):")];
    for paste in &pastes {
        lines.push(format_row(paste, now));
    }

    let mut row = Vec::new();
    if page > 1 {
        row.push(Button::new("« prev", format!("list:{}", page - 1)));
    }
    if has_next {
        row.push(Button::new("next »", format!("list:{}", page + 1)));
    }
    let buttons = if row.is_empty() {
        Vec::new()
    } else {
        vec![row]
    };
    let text = lines.join("\n");
    if buttons.is_empty() {
        send(state, chat_id, &text).await
    } else {
        state
            .messenger
            .send_with_buttons(chat_id, &text, &buttons)
            .await
    }
}

async fn mine(state: &AppState, chat_id: i64, user_id: i64) -> Result<(), AppError> {
    let owner = User::Telegram(user_id).storage_key();
    let pastes = store::run(&state.pool, move |conn| {
        Ok(store::list_owner_pastes(conn, &owner)?)
    })
    .await?;
    if pastes.is_empty() {
        return send(state, chat_id, "You have no pastes yet. Use /new.").await;
    }
    let now = now_unix();
    let mut lines = vec![format!("Your pastes ({}):", pastes.len())];
    for paste in pastes.iter().take(50) {
        lines.push(format_row(paste, now));
    }
    send(state, chat_id, &lines.join("\n")).await
}

async fn show(state: &AppState, chat_id: i64, user_id: i64, args: &str) -> Result<(), AppError> {
    let Some(id) = first_arg(args) else {
        return send(state, chat_id, "Usage: /show <id>").await;
    };
    let Some(paste) = fetch(state, id).await? else {
        return send(state, chat_id, "No paste with that id.").await;
    };
    if can_read(
        User::Telegram(user_id),
        &paste.owner,
        paste.publish_at,
        now_unix(),
    ) {
        send_paste(state, chat_id, &paste).await
    } else {
        send(state, chat_id, "That paste is not revealed yet.").await
    }
}

async fn share(state: &AppState, chat_id: i64, user_id: i64, args: &str) -> Result<(), AppError> {
    let Some(id) = first_arg(args) else {
        return send(state, chat_id, "Usage: /share <id>").await;
    };
    let Some(paste) = fetch(state, id).await? else {
        return send(state, chat_id, "No paste with that id.").await;
    };
    if !can_read(
        User::Telegram(user_id),
        &paste.owner,
        paste.publish_at,
        now_unix(),
    ) {
        return send(state, chat_id, "That paste is not revealed yet.").await;
    }
    if state.bot_username.is_empty() {
        return send(state, chat_id, "Sharing is unavailable right now.").await;
    }
    let link = format!("https://t.me/{}?start={}", state.bot_username, paste.id);
    send(state, chat_id, &format!("Share this link:\n{link}")).await
}

async fn ask_delete(
    state: &AppState,
    chat_id: i64,
    user_id: i64,
    args: &str,
) -> Result<(), AppError> {
    let Some(id) = first_arg(args) else {
        return send(state, chat_id, "Usage: /delete <id>").await;
    };
    let Some(paste) = fetch(state, id).await? else {
        return send(state, chat_id, "No paste with that id.").await;
    };
    if !can_manage(
        User::Telegram(user_id),
        &paste.owner,
        paste.publish_at,
        now_unix(),
    ) {
        return send(
            state,
            chat_id,
            "You can only delete your own pastes, and only before they are revealed.",
        )
        .await;
    }
    let buttons = vec![vec![Button::new("Delete", format!("delete:{}", paste.id))]];
    state
        .messenger
        .send_with_buttons(
            chat_id,
            &format!("Delete paste {}? This cannot be undone.", paste.id),
            &buttons,
        )
        .await
}

async fn subscribe_cmd(
    state: &AppState,
    chat_id: i64,
    user_id: i64,
    args: &str,
) -> Result<(), AppError> {
    let Some(id) = first_arg(args) else {
        return send(state, chat_id, "Usage: /subscribe <id>").await;
    };
    let Some(paste) = fetch(state, id).await? else {
        return send(state, chat_id, "No paste with that id.").await;
    };
    if paste.publish_at <= now_unix() {
        send_paste(state, chat_id, &paste).await?;
        return send(state, chat_id, "That paste is already public.").await;
    }

    let count = store::run(&state.pool, move |conn| {
        Ok(store::subscription_count(conn, user_id)?)
    })
    .await?;
    if count >= state.config.max_subscriptions {
        return send(
            state,
            chat_id,
            &format!(
                "You are already subscribed to {} pastes, the maximum.",
                state.config.max_subscriptions
            ),
        )
        .await;
    }

    let paste_id = paste.id.clone();
    store::run(&state.pool, move |conn| {
        store::subscribe(conn, user_id, &paste_id)?;
        Ok(())
    })
    .await?;
    send(
        state,
        chat_id,
        &format!(
            "Subscribed to {}. You will be notified when it is revealed at {}.",
            paste.id,
            format_rfc3339(paste.publish_at)
        ),
    )
    .await
}

async fn unsubscribe_cmd(
    state: &AppState,
    chat_id: i64,
    user_id: i64,
    args: &str,
) -> Result<(), AppError> {
    let Some(id) = first_arg(args) else {
        return send(state, chat_id, "Usage: /unsubscribe <id>").await;
    };
    let id = id.to_string();
    store::run(&state.pool, move |conn| {
        store::unsubscribe(conn, user_id, &id)?;
        Ok(())
    })
    .await?;
    send(state, chat_id, "Unsubscribed.").await
}

async fn subscriptions_cmd(state: &AppState, chat_id: i64, user_id: i64) -> Result<(), AppError> {
    let subs = store::run(&state.pool, move |conn| {
        Ok(store::list_subscriptions(conn, user_id)?)
    })
    .await?;
    if subs.is_empty() {
        return send(state, chat_id, "You have no subscriptions.").await;
    }
    let now = now_unix();
    let mut lines = vec![format!("Your subscriptions ({}):", subs.len())];
    for subscription in subs.iter().take(50) {
        let id = subscription.paste_id.clone();
        let paste = store::run(&state.pool, move |conn| Ok(store::get_paste(conn, &id)?)).await?;
        match paste {
            Some(paste) => lines.push(format_row(&paste, now)),
            None => lines.push(format!("{}  (deleted)", subscription.paste_id)),
        }
    }
    send(state, chat_id, &lines.join("\n")).await
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub async fn confirm_delete(
    state: &AppState,
    user_id: i64,
    id: &str,
    callback_id: &str,
) -> Result<(), AppError> {
    let Some(paste) = fetch(state, id).await? else {
        return state
            .messenger
            .answer_callback(callback_id, Some("That paste is already gone."))
            .await;
    };
    if !can_manage(
        User::Telegram(user_id),
        &paste.owner,
        paste.publish_at,
        now_unix(),
    ) {
        return state
            .messenger
            .answer_callback(callback_id, Some("You cannot delete that paste."))
            .await;
    }
    let id = paste.id.clone();
    store::run(&state.pool, move |conn| {
        store::delete_paste(conn, &id)?;
        Ok(())
    })
    .await?;
    state
        .messenger
        .answer_callback(callback_id, Some("Deleted."))
        .await
}

async fn fetch(state: &AppState, id: &str) -> Result<Option<Paste>, AppError> {
    let id = id.to_string();
    store::run(&state.pool, move |conn| Ok(store::get_paste(conn, &id)?)).await
}

async fn send_paste(state: &AppState, chat_id: i64, paste: &Paste) -> Result<(), AppError> {
    let status = if paste.publish_at <= now_unix() {
        "revealed"
    } else {
        "private"
    };
    let title = if paste.title.is_empty() {
        "(no title)".to_string()
    } else {
        paste.title.clone()
    };
    let meta = format!(
        "{}  {}\nstatus: {status}\npublishes: {}\ncreated: {}",
        paste.id,
        title,
        format_rfc3339(paste.publish_at),
        format_rfc3339(paste.created_at)
    );
    state.messenger.send(chat_id, &meta).await?;
    state.messenger.send(chat_id, &paste.content).await
}

fn format_row(paste: &Paste, now: i64) -> String {
    let status = if paste.publish_at <= now {
        "revealed"
    } else {
        "private"
    };
    let title = if paste.title.is_empty() {
        "(no title)"
    } else {
        &paste.title
    };
    let owner = if paste.owner == "anonymous" {
        "anon"
    } else {
        "tg"
    };
    format!(
        "{}  {}  [{}]  {}  {}",
        paste.id,
        title,
        status,
        owner,
        format_rfc3339(paste.publish_at)
    )
}

async fn send(state: &AppState, chat_id: i64, text: &str) -> Result<(), AppError> {
    state.messenger.send(chat_id, text).await
}

fn parse_command(text: &str) -> (&str, &str) {
    let trimmed = text.trim();
    let (head, rest) = match trimmed.split_once(char::is_whitespace) {
        Some((head, rest)) => (head, rest.trim()),
        None => (trimmed, ""),
    };
    let command = head.trim_start_matches('/');
    let command = command.split('@').next().unwrap_or(command);
    (command, rest)
}

fn first_arg(args: &str) -> Option<&str> {
    args.split_whitespace().next()
}

fn validate_title(title: &str) -> Result<(), String> {
    if title.len() > MAX_TITLE {
        return Err(format!("The title must be at most {MAX_TITLE} bytes."));
    }
    if title.chars().any(|c| c.is_control() || is_bidi(c)) {
        return Err("The title must be a single line without control characters.".to_string());
    }
    Ok(())
}

fn is_bidi(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(parse_command("/new"), ("new", ""));
        assert_eq!(parse_command("/show abc123"), ("show", "abc123"));
        assert_eq!(parse_command("/start@cocoon_bot abc"), ("start", "abc"));
        assert_eq!(parse_command("not a command"), ("not", "a command"));
    }

    #[test]
    fn title_validation() {
        assert!(validate_title("a normal title").is_ok());
        assert!(validate_title("").is_ok());
        assert!(validate_title("line\nbreak").is_err());
        assert!(validate_title(&"a".repeat(MAX_TITLE + 1)).is_err());
    }

    #[test]
    fn first_arg_trims() {
        assert_eq!(first_arg("  abc  def "), Some("abc"));
        assert_eq!(first_arg("   "), None);
    }

    // ---- end-to-end command flow against a fake messenger + temp database ----

    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use crate::bot::Messenger;
    use crate::state::AppState;

    type ButtonRows = Vec<Vec<(String, String)>>;

    #[derive(Default)]
    struct FakeMessenger {
        sent: Mutex<Vec<(i64, String)>>,
        buttons: Mutex<Vec<(i64, String, ButtonRows)>>,
    }

    #[async_trait]
    impl Messenger for FakeMessenger {
        async fn send(&self, chat_id: i64, text: &str) -> Result<(), AppError> {
            self.sent.lock().unwrap().push((chat_id, text.to_string()));
            Ok(())
        }

        async fn send_with_buttons(
            &self,
            chat_id: i64,
            text: &str,
            buttons: &[Vec<Button>],
        ) -> Result<(), AppError> {
            let rendered = buttons
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|button| (button.label.clone(), button.data.clone()))
                        .collect()
                })
                .collect();
            self.buttons
                .lock()
                .unwrap()
                .push((chat_id, text.to_string(), rendered));
            Ok(())
        }

        async fn answer_callback(
            &self,
            _callback_id: &str,
            _text: Option<&str>,
        ) -> Result<(), AppError> {
            Ok(())
        }
    }

    fn test_state() -> (AppState, Arc<FakeMessenger>) {
        use diesel_migrations::MigrationHarness;

        let messenger = Arc::new(FakeMessenger::default());
        let path = std::env::temp_dir().join(format!(
            "cocoon-test-{}.db",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pool = crate::store::build_pool(path.to_str().unwrap()).unwrap();
        {
            let mut conn = pool.get().unwrap();
            conn.run_pending_migrations(crate::MIGRATIONS).unwrap();
        }
        let config = crate::config::Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            db_path: path.to_string_lossy().into_owned(),
            bot_token: "123456789:test".to_string(),
            webhook_secret: "0123456789abcdef0123456789abcdef".to_string(),
            webhook_path: "/telegram/webhook".to_string(),
            public_url: None,
            register_webhook: false,
            max_subscriptions: 100,
            proxy: None,
            api_url: None,
        };
        let state = AppState {
            pool,
            config: Arc::new(config),
            messenger: messenger.clone(),
            bot_username: Arc::from("cocoon_test_bot"),
        };
        (state, messenger)
    }

    async fn send_update(state: &AppState, _update_id: i64, user_id: i64, text: &str) {
        handle_text(state, user_id, user_id, text)
            .await
            .expect("update handled");
    }

    async fn user_pastes(state: &AppState, user_id: i64) -> Vec<Paste> {
        let owner = User::Telegram(user_id).storage_key();
        crate::store::run(&state.pool, move |conn| {
            Ok(crate::store::list_owner_pastes(conn, &owner)?)
        })
        .await
        .unwrap()
    }

    async fn create(
        state: &AppState,
        user_id: i64,
        content: &str,
        when: &str,
        title: &str,
    ) -> String {
        send_update(state, 1, user_id, "/new").await;
        send_update(state, 2, user_id, content).await;
        send_update(state, 3, user_id, when).await;
        send_update(state, 4, user_id, title).await;
        user_pastes(state, user_id)
            .await
            .into_iter()
            .next()
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn new_wizard_creates_a_paste() {
        let (state, fake) = test_state();
        create(&state, 777, "hello world", "/skip", "greeting").await;
        let pastes = user_pastes(&state, 777).await;
        assert_eq!(pastes.len(), 1);
        assert_eq!(pastes[0].owner, "telegram:777");
        assert_eq!(pastes[0].title, "greeting");
        assert_eq!(pastes[0].content, "hello world");
        assert!(
            fake.sent
                .lock()
                .unwrap()
                .iter()
                .any(|(_, text)| text.contains("Created paste"))
        );
    }

    #[tokio::test]
    async fn creating_auto_subscribes_the_owner() {
        let (state, _) = test_state();
        let id = create(&state, 777, "hello", "2030-01-01 00:00", "t").await;
        let subs = crate::store::run(&state.pool, |conn| {
            Ok(crate::store::list_subscriptions(conn, 777)?)
        })
        .await
        .unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].paste_id, id);
    }

    #[tokio::test]
    async fn edit_recomputes_the_id() {
        let (state, _) = test_state();
        let before = create(&state, 777, "draft", "2030-01-01 00:00", "title").await;

        send_update(&state, 5, 777, &format!("/edit {before}")).await;
        send_update(&state, 6, 777, "draft edited").await;
        send_update(&state, 7, 777, "/skip").await;
        send_update(&state, 8, 777, "/skip").await;

        let pastes = user_pastes(&state, 777).await;
        assert_eq!(pastes.len(), 1);
        assert_eq!(pastes[0].content, "draft edited");
        assert_ne!(pastes[0].id, before);
    }

    #[tokio::test]
    async fn show_hides_unrevealed_pastes_from_others() {
        let (state, fake) = test_state();
        let id = create(&state, 777, "secret", "2030-01-01 00:00", "t").await;

        send_update(&state, 5, 888, &format!("/show {id}")).await;
        assert!(
            fake.sent
                .lock()
                .unwrap()
                .iter()
                .any(|(_, text)| text.contains("not revealed yet"))
        );
    }

    #[tokio::test]
    async fn show_returns_own_unrevealed_paste() {
        let (state, fake) = test_state();
        let id = create(&state, 777, "mine", "2030-01-01 00:00", "t").await;
        send_update(&state, 5, 777, &format!("/show {id}")).await;
        assert!(
            fake.sent
                .lock()
                .unwrap()
                .iter()
                .any(|(_, text)| text == "mine")
        );
    }

    #[tokio::test]
    async fn delete_asks_for_confirmation_then_removes() {
        let (state, fake) = test_state();
        let id = create(&state, 777, "bye", "2030-01-01 00:00", "t").await;
        send_update(&state, 5, 777, &format!("/delete {id}")).await;
        {
            let buttons = fake.buttons.lock().unwrap();
            assert_eq!(buttons.len(), 1);
            assert_eq!(buttons[0].2[0][0].1, format!("delete:{id}"));
        }

        confirm_delete(&state, 777, &id, "cb").await.unwrap();
        assert!(user_pastes(&state, 777).await.is_empty());
    }

    #[tokio::test]
    async fn list_includes_every_paste() {
        let (state, fake) = test_state();
        create(&state, 777, "a", "/skip", "one").await;
        create(&state, 888, "b", "/skip", "two").await;
        send_update(&state, 20, 999, "/list").await;
        let sent = fake.sent.lock().unwrap();
        assert!(sent.iter().any(|(_, t)| t.contains("one")));
        assert!(sent.iter().any(|(_, t)| t.contains("two")));
    }
}
