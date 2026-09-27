use diesel::prelude::*;

use super::schema::{conversations, pastes, subscriptions, updates};

#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = pastes)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct Paste {
    pub id: String,
    /// `User::storage_key()` of the owner.
    pub owner: String,
    pub content: String,
    pub title: String,
    pub publish_at: i64,
    pub created_at: i64,
    #[allow(dead_code)]
    pub updated_at: i64,
    /// Maintained for the reveal query; not read from the loaded row.
    #[allow(dead_code)]
    pub notified: bool,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = pastes)]
pub struct NewPaste<'a> {
    pub id: &'a str,
    pub owner: &'a str,
    pub content: &'a str,
    pub title: &'a str,
    pub publish_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub notified: bool,
}

#[derive(Debug, Clone, Queryable)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct PasteMeta {
    pub id: String,
    pub owner: String,
    pub title: String,
    pub publish_at: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = conversations)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct Conversation {
    /// Present in the row; the user id is the lookup key and not re-read.
    #[allow(dead_code)]
    pub user_id: i64,
    pub kind: String,
    pub step: String,
    pub paste_id: String,
    pub content: String,
    pub title: String,
    pub publish_at: Option<i64>,
    #[allow(dead_code)]
    pub updated_at: i64,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = conversations)]
pub struct NewConversation<'a> {
    pub user_id: i64,
    pub kind: &'a str,
    pub step: &'a str,
    pub paste_id: &'a str,
    pub content: &'a str,
    pub title: &'a str,
    pub publish_at: Option<i64>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = subscriptions)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct Subscription {
    #[allow(dead_code)]
    pub user_id: i64,
    pub paste_id: String,
    #[allow(dead_code)]
    pub created_at: i64,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = subscriptions)]
pub struct NewSubscription<'a> {
    pub user_id: i64,
    pub paste_id: &'a str,
    pub created_at: i64,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = updates)]
pub struct NewUpdate {
    pub update_id: i64,
    pub received_at: i64,
}
