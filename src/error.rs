use axum::{
    Json,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;

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
    UpstreamStatus(String, StatusCode),
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
            Self::UpstreamStatus(_, status) => *status,
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
            Self::UpstreamTransport(_) | Self::UpstreamStatus(_, _) => "upstream_error",
        }
    }

    fn anthropic_error_type(&self) -> &'static str {
        match self {
            Self::Unauthorized(_) => "authentication_error",
            Self::BadRequest(_) | Self::NotFound(_) => "invalid_request_error",
            Self::UpstreamStatus(_, status)
                if *status == StatusCode::UNAUTHORIZED || *status == StatusCode::FORBIDDEN =>
            {
                "authentication_error"
            }
            Self::UpstreamStatus(_, status) if status.is_server_error() => "api_error",
            Self::NoRoute(_)
            | Self::Config(_)
            | Self::Internal(_)
            | Self::Database(_)
            | Self::Migration(_)
            | Self::UpstreamTransport(_) => "api_error",
            Self::UpstreamStatus(_, _) => "invalid_request_error",
        }
    }

    pub(crate) fn into_anthropic_response(self, request_id: &str) -> Response {
        let status = self.status_code();
        let body = AnthropicErrorBody {
            envelope_type: "error",
            error: AnthropicErrorPayload {
                message: self.to_string(),
                kind: self.anthropic_error_type().to_string(),
            },
        };
        let mut response = (status, Json(body)).into_response();
        if let Ok(value) = HeaderValue::from_str(request_id) {
            response.headers_mut().insert("request-id", value);
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
