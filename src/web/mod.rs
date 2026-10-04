use askama::Template;
use axum::Form;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;

use crate::clock::{format_rfc3339, now_unix, parse_publish_at};
use crate::error::AppError;
use crate::listing::{ListOptions, ListParams, fetch_list};
use crate::state::AppState;
use crate::store;
use crate::user::{User, can_read};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list))
        .route("/p/{id}", get(read))
        .route("/new", get(new_form).post(create))
        .route("/created/{id}", get(created))
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

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

async fn list(
    State(state): State<AppState>,
    Query(params): Query<WebListParams>,
) -> Result<Response, PageError> {
    let opts = params.normalize()?;
    let now = now_unix();
    let (rows, has_next) = fetch_list(&state.pool, opts.clone(), now).await?;

    let pastes = rows
        .iter()
        .map(|meta| {
            let revealed = meta.publish_at <= now;
            ListRow {
                id: meta.id.clone(),
                title: if meta.title.is_empty() {
                    "—".to_string()
                } else {
                    meta.title.clone()
                },
                owner: if meta.owner == "anonymous" {
                    "anon".to_string()
                } else {
                    "telegram".to_string()
                },
                publish_at: format_rfc3339(meta.publish_at),
                created_at: format_rfc3339(meta.created_at),
                revealed,
                reveal_in: if revealed {
                    String::new()
                } else {
                    humanize(meta.publish_at - now)
                },
                subscribe_url: if revealed {
                    String::new()
                } else {
                    subscribe_url(&state, &meta.id).unwrap_or_default()
                },
            }
        })
        .collect();

    let tmpl = ListTemplate {
        pastes,
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
    Ok(Html(tmpl.render()?).into_response())
}

fn subscribe_url(state: &AppState, id: &str) -> Option<String> {
    if state.bot_username.is_empty() {
        None
    } else {
        Some(format!(
            "https://t.me/{}?start=subscribe_{id}",
            state.bot_username
        ))
    }
}

fn humanize(seconds: i64) -> String {
    if seconds <= 0 {
        return "now".to_string();
    }
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

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
    owner: String,
    publish_at: String,
    created_at: String,
    revealed: bool,
    reveal_in: String,
    subscribe_url: String,
}

impl ListTemplate {
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

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

async fn read(State(state): State<AppState>, Path(id): Path<String>) -> Result<Response, AppError> {
    let paste = store::run(&state.pool, move |conn| Ok(store::get_paste(conn, &id)?)).await?;
    let Some(paste) = paste else {
        return Err(AppError::NotFound);
    };
    if !can_read(User::Anonymous, &paste.owner, paste.publish_at, now_unix()) {
        return Err(AppError::TooEarly);
    }
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        paste.content,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Creation
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct NewPasteForm {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub publish_at: Option<String>,
}

impl NewPasteForm {
    fn publish_at_raw(&self) -> &str {
        self.publish_at.as_deref().unwrap_or("")
    }
}

async fn new_form() -> Result<Response, PageError> {
    render_new(None, "", "", "")
}

async fn create(
    State(state): State<AppState>,
    Form(form): Form<NewPasteForm>,
) -> Result<Response, PageError> {
    let raw_time = form.publish_at_raw().trim();
    let publish_at = if raw_time.is_empty() {
        now_unix()
    } else {
        match parse_publish_at(raw_time) {
            Ok(value) => value,
            Err(message) => {
                return render_new(Some(&message), &form.title, &form.content, raw_time);
            }
        }
    };
    let title = form.title.trim().to_string();

    let content = form.content.clone();
    match store::run(&state.pool, move |conn| {
        store::create_paste(conn, User::Anonymous, &content, &title, publish_at)
    })
    .await
    {
        Ok(id) => Ok(Redirect::to(&format!("/created/{id}")).into_response()),
        // An identical anonymous paste already exists; point at it.
        Err(AppError::Duplicate(id)) => Ok(Redirect::to(&format!("/created/{id}")).into_response()),
        Err(AppError::PayloadTooLarge { max }) => render_new(
            Some(&format!("Content is limited to {max} characters.")),
            &form.title,
            &form.content,
            form.publish_at_raw(),
        ),
        Err(AppError::BadRequest(message)) => render_new(
            Some(&message),
            &form.title,
            &form.content,
            form.publish_at_raw(),
        ),
        Err(e) => Err(e.into()),
    }
}

async fn created(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, PageError> {
    let paste = store::run(&state.pool, {
        let id = id.clone();
        move |conn| Ok(store::get_paste(conn, &id)?)
    })
    .await?;
    let Some(paste) = paste else {
        return Err(PageError(AppError::NotFound));
    };
    let tmpl = CreatedTemplate {
        id: paste.id.clone(),
        title: paste.title.clone(),
        publish_at: format_rfc3339(paste.publish_at),
        read_url: format!("/p/{}", paste.id),
        subscribe_url: subscribe_url(&state, &paste.id).unwrap_or_default(),
        revealed: paste.publish_at <= now_unix(),
    };
    Ok(Html(tmpl.render()?).into_response())
}

fn render_new(
    error: Option<&str>,
    title: &str,
    content: &str,
    publish_at: &str,
) -> Result<Response, PageError> {
    let tmpl = NewTemplate {
        error: error.unwrap_or_default().to_string(),
        title: title.to_string(),
        content: content.to_string(),
        publish_at: publish_at.to_string(),
    };
    Ok(Html(tmpl.render()?).into_response())
}

#[derive(Template)]
#[template(path = "new.html")]
struct NewTemplate {
    error: String,
    title: String,
    content: String,
    publish_at: String,
}

#[derive(Template)]
#[template(path = "created.html")]
struct CreatedTemplate {
    id: String,
    title: String,
    publish_at: String,
    read_url: String,
    subscribe_url: String,
    revealed: bool,
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorTemplate {
    status: u16,
    message: String,
}

/// Renders document routes' errors as an HTML page.
pub struct PageError(pub AppError);

impl From<AppError> for PageError {
    fn from(e: AppError) -> Self {
        PageError(e)
    }
}

impl From<askama::Error> for PageError {
    fn from(e: askama::Error) -> Self {
        PageError(AppError::Internal(e.to_string()))
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
