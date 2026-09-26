pub mod models;
pub mod schema;

use std::time::Duration;

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, CustomizeConnection, Pool};
use diesel::sqlite::SqliteConnection;

use crate::clock::now_unix;
use crate::error::AppError;
use crate::id::new_id;
use crate::user::User;

use self::models::*;
use self::schema::*;

pub type DbPool = Pool<ConnectionManager<SqliteConnection>>;

/// Maximum paste content size, matching Telegram's per-message limit.
pub const MAX_CONTENT: usize = 4096;

/// Per-connection pragmas. `journal_mode = WAL` is set once in [`build_pool`]
/// (concurrent acquisitions would otherwise race for the lock).
#[derive(Debug)]
struct SqlitePragmas;

impl CustomizeConnection<SqliteConnection, diesel::r2d2::Error> for SqlitePragmas {
    fn on_acquire(&self, conn: &mut SqliteConnection) -> Result<(), diesel::r2d2::Error> {
        if let Err(e) = conn.batch_execute(
            "PRAGMA busy_timeout = 5000;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        ) {
            tracing::error!("failed to configure sqlite connection: {e}");
        }
        Ok(())
    }
}

pub fn build_pool(db_path: &str) -> anyhow::Result<DbPool> {
    {
        let mut conn = SqliteConnection::establish(db_path)?;
        conn.batch_execute("PRAGMA busy_timeout = 5000; PRAGMA journal_mode = WAL;")?;
    }

    let manager = ConnectionManager::<SqliteConnection>::new(db_path);
    let pool = Pool::builder()
        .max_size(16)
        .connection_timeout(Duration::from_secs(10))
        .connection_customizer(Box::new(SqlitePragmas))
        .build(manager)?;
    Ok(pool)
}

/// Run a blocking database closure on the blocking pool.
pub async fn run<T, F>(pool: &DbPool, f: F) -> Result<T, AppError>
where
    F: FnOnce(&mut SqliteConnection) -> Result<T, AppError> + Send + 'static,
    T: Send + 'static,
{
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let mut conn = pool.get().map_err(|e| AppError::Pool(e.to_string()))?;
        f(&mut conn)
    })
    .await
    .map_err(|e| AppError::Internal(format!("blocking task failed: {e}")))?
}

// ---------------------------------------------------------------------------
// Pastes
// ---------------------------------------------------------------------------

pub fn get_paste(conn: &mut SqliteConnection, id: &str) -> QueryResult<Option<Paste>> {
    pastes::table.find(id).first(conn).optional()
}

/// Every paste, newest first.
pub fn list_pastes(
    conn: &mut SqliteConnection,
    limit: i64,
    offset: i64,
) -> QueryResult<Vec<Paste>> {
    pastes::table
        .order((pastes::created_at.desc(), pastes::id.asc()))
        .limit(limit)
        .offset(offset)
        .load(conn)
}

pub fn list_owner_pastes(conn: &mut SqliteConnection, owner: &str) -> QueryResult<Vec<Paste>> {
    pastes::table
        .filter(pastes::owner.eq(owner))
        .order((pastes::created_at.desc(), pastes::id.asc()))
        .load(conn)
}

#[allow(dead_code)]
pub fn count_pastes(conn: &mut SqliteConnection) -> QueryResult<i64> {
    pastes::table.count().get_result(conn)
}

fn id_taken(conn: &mut SqliteConnection, id: &str) -> QueryResult<bool> {
    diesel::select(diesel::dsl::exists(pastes::table.find(id))).get_result(conn)
}

/// Insert a new paste, deriving its id. A Telegram owner is auto-subscribed so
/// they are notified when it reveals.
pub fn create_paste(
    conn: &mut SqliteConnection,
    owner: User,
    content: &str,
    title: &str,
    publish_at: i64,
) -> Result<String, AppError> {
    if content.chars().count() > MAX_CONTENT {
        return Err(AppError::PayloadTooLarge { max: MAX_CONTENT });
    }
    let now = now_unix();
    let id = new_id(owner, content, title, publish_at);
    if id_taken(conn, &id)? {
        return Err(AppError::Duplicate(id));
    }
    let owner_key = owner.storage_key();
    conn.transaction::<_, AppError, _>(|conn| {
        diesel::insert_into(pastes::table)
            .values(&NewPaste {
                id: &id,
                owner: &owner_key,
                content,
                title,
                publish_at,
                created_at: now,
                updated_at: now,
                notified: false,
            })
            .execute(conn)?;
        if let Some(user_id) = owner.telegram_id() {
            subscribe(conn, user_id, &id)?;
        }
        Ok(())
    })?;
    Ok(id)
}

