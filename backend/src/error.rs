//! Only deliberately sanitized errors cross a process/API boundary.
use crate::model::Time;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: &'static str,
    pub retry_at: Option<Time>,
}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
            retry_at: None,
        }
    }
    pub fn upstream() -> Self {
        Self::new(
            StatusCode::BAD_GATEWAY,
            "upstream_failure",
            "The upstream request failed; check source access and configuration.",
        )
    }
    pub fn invalid() -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_value",
            "The submitted values are not permitted by the configured project.",
        )
    }
    pub fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "The requested cached item or route was not found.",
        )
    }
    pub fn unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "source_not_ready",
            "The requested source is not configured or ready.",
        )
    }
    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "cache_failure",
            "The local cache is unavailable.",
        )
    }
    pub fn backoff(retry_at: Time) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "upstream_backoff",
            message: "The upstream service requested a delay before retrying.",
            retry_at: Some(retry_at),
        }
    }
}
#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}
#[derive(Serialize)]
struct ErrorDetail {
    code: &'static str,
    message: &'static str,
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: ErrorDetail {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}
