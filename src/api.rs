use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
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
}

#[derive(Debug, Serialize)]
pub struct CreateResponse {
    pub id: String,
    pub url: String,
    pub title: String,
    pub owner: String,
    pub publish_at: String,
    pub created_at: String,
}

async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateRequest>,
) -> Result<(StatusCode, Json<CreateResponse>), ApiError> {
    let title = req.title.unwrap_or_default().trim().to_string();
    let publish_at = match &req.publish_at {
        Some(value) => parse_publish_at(value).map_err(AppError::BadRequest)?,
        None => now_unix(),
    };
    let content = req.content;

    let (id, status) = match store::run(&state.pool, move |conn| {
        store::create_paste(conn, User::Anonymous, &content, &title, publish_at)
    })
    .await
    {
        Ok(id) => (id, StatusCode::CREATED),
        Err(AppError::Duplicate(id)) => (id, StatusCode::OK),
        Err(e) => return Err(e.into()),
    };

    let paste = store::run(&state.pool, {
        let id = id.clone();
        move |conn| Ok(store::get_paste(conn, &id)?)
    })
    .await?;
    let Some(paste) = paste else {
        return Err(AppError::Internal("created paste vanished".to_string()).into());
    };

    Ok((
        status,
        Json(CreateResponse {
            url: format!("/p/{}", paste.id),
            id: paste.id,
            title: paste.title,
            owner: paste.owner,
            publish_at: format_rfc3339(paste.publish_at),
            created_at: format_rfc3339(paste.created_at),
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
        pastes: rows.iter().map(|meta| meta_json(meta, now)).collect(),
    }))
}
