pub mod models;

use std::sync::Arc;

use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::clock::now_unix;
use crate::error::AppError;
use crate::id::new_id;
use crate::user::User;

use self::models::*;

/// Shared handle to the redb database. redb serializes writers internally, so
/// no connection pool is needed; the alias is kept for call-site continuity.
pub type DbPool = Arc<Database>;

const CONTENTS: TableDefinition<&str, &str> = TableDefinition::new("pastes");
const PASTE_META: TableDefinition<&str, &str> = TableDefinition::new("paste_meta");
const CONVERSATIONS: TableDefinition<i64, &str> = TableDefinition::new("conversations");
const SUBSCRIPTIONS: TableDefinition<(i64, &str), i64> = TableDefinition::new("subscriptions");
const UPDATES: TableDefinition<i64, i64> = TableDefinition::new("updates");

#[derive(Debug, Serialize, Deserialize)]
struct PasteRecord {
    owner: String,
    title: String,
    publish_at: i64,
    created_at: i64,
    updated_at: i64,
    notified: bool,
    private: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct ConversationRecord {
    kind: String,
    step: String,
    paste_id: String,
    content: String,
    title: String,
    publish_at: Option<i64>,
    updated_at: i64,
}

struct StoredPaste {
    id: String,
    record: PasteRecord,
}

impl StoredPaste {
    fn into_paste(self, content: String) -> Paste {
        Paste {
            id: self.id,
            owner: self.record.owner,
            content,
            title: self.record.title,
            publish_at: self.record.publish_at,
            created_at: self.record.created_at,
            updated_at: self.record.updated_at,
            notified: self.record.notified,
            private: self.record.private,
        }
    }

