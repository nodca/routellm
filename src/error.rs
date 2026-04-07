use axum::{
    Json,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct UpstreamErrorMetadata {
    pub request_id: Option<String>,
    pub retry_after: Option<String>,
    pub should_retry: Option<String>,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    NoRoute(String),
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    UpstreamTransport(String),
    #[error("{0}")]
    UpstreamStatus(String, StatusCode, Option<UpstreamErrorMetadata>),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("{0}")]
    Internal(String),
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    message: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Serialize)]
struct AnthropicErrorBody {
    #[serde(rename = "type")]
    envelope_type: &'static str,
    error: AnthropicErrorPayload,
}

#[derive(Debug, Serialize)]
struct AnthropicErrorPayload {
    message: String,
    #[serde(rename = "type")]
    kind: String,
}

impl AppError {
    pub(crate) fn status_code(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::NoRoute(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Config(_) | Self::Internal(_) | Self::Database(_) | Self::Migration(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            Self::UpstreamTransport(_) => StatusCode::BAD_GATEWAY,
            Self::UpstreamStatus(_, status, _) => *status,
        }
    }

    fn error_type(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad_request",
            Self::Unauthorized(_) => "auth_error",
            Self::NotFound(_) => "not_found",
            Self::NoRoute(_) => "routing_error",
            Self::Config(_) | Self::Internal(_) | Self::Database(_) | Self::Migration(_) => {
                "internal_error"
            }
            Self::UpstreamTransport(_) | Self::UpstreamStatus(_, _, _) => "upstream_error",
        }
    }

    fn anthropic_error_type(&self) -> &'static str {
        match self {
            Self::Unauthorized(_) => "authentication_error",
            Self::BadRequest(_) | Self::NotFound(_) => "invalid_request_error",
            Self::UpstreamStatus(_, status, _)
                if *status == StatusCode::UNAUTHORIZED || *status == StatusCode::FORBIDDEN =>
            {
                "authentication_error"
            }
            Self::UpstreamStatus(_, status, _) if status.is_server_error() => "api_error",
            Self::NoRoute(_)
            | Self::Config(_)
            | Self::Internal(_)
            | Self::Database(_)
            | Self::Migration(_)
            | Self::UpstreamTransport(_) => "api_error",
            Self::UpstreamStatus(_, _, _) => "invalid_request_error",
        }
    }

    fn upstream_error_metadata(&self) -> Option<&UpstreamErrorMetadata> {
        match self {
            Self::UpstreamStatus(_, _, metadata) => metadata.as_ref(),
            _ => None,
        }
    }

    pub(crate) fn into_anthropic_response(self, request_id: &str) -> Response {
        let status = self.status_code();
        let anthropic_error_type = self.anthropic_error_type().to_string();
        let upstream_metadata = self.upstream_error_metadata().cloned();
        let body = AnthropicErrorBody {
            envelope_type: "error",
            error: AnthropicErrorPayload {
                message: self.to_string(),
                kind: anthropic_error_type,
            },
        };
        let mut response = (status, Json(body)).into_response();
        let response_request_id = upstream_metadata
            .as_ref()
            .and_then(|metadata| metadata.request_id.as_deref())
            .unwrap_or(request_id);
        if let Ok(value) = HeaderValue::from_str(response_request_id) {
            response.headers_mut().insert("request-id", value);
        }
        if let Some(metadata) = upstream_metadata.as_ref() {
            if let Some(retry_after) = metadata.retry_after.as_deref()
                && let Ok(value) = HeaderValue::from_str(retry_after)
            {
                response.headers_mut().insert("retry-after", value);
            }
            if let Some(should_retry) = metadata.should_retry.as_deref()
                && let Ok(value) = HeaderValue::from_str(should_retry)
            {
                response.headers_mut().insert("x-should-retry", value);
            }
        }
        response
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let body = ErrorBody {
            error: ErrorPayload {
                message: self.to_string(),
                kind: self.error_type().to_string(),
            },
        };
        (status, Json(body)).into_response()
    }
}