/// Update an existing paste, recomputing its id. Returns the new id.
pub fn update_paste(
    conn: &mut SqliteConnection,
    old_id: &str,
    owner: User,
    content: &str,
    title: &str,
    publish_at: i64,
) -> Result<String, AppError> {
    if content.chars().count() > MAX_CONTENT {
        return Err(AppError::PayloadTooLarge { max: MAX_CONTENT });
    }
    let new_id = new_id(owner, content, title, publish_at);
    if new_id != old_id && id_taken(conn, &new_id)? {
        return Err(AppError::Duplicate(new_id));
    }
    diesel::update(pastes::table.find(old_id))
        .set((
            pastes::id.eq(&new_id),
            pastes::content.eq(content),
            pastes::title.eq(title),
            pastes::publish_at.eq(publish_at),
            pastes::updated_at.eq(now_unix()),
            pastes::notified.eq(false),
        ))
        .execute(conn)?;
    Ok(new_id)
}

pub fn delete_paste(conn: &mut SqliteConnection, id: &str) -> QueryResult<usize> {
    diesel::delete(pastes::table.find(id)).execute(conn)
}

pub fn pending_reveals(conn: &mut SqliteConnection, now: i64) -> QueryResult<Vec<Paste>> {
    pastes::table
        .filter(pastes::notified.eq(false).and(pastes::publish_at.le(now)))
        .load(conn)
}

pub fn mark_notified(conn: &mut SqliteConnection, id: &str) -> QueryResult<usize> {
    diesel::update(pastes::table.find(id))
        .set(pastes::notified.eq(true))
        .execute(conn)
}

// ---------------------------------------------------------------------------
// Subscriptions
// ---------------------------------------------------------------------------

pub fn subscribe(conn: &mut SqliteConnection, user_id: i64, paste_id: &str) -> QueryResult<usize> {
    diesel::insert_into(subscriptions::table)
        .values(&NewSubscription {
            user_id,
            paste_id,
            created_at: now_unix(),
        })
        .on_conflict((subscriptions::user_id, subscriptions::paste_id))
        .do_nothing()
        .execute(conn)
}

pub fn unsubscribe(
    conn: &mut SqliteConnection,
    user_id: i64,
    paste_id: &str,
) -> QueryResult<usize> {
    diesel::delete(subscriptions::table.find((user_id, paste_id))).execute(conn)
}

pub fn list_subscriptions(
    conn: &mut SqliteConnection,
    user_id: i64,
) -> QueryResult<Vec<Subscription>> {
    subscriptions::table
        .filter(subscriptions::user_id.eq(user_id))
        .order(subscriptions::created_at.desc())
        .load(conn)
}

pub fn subscription_count(conn: &mut SqliteConnection, user_id: i64) -> QueryResult<i64> {
    subscriptions::table
        .filter(subscriptions::user_id.eq(user_id))
        .count()
        .get_result(conn)
}

pub fn subscribers_for_paste(conn: &mut SqliteConnection, paste_id: &str) -> QueryResult<Vec<i64>> {
    subscriptions::table
        .filter(subscriptions::paste_id.eq(paste_id))
        .select(subscriptions::user_id)
        .load(conn)
}

pub fn delete_subscriptions_for_paste(
    conn: &mut SqliteConnection,
    paste_id: &str,
) -> QueryResult<usize> {
    diesel::delete(subscriptions::table.filter(subscriptions::paste_id.eq(paste_id))).execute(conn)
}

// ---------------------------------------------------------------------------
// Update dedup
// ---------------------------------------------------------------------------

pub fn register_update(conn: &mut SqliteConnection, update_id: i64) -> QueryResult<usize> {
    diesel::insert_into(updates::table)
        .values(&NewUpdate {
            update_id,
            received_at: now_unix(),
        })
        .execute(conn)
}

pub fn forget_update(conn: &mut SqliteConnection, update_id: i64) -> QueryResult<usize> {
    diesel::delete(updates::table.find(update_id)).execute(conn)
}

// ---------------------------------------------------------------------------
// Conversations (wizard state)
// ---------------------------------------------------------------------------

pub fn get_conversation(
    conn: &mut SqliteConnection,
    user_id: i64,
) -> QueryResult<Option<Conversation>> {
    conversations::table.find(user_id).first(conn).optional()
}

pub fn set_conversation(
    conn: &mut SqliteConnection,
    conv: &NewConversation<'_>,
) -> QueryResult<usize> {
    diesel::insert_into(conversations::table)
        .values(conv)
        .on_conflict(conversations::user_id)
        .do_update()
        .set((
            conversations::kind.eq(conv.kind),
            conversations::step.eq(conv.step),
            conversations::paste_id.eq(conv.paste_id),
            conversations::content.eq(conv.content),
            conversations::title.eq(conv.title),
            conversations::publish_at.eq(conv.publish_at),
            conversations::updated_at.eq(conv.updated_at),
        ))
        .execute(conn)
}

pub fn delete_conversation(conn: &mut SqliteConnection, user_id: i64) -> QueryResult<usize> {
    diesel::delete(conversations::table.find(user_id)).execute(conn)
}

pub fn purge_stale_conversations(
    conn: &mut SqliteConnection,
    older_than: i64,
) -> QueryResult<usize> {
    diesel::delete(conversations::table.filter(conversations::updated_at.lt(older_than)))
        .execute(conn)
}
