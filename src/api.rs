use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{FromRequest, Multipart, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};

use crate::clock::{format_rfc3339, now_unix, parse_publish_at};
use crate::error::{ApiError, AppError};
use crate::listing::{ListParams, ListResponse, fetch_list, meta_json};
use crate::state::AppState;
use crate::store;
use crate::user::User;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/paste", post(create))
        .route("/api/pastes", get(list))
}

#[derive(Debug, Deserialize)]
pub struct CreateRequest {
    pub content: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub publish_at: Option<String>,
    #[serde(default)]
    pub private: Option<bool>,
}

/// Fields accepted from `multipart/form-data` and
/// `application/x-www-form-urlencoded` bodies. `c` is the fars.ee-compatible
/// field name; `content` is accepted as an alias.
#[derive(Debug, Default, Deserialize)]
struct CreateForm {
    #[serde(default)]
    c: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    publish_at: Option<String>,
    #[serde(default)]
    private: Option<String>,
}

impl CreateForm {
    fn into_input(self, fallback_content: Option<String>) -> CreateInput {
        CreateInput {
            content: self.c.or(fallback_content).unwrap_or_default(),
            title: self.title,
            publish_at: self.publish_at,
            private: self.private.as_deref().map(parse_flag),
        }
    }
}

fn parse_flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Optional metadata that can come from the query string, so raw text bodies
/// can still set a title or a publish time.
#[derive(Debug, Default, Deserialize)]
struct CreateQuery {
    title: Option<String>,
    publish_at: Option<String>,
    private: Option<String>,
}

struct CreateInput {
    content: String,
    title: Option<String>,
    publish_at: Option<String>,
    private: Option<bool>,
}

async fn parse_create(
    state: &AppState,
    request: Request,
    query: CreateQuery,
) -> Result<CreateInput, AppError> {
    let mime = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    let mut input = match mime.as_str() {
        "application/json" => {
            let Json(req) = Json::<CreateRequest>::from_request(request, state)
                .await
                .map_err(|e| AppError::BadRequest(e.body_text()))?;
            CreateInput {
                content: req.content,
                title: req.title,
                publish_at: req.publish_at,
                private: req.private,
            }
        }
        "multipart/form-data" => {
            let mut multipart = Multipart::from_request(request, state)
                .await
                .map_err(|e| AppError::BadRequest(e.body_text()))?;
            let mut form = CreateForm::default();
            while let Some(field) = multipart
                .next_field()
                .await
                .map_err(|e| AppError::BadRequest(e.to_string()))?
            {
                let name = field.name().unwrap_or_default().to_string();
                let text = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                match name.as_str() {
                    "c" | "content" => form.c = Some(text),
                    "title" => form.title = Some(text),
                    "publish_at" => form.publish_at = Some(text),
                    "private" => form.private = Some(text),
                    _ => {}
                }
            }
            form.into_input(None)
        }
        "application/x-www-form-urlencoded" => {
            let bytes = Bytes::from_request(request, state)
                .await
                .map_err(|e| AppError::BadRequest(e.body_text()))?;
            let raw = String::from_utf8(bytes.to_vec())
                .map_err(|_| AppError::BadRequest("body must be valid UTF-8".to_string()))?;
            match serde_urlencoded::from_str::<CreateForm>(&raw) {
                Ok(form) => form.into_input(Some(raw)),
                // Not form-shaped (e.g. `curl --data-binary @-`), so the body
                // itself is the paste content.
                Err(_) => CreateInput {
                    content: raw,
                    title: None,
                    publish_at: None,
                    private: None,
                },
            }
        }
        // No content type, `text/plain`, or anything else: the raw body is the
        // content.
        _ => {
            let bytes = Bytes::from_request(request, state)
                .await
                .map_err(|e| AppError::BadRequest(e.body_text()))?;
            let content = String::from_utf8(bytes.to_vec())
                .map_err(|_| AppError::BadRequest("body must be valid UTF-8".to_string()))?;
            CreateInput {
                content,
                title: None,
                publish_at: None,
                private: None,
            }
        }
    };

    if input.title.is_none() {
        input.title = query.title;
    }
    if input.publish_at.is_none() {
        input.publish_at = query.publish_at;
    }
    if input.private.is_none() {
        input.private = query.private.as_deref().map(parse_flag);
    }
    Ok(input)
}

#[derive(Debug, Serialize)]
pub struct CreateResponse {
    pub id: String,
    pub url: String,
    pub title: String,
    pub owner: String,
    pub publish_at: String,
    pub created_at: String,
    pub private: bool,
}

async fn create(
    State(state): State<AppState>,
    Query(query): Query<CreateQuery>,
    request: Request,
) -> Result<(StatusCode, Json<CreateResponse>), ApiError> {
    let req = parse_create(&state, request, query).await?;
    let title = req.title.unwrap_or_default().trim().to_string();
    let publish_at = match req.publish_at {
        Some(value) if !value.trim().is_empty() => {
            parse_publish_at(&value).map_err(AppError::BadRequest)?
        }
        _ => now_unix(),
    };
    let content = req.content;
    let private = req.private.unwrap_or(false);
    let max_content_bytes = state.config.max_content_bytes;

    let (id, status) = match store::run(&state.pool, move |conn| {
        store::create_paste(
            conn,
            User::Anonymous,
            &content,
            &title,
            publish_at,
            private,
            max_content_bytes,
        )
    })
    .await
    {
        Ok(id) => (id, StatusCode::CREATED),
        Err(AppError::Duplicate(id)) => (id, StatusCode::OK),
        Err(e) => return Err(e.into()),
    };

    let paste = store::run(&state.pool, {
        let id = id.clone();
        move |db| store::get_paste(db, &id)
    })
    .await?;
    let Some(paste) = paste else {
        return Err(AppError::Internal("created paste vanished".to_string()).into());
    };

    Ok((
        status,
        Json(CreateResponse {
            url: state.config.absolute_url(&format!("/p/{}", paste.id)),
            id: paste.id,
            title: paste.title,
            owner: paste.owner,
            publish_at: format_rfc3339(paste.publish_at),
            created_at: format_rfc3339(paste.created_at),
            private: paste.private,
        }),
    ))
}

async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<ListResponse>, ApiError> {
    let opts = params.normalize()?;
    let now = now_unix();
    let (rows, has_next) = fetch_list(&state.pool, opts.clone(), now).await?;
    Ok(Json(ListResponse {
        page: opts.page,
        per_page: opts.per_page,
        has_next,
        pastes: rows
            .iter()
            .map(|meta| meta_json(meta, now, &state.config.public_url))
            .collect(),
    }))
}
