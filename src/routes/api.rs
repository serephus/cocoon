use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::clock::{format_rfc3339, now_unix, parse_rfc3339};
use crate::error::{ApiError, AppError};
use crate::routes::{ListParams, ListResponse, create_paste, fetch_list, meta_json};

#[derive(Debug, Deserialize)]
pub struct CreateRequest {
    pub content: String,
    /// Short, public label shown in listings. Visible immediately, even before
    /// the paste is revealed.
    #[serde(default)]
    pub title: Option<String>,
    /// RFC 3339 UTC. Defaults to "now" (an immediately public paste).
    #[serde(default)]
    pub publish_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateResponse {
    pub id: String,
    pub url: String,
    pub title: String,
    pub publish_at: String,
    pub created_at: String,
}

/// `POST /api/paste`
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateRequest>,
) -> Result<(StatusCode, Json<CreateResponse>), ApiError> {
    let title = req.title.unwrap_or_default();
    let publish_at = match &req.publish_at {
        Some(s) => parse_rfc3339(s).map_err(AppError::BadRequest)?,
        None => now_unix(),
    };

    let (created, paste) = create_paste(&state, title, req.content, publish_at).await?;

    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };

    Ok((
        status,
        Json(CreateResponse {
            id: paste.id.clone(),
            url: format!("/p/{}", paste.id),
            title: paste.title,
            publish_at: format_rfc3339(paste.publish_at),
            created_at: format_rfc3339(paste.created_at),
        }),
    ))
}

/// `GET /api/pastes`
pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<ListResponse>, ApiError> {
    let opts = params.normalize()?;
    let now = now_unix();
    let (rows, has_next) = fetch_list(&state, opts.clone(), now).await?;

    Ok(Json(ListResponse {
        page: opts.page,
        per_page: opts.per_page,
        has_next,
        pastes: rows.iter().map(|m| meta_json(m, now)).collect(),
    }))
}
