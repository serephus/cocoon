//! Shared listing query for the web page and the JSON API.

use diesel::prelude::*;
use diesel::sqlite::Sqlite;
use serde::{Deserialize, Serialize};

use crate::clock::{format_rfc3339, parse_publish_at};
use crate::error::AppError;
use crate::store::{self, DbPool, models::PasteMeta, schema::pastes};

pub const DEFAULT_PER_PAGE: i64 = 50;
pub const MAX_PER_PAGE: i64 = 100;

/// A pair of visibility filters. Both true is "everything"; both false matches
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusFilter {
    pub revealed: bool,
    pub scheduled: bool,
}

impl StatusFilter {
    pub const ALL: Self = Self {
        revealed: true,
        scheduled: true,
    };
    pub const REVEALED: Self = Self {
        revealed: true,
        scheduled: false,
    };
    pub const SCHEDULED: Self = Self {
        revealed: false,
        scheduled: true,
    };
    pub const NONE: Self = Self {
        revealed: false,
        scheduled: false,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    CreatedAt,
    PublishAt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}

impl SortKey {
    pub fn as_str(self) -> &'static str {
        match self {
            SortKey::CreatedAt => "created_at",
            SortKey::PublishAt => "publish_at",
        }
    }
}

impl SortOrder {
    pub fn as_str(self) -> &'static str {
        match self {
            SortOrder::Asc => "asc",
            SortOrder::Desc => "desc",
        }
    }
}

/// Query parameters for the JSON API.
#[derive(Debug, Default, Deserialize)]
pub struct ListParams {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub status: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub q: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ListOptions {
    pub page: i64,
    pub per_page: i64,
    pub status: StatusFilter,
    pub sort: SortKey,
    pub order: SortOrder,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub query: Option<String>,
}

impl ListParams {
    pub fn normalize(&self) -> Result<ListOptions, AppError> {
        let page = self.page.unwrap_or(1).max(1);
        let per_page = self
            .per_page
            .unwrap_or(DEFAULT_PER_PAGE)
            .clamp(1, MAX_PER_PAGE);

        let status = match self.status.as_deref() {
            None | Some("all") => StatusFilter::ALL,
            Some("revealed") => StatusFilter::REVEALED,
            Some("scheduled") | Some("private") => StatusFilter::SCHEDULED,
            Some("none") => StatusFilter::NONE,
            Some(other) => return Err(AppError::BadRequest(format!("invalid status '{other}'"))),
        };
        let sort = match self.sort.as_deref() {
            None | Some("created_at") => SortKey::CreatedAt,
            Some("publish_at") => SortKey::PublishAt,
            Some(other) => return Err(AppError::BadRequest(format!("invalid sort '{other}'"))),
        };
        let order = match self.order.as_deref() {
            None | Some("desc") => SortOrder::Desc,
            Some("asc") => SortOrder::Asc,
            Some(other) => return Err(AppError::BadRequest(format!("invalid order '{other}'"))),
        };

        let from = self
            .from
            .as_deref()
            .map(parse_publish_at)
            .transpose()
            .map_err(AppError::BadRequest)?;
        let to = self
            .to
            .as_deref()
            .map(parse_publish_at)
            .transpose()
            .map_err(AppError::BadRequest)?;
        let query = match self.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(q) if q.chars().count() > 256 => {
                return Err(AppError::BadRequest(
                    "search query must be at most 256 characters".to_string(),
                ));
            }
            Some(q) => Some(q.to_string()),
            None => None,
        };

        Ok(ListOptions {
            page,
            per_page,
            status,
            sort,
            order,
            from,
            to,
            query,
        })
    }
}

/// Fetch one page of paste metadata. Fetches `per_page + 1` rows to learn
/// whether a next page exists without a separate count.
pub async fn fetch_list(
    pool: &DbPool,
    opts: ListOptions,
    now: i64,
) -> Result<(Vec<PasteMeta>, bool), AppError> {
    store::run(pool, move |conn| {
        let mut q = pastes::table
            .select((
                pastes::id,
                pastes::owner,
                pastes::title,
                pastes::publish_at,
                pastes::created_at,
            ))
            .into_boxed::<Sqlite>();

        q = match (opts.status.revealed, opts.status.scheduled) {
            (true, true) => q,
            (true, false) => q.filter(pastes::publish_at.le(now)),
            (false, true) => q.filter(pastes::publish_at.gt(now)),
            (false, false) => q.filter(pastes::publish_at.gt(now).and(pastes::publish_at.le(now))),
        };
        if let Some(query) = &opts.query {
            q = q.filter(pastes::title.like(format!("%{query}%")));
        }
        if let Some(from) = opts.from {
            q = q.filter(pastes::publish_at.ge(from));
        }
        if let Some(to) = opts.to {
            q = q.filter(pastes::publish_at.le(to));
        }

        let q = match (opts.sort, opts.order) {
            (SortKey::CreatedAt, SortOrder::Desc) => {
                q.order((pastes::created_at.desc(), pastes::id.asc()))
            }
            (SortKey::CreatedAt, SortOrder::Asc) => {
                q.order((pastes::created_at.asc(), pastes::id.asc()))
            }
            (SortKey::PublishAt, SortOrder::Desc) => {
                q.order((pastes::publish_at.desc(), pastes::id.asc()))
            }
            (SortKey::PublishAt, SortOrder::Asc) => {
                q.order((pastes::publish_at.asc(), pastes::id.asc()))
            }
        };

        let limit = opts.per_page + 1;
        let offset = (opts.page - 1) * opts.per_page;
        let mut rows = q.limit(limit).offset(offset).load::<PasteMeta>(conn)?;
        let has_next = rows.len() as i64 > opts.per_page;
        if has_next {
            rows.truncate(opts.per_page as usize);
        }
        Ok((rows, has_next))
    })
    .await
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub page: i64,
    pub per_page: i64,
    pub has_next: bool,
    pub pastes: Vec<MetaJson>,
}

#[derive(Debug, Serialize)]
pub struct MetaJson {
    pub id: String,
    pub url: String,
    pub title: String,
    pub owner: String,
    pub publish_at: String,
    pub created_at: String,
    pub revealed: bool,
}

pub fn meta_json(meta: &PasteMeta, now: i64) -> MetaJson {
    MetaJson {
        url: format!("/p/{}", meta.id),
        id: meta.id.clone(),
        title: meta.title.clone(),
        owner: meta.owner.clone(),
        publish_at: format_rfc3339(meta.publish_at),
        created_at: format_rfc3339(meta.created_at),
        revealed: meta.publish_at <= now,
    }
}
