use axum::Router;
use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::http::Request;
use axum::routing::{get, post};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::{DefaultOnFailure, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::bot::webhook;
use crate::state::AppState;
use crate::{api, web};

pub fn app(state: AppState) -> Router {
    let webhook_path = format!(
        "{}/{{secret}}",
        state.config.webhook_path.trim_end_matches('/')
    );
    // Leave room for JSON/urlencoded escaping overhead on top of the content.
    let body_limit = state
        .config
        .max_content_bytes
        .saturating_mul(4)
        .saturating_add(64 * 1024);

    Router::new()
        .merge(web::router())
        .merge(api::router())
        .route("/healthz", get(healthz))
        .route(&webhook_path, post(webhook::handle))
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(PropagateRequestIdLayer::x_request_id())
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
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}
