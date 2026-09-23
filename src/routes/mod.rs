pub mod api;
pub mod web;

use axum::Router;
use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::http::Request;
use axum::routing::{get, post};
use diesel::prelude::*;
use diesel::sqlite::Sqlite;
use serde::{Deserialize, Serialize};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::{DefaultOnFailure, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::AppState;
use crate::clock::{format_rfc3339, now_unix, parse_rfc3339};
use crate::db::models::{NewPaste, PasteMeta};
use crate::db::schema::pastes;
use crate::error::AppError;
use crate::id::{compute_id, encode_id};
use diesel::result::{DatabaseErrorKind, Error as DieselError};

/// Maximum accepted content size (bytes).
pub const MAX_CONTENT_BYTES: usize = 65_536;
/// Maximum accepted title size (bytes).
pub const MAX_TITLE_BYTES: usize = 256;
pub const DEFAULT_PER_PAGE: i64 = 50;
pub const MAX_PER_PAGE: i64 = 100;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(web::list))
        .route("/new", get(web::new_form).post(web::create))
        .route("/p/{id}", get(web::read))
        .route("/api/paste", post(api::create))
        .route("/api/pastes", get(api::list))
        .route("/healthz", get(healthz))
        // JSON body limit; the 64 KiB content limit is enforced manually so we
        // can return a precise 413.
        .layer(DefaultBodyLimit::max(1024 * 1024))
        // Copy the request id onto the response (innermost of the three).
        .layer(PropagateRequestIdLayer::x_request_id())
        // Log each request at INFO, with the id attached to the span.
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request<Body>| {
                    let request_id = request
                        .headers()
                        .get("x-request-id")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("-");
                    tracing::info_span!(
                        "http",
                        method = %request.method(),
                        uri = %request.uri(),
                        request_id = %request_id,
                    )
                })
                .on_response(DefaultOnResponse::new().level(Level::INFO))
                .on_failure(DefaultOnFailure::new().level(Level::ERROR)),
        )
        // Assign the id first so both the span and the response header see it.
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

/// Validate a user-supplied title: bounded, single line, no control or
/// bidirectional-override characters.
///
/// Titles are public immediately (including for scheduled pastes), so callers
/// should treat them as non-secret labels.
pub fn validate_title(title: &str) -> Result<(), AppError> {
    if title.len() > MAX_TITLE_BYTES {
        return Err(AppError::BadRequest(format!(
            "title exceeds {MAX_TITLE_BYTES} bytes"
        )));
    }
    if title.chars().any(|c| c.is_control() || is_bidi_control(c)) {
        return Err(AppError::BadRequest(
            "title must be a single line without control or bidirectional characters".to_string(),
        ));
    }
    Ok(())
}

fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
    )
}

/// Run a blocking database closure on the blocking thread pool.
pub async fn db<T, F>(state: &AppState, f: F) -> Result<T, AppError>
where
    F: FnOnce(&mut diesel::SqliteConnection) -> Result<T, AppError> + Send + 'static,
    T: Send + 'static,
{
    let pool = state.pool.clone();
    tokio::task::spawn_blocking(move || {
        let mut conn = pool.get().map_err(|e| AppError::Pool(e.to_string()))?;
        f(&mut conn)
    })
    .await
    .map_err(|e| AppError::Internal(format!("blocking task failed: {e}")))?
}

// ---------------------------------------------------------------------------
// Creation
// ---------------------------------------------------------------------------

/// Metadata for a freshly created (or deduplicated) paste.
pub struct CreatedPaste {
    pub id: String,
    pub title: String,
    pub publish_at: i64,
    pub created_at: i64,
}

