use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Errors raised anywhere in the request path.
///
/// All variants are `Send + 'static` so they can cross the `spawn_blocking`
/// boundary used for database work.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("payload too large: content exceeds 65536 bytes")]
    PayloadTooLarge,
    #[error("not found")]
    NotFound,
    #[error("too early: paste is not public yet")]
    TooEarly,
    #[error("conflict: this id already maps to different content")]
    Conflict,
    #[error("database error: {0}")]
    Db(#[from] diesel::result::Error),
    #[error("connection pool error: {0}")]
    Pool(String),
    #[error("template error: {0}")]
    Template(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<askama::Error> for AppError {
    fn from(e: askama::Error) -> Self {
        AppError::Template(e.to_string())
    }
}

impl AppError {
    pub fn status(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::TooEarly => StatusCode::from_u16(425).expect("425 is a valid status"),
            AppError::Conflict => StatusCode::CONFLICT,
            AppError::Db(_) | AppError::Pool(_) | AppError::Template(_) | AppError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    fn log_if_internal(&self) {
        if self.status() == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = %self, "internal error");
        }
    }
}

/// Plain-text rendering, used by `/p/{id}`.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        self.log_if_internal();
        (self.status(), self.to_string()).into_response()
    }
}

/// JSON rendering, used by `/api/*`.
pub struct ApiError(pub AppError);

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        self.0.log_if_internal();
        let status = self.0.status();
        (status, Json(json!({ "error": self.0.to_string() }))).into_response()
    }
}
