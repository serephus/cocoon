use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use diesel::prelude::*;
use diesel::result::{DatabaseErrorKind, Error as DieselError};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::clock::{format_rfc3339, now_unix, parse_rfc3339};
use crate::db::models::NewPaste;
use crate::db::schema::pastes;
use crate::error::{ApiError, AppError};
use crate::id::{compute_id, encode_id};
use crate::routes::{
    ListParams, ListResponse, MAX_CONTENT_BYTES, db, fetch_list, meta_json, validate_title,
};

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

enum InsertOutcome {
    Created,
    /// Exact duplicate: same id, same `(title, content, publish_at)`.
    Duplicate {
        created_at: i64,
    },
}

/// `POST /api/paste`
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateRequest>,
) -> Result<(StatusCode, Json<CreateResponse>), ApiError> {
    if req.content.len() > MAX_CONTENT_BYTES {
        return Err(AppError::PayloadTooLarge.into());
    }

    let title = req.title.unwrap_or_default();
    validate_title(&title)?;

    let publish_at = match &req.publish_at {
        Some(s) => parse_rfc3339(s).map_err(AppError::BadRequest)?,
        None => now_unix(),
    };
    let created_at = now_unix();

    let content = req.content.into_bytes();
    let id = compute_id(&state.secret, publish_at, title.as_bytes(), &content);
    let id_string = encode_id(&id);

    let title_for_db = title.clone();
    let outcome = db(&state, move |conn| {
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
            Ok(_) => Ok(InsertOutcome::Created),
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
                    Some((existing_title, existing_content, existing_publish_at, created))
                        if existing_title == title_for_db
                            && existing_content == content
                            && existing_publish_at == publish_at =>
                    {
                        Ok(InsertOutcome::Duplicate {
                            created_at: created,
                        })
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

    let (status, created) = match outcome {
        InsertOutcome::Created => (StatusCode::CREATED, created_at),
        InsertOutcome::Duplicate { created_at } => (StatusCode::OK, created_at),
    };

    Ok((
        status,
        Json(CreateResponse {
            id: id_string.clone(),
            url: format!("/p/{id_string}"),
            title,
            publish_at: format_rfc3339(publish_at),
            created_at: format_rfc3339(created),
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
