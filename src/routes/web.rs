use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{Html, IntoResponse, Response};
use diesel::prelude::*;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;

use crate::AppState;
use crate::clock::{format_rfc3339, humanize_delta, now_unix};
use crate::db::schema::pastes;
use crate::error::AppError;
use crate::id::{decode_id, encode_id};
use crate::routes::{ListOptions, ListParams, db, fetch_list};

/// Query parameters for the HTML listing.
///
/// The visibility filters are two independent flags:
/// `revealed=0|1` and `private=0|1`. They are always written explicitly by the
/// page's own links; an absent flag defaults to included. `q` is a
/// case-insensitive substring match on the title.
#[derive(Debug, Default, Deserialize)]
pub struct WebListParams {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub revealed: Option<String>,
    pub private: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub q: Option<String>,
}

impl WebListParams {
    fn normalize(&self) -> Result<ListOptions, AppError> {
        let show_revealed = self.revealed.as_deref() != Some("0");
        let show_private = self.private.as_deref() != Some("0");
        let status = match (show_revealed, show_private) {
            (true, true) => "all",
            (true, false) => "revealed",
            (false, true) => "scheduled",
            (false, false) => "none",
        };

        ListParams {
            page: self.page,
            per_page: self.per_page,
            status: Some(status.to_string()),
            sort: self.sort.clone(),
            order: self.order.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
            q: self.q.clone(),
        }
        .normalize()
    }
}

/// `GET /`
pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<WebListParams>,
) -> Result<Response, PageError> {
    let opts = params.normalize()?;
    let now = now_unix();
    let (rows, has_next) = fetch_list(&state, opts.clone(), now).await?;

    let rows = rows
        .iter()
        .map(|m| {
            let id = encode_id(&m.id);
            ListRow {
                id,
                title: if m.title.is_empty() {
                    "—".to_string()
                } else {
                    m.title.clone()
                },
                publish_at: format_rfc3339(m.publish_at),
                created_at: format_rfc3339(m.created_at),
                revealed: m.publish_at <= now,
                reveal_in: if m.publish_at > now {
                    humanize_delta(m.publish_at - now)
                } else {
                    String::new()
                },
            }
        })
        .collect();

    let tmpl = ListTemplate {
        pastes: rows,
        page: opts.page,
        per_page: opts.per_page,
        show_revealed: opts.status.revealed,
        show_private: opts.status.scheduled,
        sort: opts.sort.as_str().to_string(),
        order: opts.order.as_str().to_string(),
        query: opts.query.clone().unwrap_or_default(),
        has_next,
        has_prev: opts.page > 1,
    };

    let html = tmpl.render().map_err(AppError::from)?;
    Ok(Html(html).into_response())
}

/// `GET /p/{id}` — raw text once the paste is public.
pub async fn read(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let Some(id) = decode_id(&id) else {
        return Err(AppError::NotFound);
    };

    let now = now_unix();
    let row = db(&state, move |conn| {
        pastes::table
            .filter(pastes::id.eq(id.to_vec()))
            .select((pastes::content, pastes::publish_at))
            .first::<(Vec<u8>, i64)>(conn)
            .optional()
            .map_err(AppError::Db)
    })
    .await?;

    let Some((content, publish_at)) = row else {
        return Err(AppError::NotFound);
    };

    if now < publish_at {
        return Err(AppError::TooEarly);
    }

    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        content,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "list.html")]
struct ListTemplate {
    pastes: Vec<ListRow>,
    page: i64,
    per_page: i64,
    show_revealed: bool,
    show_private: bool,
    sort: String,
    order: String,
    query: String,
    has_next: bool,
    has_prev: bool,
}

struct ListRow {
    id: String,
    title: String,
    publish_at: String,
    created_at: String,
    revealed: bool,
    reveal_in: String,
}

impl ListTemplate {
    /// Build a listing URL with the given state, preserving the search term
    /// (omitted when empty).
    fn build_url(
        &self,
        page: i64,
        revealed: bool,
        private: bool,
        sort: &str,
        order: &str,
        query: &str,
    ) -> String {
        let mut url = format!(
            "/?page={page}&per_page={}&revealed={}&private={}&sort={sort}&order={order}",
            self.per_page, revealed as u8, private as u8
        );
        if !query.is_empty() {
            url.push_str("&q=");
            url.push_str(&utf8_percent_encode(query, NON_ALPHANUMERIC).to_string());
        }
        url
    }

    fn url(&self, page: i64, revealed: bool, private: bool, sort: &str, order: &str) -> String {
        self.build_url(page, revealed, private, sort, order, &self.query)
    }

    fn clear_search_url(&self) -> String {
        self.build_url(
            1,
            self.show_revealed,
            self.show_private,
            &self.sort,
            &self.order,
            "",
        )
    }

    fn toggle_revealed_url(&self) -> String {
        self.url(
            1,
            !self.show_revealed,
            self.show_private,
            &self.sort,
            &self.order,
        )
    }

    fn toggle_private_url(&self) -> String {
        self.url(
            1,
            self.show_revealed,
            !self.show_private,
            &self.sort,
            &self.order,
        )
    }

    /// Clicking the active column flips its direction; any other column becomes
    /// the active one with a descending default. Filtering resets to page 1.
    fn sort_target_url(&self, key: &str) -> String {
        let (sort, order) = if self.sort == key {
            (key, if self.order == "asc" { "desc" } else { "asc" })
        } else {
            (key, "desc")
        };
        self.url(1, self.show_revealed, self.show_private, sort, order)
    }

    fn arrow_for(&self, key: &str) -> &'static str {
        if self.sort != key {
            "↕"
        } else if self.order == "asc" {
            "▲"
        } else {
            "▼"
        }
    }

    fn created_url(&self) -> String {
        self.sort_target_url("created_at")
    }

    fn created_arrow(&self) -> &'static str {
        self.arrow_for("created_at")
    }

    fn publish_url(&self) -> String {
        self.sort_target_url("publish_at")
    }

    fn publish_arrow(&self) -> &'static str {
        self.arrow_for("publish_at")
    }

    fn prev_url(&self) -> String {
        self.url(
            self.page - 1,
            self.show_revealed,
            self.show_private,
            &self.sort,
            &self.order,
        )
    }

    fn next_url(&self) -> String {
        self.url(
            self.page + 1,
            self.show_revealed,
            self.show_private,
            &self.sort,
            &self.order,
        )
    }

    fn revealed_flag(&self) -> u8 {
        self.show_revealed as u8
    }

    fn private_flag(&self) -> u8 {
        self.show_private as u8
    }

    fn has_query(&self) -> bool {
        !self.query.is_empty()
    }
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorTemplate {
    status: u16,
    message: String,
}

/// Renders `/` errors as an HTML page.
pub struct PageError(pub AppError);

impl From<AppError> for PageError {
    fn from(e: AppError) -> Self {
        PageError(e)
    }
}

impl IntoResponse for PageError {
    fn into_response(self) -> Response {
        if self.0.status() == axum::http::StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = %self.0, "internal error");
        }
        let status = self.0.status();
        let tmpl = ErrorTemplate {
            status: status.as_u16(),
            message: self.0.to_string(),
        };
        match tmpl.render() {
            Ok(html) => (status, Html(html)).into_response(),
            Err(_) => (status, self.0.to_string()).into_response(),
        }
    }
}
