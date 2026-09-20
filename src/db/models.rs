use diesel::prelude::*;

use super::schema::pastes;

/// Metadata-only projection used by the listing endpoints.
#[derive(Debug, Queryable, Selectable)]
#[diesel(table_name = pastes)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct PasteMeta {
    pub id: Vec<u8>,
    pub title: String,
    pub publish_at: i64,
    pub created_at: i64,
}

/// Insert payload.
#[derive(Debug, Insertable)]
#[diesel(table_name = pastes)]
pub struct NewPaste<'a> {
    pub id: &'a [u8],
    pub title: &'a str,
    pub content: &'a [u8],
    pub publish_at: i64,
    pub created_at: i64,
}
