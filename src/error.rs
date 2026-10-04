use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Errors that can cross the async/blocking boundary.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("content exceeds {max} characters")]
    PayloadTooLarge { max: usize },
    #[error("not found")]
    NotFound,
    #[error("too early: paste is not public yet")]
    TooEarly,
    #[error("database error: {0}")]
    Db(#[from] diesel::result::Error),
    #[error("connection pool error: {0}")]
    Pool(String),
    #[error("telegram error: {0}")]
    Telegram(String),
    #[error("a paste with identical content already exists: {0}")]
    Duplicate(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    pub fn status(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::PayloadTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::TooEarly => StatusCode::from_u16(425).expect("425 is valid"),
            AppError::Duplicate(_) => StatusCode::CONFLICT,
            AppError::Db(_) | AppError::Pool(_) | AppError::Telegram(_) | AppError::Internal(_) => {
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

impl From<teloxide::RequestError> for AppError {
    fn from(e: teloxide::RequestError) -> Self {
        AppError::Telegram(e.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        AppError::Telegram(e.to_string())
    }
}

/// Plain-text rendering, used by `/p/{id}` and the form routes.
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