/// Shared creation path used by the JSON API and the web form.
///
/// Validates and inserts a paste. The returned `bool` is `true` for a new
/// paste and `false` for an exact duplicate (idempotent).
pub async fn create_paste(
    state: &AppState,
    title: String,
    content: String,
    publish_at: i64,
) -> Result<(bool, CreatedPaste), AppError> {
    if content.len() > MAX_CONTENT_BYTES {
        return Err(AppError::PayloadTooLarge);
    }
    validate_title(&title)?;

    let created_at = now_unix();
    let content = content.into_bytes();
    let id = compute_id(&state.secret, publish_at, title.as_bytes(), &content);
    let id_string = encode_id(&id);

    let title_for_db = title.clone();
    let outcome = db(state, move |conn| {
        // Scope the insert value so the borrows end before we compare below.
        let inserted = {
            let new = NewPaste {
                id: &id,
                title: &title_for_db,
                content: &content,
                publish_at,
                created_at,
            };
            diesel::insert_into(pastes::table)
                .values(&new)
                .execute(conn)
        };

        match inserted {
            Ok(_) => Ok((true, created_at)),
            Err(DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, _)) => {
                let existing = pastes::table
                    .filter(pastes::id.eq(id.to_vec()))
                    .select((
                        pastes::title,
                        pastes::content,
                        pastes::publish_at,
                        pastes::created_at,
                    ))
                    .first::<(String, Vec<u8>, i64, i64)>(conn)
                    .optional()?;

                match existing {
                    // Identical title + content + timestamp: idempotent success.
                    Some((stored_title, stored_content, stored_publish_at, stored_created_at))
                        if stored_title == title_for_db
                            && stored_content == content
                            && stored_publish_at == publish_at =>
                    {
                        Ok((false, stored_created_at))
                    }
                    // A genuine HMAC collision: never overwrite.
                    Some(_) => Err(AppError::Conflict),
                    None => Err(AppError::Internal(
                        "unique violation but no existing row found".to_string(),
                    )),
                }
            }
            Err(e) => Err(AppError::Db(e)),
        }
    })
    .await?;

    let (created, stored_created_at) = outcome;
    Ok((
        created,
        CreatedPaste {
            id: id_string,
            title,
            publish_at,
            created_at: stored_created_at,
        },
    ))
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

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

#[derive(Debug, Default, Deserialize)]
pub struct ListParams {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub status: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    /// Case-insensitive substring match on the title.
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
            .map(parse_rfc3339)
            .transpose()
            .map_err(AppError::BadRequest)?;
        let to = self
            .to
            .as_deref()
            .map(parse_rfc3339)
            .transpose()
            .map_err(AppError::BadRequest)?;

        let query = match self.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(q) if q.len() > MAX_TITLE_BYTES => {
                return Err(AppError::BadRequest(format!(
                    "search query exceeds {MAX_TITLE_BYTES} bytes"
                )));
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

/// Fetch one page of paste metadata. Fetches `per_page + 1` rows to determine
/// whether a next page exists without a separate `COUNT(*)`.
pub async fn fetch_list(
    state: &AppState,
    opts: ListOptions,
    now: i64,
) -> Result<(Vec<PasteMeta>, bool), AppError> {
    db(state, move |conn| {
        let mut q = pastes::table
            .select((
                pastes::id,
                pastes::title,
                pastes::publish_at,
                pastes::created_at,
            ))
            .into_boxed::<Sqlite>();

        q = match (opts.status.revealed, opts.status.scheduled) {
            (true, true) => q,
            (true, false) => q.filter(pastes::publish_at.le(now)),
            (false, true) => q.filter(pastes::publish_at.gt(now)),
            // Nothing selected: match no rows.
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

        // `id` tie-break keeps pagination stable when the primary sort key ties.
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
    pub publish_at: String,
    pub created_at: String,
    pub revealed: bool,
}

pub fn meta_json(meta: &PasteMeta, now: i64) -> MetaJson {
    let id = encode_id(&meta.id);
    MetaJson {
        url: format!("/p/{id}"),
        id,
        title: meta.title.clone(),
        publish_at: format_rfc3339(meta.publish_at),
        created_at: format_rfc3339(meta.created_at),
        revealed: meta.publish_at <= now,
    }
}
