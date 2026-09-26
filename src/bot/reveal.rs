use crate::clock::now_unix;
use crate::error::AppError;
use crate::state::AppState;
use crate::store::{self, models::Paste};

/// How often to look for pastes that have reached their publish time.
const REVEAL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// Stale wizard state is discarded after this many seconds.
const CONVERSATION_TTL: i64 = 30 * 60;

pub async fn run(state: AppState) {
    let mut ticker = tokio::time::interval(REVEAL_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        if let Err(e) = tick(&state).await {
            tracing::error!("reveal task failed: {e}");
        }
    }
}

async fn tick(state: &AppState) -> Result<(), AppError> {
    let now = now_unix();
    store::run(&state.pool, move |conn| {
        store::purge_stale_conversations(conn, now - CONVERSATION_TTL)?;
        Ok(())
    })
    .await?;

    let due = store::run(&state.pool, move |conn| {
        Ok(store::pending_reveals(conn, now)?)
    })
    .await?;
    for paste in due {
        notify_subscribers(state, &paste).await;

        let id = paste.id.clone();
        store::run(&state.pool, move |conn| {
            store::delete_subscriptions_for_paste(conn, &id)?;
            store::mark_notified(conn, &id)?;
            Ok(())
        })
        .await?;
    }
    Ok(())
}

/// DM every subscriber the revealed paste. Failures are logged and skipped.
async fn notify_subscribers(state: &AppState, paste: &Paste) {
    let id = paste.id.clone();
    let subscribers = match store::run(&state.pool, move |conn| {
        Ok(store::subscribers_for_paste(conn, &id)?)
    })
    .await
    {
        Ok(subscribers) => subscribers,
        Err(e) => {
            tracing::error!("failed to load subscribers for {}: {e}", paste.id);
            return;
        }
    };

    for user_id in subscribers {
        if let Err(e) = notify(state, user_id, paste).await {
            tracing::error!("failed to notify {user_id} about {}: {e}", paste.id);
        }
    }
}

async fn notify(state: &AppState, user_id: i64, paste: &Paste) -> Result<(), AppError> {
    let title = if paste.title.is_empty() {
        String::new()
    } else {
        format!("  {}", paste.title)
    };
    state
        .messenger
        .send(
            user_id,
            &format!("Paste {}{title} is now public.", paste.id),
        )
        .await?;
    state.messenger.send(user_id, &paste.content).await
}
