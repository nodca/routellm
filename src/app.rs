use std::time::Duration;

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{
        Request,
        header::{AUTHORIZATION, HeaderValue},
    },
    middleware::{self, Next},
    response::Response,
};
use reqwest::Client;

use crate::{
    config::{Config, CooldownPolicy, ManualInterventionPolicy},
    http,
    store::SqliteStore,
};

#[derive(Clone)]
pub struct AppState {
    pub store: SqliteStore,
    pub upstream_client: Client,
    pub upstream_stream_client: Client,
    pub master_key: Option<String>,
    pub cooldown_policy: CooldownPolicy,
    pub manual_intervention_policy: ManualInterventionPolicy,
}

pub async fn build_state(config: &Config) -> Result<AppState, crate::error::AppError> {
    let store = SqliteStore::connect(config).await?;
    let upstream_client =
        build_upstream_client(Some(Duration::from_secs(config.request_timeout_secs)))?;
    let upstream_stream_client = build_upstream_client(None)?;

    Ok(AppState {
        store,
        upstream_client,
        upstream_stream_client,
        master_key: config.master_key.clone(),
        cooldown_policy: config.cooldown_policy.clone(),
        manual_intervention_policy: config.manual_intervention_policy.clone(),
    })
}

fn build_upstream_client(timeout: Option<Duration>) -> Result<Client, crate::error::AppError> {
    let mut builder = Client::builder();
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    builder.build().map_err(|error| {
        crate::error::AppError::Internal(format!("failed to build http client: {error}"))
    })
}

pub fn build_router(state: AppState) -> Router {
    let auth_layer = middleware::from_fn_with_state(state.clone(), require_bearer_auth);
    let api_router = Router::new()
        .route("/routes/decision", axum::routing::get(http::route_decision))
        .route(
            "/routes",
            axum::routing::get(http::list_routes).post(http::create_route),
        )
        .route(
            "/routes/{route_id}",
            axum::routing::delete(http::delete_route),
        )
        .route(
            "/routes/{route_id}/channels",
            axum::routing::get(http::list_route_channels).post(http::create_route_channel),
        )
        .route(
            "/routes/{route_id}/logs",
            axum::routing::get(http::list_route_logs),
        )
        .route(
            "/channels/{channel_id}/enable",
            axum::routing::post(http::enable_channel),
        )
        .route(
            "/channels/{channel_id}/prefill",
            axum::routing::get(http::get_channel_prefill),
        )
        .route(
            "/channels/{channel_id}/probe",
            axum::routing::post(http::probe_channel),
        )
        .route(
            "/channels/{channel_id}",
            axum::routing::patch(http::update_channel).delete(http::delete_channel),
        )
        .route(
            "/channels/{channel_id}/disable",
            axum::routing::post(http::disable_channel),
        )
        .route(
            "/channels/{channel_id}/reset-cooldown",
            axum::routing::post(http::reset_channel_cooldown),
        );
    let v1_router = Router::new()
        .route("/responses", axum::routing::post(http::create_response))
        .route(
            "/chat/completions",
            axum::routing::post(http::create_chat_completion),
        )
        .route("/messages", axum::routing::post(http::create_message));
    let compat_router = Router::new()
        .route("/responses", axum::routing::post(http::create_response))
        .route(
            "/chat/completions",
            axum::routing::post(http::create_chat_completion),
        )
        .route("/messages", axum::routing::post(http::create_message))
        .route(
            "/v1beta/openai/chat/completions",
            axum::routing::post(http::create_chat_completion),
        )
        .route(
            "/v1beta/models/{tail}",
            axum::routing::post(http::create_gemini_content),
        )
        .route(
            "/v1/models/{tail}",
            axum::routing::post(http::create_gemini_content),
        );

    Router::new()
        .route("/healthz", axum::routing::get(http::healthz))
        .merge(compat_router.route_layer(auth_layer.clone()))
        .nest("/api", api_router.route_layer(auth_layer.clone()))
        .nest("/v1", v1_router.route_layer(auth_layer))
        .with_state(state)
}

async fn require_bearer_auth(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, crate::error::AppError> {
    let Some(expected) = state.master_key.as_deref() else {
        return Ok(next.run(request).await);
    };

    let token = parse_bearer_token(request.headers().get(AUTHORIZATION));
    if token == Some(expected) {
        return Ok(next.run(request).await);
    }

    Err(crate::error::AppError::Unauthorized(
        "missing or invalid bearer token".to_string(),
    ))
}

fn parse_bearer_token(header: Option<&HeaderValue>) -> Option<&str> {
    let value = header?.to_str().ok()?;
    value.strip_prefix("Bearer ")?.trim().into()
}

pub async fn build_app(config: &Config) -> Result<Router, crate::error::AppError> {
    Ok(build_router(build_state(config).await?))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tempfile::tempdir;
    use tower::ServiceExt;

    use crate::config::Config;

    use super::build_app;

    fn database_url(path: &std::path::Path) -> String {
        format!("sqlite://{}", path.display())
    }

    #[tokio::test]
    async fn healthz_is_not_protected_by_master_key() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(&db_path),
            request_timeout_secs: 30,
            master_key: Some("sk-llmrouter-test".to_string()),
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = build_app(&config).await.unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_and_v1_routes_require_matching_bearer_token_when_master_key_is_set() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(&db_path),
            request_timeout_secs: 30,
            master_key: Some("sk-llmrouter-test".to_string()),
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = build_app(&config).await.unwrap();

        let api_unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/routes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(api_unauthorized.status(), StatusCode::UNAUTHORIZED);

        let body = to_bytes(api_unauthorized.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["type"], "auth_error");

        let api_authorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/routes")
                    .header("Authorization", "Bearer sk-llmrouter-test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(api_authorized.status(), StatusCode::OK);

        let v1_unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/responses")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"model":"gpt-5.4","input":"ping"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(v1_unauthorized.status(), StatusCode::UNAUTHORIZED);

        let v1_authorized = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/responses")
                    .header("Authorization", "Bearer sk-llmrouter-test")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"model":"gpt-5.4","input":"ping"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(v1_authorized.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
