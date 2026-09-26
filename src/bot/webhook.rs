use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use teloxide::types::Update;

use crate::bot::commands;
use crate::error::AppError;
use crate::state::AppState;
use crate::store;

/// `POST /<webhook_path>/<secret>` — receives Telegram updates.
pub async fn handle(
    State(state): State<AppState>,
    Path(secret): Path<String>,
    headers: HeaderMap,
    Json(update): Json<Update>,
) -> StatusCode {
    if !verify(&state, &secret, &headers) {
        tracing::warn!("rejected telegram webhook with a bad secret");
        return StatusCode::UNAUTHORIZED;
    }

    let update_id = update.id.0 as i64;
    match store::run(&state.pool, move |conn| {
        store::register_update(conn, update_id)?;
        Ok(())
    })
    .await
    {
        Ok(()) => {}
        // Duplicate delivery: already handled.
        Err(AppError::Db(diesel::result::Error::DatabaseError(
            diesel::result::DatabaseErrorKind::UniqueViolation,
            _,
        ))) => return StatusCode::OK,
        Err(e) => {
            tracing::error!("failed to register update: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    }

    match commands::handle_update(&state, update).await {
        Ok(()) => StatusCode::OK,
        Err(e) => {
            tracing::error!("failed to handle update: {e}");
            // Release the claim so Telegram's retry reprocesses the update.
            let _ = store::run(&state.pool, move |conn| {
                store::forget_update(conn, update_id)?;
                Ok(())
            })
            .await;
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

fn verify(state: &AppState, path_secret: &str, headers: &HeaderMap) -> bool {
    let header_ok = headers
        .get("x-telegram-bot-api-secret-token")
        .and_then(|value| value.to_str().ok())
        .map(|value| constant_time_eq(value.as_bytes(), state.config.webhook_secret.as_bytes()))
        .unwrap_or(false);
    let path_ok = constant_time_eq(
        path_secret.as_bytes(),
        state.config.webhook_secret.as_bytes(),
    );
    header_ok && path_ok
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_equality() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secrez"));
        assert!(!constant_time_eq(b"secret", b"secret-longer"));
        assert!(constant_time_eq(b"", b""));
    }
}