    fn meta(&self) -> PasteMeta {
        PasteMeta {
            id: self.id.clone(),
            owner: self.record.owner.clone(),
            title: self.record.title.clone(),
            publish_at: self.record.publish_at,
            created_at: self.record.created_at,
        }
    }
}

pub fn build_pool(db_path: &str) -> anyhow::Result<DbPool> {
    let db = Database::create(db_path)?;
    let txn = db.begin_write()?;
    {
        txn.open_table(CONTENTS)?;
        txn.open_table(PASTE_META)?;
        txn.open_table(CONVERSATIONS)?;
        txn.open_table(SUBSCRIPTIONS)?;
        txn.open_table(UPDATES)?;
    }
    txn.commit()?;
    Ok(Arc::new(db))
}

/// Run a blocking database closure on the blocking pool.
pub async fn run<T, F>(db: &DbPool, f: F) -> Result<T, AppError>
where
    F: FnOnce(&Database) -> Result<T, AppError> + Send + 'static,
    T: Send + 'static,
{
    let db = db.clone();
    tokio::task::spawn_blocking(move || f(&db))
        .await
        .map_err(|e| AppError::Internal(format!("blocking task failed: {e}")))?
}

fn all_pastes(db: &Database) -> Result<Vec<StoredPaste>, AppError> {
    let read = db.begin_read()?;
    let table = read.open_table(PASTE_META)?;
    let mut rows = Vec::new();
    for entry in table.iter()? {
        let (key, value) = entry?;
        let record = serde_json::from_str(value.value())?;
        rows.push(StoredPaste {
            id: key.value().to_string(),
            record,
        });
    }
    Ok(rows)
}

fn public_pastes(db: &Database) -> Result<Vec<StoredPaste>, AppError> {
    let mut rows = all_pastes(db)?;
    rows.retain(|paste| !paste.record.private);
    rows.sort_by(|a, b| {
        b.record
            .created_at
            .cmp(&a.record.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(rows)
}

fn hydrate(db: &Database, rows: Vec<StoredPaste>) -> Result<Vec<Paste>, AppError> {
    let read = db.begin_read()?;
    let table = read.open_table(CONTENTS)?;
    let mut pastes = Vec::with_capacity(rows.len());
    for row in rows {
        let content = table
            .get(row.id.as_str())?
            .map(|value| value.value().to_string())
            .ok_or_else(|| AppError::Internal(format!("paste {} has no content", row.id)))?;
        pastes.push(row.into_paste(content));
    }
    Ok(pastes)
}

// ---------------------------------------------------------------------------
// Pastes
// ---------------------------------------------------------------------------

pub fn get_paste(db: &Database, id: &str) -> Result<Option<Paste>, AppError> {
    let read = db.begin_read()?;
    let meta = read.open_table(PASTE_META)?;
    let Some(value) = meta.get(id)? else {
        return Ok(None);
    };
    let record: PasteRecord = serde_json::from_str(value.value())?;
    drop(value);

    let contents = read.open_table(CONTENTS)?;
    let content = contents
        .get(id)?
        .map(|value| value.value().to_string())
        .ok_or_else(|| AppError::Internal(format!("paste {id} has no content")))?;
    Ok(Some(
        StoredPaste {
            id: id.to_string(),
            record,
        }
        .into_paste(content),
    ))
}

/// Every public paste, newest first. Private pastes are only visible to their
/// owner through [`list_owner_pastes`].
pub fn list_pastes(db: &Database, limit: i64, offset: i64) -> Result<Vec<Paste>, AppError> {
    let rows = public_pastes(db)?;
    let offset = usize::try_from(offset.max(0)).unwrap_or(usize::MAX);
    let limit = usize::try_from(limit.max(0)).unwrap_or(usize::MAX);
    let page = rows.into_iter().skip(offset).take(limit).collect();
    hydrate(db, page)
}

/// Metadata for every public paste, in no particular order.
pub fn public_paste_meta(db: &Database) -> Result<Vec<PasteMeta>, AppError> {
    Ok(public_pastes(db)?.iter().map(StoredPaste::meta).collect())
}

pub fn list_owner_pastes(db: &Database, owner: &str) -> Result<Vec<Paste>, AppError> {
    let mut rows = all_pastes(db)?;
    rows.retain(|paste| paste.record.owner == owner);
    rows.sort_by(|a, b| {
        b.record
            .created_at
            .cmp(&a.record.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    hydrate(db, rows)
}

#[allow(dead_code)]
pub fn count_pastes(db: &Database) -> Result<i64, AppError> {
    Ok(all_pastes(db)?.len() as i64)
}

/// Insert a new paste, deriving its id. A Telegram owner is auto-subscribed so
/// they are notified when it reveals.
pub fn create_paste(
    db: &Database,
    owner: User,
    content: &str,
    title: &str,
    publish_at: i64,
    private: bool,
    max_content_bytes: usize,
) -> Result<String, AppError> {
    if content.len() > max_content_bytes {
        return Err(AppError::PayloadTooLarge {
            max: max_content_bytes,
        });
    }
    let now = now_unix();
    let id = new_id(owner, content, title, publish_at, private);
    let owner_key = owner.storage_key();
    let txn = db.begin_write()?;
    {
        let mut meta = txn.open_table(PASTE_META)?;
        if meta.get(id.as_str())?.is_some() {
            return Err(AppError::Duplicate(id));
        }
        let mut contents = txn.open_table(CONTENTS)?;
        contents.insert(id.as_str(), content)?;
        let record = PasteRecord {
            owner: owner_key,
            title: title.to_string(),
            publish_at,
            created_at: now,
            updated_at: now,
            notified: false,
            private,
        };
        let json = serde_json::to_string(&record)?;
        meta.insert(id.as_str(), json.as_str())?;
        if let Some(user_id) = owner.telegram_id() {
            let mut subscriptions = txn.open_table(SUBSCRIPTIONS)?;
            if subscriptions.get((user_id, id.as_str()))?.is_none() {
                subscriptions.insert((user_id, id.as_str()), now)?;
            }
        }
    }
    txn.commit()?;
    Ok(id)
}

/// Update an existing paste, recomputing its id. Returns the new id.
#[allow(clippy::too_many_arguments)]
pub fn update_paste(
    db: &Database,
    old_id: &str,
    owner: User,
    content: &str,
    title: &str,
    publish_at: i64,
    private: bool,
    max_content_bytes: usize,
) -> Result<String, AppError> {
    if content.len() > max_content_bytes {
        return Err(AppError::PayloadTooLarge {
            max: max_content_bytes,
        });
    }
    let new_id = new_id(owner, content, title, publish_at, private);
    let owner_key = owner.storage_key();
    let txn = db.begin_write()?;
    {
        let mut meta = txn.open_table(PASTE_META)?;
        let old: PasteRecord = match meta.get(old_id)? {
            Some(value) => serde_json::from_str(value.value())?,
            None => return Err(AppError::NotFound),
        };
        if new_id != old_id && meta.get(new_id.as_str())?.is_some() {
            return Err(AppError::Duplicate(new_id));
        }
        let mut contents = txn.open_table(CONTENTS)?;
        contents.remove(old_id)?;
        meta.remove(old_id)?;
        contents.insert(new_id.as_str(), content)?;
        let record = PasteRecord {
            owner: owner_key,
            title: title.to_string(),
            publish_at,
            created_at: old.created_at,
            updated_at: now_unix(),
            notified: false,
            private,
        };
        let json = serde_json::to_string(&record)?;
        meta.insert(new_id.as_str(), json.as_str())?;
    }
    txn.commit()?;
    Ok(new_id)
}

pub fn delete_paste(db: &Database, id: &str) -> Result<usize, AppError> {
    let txn = db.begin_write()?;
    let removed;
    {
        let mut meta = txn.open_table(PASTE_META)?;
        removed = usize::from(meta.remove(id)?.is_some());
        let mut contents = txn.open_table(CONTENTS)?;
        contents.remove(id)?;
    }
    txn.commit()?;
    Ok(removed)
}

pub fn pending_reveals(db: &Database, now: i64) -> Result<Vec<Paste>, AppError> {
    let due = all_pastes(db)?
        .into_iter()
        .filter(|paste| !paste.record.notified && paste.record.publish_at <= now)
        .collect();
    hydrate(db, due)
}

pub fn mark_notified(db: &Database, id: &str) -> Result<usize, AppError> {
    let txn = db.begin_write()?;
    let updated;
    {
        let mut meta = txn.open_table(PASTE_META)?;
        let record = match meta.get(id)? {
            Some(value) => Some(serde_json::from_str::<PasteRecord>(value.value())?),
            None => None,
        };
        match record {
            Some(mut record) => {
                record.notified = true;
                let json = serde_json::to_string(&record)?;
                meta.insert(id, json.as_str())?;
                updated = 1;
            }
            None => updated = 0,
        }
    }
    txn.commit()?;
    Ok(updated)
}

// ---------------------------------------------------------------------------
// Subscriptions
// ---------------------------------------------------------------------------

pub fn subscribe(db: &Database, user_id: i64, paste_id: &str) -> Result<usize, AppError> {
    let txn = db.begin_write()?;
    let inserted;
    {
        let mut table = txn.open_table(SUBSCRIPTIONS)?;
        if table.get((user_id, paste_id))?.is_some() {
            inserted = 0;
        } else {
            table.insert((user_id, paste_id), now_unix())?;
            inserted = 1;
        }
    }
    txn.commit()?;
    Ok(inserted)
}

pub fn unsubscribe(db: &Database, user_id: i64, paste_id: &str) -> Result<usize, AppError> {
    let txn = db.begin_write()?;
    let removed;
    {
        let mut table = txn.open_table(SUBSCRIPTIONS)?;
        removed = usize::from(table.remove((user_id, paste_id))?.is_some());
    }
    txn.commit()?;
    Ok(removed)
}

pub fn list_subscriptions(db: &Database, user_id: i64) -> Result<Vec<Subscription>, AppError> {
    let read = db.begin_read()?;
    let table = read.open_table(SUBSCRIPTIONS)?;
    let mut subscriptions = Vec::new();
    for entry in table.iter()? {
        let (key, value) = entry?;
        let (user, paste_id) = key.value();
        if user == user_id {
            subscriptions.push(Subscription {
                user_id,
                paste_id: paste_id.to_string(),
                created_at: value.value(),
            });
        }
    }
    subscriptions.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| a.paste_id.cmp(&b.paste_id))
    });
    Ok(subscriptions)
}

pub fn subscription_count(db: &Database, user_id: i64) -> Result<i64, AppError> {
    let read = db.begin_read()?;
    let table = read.open_table(SUBSCRIPTIONS)?;
    let mut count = 0;
    for entry in table.iter()? {
        let (key, _) = entry?;
        if key.value().0 == user_id {
            count += 1;
        }
    }
    Ok(count)
}

pub fn subscribers_for_paste(db: &Database, paste_id: &str) -> Result<Vec<i64>, AppError> {
    let read = db.begin_read()?;
    let table = read.open_table(SUBSCRIPTIONS)?;
    let mut subscribers = Vec::new();
    for entry in table.iter()? {
        let (key, _) = entry?;
        let (user, paste) = key.value();
        if paste == paste_id {
            subscribers.push(user);
        }
    }
    Ok(subscribers)
}

pub fn delete_subscriptions_for_paste(db: &Database, paste_id: &str) -> Result<usize, AppError> {
    let txn = db.begin_write()?;
    let removed;
    {
        let mut table = txn.open_table(SUBSCRIPTIONS)?;
        let mut keys = Vec::new();
        for entry in table.iter()? {
            let (key, _) = entry?;
            let (user, paste) = key.value();
            if paste == paste_id {
                keys.push((user, paste.to_string()));
            }
        }
        removed = keys.len();
        for (user, paste) in keys {
            table.remove((user, paste.as_str()))?;
        }
    }
    txn.commit()?;
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Update dedup
// ---------------------------------------------------------------------------

/// Record an update id. Returns `false` if it was already registered.
pub fn register_update(db: &Database, update_id: i64) -> Result<bool, AppError> {
    let txn = db.begin_write()?;
    let inserted;
    {
        let mut table = txn.open_table(UPDATES)?;
        if table.get(update_id)?.is_some() {
            inserted = false;
        } else {
            table.insert(update_id, now_unix())?;
            inserted = true;
        }
    }
    txn.commit()?;
    Ok(inserted)
}

pub fn forget_update(db: &Database, update_id: i64) -> Result<usize, AppError> {
    let txn = db.begin_write()?;
    let removed;
    {
        let mut table = txn.open_table(UPDATES)?;
        removed = usize::from(table.remove(update_id)?.is_some());
    }
    txn.commit()?;
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Conversations (wizard state)
// ---------------------------------------------------------------------------

pub fn get_conversation(db: &Database, user_id: i64) -> Result<Option<Conversation>, AppError> {
    let read = db.begin_read()?;
    let table = read.open_table(CONVERSATIONS)?;
    let Some(value) = table.get(user_id)? else {
        return Ok(None);
    };
    let record: ConversationRecord = serde_json::from_str(value.value())?;
    Ok(Some(Conversation {
        user_id,
        kind: record.kind,
        step: record.step,
        paste_id: record.paste_id,
        content: record.content,
        title: record.title,
        publish_at: record.publish_at,
        updated_at: record.updated_at,
    }))
}

pub fn set_conversation(db: &Database, conv: &NewConversation<'_>) -> Result<usize, AppError> {
    let record = ConversationRecord {
        kind: conv.kind.to_string(),
        step: conv.step.to_string(),
        paste_id: conv.paste_id.to_string(),
        content: conv.content.to_string(),
        title: conv.title.to_string(),
        publish_at: conv.publish_at,
        updated_at: conv.updated_at,
    };
    let json = serde_json::to_string(&record)?;
    let txn = db.begin_write()?;
    {
        let mut table = txn.open_table(CONVERSATIONS)?;
        table.insert(conv.user_id, json.as_str())?;
    }
    txn.commit()?;
    Ok(1)
}

pub fn delete_conversation(db: &Database, user_id: i64) -> Result<usize, AppError> {
    let txn = db.begin_write()?;
    let removed;
    {
        let mut table = txn.open_table(CONVERSATIONS)?;
        removed = usize::from(table.remove(user_id)?.is_some());
    }
    txn.commit()?;
    Ok(removed)
}

pub fn purge_stale_conversations(db: &Database, older_than: i64) -> Result<usize, AppError> {
    let txn = db.begin_write()?;
    let removed;
    {
        let mut table = txn.open_table(CONVERSATIONS)?;
        let mut stale = Vec::new();
        for entry in table.iter()? {
            let (key, value) = entry?;
            let record: ConversationRecord = serde_json::from_str(value.value())?;
            if record.updated_at < older_than {
                stale.push(key.value());
            }
        }
        removed = stale.len();
        for user_id in stale {
            table.remove(user_id)?;
        }
    }
    txn.commit()?;
    Ok(removed)
}
