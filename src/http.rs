use std::{
    collections::{HashMap, HashSet},
    error::Error as StdError,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{
        HeaderMap, StatusCode,
        header::{CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, TRANSFER_ENCODING, UPGRADE},
    },
    response::Response,
};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::{
    app::AppState,
    config::{CooldownPolicy, ManualInterventionPolicy},
    domain::{
        AdminRouteRow, ChannelRow, ChannelRuntimeStats, ModelRouteRow, RequestLogRow,
        RequestLogWrite,
    },
    error::AppError,
    protocol::Protocol,
    routing,
};

const AUTO_COOLDOWN_FAILURE_THRESHOLD: i64 = 3;

#[derive(Debug, Deserialize)]
pub struct RouteDecisionQuery {
    model: String,
    protocol: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RouteLogsQuery {
    limit: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CreateRouteChannelRequest {
    base_url: String,
    api_key: String,
    upstream_model: Option<String>,
    protocol: String,
    priority: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CreateRouteRequest {
    route_model: String,
    cooldown_seconds: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct UpdateChannelRequest {
    base_url: String,
    api_key: String,
    upstream_model: String,
    protocol: String,
    priority: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseAdapter {
    Passthrough,
    ResponsesToChatCompletions,
}

#[derive(Debug)]
struct UpstreamDispatch {
    upstream_protocol: Protocol,
    payload: Value,
    response_adapter: ResponseAdapter,
}

fn build_upstream_url(base_url: &str, protocol: Protocol) -> String {
    let trimmed = base_url.trim_end_matches('/');
    match protocol {
        Protocol::Responses => {
            if trimmed.ends_with("/v1/responses") {
                trimmed.to_string()
            } else if trimmed.ends_with("/v1") {
                format!("{trimmed}/responses")
            } else {
                format!("{trimmed}/v1/responses")
            }
        }
        Protocol::ChatCompletions => {
            if trimmed.ends_with("/v1/chat/completions") || trimmed.ends_with("/chat/completions") {
                trimmed.to_string()
            } else if trimmed.ends_with("/v1") || trimmed.ends_with("/openai") {
                format!("{trimmed}/chat/completions")
            } else {
                format!("{trimmed}/v1/chat/completions")
            }
        }
        Protocol::Messages => {
            if trimmed.ends_with("/v1/messages") || trimmed.ends_with("/messages") {
                trimmed.to_string()
            } else if trimmed.ends_with("/v1") {
                format!("{trimmed}/messages")
            } else {
                format!("{trimmed}/v1/messages")
            }
        }
    }
}

#[derive(Debug)]
struct ProbeOutcome {
    http_status: Option<u16>,
    error_message: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct TokenUsage {
    input_tokens: i64,
    output_tokens: i64,
    total_tokens: i64,
}

const FIRST_TOKEN_TIMEOUT_SECS: u64 = 50;
const FIRST_TOKEN_TIMEOUT_INITIAL_COOLDOWN_SECS: i64 = 120;
const FIRST_TOKEN_TIMEOUT_MAX_COOLDOWN_SECS: i64 = 1920;

pub async fn healthz() -> Json<Value> {
    Json(json!({ "ok": true }))
}

pub async fn route_decision(
    State(state): State<AppState>,
    Query(query): Query<RouteDecisionQuery>,
) -> Result<Json<Value>, AppError> {
    let request_protocol = query
        .protocol
        .as_deref()
        .map(Protocol::parse)
        .transpose()?
        .unwrap_or(Protocol::Responses);
    let route = state.store.find_route(&query.model).await?;
    let channels = state.store.load_channels(route.id).await?;
    let decision =
        routing::decide_route(&query.model, &route, channels, request_protocol, now_ts())?;
    let view = routing::to_decision_view(&query.model, &route, &decision);

    Ok(Json(json!({
        "success": true,
        "decision": view
    })))
}

pub async fn list_routes(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let routes = state
        .store
        .list_routes(now_ts())
        .await?
        .into_iter()
        .map(route_admin_view)
        .collect::<Vec<_>>();

    Ok(Json(json!({ "data": routes })))
}

pub async fn list_route_channels(
    State(state): State<AppState>,
    Path(route_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let route = state.store.get_route(route_id).await?;
    let now = now_ts();
    let channels = state.store.load_channels(route_id).await?;
    let runtime_stats = state.store.list_channel_runtime_stats(route_id).await?;
    let candidates = routing::inspect_candidates(channels, None, now)
        .into_iter()
        .map(|candidate| {
            channel_admin_view(
                &candidate,
                now,
                runtime_stats.get(&candidate.channel.channel_id),
            )
        })
        .collect::<Vec<_>>();

    Ok(Json(json!({
        "data": {
            "route": route_detail_view(&route),
            "channels": candidates
        }
    })))
}

pub async fn list_route_logs(
    State(state): State<AppState>,
    Path(route_id): Path<i64>,
    Query(query): Query<RouteLogsQuery>,
) -> Result<Json<Value>, AppError> {
    let route = state.store.get_route(route_id).await?;
    let logs = state
        .store
        .list_route_request_logs(route_id, query.limit.unwrap_or(20))
        .await?
        .into_iter()
        .map(request_log_admin_view)
        .collect::<Vec<_>>();

    Ok(Json(json!({
        "data": {
            "route": route_detail_view(&route),
            "logs": logs
        }
    })))
}

pub async fn delete_route(
    State(state): State<AppState>,
    Path(route_id): Path<i64>,
) -> Result<Response<Body>, AppError> {
    let outcome = state.store.delete_route(route_id).await?;

    build_json_response(
        StatusCode::OK,
        &json!({
            "data": {
                "route_id": outcome.route.id,
                "route_model": outcome.route.model_pattern,
                "deleted_channel_count": outcome.deleted_channel_count,
                "deleted": true
            }
        }),
    )
}

pub async fn get_channel_prefill(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let channel = state.store.load_channel(channel_id).await?;

    Ok(Json(json!({
        "data": {
            "base_url": channel.site_base_url,
            "api_key": channel.account_api_key,
            "upstream_model": channel.upstream_model,
            "protocol": channel.protocol
        }
    })))
}

pub async fn probe_channel(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
) -> Result<Response<Body>, AppError> {
    let channel = state.store.load_channel(channel_id).await?;
    let started_at = Instant::now();
    let outcome = execute_channel_probe(&state, &channel).await;
    let latency_ms = started_at.elapsed().as_millis() as i64;

    match outcome {
        Ok(http_status) => {
            state
                .store
                .mark_channel_success(channel_id, http_status, None)
                .await?;
            let updated = state.store.load_channel(channel_id).await?;
            let now = now_ts();

            build_json_response(
                StatusCode::OK,
                &json!({
                    "data": channel_admin_json(&state, updated, now).await?,
                    "meta": {
                        "probe": {
                            "ok": true,
                            "http_status": http_status,
                            "latency_ms": latency_ms
                        }
                    }
                }),
            )
        }
        Err(outcome) => {
            state
                .store
                .mark_channel_failure(
                    channel_id,
                    outcome.http_status,
                    outcome.error_message.as_deref().unwrap_or("probe failed"),
                    None,
                    true,
                )
                .await?;
            let updated = state.store.load_channel(channel_id).await?;
            let now = now_ts();

            build_json_response(
                StatusCode::OK,
                &json!({
                    "data": channel_admin_json(&state, updated, now).await?,
                    "meta": {
                        "probe": {
                            "ok": false,
                            "http_status": outcome.http_status,
                            "latency_ms": latency_ms,
                            "error_message": outcome.error_message
                        }
                    }
                }),
            )
        }
    }
}

pub async fn create_route_channel(
    State(state): State<AppState>,
    Path(route_id): Path<i64>,
    Json(payload): Json<CreateRouteChannelRequest>,
) -> Result<Response<Body>, AppError> {
    let route = state.store.get_route(route_id).await?;
    let protocol = parse_required_protocol(&payload.protocol)?;
    let channel = state
        .store
        .create_channel_for_route(
            &route,
            &payload.base_url,
            &payload.api_key,
            payload.upstream_model.as_deref(),
            protocol.as_str(),
            payload.priority.unwrap_or(0),
        )
        .await?;
    let now = now_ts();

    build_json_response(
        StatusCode::CREATED,
        &json!({
            "data": channel_admin_json(&state, channel, now).await?
        }),
    )
}

pub async fn create_route(
    State(state): State<AppState>,
    Json(payload): Json<CreateRouteRequest>,
) -> Result<Response<Body>, AppError> {
    let (route, created) = state
        .store
        .create_or_get_route(
            &payload.route_model,
            payload.cooldown_seconds.unwrap_or(300),
        )
        .await?;

    build_json_response(
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        &json!({
            "data": {
                "created": created,
                "route": route_detail_view(&route)
            }
        }),
    )
}

pub async fn enable_channel(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
) -> Result<Response<Body>, AppError> {
    update_channel_state_response(
        &state,
        state.store.set_channel_enabled(channel_id, true).await?,
        now_ts(),
    )
    .await
}

pub async fn disable_channel(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
) -> Result<Response<Body>, AppError> {
    update_channel_state_response(
        &state,
        state.store.set_channel_enabled(channel_id, false).await?,
        now_ts(),
    )
    .await
}

pub async fn reset_channel_cooldown(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
) -> Result<Response<Body>, AppError> {
    update_channel_state_response(
        &state,
        state.store.reset_channel_cooldown(channel_id).await?,
        now_ts(),
    )
    .await
}

pub async fn update_channel(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    Json(payload): Json<UpdateChannelRequest>,
) -> Result<Response<Body>, AppError> {
    let protocol = parse_required_protocol(&payload.protocol)?;
    update_channel_state_response(
        &state,
        state
            .store
            .update_channel(
                channel_id,
                &payload.base_url,
                &payload.api_key,
                &payload.upstream_model,
                protocol.as_str(),
                payload.priority,
            )
            .await?,
        now_ts(),
    )
    .await
}

pub async fn delete_channel(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
) -> Result<Response<Body>, AppError> {
    let outcome = state.store.delete_channel(channel_id).await?;
    let channel = outcome.channel;

    build_json_response(
        StatusCode::OK,
        &json!({
            "data": {
                "channel_id": channel.channel_id,
                "route_id": channel.route_id,
                "route_model": outcome.route_model,
                "channel_label": channel.channel_label,
                "site_name": channel.site_name,
                "route_deleted": false,
                "deleted": true
            }
        }),
    )
}

pub async fn create_response(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> Result<Response<Body>, AppError> {
    let requested_model = payload
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadRequest("field `model` is required".to_string()))?
        .to_string();

    proxy_request(state, requested_model, payload, Protocol::Responses).await
}

pub async fn create_chat_completion(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> Result<Response<Body>, AppError> {
    let requested_model = payload
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadRequest("field `model` is required".to_string()))?
        .to_string();
    proxy_request(state, requested_model, payload, Protocol::ChatCompletions).await
}

pub async fn create_message(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> Result<Response<Body>, AppError> {
    let requested_model = payload
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadRequest("field `model` is required".to_string()))?
        .to_string();

    proxy_request(state, requested_model, payload, Protocol::Messages).await
}

async fn proxy_request(
    state: AppState,
    requested_model: String,
    payload: Value,
    downstream_protocol: Protocol,
) -> Result<Response<Body>, AppError> {
    let route = state.store.find_route(&requested_model).await?;
    let channels = state.store.load_channels(route.id).await?;
    let request_id = Uuid::new_v4().to_string();
    let candidates = routing::inspect_candidates(channels, Some(downstream_protocol), now_ts());
    let ordered_channels = routing::ordered_eligible_channels(&candidates);
    if ordered_channels.is_empty() {
        if let Some(selected) =
            select_last_chance_channel(&candidates, downstream_protocol, now_ts())
        {
            return attempt_proxy_request(
                state,
                route,
                selected,
                &request_id,
                &requested_model,
                payload,
                downstream_protocol,
            )
            .await;
        }
        return Err(AppError::NoRoute(format!(
            "no eligible channel for model: {requested_model}"
        )));
    }

    let mut last_error: Option<AppError> = None;
    for selected in ordered_channels {
        match attempt_proxy_request(
            state.clone(),
            route.clone(),
            selected,
            &request_id,
            &requested_model,
            payload.clone(),
            downstream_protocol,
        )
        .await
        {
            Ok(response) => return Ok(response),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        AppError::NoRoute(format!("no eligible channel for model: {requested_model}"))
    }))
}

async fn attempt_proxy_request(
    state: AppState,
    route: ModelRouteRow,
    selected: ChannelRow,
    request_id: &str,
    requested_model: &str,
    payload: Value,
    downstream_protocol: Protocol,
) -> Result<Response<Body>, AppError> {
    let channel_protocol = Protocol::parse(&selected.protocol)?;
    let dispatch = build_upstream_dispatch(payload, downstream_protocol, channel_protocol)?;
    let started_at = Instant::now();

    let upstream_url = build_upstream_url(&selected.site_base_url, dispatch.upstream_protocol);
    let upstream_response = timeout(
        Duration::from_secs(FIRST_TOKEN_TIMEOUT_SECS),
        apply_upstream_auth(
            state.upstream_client.post(&upstream_url),
            dispatch.upstream_protocol,
            &selected.account_api_key,
        )
        .json(&dispatch.payload)
        .send(),
    )
    .await
    .map_err(|_| AppError::UpstreamTransport(first_token_timeout_message("response headers")))
    .and_then(|response| {
        response.map_err(|error| {
            AppError::UpstreamTransport(describe_reqwest_error(
                "send_upstream_request",
                Some(&upstream_url),
                &error,
            ))
        })
    });

    let upstream_response = match upstream_response {
        Ok(response) => response,
        Err(error) => {
            record_failure(
                &state,
                &route,
                &selected,
                request_id,
                requested_model,
                downstream_protocol,
                dispatch.upstream_protocol,
                None,
                started_at.elapsed().as_millis() as i64,
                error.to_string(),
            )
            .await?;
            return Err(error);
        }
    };

    let status = upstream_response.status();
    let headers = upstream_response.headers().clone();
    let is_stream = dispatch
        .payload
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if !status.is_success() {
        let message = match upstream_response.bytes().await {
            Ok(body) => truncate(String::from_utf8_lossy(&body).as_ref(), 800),
            Err(error) => format!(
                "upstream returned status={} but failed to read error body: {}",
                status.as_u16(),
                describe_reqwest_error("read_error_response_body", Some(&upstream_url), &error)
            ),
        };
        record_failure(
            &state,
            &route,
            &selected,
            request_id,
            requested_model,
            downstream_protocol,
            dispatch.upstream_protocol,
            Some(status.as_u16()),
            started_at.elapsed().as_millis() as i64,
            message.clone(),
        )
        .await?;
        return Err(AppError::UpstreamStatus(message, status));
    }

    if dispatch.response_adapter == ResponseAdapter::Passthrough {
        return proxy_passthrough_stream(
            state,
            route,
            selected,
            request_id.to_string(),
            requested_model.to_string(),
            status,
            headers,
            upstream_response,
            started_at,
            downstream_protocol,
            dispatch.upstream_protocol,
            !is_stream,
        )
        .await;
    }

    if is_stream {
        return proxy_chat_completions_stream(
            state,
            route,
            selected,
            request_id.to_string(),
            requested_model.to_string(),
            upstream_response,
            started_at,
            dispatch.upstream_protocol,
        )
        .await;
    }

    let body = match read_response_body(upstream_response).await {
        Ok(body) => body,
        Err(error) => {
            let message = format!("failed to read upstream body: {error}");
            record_failure(
                &state,
                &route,
                &selected,
                request_id,
                requested_model,
                downstream_protocol,
                dispatch.upstream_protocol,
                Some(status.as_u16()),
                started_at.elapsed().as_millis() as i64,
                message.clone(),
            )
            .await?;
            return Err(AppError::UpstreamTransport(message));
        }
    };
    let response_value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            let message = format!("invalid upstream json body: {error}");
            record_failure(
                &state,
                &route,
                &selected,
                request_id,
                requested_model,
                downstream_protocol,
                dispatch.upstream_protocol,
                Some(status.as_u16()),
                started_at.elapsed().as_millis() as i64,
                message.clone(),
            )
            .await?;
            return Err(AppError::UpstreamTransport(message));
        }
    };
    let token_usage = extract_usage_from_value(dispatch.upstream_protocol, &response_value);
    let chat_response =
        responses_json_to_chat_completion(&response_value, requested_model, request_id);
    record_success(
        &state,
        &selected,
        request_id,
        requested_model,
        downstream_protocol,
        dispatch.upstream_protocol,
        status.as_u16(),
        started_at.elapsed().as_millis() as i64,
        token_usage,
    )
    .await?;
    build_json_response(StatusCode::OK, &chat_response)
}

async fn read_response_body(upstream_response: reqwest::Response) -> Result<Vec<u8>, String> {
    let upstream_url = upstream_response.url().to_string();
    let mut upstream_stream = upstream_response.bytes_stream();
    let first_chunk = await_first_upstream_chunk(
        "read_nonstream_first_chunk",
        &upstream_url,
        &mut upstream_stream,
    )
    .await?;
    let mut body = Vec::new();
    if let Some(chunk) = first_chunk {
        body.extend_from_slice(&chunk);
    }

    while let Some(next_chunk) = upstream_stream.next().await {
        let chunk = next_chunk.map_err(|error| {
            describe_reqwest_error("read_nonstream_body_chunk", Some(&upstream_url), &error)
        })?;
        body.extend_from_slice(&chunk);
    }

    Ok(body)
}

async fn await_first_upstream_chunk<S>(
    stage: &str,
    upstream_url: &str,
    upstream_stream: &mut S,
) -> Result<Option<Bytes>, String>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    let first_chunk = timeout(
        Duration::from_secs(FIRST_TOKEN_TIMEOUT_SECS),
        upstream_stream.next(),
    )
    .await
    .map_err(|_| first_token_timeout_message("first response chunk"))?;

    match first_chunk {
        Some(Ok(chunk)) => Ok(Some(chunk)),
        Some(Err(error)) => Err(describe_reqwest_error(stage, Some(upstream_url), &error)),
        None => Ok(None),
    }
}

fn first_token_timeout_message(stage: &str) -> String {
    format!("first token timeout after {FIRST_TOKEN_TIMEOUT_SECS}s while waiting for {stage}")
}

fn first_token_timeout_cooldown_seconds(consecutive_fail_count: i64) -> i64 {
    let attempts = consecutive_fail_count.max(1);
    let multiplier = 2_i64.saturating_pow((attempts - 1) as u32);
    (FIRST_TOKEN_TIMEOUT_INITIAL_COOLDOWN_SECS * multiplier)
        .min(FIRST_TOKEN_TIMEOUT_MAX_COOLDOWN_SECS)
}

async fn proxy_passthrough_stream(
    state: AppState,
    route: ModelRouteRow,
    selected: ChannelRow,
    request_id: String,
    requested_model: String,
    status: StatusCode,
    headers: HeaderMap,
    upstream_response: reqwest::Response,
    started_at: Instant,
    downstream_protocol: Protocol,
    upstream_protocol: Protocol,
    capture_usage: bool,
) -> Result<Response<Body>, AppError> {
    let upstream_url = upstream_response.url().to_string();
    let mut upstream_stream = upstream_response.bytes_stream();
    let first_chunk = match await_first_upstream_chunk(
        "read_passthrough_first_chunk",
        &upstream_url,
        &mut upstream_stream,
    )
    .await
    {
        Ok(chunk) => chunk,
        Err(message) => {
            record_failure(
                &state,
                &route,
                &selected,
                &request_id,
                &requested_model,
                downstream_protocol,
                upstream_protocol,
                Some(status.as_u16()),
                started_at.elapsed().as_millis() as i64,
                message.clone(),
            )
            .await?;
            return Err(AppError::UpstreamTransport(message));
        }
    };

    let (tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    let state_for_task = state.clone();
    let selected_for_task = selected.clone();
    let request_id_for_task = request_id.clone();
    let requested_model_for_task = requested_model.clone();

    tokio::spawn(async move {
        let mut stream_error: Option<String> = None;
        let mut downstream_disconnected = false;
        let mut usage_buffer = capture_usage.then(Vec::new);

        if let Some(chunk) = first_chunk {
            if let Some(buffer) = usage_buffer.as_mut() {
                buffer.extend_from_slice(&chunk);
            }
            if tx.send(Ok(chunk)).await.is_err() {
                downstream_disconnected = true;
            }
        }

        if !downstream_disconnected {
            while let Some(next_chunk) = upstream_stream.next().await {
                match next_chunk {
                    Ok(chunk) => {
                        if let Some(buffer) = usage_buffer.as_mut() {
                            buffer.extend_from_slice(&chunk);
                        }
                        if tx.send(Ok(chunk)).await.is_err() {
                            downstream_disconnected = true;
                            break;
                        }
                    }
                    Err(error) => {
                        let message = describe_reqwest_error(
                            "read_passthrough_stream_chunk",
                            Some(&upstream_url),
                            &error,
                        );
                        let _ = tx.send(Err(std::io::Error::other(message.clone()))).await;
                        stream_error = Some(message);
                        break;
                    }
                }
            }
        }

        drop(tx);

        let latency_ms = started_at.elapsed().as_millis() as i64;
        if let Some(message) = stream_error {
            if let Err(error) = record_failure(
                &state_for_task,
                &route,
                &selected_for_task,
                &request_id_for_task,
                &requested_model_for_task,
                downstream_protocol,
                upstream_protocol,
                Some(status.as_u16()),
                latency_ms,
                message,
            )
            .await
            {
                tracing::error!("failed to persist stream failure: {error}");
            }
            return;
        }

        let success_note = downstream_disconnected
            .then(|| "downstream disconnected before stream completion".to_string());
        let token_usage = usage_buffer
            .as_deref()
            .and_then(|body| extract_usage_from_body(upstream_protocol, body));

        if let Err(error) = record_success_with_note(
            &state_for_task,
            &selected_for_task,
            &request_id_for_task,
            &requested_model_for_task,
            downstream_protocol,
            upstream_protocol,
            status.as_u16(),
            latency_ms,
            success_note,
            token_usage,
        )
        .await
        {
            tracing::error!("failed to persist stream success: {error}");
        }
    });

    build_response(status, &headers, Body::from_stream(ReceiverStream::new(rx)))
}

async fn proxy_chat_completions_stream(
    state: AppState,
    route: ModelRouteRow,
    selected: ChannelRow,
    request_id: String,
    requested_model: String,
    upstream_response: reqwest::Response,
    started_at: Instant,
    upstream_protocol: Protocol,
) -> Result<Response<Body>, AppError> {
    let upstream_url = upstream_response.url().to_string();
    let mut upstream_stream = upstream_response.bytes_stream();
    let first_chunk = match await_first_upstream_chunk(
        "read_chat_adapter_first_chunk",
        &upstream_url,
        &mut upstream_stream,
    )
    .await
    {
        Ok(chunk) => chunk,
        Err(message) => {
            record_failure(
                &state,
                &route,
                &selected,
                &request_id,
                &requested_model,
                Protocol::ChatCompletions,
                upstream_protocol,
                Some(StatusCode::OK.as_u16()),
                started_at.elapsed().as_millis() as i64,
                message.clone(),
            )
            .await?;
            return Err(AppError::UpstreamTransport(message));
        }
    };

    let (tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    let state_for_task = state.clone();
    let selected_for_task = selected.clone();
    let request_id_for_task = request_id.clone();
    let requested_model_for_task = requested_model.clone();
    let chat_id = format!("chatcmpl-{request_id}");

    tokio::spawn(async move {
        let mut buffer = String::new();
        let mut sent_role = false;
        let mut sent_done = false;
        let mut saw_tool_call = false;
        let mut tool_call_indices = HashMap::<i64, usize>::new();
        let mut tool_call_argument_emitted = HashSet::<i64>::new();
        let mut next_tool_call_index = 0usize;
        let mut stream_error: Option<String> = None;
        let mut downstream_disconnected = false;

        if let Some(chunk) = first_chunk {
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            let frames = drain_sse_frames(&mut buffer);
            'first_chunk: for frame in frames {
                match transform_responses_frame_to_chat_sse(
                    &frame,
                    &chat_id,
                    &requested_model_for_task,
                    &mut sent_role,
                    &mut sent_done,
                    &mut saw_tool_call,
                    &mut tool_call_indices,
                    &mut tool_call_argument_emitted,
                    &mut next_tool_call_index,
                ) {
                    Ok(lines) => {
                        for line in lines {
                            if tx.send(Ok(Bytes::from(line))).await.is_err() {
                                downstream_disconnected = true;
                                break 'first_chunk;
                            }
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(Err(std::io::Error::other(error.clone()))).await;
                        stream_error = Some(error);
                        break 'first_chunk;
                    }
                }
            }
        }

        if !downstream_disconnected && stream_error.is_none() {
            'outer: while let Some(next_chunk) = upstream_stream.next().await {
                match next_chunk {
                    Ok(chunk) => {
                        buffer.push_str(&String::from_utf8_lossy(&chunk));
                        let frames = drain_sse_frames(&mut buffer);
                        for frame in frames {
                            match transform_responses_frame_to_chat_sse(
                                &frame,
                                &chat_id,
                                &requested_model_for_task,
                                &mut sent_role,
                                &mut sent_done,
                                &mut saw_tool_call,
                                &mut tool_call_indices,
                                &mut tool_call_argument_emitted,
                                &mut next_tool_call_index,
                            ) {
                                Ok(lines) => {
                                    for line in lines {
                                        if tx.send(Ok(Bytes::from(line))).await.is_err() {
                                            downstream_disconnected = true;
                                            break 'outer;
                                        }
                                    }
                                }
                                Err(error) => {
                                    let _ =
                                        tx.send(Err(std::io::Error::other(error.clone()))).await;
                                    stream_error = Some(error);
                                    break 'outer;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        let message = describe_reqwest_error(
                            "read_chat_adapter_stream_chunk",
                            Some(&upstream_url),
                            &error,
                        );
                        let _ = tx.send(Err(std::io::Error::other(message.clone()))).await;
                        stream_error = Some(message);
                        break;
                    }
                }
            }
        }

        if stream_error.is_none() && !sent_done && !downstream_disconnected {
            let final_lines = vec![
                chat_chunk_finish_line(
                    &chat_id,
                    &requested_model_for_task,
                    if saw_tool_call { "tool_calls" } else { "stop" },
                ),
                "data: [DONE]\n\n".to_string(),
            ];
            for line in final_lines {
                if tx.send(Ok(Bytes::from(line))).await.is_err() {
                    downstream_disconnected = true;
                    break;
                }
            }
        }

        drop(tx);

        let latency_ms = started_at.elapsed().as_millis() as i64;
        if let Some(message) = stream_error {
            if let Err(error) = record_failure(
                &state_for_task,
                &route,
                &selected_for_task,
                &request_id_for_task,
                &requested_model_for_task,
                Protocol::ChatCompletions,
                upstream_protocol,
                Some(StatusCode::OK.as_u16()),
                latency_ms,
                message,
            )
            .await
            {
                tracing::error!("failed to persist chat stream failure: {error}");
            }
            return;
        }

        let success_note = downstream_disconnected
            .then(|| "downstream disconnected before stream completion".to_string());

        if let Err(error) = record_success_with_note(
            &state_for_task,
            &selected_for_task,
            &request_id_for_task,
            &requested_model_for_task,
            Protocol::ChatCompletions,
            upstream_protocol,
            StatusCode::OK.as_u16(),
            latency_ms,
            success_note,
            None,
        )
        .await
        {
            tracing::error!("failed to persist chat stream success: {error}");
        }
    });

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, "text/event-stream".parse().unwrap());
    build_response(
        StatusCode::OK,
        &headers,
        Body::from_stream(ReceiverStream::new(rx)),
    )
}

fn build_response(
    status: StatusCode,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response<Body>, AppError> {
    let mut builder = Response::builder().status(status);
    for (name, value) in headers {
        if name == CONTENT_LENGTH
            || name == CONNECTION
            || name == TRANSFER_ENCODING
            || name == UPGRADE
        {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder
        .body(body)
        .map_err(|error| AppError::Internal(format!("failed to build response: {error}")))
}

fn route_admin_view(route: AdminRouteRow) -> Value {
    json!({
        "id": route.id,
        "model_pattern": route.model_pattern,
        "enabled": route.enabled != 0,
        "routing_strategy": route.routing_strategy,
        "cooldown_seconds": route.cooldown_seconds,
        "channel_count": route.channel_count,
        "enabled_channel_count": route.enabled_channel_count,
        "ready_channel_count": route.ready_channel_count,
        "cooling_channel_count": route.cooling_channel_count,
        "manual_blocked_channel_count": route.manual_blocked_channel_count
    })
}

fn route_detail_view(route: &ModelRouteRow) -> Value {
    json!({
        "id": route.id,
        "model_pattern": route.model_pattern,
        "enabled": route.enabled != 0,
        "routing_strategy": route.routing_strategy,
        "cooldown_seconds": route.cooldown_seconds
    })
}

fn channel_admin_view(
    candidate: &routing::CandidateEvaluation,
    now_ts: i64,
    runtime_stats: Option<&ChannelRuntimeStats>,
) -> Value {
    let channel = &candidate.channel;
    let cooldown_remaining_seconds = channel
        .cooldown_until
        .filter(|until| *until > now_ts)
        .map(|until| until - now_ts);
    let (last_error_kind, last_error_hint) =
        classify_upstream_error(channel.last_status, channel.last_error.as_deref());
    let avg_latency_ms = runtime_stats
        .and_then(|stats| stats.avg_latency_ms)
        .or(channel.avg_latency_ms);
    let requests_24h = runtime_stats
        .map(|stats| stats.requests_24h)
        .unwrap_or_default();
    let success_requests_24h = runtime_stats
        .map(|stats| stats.success_requests_24h)
        .unwrap_or_default();
    let input_tokens_24h = runtime_stats
        .map(|stats| stats.input_tokens_24h)
        .unwrap_or_default();
    let output_tokens_24h = runtime_stats
        .map(|stats| stats.output_tokens_24h)
        .unwrap_or_default();
    let total_tokens_24h = runtime_stats
        .map(|stats| stats.total_tokens_24h)
        .unwrap_or_default();

    json!({
        "channel_id": channel.channel_id,
        "route_id": channel.route_id,
        "account_id": channel.account_id,
        "site_name": channel.site_name,
        "site_base_url": channel.site_base_url,
        "account_label": channel.account_label,
        "account_status": channel.account_status,
        "channel_label": channel.channel_label,
        "site_status": channel.site_status,
        "upstream_model": channel.upstream_model,
        "protocol": channel.protocol,
        "enabled": channel.enabled != 0,
        "priority": channel.priority,
        "avg_latency_ms": avg_latency_ms,
        "cooldown_until": channel.cooldown_until,
        "manual_blocked": channel.manual_blocked != 0,
        "cooldown_remaining_seconds": cooldown_remaining_seconds,
        "consecutive_fail_count": channel.consecutive_fail_count,
        "last_status": channel.last_status,
        "last_error": channel.last_error,
        "last_error_kind": last_error_kind,
        "last_error_hint": last_error_hint,
        "eligible": candidate.eligible,
        "state": channel_state(channel, now_ts),
        "reason": candidate.reason,
        "requests_24h": requests_24h,
        "success_requests_24h": success_requests_24h,
        "input_tokens_24h": input_tokens_24h,
        "output_tokens_24h": output_tokens_24h,
        "total_tokens_24h": total_tokens_24h
    })
}

fn request_log_admin_view(log: RequestLogRow) -> Value {
    let (error_kind, error_hint) =
        classify_upstream_error(log.http_status, log.error_message.as_deref());

    json!({
        "id": log.id,
        "request_id": log.request_id,
        "downstream_path": log.downstream_path,
        "upstream_path": log.upstream_path,
        "model_requested": log.model_requested,
        "channel_id": log.channel_id,
        "channel_label": log.channel_label,
        "site_name": log.site_name,
        "upstream_model": log.upstream_model,
        "http_status": log.http_status,
        "latency_ms": log.latency_ms,
        "error_message": log.error_message,
        "error_kind": error_kind,
        "error_hint": error_hint,
        "created_at": log.created_at
    })
}

async fn channel_admin_json(
    state: &AppState,
    channel: ChannelRow,
    now_ts: i64,
) -> Result<Value, AppError> {
    let stats = state
        .store
        .load_channel_runtime_stats(channel.channel_id)
        .await?;
    let candidate = routing::inspect_candidates(vec![channel], None, now_ts)
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Internal("failed to evaluate updated channel".to_string()))?;

    Ok(channel_admin_view(&candidate, now_ts, Some(&stats)))
}

async fn update_channel_state_response(
    state: &AppState,
    channel: ChannelRow,
    now_ts: i64,
) -> Result<Response<Body>, AppError> {
    build_json_response(
        StatusCode::OK,
        &json!({
            "data": channel_admin_json(state, channel, now_ts).await?
        }),
    )
}

fn channel_state(channel: &ChannelRow, now_ts: i64) -> &'static str {
    if channel.enabled == 0 {
        "disabled"
    } else if channel.account_status != "active" {
        "account_inactive"
    } else if channel.site_status != "active" {
        "site_inactive"
    } else if channel.manual_blocked != 0 {
        "manual_intervention_required"
    } else if channel.cooldown_until.is_some_and(|until| until > now_ts) {
        "cooling_down"
    } else {
        "ready"
    }
}

fn build_upstream_dispatch(
    payload: Value,
    downstream_protocol: Protocol,
    channel_protocol: Protocol,
) -> Result<UpstreamDispatch, AppError> {
    match (downstream_protocol, channel_protocol) {
        (Protocol::Responses, Protocol::Responses)
        | (Protocol::ChatCompletions, Protocol::ChatCompletions)
        | (Protocol::Messages, Protocol::Messages) => Ok(UpstreamDispatch {
            upstream_protocol: channel_protocol,
            payload,
            response_adapter: ResponseAdapter::Passthrough,
        }),
        (Protocol::ChatCompletions, Protocol::Responses) => Ok(UpstreamDispatch {
            upstream_protocol: Protocol::Responses,
            payload: chat_completions_to_responses_payload(&payload)?,
            response_adapter: ResponseAdapter::ResponsesToChatCompletions,
        }),
        _ => Err(AppError::NoRoute(format!(
            "protocol mismatch: request={} channel={}",
            downstream_protocol.as_str(),
            channel_protocol.as_str()
        ))),
    }
}

fn build_json_response(status: StatusCode, body: &Value) -> Result<Response<Body>, AppError> {
    let bytes = serde_json::to_vec(body).map_err(|error| {
        AppError::Internal(format!("failed to serialize json response: {error}"))
    })?;
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
    build_response(status, &headers, Body::from(bytes))
}

fn apply_upstream_auth(
    request: reqwest::RequestBuilder,
    protocol: Protocol,
    api_key: &str,
) -> reqwest::RequestBuilder {
    match protocol {
        Protocol::Messages => request
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01"),
        Protocol::Responses | Protocol::ChatCompletions => request.bearer_auth(api_key),
    }
}

async fn record_success(
    state: &AppState,
    selected: &ChannelRow,
    request_id: &str,
    requested_model: &str,
    downstream_protocol: Protocol,
    upstream_protocol: Protocol,
    http_status: u16,
    latency_ms: i64,
    token_usage: Option<TokenUsage>,
) -> Result<(), AppError> {
    record_success_with_note(
        state,
        selected,
        request_id,
        requested_model,
        downstream_protocol,
        upstream_protocol,
        http_status,
        latency_ms,
        None,
        token_usage,
    )
    .await
}

async fn record_success_with_note(
    state: &AppState,
    selected: &ChannelRow,
    request_id: &str,
    requested_model: &str,
    downstream_protocol: Protocol,
    upstream_protocol: Protocol,
    http_status: u16,
    latency_ms: i64,
    error_message: Option<String>,
    token_usage: Option<TokenUsage>,
) -> Result<(), AppError> {
    let log = RequestLogWrite {
        request_id: request_id.to_string(),
        downstream_path: downstream_protocol.path().to_string(),
        upstream_path: upstream_protocol.path().to_string(),
        model_requested: requested_model.to_string(),
        channel_id: selected.channel_id,
        http_status: Some(i64::from(http_status)),
        latency_ms,
        error_message,
        input_tokens: token_usage.map(|usage| usage.input_tokens),
        output_tokens: token_usage.map(|usage| usage.output_tokens),
        total_tokens: token_usage.map(|usage| usage.total_tokens),
    };
    state.store.record_request(&log).await?;
    state
        .store
        .mark_channel_success(selected.channel_id, http_status, Some(latency_ms))
        .await?;
    Ok(())
}

async fn record_failure(
    state: &AppState,
    route: &ModelRouteRow,
    selected: &ChannelRow,
    request_id: &str,
    requested_model: &str,
    downstream_protocol: Protocol,
    upstream_protocol: Protocol,
    http_status: Option<u16>,
    latency_ms: i64,
    error_message: String,
) -> Result<(), AppError> {
    let (error_kind, _) = classify_upstream_error(http_status.map(i64::from), Some(&error_message));
    let next_fail_count = selected.consecutive_fail_count + 1;
    let (cooldown_until, manual_intervention_required) = failure_disposition(
        &state.cooldown_policy,
        &state.manual_intervention_policy,
        route.cooldown_seconds,
        error_kind,
        next_fail_count,
    );
    let log = RequestLogWrite {
        request_id: request_id.to_string(),
        downstream_path: downstream_protocol.path().to_string(),
        upstream_path: upstream_protocol.path().to_string(),
        model_requested: requested_model.to_string(),
        channel_id: selected.channel_id,
        http_status: http_status.map(i64::from),
        latency_ms,
        error_message: Some(error_message.clone()),
        input_tokens: None,
        output_tokens: None,
        total_tokens: None,
    };
    state.store.record_request(&log).await?;
    state
        .store
        .mark_channel_failure(
            selected.channel_id,
            http_status,
            &error_message,
            cooldown_until,
            manual_intervention_required,
        )
        .await?;
    Ok(())
}

fn failure_disposition(
    cooldown_policy: &CooldownPolicy,
    manual_policy: &ManualInterventionPolicy,
    route_cooldown_seconds: i64,
    error_kind: &str,
    next_fail_count: i64,
) -> (Option<i64>, bool) {
    let manual_intervention_required = requires_immediate_manual_block(error_kind)
        || requires_manual_intervention(manual_policy, error_kind);
    let cooldown_until = if manual_intervention_required || !should_enter_cooldown(next_fail_count)
    {
        None
    } else {
        Some(
            now_ts()
                + resolve_cooldown_seconds(
                    cooldown_policy,
                    route_cooldown_seconds,
                    error_kind,
                    cooldown_fail_count(next_fail_count),
                ),
        )
    };
    (cooldown_until, manual_intervention_required)
}

async fn execute_channel_probe(
    state: &AppState,
    channel: &ChannelRow,
) -> Result<u16, ProbeOutcome> {
    let protocol = Protocol::parse(&channel.protocol).map_err(|error| ProbeOutcome {
        http_status: None,
        error_message: Some(error.to_string()),
    })?;

    run_upstream_probe(
        state,
        &channel.site_base_url,
        &channel.account_api_key,
        &channel.upstream_model,
        protocol,
    )
    .await
}

async fn background_recover_channel(
    state: &AppState,
    route: &ModelRouteRow,
    channel: &ChannelRow,
) -> Result<bool, AppError> {
    let started_at = Instant::now();
    match execute_channel_probe(state, channel).await {
        Ok(http_status) => {
            state
                .store
                .mark_channel_success(
                    channel.channel_id,
                    http_status,
                    Some(started_at.elapsed().as_millis() as i64),
                )
                .await?;
            Ok(true)
        }
        Err(outcome) => {
            let error_message = outcome
                .error_message
                .unwrap_or_else(|| "background recovery probe failed".to_string());
            let (error_kind, _) =
                classify_upstream_error(outcome.http_status.map(i64::from), Some(&error_message));
            let (cooldown_until, manual_blocked) = failure_disposition(
                &state.cooldown_policy,
                &state.manual_intervention_policy,
                route.cooldown_seconds,
                error_kind,
                channel.consecutive_fail_count + 1,
            );
            state
                .store
                .mark_channel_failure(
                    channel.channel_id,
                    outcome.http_status,
                    &error_message,
                    cooldown_until,
                    manual_blocked,
                )
                .await?;
            Ok(false)
        }
    }
}

fn select_background_recovery_candidate(
    channels: &[ChannelRow],
    now_ts: i64,
) -> Option<ChannelRow> {
    let mut candidates = channels
        .iter()
        .filter(|channel| {
            channel.enabled != 0
                && channel.manual_blocked == 0
                && channel.account_status == "active"
                && channel.site_status == "active"
                && channel.cooldown_until.is_some_and(|until| until > now_ts)
        })
        .cloned()
        .collect::<Vec<_>>();

    candidates.sort_by_key(|channel| {
        (
            channel.priority,
            channel.avg_latency_ms.unwrap_or(i64::MAX),
            channel.cooldown_until.unwrap_or(i64::MAX),
            channel.channel_id,
        )
    });

    candidates.into_iter().next()
}

fn route_admin_to_model_route(route: &AdminRouteRow) -> ModelRouteRow {
    ModelRouteRow {
        id: route.id,
        model_pattern: route.model_pattern.clone(),
        enabled: route.enabled,
        routing_strategy: route.routing_strategy.clone(),
        cooldown_seconds: route.cooldown_seconds,
    }
}

pub async fn run_background_recovery_cycle_with_memory(
    state: AppState,
    zero_ready_probed_routes: &mut HashSet<i64>,
) -> Result<usize, AppError> {
    let now = now_ts();
    let routes = state.store.list_routes(now).await?;
    let mut recovered_count = 0usize;

    for route in routes {
        if route.ready_channel_count > 0 {
            zero_ready_probed_routes.remove(&route.id);
            continue;
        }

        if !zero_ready_probed_routes.insert(route.id) {
            continue;
        }

        let channels = state.store.load_channels(route.id).await?;
        let Some(channel) = select_background_recovery_candidate(&channels, now) else {
            continue;
        };

        if background_recover_channel(&state, &route_admin_to_model_route(&route), &channel).await?
        {
            recovered_count += 1;
        }
    }

    Ok(recovered_count)
}

async fn run_upstream_probe(
    state: &AppState,
    base_url: &str,
    api_key: &str,
    upstream_model: &str,
    protocol: Protocol,
) -> Result<u16, ProbeOutcome> {
    let probe_url = build_upstream_url(base_url, protocol);
    let probe_payload = build_probe_payload(protocol, upstream_model);

    let response = apply_upstream_auth(state.upstream_client.post(&probe_url), protocol, api_key)
        .json(&probe_payload)
        .send()
        .await
        .map_err(|error| ProbeOutcome {
            http_status: None,
            error_message: Some(describe_reqwest_error(
                "probe_send_upstream_request",
                Some(&probe_url),
                &error,
            )),
        })?;

    if response.status().is_success() {
        return Ok(response.status().as_u16());
    }

    let status = response.status();
    let message = match response.bytes().await {
        Ok(body) => truncate(String::from_utf8_lossy(&body).as_ref(), 400),
        Err(error) => format!(
            "status={} but failed to read probe error body: {}",
            status.as_u16(),
            describe_reqwest_error("read_probe_error_body", Some(&probe_url), &error)
        ),
    };
    let (kind, hint) = classify_upstream_error(Some(i64::from(status.as_u16())), Some(&message));
    let hint_suffix = hint
        .as_deref()
        .map(|value| format!("; {value}"))
        .unwrap_or_default();

    Err(ProbeOutcome {
        http_status: Some(status.as_u16()),
        error_message: Some(format!(
            "status={} kind={} message={}{}",
            status.as_u16(),
            kind,
            message,
            hint_suffix
        )),
    })
}

fn build_probe_payload(protocol: Protocol, upstream_model: &str) -> Value {
    match protocol {
        Protocol::Responses => json!({
            "model": upstream_model,
            "input": "ping",
            "max_output_tokens": 1
        }),
        Protocol::ChatCompletions => json!({
            "model": upstream_model,
            "messages": [{ "role": "user", "content": "ping" }],
            "max_tokens": 1
        }),
        Protocol::Messages => json!({
            "model": upstream_model,
            "messages": [{ "role": "user", "content": "ping" }],
            "max_tokens": 1
        }),
    }
}

fn parse_required_protocol(value: &str) -> Result<Protocol, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(
            "field `protocol` is required".to_string(),
        ));
    }
    Protocol::parse(trimmed)
}

fn classify_upstream_error(
    last_status: Option<i64>,
    last_error: Option<&str>,
) -> (&'static str, Option<String>) {
    let message = last_error.unwrap_or_default();
    let normalized = message.to_ascii_lowercase();

    if normalized.contains("first token timeout") {
        return (
            "first_token_timeout",
            Some(format!(
                "upstream produced no response bytes within {FIRST_TOKEN_TIMEOUT_SECS}s"
            )),
        );
    }

    if last_status == Some(403) && normalized.contains("1010") {
        return (
            "edge_blocked",
            Some("edge/WAF blocked this exit IP or client fingerprint".to_string()),
        );
    }

    if last_status == Some(404) && normalized.contains("page not found") {
        return (
            "upstream_path_error",
            Some("check base_url and protocol path; full endpoint and /v1-style prefixes are accepted".to_string()),
        );
    }

    if last_status == Some(401)
        || normalized.contains("未提供认证信息")
        || normalized.contains("invalid api key")
        || normalized.contains("unauthorized")
    {
        return (
            "auth_error",
            Some("check api_key or upstream authentication policy".to_string()),
        );
    }

    if last_status == Some(429) || normalized.contains("rate limit") {
        return (
            "rate_limited",
            Some("upstream rate limited this channel".to_string()),
        );
    }

    if last_status.is_some_and(|status| status >= 500) {
        return (
            "upstream_server_error",
            Some("upstream returned a server-side error".to_string()),
        );
    }

    if normalized.contains("transport error")
        || normalized.contains("connection")
        || normalized.contains("timed out")
    {
        return (
            "transport_error",
            Some("network transport to upstream failed".to_string()),
        );
    }

    ("unknown_error", None)
}

fn resolve_cooldown_seconds(
    policy: &CooldownPolicy,
    route_default_seconds: i64,
    error_kind: &str,
    consecutive_fail_count: i64,
) -> i64 {
    match error_kind {
        "first_token_timeout" => Some(first_token_timeout_cooldown_seconds(consecutive_fail_count)),
        "auth_error" => policy.auth_error_seconds,
        "rate_limited" => policy.rate_limited_seconds,
        "upstream_server_error" => policy.upstream_server_error_seconds,
        "transport_error" => policy.transport_error_seconds,
        "edge_blocked" => policy.edge_blocked_seconds,
        "upstream_path_error" => policy.upstream_path_error_seconds,
        "unknown_error" => policy.unknown_error_seconds,
        _ => None,
    }
    .unwrap_or(route_default_seconds)
}

fn should_enter_cooldown(consecutive_fail_count: i64) -> bool {
    consecutive_fail_count >= AUTO_COOLDOWN_FAILURE_THRESHOLD
}

fn cooldown_fail_count(consecutive_fail_count: i64) -> i64 {
    (consecutive_fail_count - AUTO_COOLDOWN_FAILURE_THRESHOLD + 1).max(1)
}

fn requires_immediate_manual_block(error_kind: &str) -> bool {
    matches!(
        error_kind,
        "auth_error" | "upstream_path_error" | "edge_blocked"
    )
}

fn requires_manual_intervention(policy: &ManualInterventionPolicy, error_kind: &str) -> bool {
    match error_kind {
        "first_token_timeout" => false,
        "auth_error" => policy.auth_error,
        "rate_limited" => policy.rate_limited,
        "upstream_server_error" => policy.upstream_server_error,
        "transport_error" => policy.transport_error,
        "edge_blocked" => policy.edge_blocked,
        "upstream_path_error" => policy.upstream_path_error,
        "unknown_error" => policy.unknown_error,
        _ => false,
    }
}

fn select_last_chance_channel(
    candidates: &[routing::CandidateEvaluation],
    request_protocol: Protocol,
    now_ts: i64,
) -> Option<ChannelRow> {
    let mut cooling_candidates = candidates
        .iter()
        .filter_map(|candidate| {
            let channel = &candidate.channel;
            if channel.enabled == 0
                || channel.manual_blocked != 0
                || channel.account_status != "active"
                || channel.site_status != "active"
            {
                return None;
            }

            let cooldown_until = channel.cooldown_until?;
            if cooldown_until <= now_ts {
                return None;
            }

            let channel_protocol = Protocol::parse(&channel.protocol).ok()?;
            let protocol_cost =
                crate::protocol::compatibility_cost(channel_protocol, request_protocol)?;

            Some((channel.clone(), protocol_cost, cooldown_until))
        })
        .collect::<Vec<_>>();

    cooling_candidates.sort_by_key(|(channel, protocol_cost, cooldown_until)| {
        (
            channel.priority,
            *protocol_cost,
            channel.avg_latency_ms.unwrap_or(i64::MAX),
            *cooldown_until,
            channel.channel_id,
        )
    });

    cooling_candidates
        .into_iter()
        .map(|(channel, _, _)| channel)
        .next()
}

fn chat_completions_to_responses_payload(payload: &Value) -> Result<Value, AppError> {
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadRequest("field `model` is required".to_string()))?;
    let messages = payload
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("field `messages` is required".to_string()))?;

    let mut body = Map::new();
    body.insert("model".to_string(), Value::String(model.to_string()));
    body.insert(
        "input".to_string(),
        Value::Array(chat_messages_to_responses_input(messages)?),
    );

    copy_optional_field(payload, &mut body, "stream");
    copy_optional_field(payload, &mut body, "temperature");
    copy_optional_field(payload, &mut body, "top_p");
    copy_optional_field(payload, &mut body, "parallel_tool_calls");

    if let Some(tools) = payload.get("tools") {
        let tools = tools
            .as_array()
            .ok_or_else(|| AppError::BadRequest("field `tools` must be an array".to_string()))?
            .iter()
            .map(chat_tool_to_responses_tool)
            .collect::<Result<Vec<_>, _>>()?;
        body.insert("tools".to_string(), Value::Array(tools));
    }

    if let Some(tool_choice) = payload.get("tool_choice") {
        body.insert(
            "tool_choice".to_string(),
            chat_tool_choice_to_responses_tool_choice(tool_choice)?,
        );
    }

    if let Some(max_tokens) = payload
        .get("max_completion_tokens")
        .cloned()
        .or_else(|| payload.get("max_tokens").cloned())
    {
        body.insert("max_output_tokens".to_string(), max_tokens);
    }

    Ok(Value::Object(body))
}

fn chat_messages_to_responses_input(messages: &[Value]) -> Result<Vec<Value>, AppError> {
    let mut items = Vec::new();
    for message in messages {
        items.extend(chat_message_to_responses_items(message)?);
    }
    Ok(items)
}

fn chat_message_to_responses_items(message: &Value) -> Result<Vec<Value>, AppError> {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadRequest("chat message role is required".to_string()))?;
    if role == "tool" {
        return Ok(vec![chat_tool_message_to_responses_item(message)?]);
    }

    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut items = Vec::new();

    if let Some(content) = message.get("content") {
        if let Some(message_item) = chat_content_message_to_responses_item(role, content)? {
            items.push(message_item);
        }
    } else if tool_calls.is_empty() {
        return Err(AppError::BadRequest(
            "chat message content is required".to_string(),
        ));
    }

    if role == "assistant" {
        for tool_call in &tool_calls {
            items.push(chat_tool_call_to_responses_item(tool_call)?);
        }
    }

    Ok(items)
}

fn chat_content_message_to_responses_item(
    role: &str,
    content: &Value,
) -> Result<Option<Value>, AppError> {
    let content = chat_message_content_to_responses_content(content)?;
    if content.is_empty() {
        return Ok(None);
    }

    Ok(Some(json!({
        "role": role,
        "content": content
    })))
}

fn chat_message_content_to_responses_content(content: &Value) -> Result<Vec<Value>, AppError> {
    match content {
        Value::Null => Ok(Vec::new()),
        Value::String(text) => Ok(vec![json!({
            "type": "input_text",
            "text": text
        })]),
        Value::Array(parts) => Ok(parts
            .iter()
            .filter_map(chat_content_part_to_input_part)
            .collect()),
        _ => Err(AppError::BadRequest(
            "chat message content must be string or array".to_string(),
        )),
    }
}

fn chat_tool_message_to_responses_item(message: &Value) -> Result<Value, AppError> {
    let call_id = message
        .get("tool_call_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AppError::BadRequest("tool message `tool_call_id` is required".to_string())
        })?;
    let output =
        extract_chat_text_content(message.get("content").ok_or_else(|| {
            AppError::BadRequest("tool message content is required".to_string())
        })?)?;

    Ok(json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": output
    }))
}

fn chat_tool_call_to_responses_item(tool_call: &Value) -> Result<Value, AppError> {
    let tool_type = tool_call
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("function");
    if tool_type != "function" {
        return Err(AppError::BadRequest(format!(
            "unsupported chat tool call type: {tool_type}"
        )));
    }

    let function = tool_call
        .get("function")
        .and_then(Value::as_object)
        .ok_or_else(|| AppError::BadRequest("tool call `function` is required".to_string()))?;
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadRequest("tool call function name is required".to_string()))?;
    let arguments = function
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let call_id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadRequest("tool call `id` is required".to_string()))?;

    Ok(json!({
        "type": "function_call",
        "call_id": call_id,
        "name": name,
        "arguments": arguments
    }))
}

fn extract_chat_text_content(content: &Value) -> Result<String, AppError> {
    match content {
        Value::String(text) => Ok(text.clone()),
        Value::Array(parts) => Ok(parts
            .iter()
            .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text") | Some("input_text") => part
                    .get("text")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")),
        _ => Err(AppError::BadRequest(
            "chat message content must be string or array".to_string(),
        )),
    }
}

fn chat_tool_to_responses_tool(tool: &Value) -> Result<Value, AppError> {
    let tool_type = tool
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadRequest("tool type is required".to_string()))?;

    if tool_type != "function" {
        return Ok(tool.clone());
    }

    let function = tool
        .get("function")
        .and_then(Value::as_object)
        .ok_or_else(|| AppError::BadRequest("function tool definition is required".to_string()))?;

    let mut mapped = Map::new();
    mapped.insert("type".to_string(), Value::String("function".to_string()));
    for key in ["name", "description", "parameters", "strict"] {
        if let Some(value) = function.get(key).cloned() {
            mapped.insert(key.to_string(), value);
        }
    }

    Ok(Value::Object(mapped))
}

fn chat_tool_choice_to_responses_tool_choice(tool_choice: &Value) -> Result<Value, AppError> {
    match tool_choice {
        Value::String(_) => Ok(tool_choice.clone()),
        Value::Object(object) => {
            let tool_type = object
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if tool_type == "function" {
                if let Some(function_name) = object
                    .get("function")
                    .and_then(Value::as_object)
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                {
                    return Ok(json!({
                        "type": "function",
                        "name": function_name
                    }));
                }
            }
            Ok(tool_choice.clone())
        }
        _ => Err(AppError::BadRequest(
            "field `tool_choice` must be a string or object".to_string(),
        )),
    }
}

fn chat_content_part_to_input_part(part: &Value) -> Option<Value> {
    let part_type = part.get("type").and_then(Value::as_str)?;
    match part_type {
        "text" | "input_text" => Some(json!({
            "type": "input_text",
            "text": part.get("text").and_then(Value::as_str).unwrap_or_default()
        })),
        "image_url" => {
            let image_url = part
                .get("image_url")
                .and_then(Value::as_object)
                .and_then(|image_url| image_url.get("url"))
                .and_then(Value::as_str)?;
            Some(json!({
                "type": "input_image",
                "image_url": image_url
            }))
        }
        "input_image" => Some(part.clone()),
        _ => None,
    }
}

fn copy_optional_field(source: &Value, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).cloned() {
        target.insert(key.to_string(), value);
    }
}

fn responses_json_to_chat_completion(
    response: &Value,
    requested_model: &str,
    request_id: &str,
) -> Value {
    let id = response
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("chatcmpl-{request_id}"));
    let content = extract_response_output_text(response);
    let tool_calls = extract_response_tool_calls(response);
    let finish_reason = if tool_calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    let message_content = if content.is_empty() && !tool_calls.is_empty() {
        Value::Null
    } else {
        Value::String(content)
    };

    json!({
        "id": id,
        "object": "chat.completion",
        "created": now_ts(),
        "model": requested_model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": message_content,
                "tool_calls": tool_calls
            },
            "finish_reason": finish_reason
        }],
        "usage": map_usage_to_chat(response.get("usage"))
    })
}

fn extract_response_output_text(response: &Value) -> String {
    if let Some(text) = response.get("output_text").and_then(Value::as_str) {
        return text.to_string();
    }

    let mut parts = Vec::new();
    if let Some(outputs) = response.get("output").and_then(Value::as_array) {
        for output in outputs {
            if let Some(content_items) = output.get("content").and_then(Value::as_array) {
                for item in content_items {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        parts.push(text.to_string());
                        continue;
                    }
                    if let Some(text) = item.get("output_text").and_then(Value::as_str) {
                        parts.push(text.to_string());
                    }
                }
            }
        }
    }
    parts.join("")
}

fn extract_response_tool_calls(response: &Value) -> Vec<Value> {
    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(response_output_item_to_chat_tool_call)
        .collect()
}

fn response_output_item_to_chat_tool_call(item: &Value) -> Option<Value> {
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return None;
    }

    let name = item.get("name").and_then(Value::as_str)?;
    let arguments = item
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let id = item
        .get("call_id")
        .and_then(Value::as_str)
        .or_else(|| item.get("id").and_then(Value::as_str))?;

    Some(json!({
        "id": id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": arguments
        }
    }))
}

fn map_usage_to_chat(usage: Option<&Value>) -> Value {
    let usage = usage.unwrap_or(&Value::Null);
    json!({
        "prompt_tokens": usage.get("input_tokens").and_then(Value::as_i64).unwrap_or(0),
        "completion_tokens": usage.get("output_tokens").and_then(Value::as_i64).unwrap_or(0),
        "total_tokens": usage.get("total_tokens").and_then(Value::as_i64).unwrap_or(0)
    })
}

fn extract_usage_from_body(protocol: Protocol, body: &[u8]) -> Option<TokenUsage> {
    let response: Value = serde_json::from_slice(body).ok()?;
    extract_usage_from_value(protocol, &response)
}

fn extract_usage_from_value(protocol: Protocol, response: &Value) -> Option<TokenUsage> {
    let usage = response.get("usage")?;
    let (input_tokens, output_tokens, total_tokens) = match protocol {
        Protocol::Responses | Protocol::Messages => {
            let input_tokens = usage.get("input_tokens").and_then(Value::as_i64)?;
            let output_tokens = usage.get("output_tokens").and_then(Value::as_i64)?;
            let total_tokens = usage
                .get("total_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(input_tokens + output_tokens);
            (input_tokens, output_tokens, total_tokens)
        }
        Protocol::ChatCompletions => {
            let input_tokens = usage.get("prompt_tokens").and_then(Value::as_i64)?;
            let output_tokens = usage.get("completion_tokens").and_then(Value::as_i64)?;
            let total_tokens = usage
                .get("total_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(input_tokens + output_tokens);
            (input_tokens, output_tokens, total_tokens)
        }
    };

    Some(TokenUsage {
        input_tokens,
        output_tokens,
        total_tokens,
    })
}

fn drain_sse_frames(buffer: &mut String) -> Vec<String> {
    let mut frames = Vec::new();
    while let Some(index) = buffer.find("\n\n") {
        let frame = buffer[..index].to_string();
        buffer.drain(..index + 2);
        if !frame.trim().is_empty() {
            frames.push(frame);
        }
    }
    frames
}

fn extract_sse_data(frame: &str) -> Option<String> {
    let data = frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    (!data.is_empty()).then_some(data)
}

fn transform_responses_frame_to_chat_sse(
    frame: &str,
    chat_id: &str,
    requested_model: &str,
    sent_role: &mut bool,
    sent_done: &mut bool,
    saw_tool_call: &mut bool,
    tool_call_indices: &mut HashMap<i64, usize>,
    tool_call_argument_emitted: &mut HashSet<i64>,
    next_tool_call_index: &mut usize,
) -> Result<Vec<String>, String> {
    let Some(data) = extract_sse_data(frame) else {
        return Ok(Vec::new());
    };

    if data == "[DONE]" {
        if *sent_done {
            return Ok(Vec::new());
        }
        *sent_done = true;
        return Ok(vec!["data: [DONE]\n\n".to_string()]);
    }

    let event: Value = serde_json::from_str(&data)
        .map_err(|error| format!("invalid upstream sse json: {error}"))?;
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match event_type {
        "response.created" => Ok(Vec::new()),
        "response.output_item.added" => {
            let item = event.get("item").ok_or("missing response output item")?;
            if item.get("type").and_then(Value::as_str) != Some("function_call") {
                return Ok(Vec::new());
            }

            let output_index = event
                .get("output_index")
                .and_then(Value::as_i64)
                .unwrap_or(*next_tool_call_index as i64);
            let tool_call_index = lookup_or_assign_tool_call_index(
                tool_call_indices,
                next_tool_call_index,
                output_index,
            );
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .or_else(|| item.get("id").and_then(Value::as_str))
                .unwrap_or_default();
            let name = item.get("name").and_then(Value::as_str).unwrap_or_default();

            *saw_tool_call = true;
            let mut lines = Vec::new();
            if !*sent_role {
                *sent_role = true;
                lines.push(chat_chunk_role_line(chat_id, requested_model));
            }
            lines.push(chat_chunk_tool_call_start_line(
                chat_id,
                requested_model,
                tool_call_index,
                call_id,
                name,
            ));
            Ok(lines)
        }
        "response.output_text.delta" => {
            let delta = event
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut lines = Vec::new();
            if !*sent_role {
                *sent_role = true;
                lines.push(chat_chunk_role_line(chat_id, requested_model));
            }
            if !delta.is_empty() {
                lines.push(chat_chunk_content_line(chat_id, requested_model, delta));
            }
            Ok(lines)
        }
        "response.function_call_arguments.delta" => {
            let output_index = event
                .get("output_index")
                .and_then(Value::as_i64)
                .unwrap_or(*next_tool_call_index as i64);
            let delta = event
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if delta.is_empty() {
                return Ok(Vec::new());
            }

            let tool_call_index = lookup_or_assign_tool_call_index(
                tool_call_indices,
                next_tool_call_index,
                output_index,
            );
            tool_call_argument_emitted.insert(output_index);
            *saw_tool_call = true;

            let mut lines = Vec::new();
            if !*sent_role {
                *sent_role = true;
                lines.push(chat_chunk_role_line(chat_id, requested_model));
            }
            lines.push(chat_chunk_tool_call_arguments_line(
                chat_id,
                requested_model,
                tool_call_index,
                delta,
            ));
            Ok(lines)
        }
        "response.function_call_arguments.done" => {
            let output_index = event
                .get("output_index")
                .and_then(Value::as_i64)
                .unwrap_or(*next_tool_call_index as i64);
            let item = event.get("item");
            let mut lines = Vec::new();

            if let Some(item) = item {
                let tool_call_index = lookup_or_assign_tool_call_index(
                    tool_call_indices,
                    next_tool_call_index,
                    output_index,
                );
                *saw_tool_call = true;
                if !*sent_role {
                    *sent_role = true;
                    lines.push(chat_chunk_role_line(chat_id, requested_model));
                }

                if !tool_call_argument_emitted.contains(&output_index) {
                    let call_id = item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .or_else(|| item.get("id").and_then(Value::as_str))
                        .unwrap_or_default();
                    let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
                    lines.push(chat_chunk_tool_call_start_line(
                        chat_id,
                        requested_model,
                        tool_call_index,
                        call_id,
                        name,
                    ));
                    if let Some(arguments) = item.get("arguments").and_then(Value::as_str) {
                        if !arguments.is_empty() {
                            lines.push(chat_chunk_tool_call_arguments_line(
                                chat_id,
                                requested_model,
                                tool_call_index,
                                arguments,
                            ));
                        }
                    }
                }
            }

            Ok(lines)
        }
        "response.completed" => {
            if *sent_done {
                return Ok(Vec::new());
            }
            *sent_done = true;
            Ok(vec![
                chat_chunk_finish_line(
                    chat_id,
                    requested_model,
                    if *saw_tool_call { "tool_calls" } else { "stop" },
                ),
                "data: [DONE]\n\n".to_string(),
            ])
        }
        "response.failed" | "error" => {
            let message = event
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .or_else(|| event.get("message").and_then(Value::as_str))
                .unwrap_or("upstream reported stream failure");
            Err(message.to_string())
        }
        _ => Ok(Vec::new()),
    }
}

fn lookup_or_assign_tool_call_index(
    tool_call_indices: &mut HashMap<i64, usize>,
    next_tool_call_index: &mut usize,
    output_index: i64,
) -> usize {
    if let Some(index) = tool_call_indices.get(&output_index) {
        return *index;
    }
    let assigned = *next_tool_call_index;
    tool_call_indices.insert(output_index, assigned);
    *next_tool_call_index += 1;
    assigned
}

fn chat_chunk_role_line(chat_id: &str, requested_model: &str) -> String {
    sse_data_line(json!({
        "id": chat_id,
        "object": "chat.completion.chunk",
        "created": now_ts(),
        "model": requested_model,
        "choices": [{
            "index": 0,
            "delta": { "role": "assistant" },
            "finish_reason": null
        }]
    }))
}

fn chat_chunk_content_line(chat_id: &str, requested_model: &str, delta: &str) -> String {
    sse_data_line(json!({
        "id": chat_id,
        "object": "chat.completion.chunk",
        "created": now_ts(),
        "model": requested_model,
        "choices": [{
            "index": 0,
            "delta": { "content": delta },
            "finish_reason": null
        }]
    }))
}

fn chat_chunk_tool_call_start_line(
    chat_id: &str,
    requested_model: &str,
    tool_call_index: usize,
    call_id: &str,
    name: &str,
) -> String {
    sse_data_line(json!({
        "id": chat_id,
        "object": "chat.completion.chunk",
        "created": now_ts(),
        "model": requested_model,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": tool_call_index,
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": ""
                    }
                }]
            },
            "finish_reason": null
        }]
    }))
}

fn chat_chunk_tool_call_arguments_line(
    chat_id: &str,
    requested_model: &str,
    tool_call_index: usize,
    delta: &str,
) -> String {
    sse_data_line(json!({
        "id": chat_id,
        "object": "chat.completion.chunk",
        "created": now_ts(),
        "model": requested_model,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": tool_call_index,
                    "function": {
                        "arguments": delta
                    }
                }]
            },
            "finish_reason": null
        }]
    }))
}

fn chat_chunk_finish_line(chat_id: &str, requested_model: &str, finish_reason: &str) -> String {
    sse_data_line(json!({
        "id": chat_id,
        "object": "chat.completion.chunk",
        "created": now_ts(),
        "model": requested_model,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": finish_reason
        }]
    }))
}

fn sse_data_line(payload: Value) -> String {
    format!("data: {}\n\n", serde_json::to_string(&payload).unwrap())
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn describe_reqwest_error(
    stage: &str,
    request_url: Option<&str>,
    error: &reqwest::Error,
) -> String {
    let mut flags = Vec::new();
    if error.is_timeout() {
        flags.push("timeout");
    }
    if error.is_connect() {
        flags.push("connect");
    }
    if error.is_body() {
        flags.push("body");
    }
    if error.is_decode() {
        flags.push("decode");
    }
    if error.is_request() {
        flags.push("request");
    }
    if error.is_redirect() {
        flags.push("redirect");
    }
    if error.is_status() {
        flags.push("status");
    }
    if flags.is_empty() {
        flags.push("other");
    }

    let status = error
        .status()
        .map(|status| status.as_u16().to_string())
        .unwrap_or_else(|| "-".to_string());
    let url = error
        .url()
        .map(|url| url.as_str().to_string())
        .or_else(|| request_url.map(str::to_string))
        .unwrap_or_else(|| "-".to_string());

    let mut sources = Vec::new();
    let mut current = error.source();
    while let Some(source) = current {
        sources.push(source.to_string());
        current = source.source();
        if sources.len() >= 6 {
            break;
        }
    }
    let sources = if sources.is_empty() {
        String::new()
    } else {
        format!("; sources={}", truncate(&sources.join(" <- "), 280))
    };

    truncate(
        &format!(
            "upstream transport error [stage={stage} flags={} status={status} url={url}]: {error}{sources}",
            flags.join(","),
        ),
        800,
    )
}

fn truncate(message: &str, max_len: usize) -> String {
    if message.len() <= max_len {
        return message.to_string();
    }
    message[..max_len].to_string()
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, path::PathBuf, time::Duration};

    use axum::{
        Json, Router,
        body::{Body, Bytes, to_bytes},
        http::{HeaderMap, Request, StatusCode, header::CONTENT_TYPE},
        response::Response,
        routing::post,
    };
    use serde_json::{Value, json};
    use sqlx::SqlitePool;
    use tempfile::tempdir;
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    use crate::{
        app,
        config::{Config, CooldownPolicy, ManualInterventionPolicy},
        protocol::Protocol,
    };

    async fn spawn_streaming_upstream() -> SocketAddr {
        async fn handler() -> Response<Body> {
            let chunks = vec![
                Ok::<Bytes, std::io::Error>(Bytes::from_static(
                    b"data: {\"type\":\"response.created\"}\n\n",
                )),
                Ok::<Bytes, std::io::Error>(Bytes::from_static(
                    b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hel\"}\n\n",
                )),
                Ok::<Bytes, std::io::Error>(Bytes::from_static(
                    b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}\n\n",
                )),
                Ok::<Bytes, std::io::Error>(Bytes::from_static(
                    b"data: {\"type\":\"response.completed\"}\n\n",
                )),
            ];
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(tokio_stream::iter(chunks)))
                .unwrap()
        }

        let app = Router::new().route("/v1/responses", post(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    async fn spawn_json_upstream() -> SocketAddr {
        async fn handler() -> Json<Value> {
            Json(json!({
                "id": "resp_123",
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": "hello from upstream"
                    }]
                }],
                "usage": {
                    "input_tokens": 11,
                    "output_tokens": 7,
                    "total_tokens": 18
                }
            }))
        }

        let app = Router::new().route("/v1/responses", post(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    async fn spawn_broken_json_upstream() -> SocketAddr {
        async fn handler() -> Response<Body> {
            let chunks = vec![
                Ok::<Bytes, std::io::Error>(Bytes::from_static(
                    b"{\"id\":\"resp_broken\",\"output\":[",
                )),
                Err::<Bytes, std::io::Error>(std::io::Error::other("error decoding response body")),
            ];
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from_stream(tokio_stream::iter(chunks)))
                .unwrap()
        }

        let app = Router::new().route("/v1/responses", post(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    async fn spawn_edge_blocked_upstream() -> SocketAddr {
        async fn handler() -> Response<Body> {
            Response::builder()
                .status(StatusCode::FORBIDDEN)
                .header(CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(Body::from("error code: 1010"))
                .unwrap()
        }

        let app = Router::new().route("/v1/responses", post(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    async fn spawn_auth_error_upstream() -> SocketAddr {
        async fn handler() -> Response<Body> {
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"error":{"message":"invalid api key","type":"invalid_request_error"}}"#,
                ))
                .unwrap()
        }

        let app = Router::new().route("/v1/responses", post(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    async fn spawn_server_error_upstream() -> SocketAddr {
        async fn handler() -> Response<Body> {
            Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"error":{"message":"upstream unavailable","type":"server_error"}}"#,
                ))
                .unwrap()
        }

        let app = Router::new().route("/v1/responses", post(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    async fn spawn_tool_call_json_upstream() -> SocketAddr {
        async fn handler() -> Json<Value> {
            Json(json!({
                "id": "resp_tool_123",
                "output": [{
                    "type": "function_call",
                    "id": "fc_123",
                    "call_id": "call_123",
                    "name": "get_weather",
                    "arguments": "{\"city\":\"Paris\"}"
                }],
                "usage": {
                    "input_tokens": 14,
                    "output_tokens": 5,
                    "total_tokens": 19
                }
            }))
        }

        let app = Router::new().route("/v1/responses", post(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    async fn spawn_tool_call_streaming_upstream() -> SocketAddr {
        async fn handler() -> Response<Body> {
            let chunks = vec![
                Ok::<Bytes, std::io::Error>(Bytes::from_static(
                    b"data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_123\",\"call_id\":\"call_123\",\"name\":\"get_weather\",\"arguments\":\"\"}}\n\n",
                )),
                Ok::<Bytes, std::io::Error>(Bytes::from_static(
                    b"data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"item_id\":\"fc_123\",\"delta\":\"{\\\"city\\\":\\\"Par\"}\n\n",
                )),
                Ok::<Bytes, std::io::Error>(Bytes::from_static(
                    b"data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"item_id\":\"fc_123\",\"delta\":\"is\\\"}\"}\n\n",
                )),
                Ok::<Bytes, std::io::Error>(Bytes::from_static(
                    b"data: {\"type\":\"response.completed\"}\n\n",
                )),
            ];
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(tokio_stream::iter(chunks)))
                .unwrap()
        }

        let app = Router::new().route("/v1/responses", post(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    async fn spawn_claude_upstream() -> SocketAddr {
        async fn handler(headers: HeaderMap) -> Response<Body> {
            let api_key_ok = headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
                == Some("test-key");
            let version_ok = headers
                .get("anthropic-version")
                .and_then(|value| value.to_str().ok())
                == Some("2023-06-01");

            if !api_key_ok || !version_ok {
                return Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"error":{"message":"missing anthropic auth headers"}}"#,
                    ))
                    .unwrap();
            }

            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "id": "msg_123",
                        "type": "message",
                        "role": "assistant",
                        "model": "claude-sonnet-4",
                        "content": [{
                            "type": "text",
                            "text": "hello from claude upstream"
                        }],
                        "stop_reason": "end_turn"
                    }))
                    .unwrap(),
                ))
                .unwrap()
        }

        let app = Router::new().route("/v1/messages", post(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    async fn spawn_gemini_openai_upstream() -> SocketAddr {
        async fn handler(headers: HeaderMap) -> Response<Body> {
            let auth_ok = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                == Some("Bearer test-key");

            if !auth_ok {
                return Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"error":{"message":"missing bearer auth"}}"#))
                    .unwrap();
            }

            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "id": "chatcmpl_gemini_123",
                        "object": "chat.completion",
                        "model": "gemini-2.5-pro",
                        "choices": [{
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": "hello from gemini-compatible upstream"
                            },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 9,
                            "completion_tokens": 6,
                            "total_tokens": 15
                        }
                    }))
                    .unwrap(),
                ))
                .unwrap()
        }

        let app = Router::new().route("/v1beta/openai/chat/completions", post(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    async fn seed_database(database_url: &str, upstream_addr: SocketAddr) {
        let pool = SqlitePool::connect(database_url).await.unwrap();
        sqlx::query("insert into sites (name, base_url, status) values (?, ?, 'active')")
            .bind("test-site")
            .bind(format!("http://{upstream_addr}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "insert into accounts (site_id, label, api_key, status) values (1, 'test-account', 'test-key', 'active')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into model_routes (model_pattern, enabled, routing_strategy, cooldown_seconds) values ('gpt-5.4', 1, 'weighted', 300)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into channels (route_id, account_id, label, upstream_model, supports_responses, enabled, weight, priority) values (1, 1, 'default', 'gpt-5.4', 1, 1, 10, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
    }

    async fn seed_management_data(database_url: &str) {
        let pool = SqlitePool::connect(database_url).await.unwrap();
        let cooling_until = super::now_ts() + 600;

        sqlx::query(
            r#"
            update channels
            set cooldown_until = ?,
                consecutive_fail_count = 2,
                last_status = 429,
                last_error = 'rate limited'
            where id = 1
            "#,
        )
        .bind(cooling_until)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "insert into channels (route_id, account_id, label, upstream_model, supports_responses, enabled, weight, priority) values (1, 1, 'backup', 'gpt-5.4-mini', 1, 1, 5, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "insert into model_routes (model_pattern, enabled, routing_strategy, cooldown_seconds) values ('gpt-4.1', 1, 'weighted', 120)",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "insert into channels (route_id, account_id, label, upstream_model, supports_responses, enabled, weight, priority) values (2, 1, 'disabled-fallback', 'gpt-4.1', 1, 0, 5, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            insert into request_logs (
              request_id,
              downstream_path,
              upstream_path,
              model_requested,
              channel_id,
              http_status,
              latency_ms,
              error_message
            ) values
              ('req-cooling', '/v1/responses', '/v1/responses', 'gpt-5.4', 1, 429, 812, 'rate limited'),
              ('req-ready', '/v1/chat/completions', '/v1/responses', 'gpt-5.4', 2, 200, 145, null)
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
    }

    fn database_url(path: PathBuf) -> String {
        format!("sqlite://{}", path.display())
    }

    #[tokio::test]
    async fn responses_stream_is_proxied_end_to_end() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_streaming_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "stream": true,
                    "input": "ping"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "text/event-stream"
        );

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("response.output_text.delta"));
        assert!(text.contains("response.completed"));
    }

    #[tokio::test]
    async fn chat_completions_non_stream_maps_from_responses() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "messages": [{ "role": "user", "content": "hello" }]
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/json"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["object"], "chat.completion");
        assert_eq!(
            value["choices"][0]["message"]["content"],
            "hello from upstream"
        );
        assert_eq!(value["usage"]["prompt_tokens"], 11);
        assert_eq!(value["usage"]["completion_tokens"], 7);
    }

    #[tokio::test]
    async fn successful_request_persists_usage_and_latency_metrics() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "input": "ping"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let pool = SqlitePool::connect(&config.database_url).await.unwrap();
        let usage_row = sqlx::query_as::<_, (Option<i64>, Option<i64>, Option<i64>)>(
            r#"
            select input_tokens, output_tokens, total_tokens
            from request_logs
            order by id desc
            limit 1
            "#,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(usage_row.0, Some(11));
        assert_eq!(usage_row.1, Some(7));
        assert_eq!(usage_row.2, Some(18));

        let avg_latency_ms = sqlx::query_scalar::<_, Option<i64>>(
            "select avg_latency_ms from channels where id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(avg_latency_ms.is_some_and(|value| value >= 0));
    }

    #[tokio::test]
    async fn chat_completions_stream_maps_from_responses_sse() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_streaming_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "stream": true,
                    "messages": [{ "role": "user", "content": "hello" }]
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "text/event-stream"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("\"object\":\"chat.completion.chunk\""));
        assert!(text.contains("\"role\":\"assistant\""));
        assert!(text.contains("\"content\":\"hel\""));
        assert!(text.contains("\"content\":\"lo\""));
        assert!(text.contains("[DONE]"));
    }

    #[tokio::test]
    async fn request_retries_next_channel_within_same_call_before_returning_error() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let failing_upstream = spawn_auth_error_upstream().await;
        let healthy_upstream = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, failing_upstream).await;

        let pool = SqlitePool::connect(&config.database_url).await.unwrap();
        sqlx::query("insert into sites (name, base_url, status) values (?, ?, 'active')")
            .bind("backup-site")
            .bind(format!("http://{healthy_upstream}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "insert into accounts (site_id, label, api_key, status) values (2, 'backup-account', 'test-key', 'active')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into channels (route_id, account_id, label, upstream_model, supports_responses, enabled, weight, priority, protocol) values (1, 2, 'backup', 'gpt-5.4', 1, 1, 10, 0, 'responses')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let request = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "input": "ping"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["output"][0]["content"][0]["text"],
            "hello from upstream"
        );

        let log_count =
            sqlx::query_scalar::<_, i64>("select count(*) from request_logs where request_id = (select request_id from request_logs order by id desc limit 1)")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(log_count, 2);

        let first_status =
            sqlx::query_scalar::<_, Option<i64>>("select last_status from channels where id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(first_status, Some(401));

        let first_cooldown_until = sqlx::query_scalar::<_, Option<i64>>(
            "select cooldown_until from channels where id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(first_cooldown_until.is_none());

        let first_fail_count = sqlx::query_scalar::<_, i64>(
            "select consecutive_fail_count from channels where id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(first_fail_count, 1);
    }

    #[tokio::test]
    async fn hard_failures_are_blocked_on_first_hit() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_auth_error_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "input": "ping"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let pool = SqlitePool::connect(&config.database_url).await.unwrap();
        let row = sqlx::query_as::<_, (i64, Option<i64>, i64)>(
            "select manual_blocked, cooldown_until, consecutive_fail_count from channels where id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, 1);
        assert!(row.1.is_none());
        assert_eq!(row.2, 1);
    }

    #[tokio::test]
    async fn last_chance_attempt_can_recover_route_with_zero_ready_channels() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let pool = SqlitePool::connect(&config.database_url).await.unwrap();
        sqlx::query(
            r#"
            update channels
            set cooldown_until = ?,
                consecutive_fail_count = 3,
                last_status = 503,
                last_error = 'temporary upstream failure'
            where id = 1
            "#,
        )
        .bind(super::now_ts() + 600)
        .execute(&pool)
        .await
        .unwrap();

        let request = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "input": "ping"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;

        let row = sqlx::query_as::<_, (Option<i64>, i64, Option<i64>)>(
            "select cooldown_until, consecutive_fail_count, last_status from channels where id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(row.0.is_none());
        assert_eq!(row.1, 0);
        assert_eq!(row.2, Some(200));
    }

    #[tokio::test]
    async fn channel_only_enters_cooldown_after_third_consecutive_failure() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_server_error_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let pool = SqlitePool::connect(&config.database_url).await.unwrap();

        for attempt in 1..=3 {
            let request = Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "model": "gpt-5.4",
                        "input": "ping"
                    }))
                    .unwrap(),
                ))
                .unwrap();

            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

            let fail_count = sqlx::query_scalar::<_, i64>(
                "select consecutive_fail_count from channels where id = 1",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(fail_count, attempt);

            let cooldown_until = sqlx::query_scalar::<_, Option<i64>>(
                "select cooldown_until from channels where id = 1",
            )
            .fetch_one(&pool)
            .await
            .unwrap();

            if attempt < 3 {
                assert!(cooldown_until.is_none());
            } else {
                assert!(cooldown_until.is_some());
            }
        }
    }

    #[tokio::test]
    async fn non_stream_body_decode_failure_is_recorded_in_request_logs() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_broken_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "input": "ping"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

        let pool = SqlitePool::connect(&config.database_url).await.unwrap();
        let error_message = sqlx::query_scalar::<_, Option<String>>(
            "select error_message from request_logs order by id desc limit 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap();
        assert!(!error_message.trim().is_empty());
        assert!(error_message.contains("stage="));
        assert!(error_message.contains("flags="));
        assert!(error_message.contains("url="));
    }

    #[tokio::test]
    async fn management_routes_list_aggregates_channel_counts() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;
        seed_management_data(&config.database_url).await;

        let request = Request::builder()
            .method("GET")
            .uri("/api/routes")
            .body(Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let routes = value["data"].as_array().unwrap();
        assert_eq!(routes.len(), 2);

        let gpt54 = routes
            .iter()
            .find(|route| route["model_pattern"] == "gpt-5.4")
            .unwrap();
        assert_eq!(gpt54["channel_count"], 2);
        assert_eq!(gpt54["enabled_channel_count"], 2);
        assert_eq!(gpt54["ready_channel_count"], 1);
        assert_eq!(gpt54["cooling_channel_count"], 1);

        let gpt41 = routes
            .iter()
            .find(|route| route["model_pattern"] == "gpt-4.1")
            .unwrap();
        assert_eq!(gpt41["channel_count"], 1);
        assert_eq!(gpt41["enabled_channel_count"], 0);
        assert_eq!(gpt41["ready_channel_count"], 0);
    }

    #[tokio::test]
    async fn management_route_channels_show_cooldown_and_reason() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;
        seed_management_data(&config.database_url).await;

        let request = Request::builder()
            .method("GET")
            .uri("/api/routes/1/channels")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["data"]["route"]["model_pattern"], "gpt-5.4");

        let channels = value["data"]["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 2);

        let cooling = channels
            .iter()
            .find(|channel| channel["channel_label"] == "default")
            .unwrap();
        assert_eq!(cooling["state"], "cooling_down");
        assert_eq!(cooling["eligible"], false);
        assert_eq!(cooling["last_status"], 429);
        assert_eq!(cooling["last_error"], "rate limited");
        assert_eq!(cooling["last_error_kind"], "rate_limited");
        assert_eq!(cooling["consecutive_fail_count"], 2);
        assert!(cooling["cooldown_remaining_seconds"].as_i64().unwrap() > 0);
        assert!(
            cooling["reason"]
                .as_str()
                .unwrap()
                .contains("cooling down until")
        );

        let ready = channels
            .iter()
            .find(|channel| channel["channel_label"] == "backup")
            .unwrap();
        assert_eq!(ready["state"], "ready");
        assert_eq!(ready["eligible"], true);
        assert_eq!(ready["upstream_model"], "gpt-5.4-mini");
    }

    #[tokio::test]
    async fn management_route_logs_list_recent_requests() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;
        seed_management_data(&config.database_url).await;

        let request = Request::builder()
            .method("GET")
            .uri("/api/routes/1/logs?limit=5")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["data"]["route"]["model_pattern"], "gpt-5.4");

        let logs = value["data"]["logs"].as_array().unwrap();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0]["channel_label"], "backup");
        assert_eq!(logs[0]["site_name"], "test-site");
        assert_eq!(logs[0]["http_status"], 200);
        assert_eq!(logs[0]["error_message"], Value::Null);
        assert_eq!(logs[0]["error_kind"], "unknown_error");

        assert_eq!(logs[1]["channel_label"], "default");
        assert_eq!(logs[1]["http_status"], 429);
        assert_eq!(logs[1]["error_message"], "rate limited");
        assert_eq!(logs[1]["error_kind"], "rate_limited");
        assert_eq!(logs[1]["downstream_path"], "/v1/responses");
        assert_eq!(logs[1]["upstream_model"], "gpt-5.4");
    }

    #[tokio::test]
    async fn management_channel_prefill_returns_base_url_and_api_key() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let request = Request::builder()
            .method("GET")
            .uri("/api/channels/1/prefill")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["data"]["base_url"], format!("http://{upstream_addr}"));
        assert_eq!(value["data"]["api_key"], "test-key");
        assert_eq!(value["data"]["upstream_model"], "gpt-5.4");
    }

    #[tokio::test]
    async fn update_channel_edits_base_url_api_key_and_routing_fields() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let replacement_upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let request = Request::builder()
            .method("PATCH")
            .uri("/api/channels/1")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "base_url": format!("http://{replacement_upstream_addr}/v1"),
                    "api_key": "replacement-key",
                    "upstream_model": "gpt-5.4-mini",
                    "protocol": "messages",
                    "priority": 2,
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["data"]["site_base_url"],
            format!("http://{replacement_upstream_addr}/v1")
        );
        assert_eq!(value["data"]["upstream_model"], "gpt-5.4-mini");
        assert_eq!(value["data"]["protocol"], "messages");
        assert_eq!(value["data"]["priority"], 2);

        let prefill_request = Request::builder()
            .method("GET")
            .uri("/api/channels/1/prefill")
            .body(Body::empty())
            .unwrap();
        let prefill_response = app.oneshot(prefill_request).await.unwrap();
        assert_eq!(prefill_response.status(), StatusCode::OK);

        let prefill_body = to_bytes(prefill_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let prefill_value: Value = serde_json::from_slice(&prefill_body).unwrap();
        assert_eq!(
            prefill_value["data"]["base_url"],
            format!("http://{replacement_upstream_addr}/v1")
        );
        assert_eq!(prefill_value["data"]["api_key"], "replacement-key");
    }

    #[test]
    fn resolve_cooldown_seconds_prefers_error_specific_override() {
        let policy = CooldownPolicy {
            rate_limited_seconds: Some(45),
            auth_error_seconds: Some(1800),
            ..Default::default()
        };

        assert_eq!(
            super::resolve_cooldown_seconds(&policy, 300, "rate_limited", 1),
            45
        );
        assert_eq!(
            super::resolve_cooldown_seconds(&policy, 300, "auth_error", 1),
            1800
        );
        assert_eq!(
            super::resolve_cooldown_seconds(&policy, 300, "unknown_error", 1),
            300
        );
    }

    #[test]
    fn first_token_timeout_uses_exponential_cooldown_with_cap() {
        let policy = CooldownPolicy::default();

        assert_eq!(
            super::resolve_cooldown_seconds(&policy, 300, "first_token_timeout", 1),
            120
        );
        assert_eq!(
            super::resolve_cooldown_seconds(&policy, 300, "first_token_timeout", 2),
            240
        );
        assert_eq!(
            super::resolve_cooldown_seconds(&policy, 300, "first_token_timeout", 3),
            480
        );
        assert_eq!(
            super::resolve_cooldown_seconds(&policy, 300, "first_token_timeout", 4),
            960
        );
        assert_eq!(
            super::resolve_cooldown_seconds(&policy, 300, "first_token_timeout", 5),
            1920
        );
        assert_eq!(
            super::resolve_cooldown_seconds(&policy, 300, "first_token_timeout", 9),
            1920
        );
    }

    #[test]
    fn cooldown_starts_on_third_consecutive_failure() {
        assert!(!super::should_enter_cooldown(1));
        assert!(!super::should_enter_cooldown(2));
        assert!(super::should_enter_cooldown(3));
        assert!(super::should_enter_cooldown(4));
    }

    #[test]
    fn cooldown_backoff_restarts_when_threshold_is_reached() {
        assert_eq!(super::cooldown_fail_count(1), 1);
        assert_eq!(super::cooldown_fail_count(2), 1);
        assert_eq!(super::cooldown_fail_count(3), 1);
        assert_eq!(super::cooldown_fail_count(4), 2);
        assert_eq!(super::cooldown_fail_count(5), 3);
    }

    #[test]
    fn classify_upstream_error_detects_first_token_timeout() {
        let (kind, hint) = super::classify_upstream_error(
            Some(200),
            Some("first token timeout after 50s while waiting for first response chunk"),
        );

        assert_eq!(kind, "first_token_timeout");
        assert!(hint.unwrap().contains("50s"));
    }

    #[tokio::test]
    async fn auth_error_can_require_manual_intervention_until_reset() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_auth_error_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: ManualInterventionPolicy {
                auth_error: true,
                ..Default::default()
            },
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let proxy_request = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "input": "ping"
                }))
                .unwrap(),
            ))
            .unwrap();

        let proxy_response = app.clone().oneshot(proxy_request).await.unwrap();
        assert_eq!(proxy_response.status(), StatusCode::UNAUTHORIZED);
        let proxy_body = to_bytes(proxy_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let proxy_value: Value = serde_json::from_slice(&proxy_body).unwrap();
        assert!(
            proxy_value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("invalid api key")
        );

        let channels_request = Request::builder()
            .method("GET")
            .uri("/api/routes/1/channels")
            .body(Body::empty())
            .unwrap();

        let channels_response = app.clone().oneshot(channels_request).await.unwrap();
        assert_eq!(channels_response.status(), StatusCode::OK);
        let channels_body = to_bytes(channels_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let channels_value: Value = serde_json::from_slice(&channels_body).unwrap();
        let channels = channels_value["data"]["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0]["manual_blocked"], true);
        assert_eq!(channels[0]["state"], "manual_intervention_required");
        assert_eq!(channels[0]["eligible"], false);
        assert_eq!(channels[0]["last_status"], 401);
        assert_eq!(channels[0]["last_error_kind"], "auth_error");
        assert!(channels[0]["cooldown_until"].is_null());

        let routes_request = Request::builder()
            .method("GET")
            .uri("/api/routes")
            .body(Body::empty())
            .unwrap();

        let routes_response = app.clone().oneshot(routes_request).await.unwrap();
        assert_eq!(routes_response.status(), StatusCode::OK);
        let routes_body = to_bytes(routes_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let routes_value: Value = serde_json::from_slice(&routes_body).unwrap();
        let routes = routes_value["data"].as_array().unwrap();
        assert_eq!(routes[0]["manual_blocked_channel_count"], 1);
        assert_eq!(routes[0]["ready_channel_count"], 0);

        let reset_request = Request::builder()
            .method("POST")
            .uri("/api/channels/1/reset-cooldown")
            .body(Body::empty())
            .unwrap();

        let reset_response = app.clone().oneshot(reset_request).await.unwrap();
        assert_eq!(reset_response.status(), StatusCode::OK);
        let reset_body = to_bytes(reset_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let reset_value: Value = serde_json::from_slice(&reset_body).unwrap();
        assert_eq!(reset_value["data"]["manual_blocked"], false);
        assert_eq!(reset_value["data"]["state"], "ready");
        assert_eq!(reset_value["data"]["eligible"], true);
        assert!(reset_value["data"]["cooldown_until"].is_null());
        assert_eq!(reset_value["data"]["consecutive_fail_count"], 0);
    }

    #[tokio::test]
    async fn delete_channel_removes_selected_channel() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;
        seed_management_data(&config.database_url).await;

        let delete_request = Request::builder()
            .method("DELETE")
            .uri("/api/channels/2")
            .body(Body::empty())
            .unwrap();

        let delete_response = app.clone().oneshot(delete_request).await.unwrap();
        assert_eq!(delete_response.status(), StatusCode::OK);
        let delete_body = to_bytes(delete_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let deleted: Value = serde_json::from_slice(&delete_body).unwrap();
        assert_eq!(deleted["data"]["channel_id"], 2);
        assert_eq!(deleted["data"]["deleted"], true);
        assert_eq!(deleted["data"]["route_deleted"], false);

        let list_request = Request::builder()
            .method("GET")
            .uri("/api/routes/1/channels")
            .body(Body::empty())
            .unwrap();

        let list_response = app.oneshot(list_request).await.unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);
        let list_body = to_bytes(list_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let listed: Value = serde_json::from_slice(&list_body).unwrap();
        let channels = listed["data"]["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0]["channel_label"], "default");
    }

    #[tokio::test]
    async fn deleting_last_channel_keeps_empty_route() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let delete_request = Request::builder()
            .method("DELETE")
            .uri("/api/channels/1")
            .body(Body::empty())
            .unwrap();

        let delete_response = app.clone().oneshot(delete_request).await.unwrap();
        assert_eq!(delete_response.status(), StatusCode::OK);
        let delete_body = to_bytes(delete_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let deleted: Value = serde_json::from_slice(&delete_body).unwrap();
        assert_eq!(deleted["data"]["route_deleted"], false);
        assert_eq!(deleted["data"]["route_model"], "gpt-5.4");

        let routes_request = Request::builder()
            .method("GET")
            .uri("/api/routes")
            .body(Body::empty())
            .unwrap();

        let routes_response = app.clone().oneshot(routes_request).await.unwrap();
        assert_eq!(routes_response.status(), StatusCode::OK);
        let routes_body = to_bytes(routes_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let routes: Value = serde_json::from_slice(&routes_body).unwrap();
        assert_eq!(routes["data"].as_array().unwrap().len(), 1);
        assert_eq!(routes["data"][0]["model_pattern"], "gpt-5.4");
        assert_eq!(routes["data"][0]["channel_count"], 0);
    }

    #[tokio::test]
    async fn delete_route_requires_empty_route_and_deletes_when_empty() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let reject_request = Request::builder()
            .method("DELETE")
            .uri("/api/routes/1")
            .body(Body::empty())
            .unwrap();
        let reject_response = app.clone().oneshot(reject_request).await.unwrap();
        assert_eq!(reject_response.status(), StatusCode::BAD_REQUEST);

        let pool = SqlitePool::connect(&config.database_url).await.unwrap();
        sqlx::query(
            "insert into model_routes (model_pattern, enabled, routing_strategy, cooldown_seconds) values ('gpt-empty', 1, 'weighted', 60)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let delete_request = Request::builder()
            .method("DELETE")
            .uri("/api/routes/2")
            .body(Body::empty())
            .unwrap();
        let delete_response = app.clone().oneshot(delete_request).await.unwrap();
        assert_eq!(delete_response.status(), StatusCode::OK);
        let delete_body = to_bytes(delete_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let deleted: Value = serde_json::from_slice(&delete_body).unwrap();
        assert_eq!(deleted["data"]["route_model"], "gpt-empty");
        assert_eq!(deleted["data"]["deleted_channel_count"], 0);

        let routes_request = Request::builder()
            .method("GET")
            .uri("/api/routes")
            .body(Body::empty())
            .unwrap();
        let routes_response = app.oneshot(routes_request).await.unwrap();
        assert_eq!(routes_response.status(), StatusCode::OK);
        let routes_body = to_bytes(routes_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let routes: Value = serde_json::from_slice(&routes_body).unwrap();
        let models = routes["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|route| route["model_pattern"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(models, vec!["gpt-5.4"]);
    }

    #[tokio::test]
    async fn create_route_channel_adds_channel_to_selected_route() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let create_request = Request::builder()
            .method("POST")
            .uri("/api/routes/1/channels")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "base_url": "https://provider.example.com/v1",
                    "api_key": "new-key",
                    "protocol": "responses"
                }))
                .unwrap(),
            ))
            .unwrap();

        let create_response = app.clone().oneshot(create_request).await.unwrap();
        assert_eq!(create_response.status(), StatusCode::CREATED);
        let create_body = to_bytes(create_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: Value = serde_json::from_slice(&create_body).unwrap();
        assert_eq!(created["data"]["route_id"], 1);
        assert_eq!(
            created["data"]["site_base_url"],
            "https://provider.example.com/v1"
        );
        assert_eq!(created["data"]["upstream_model"], "gpt-5.4");
        assert_eq!(created["data"]["channel_label"], "ch-2");
        assert_eq!(created["data"]["state"], "ready");

        let list_request = Request::builder()
            .method("GET")
            .uri("/api/routes/1/channels")
            .body(Body::empty())
            .unwrap();

        let list_response = app.oneshot(list_request).await.unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);
        let list_body = to_bytes(list_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let listed: Value = serde_json::from_slice(&list_body).unwrap();
        let channels = listed["data"]["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 2);
        assert!(
            channels
                .iter()
                .any(|channel| channel["site_base_url"] == "https://provider.example.com/v1")
        );
    }

    #[tokio::test]
    async fn create_route_channel_requires_protocol() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let request = Request::builder()
            .method("POST")
            .uri("/api/routes/1/channels")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "base_url": "https://provider.example.com/v1",
                    "api_key": "new-key"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn create_route_channel_accepts_base_url_with_v1_suffix_for_proxying() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();

        let pool = SqlitePool::connect(&config.database_url).await.unwrap();
        sqlx::query("insert into model_routes (model_pattern, enabled, routing_strategy, cooldown_seconds) values ('gpt-5.4', 1, 'weighted', 300)")
            .execute(&pool)
            .await
            .unwrap();

        let create_channel_request = Request::builder()
            .method("POST")
            .uri("/api/routes/1/channels")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "base_url": format!("http://{upstream_addr}/v1"),
                    "api_key": "test-key",
                    "upstream_model": "gpt-5.4",
                    "protocol": "responses"
                }))
                .unwrap(),
            ))
            .unwrap();

        let create_response = app.clone().oneshot(create_channel_request).await.unwrap();
        assert_eq!(create_response.status(), StatusCode::CREATED);

        let proxy_request = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "input": "ping"
                }))
                .unwrap(),
            ))
            .unwrap();

        let proxy_response = app.oneshot(proxy_request).await.unwrap();
        assert_eq!(proxy_response.status(), StatusCode::OK);
        let proxy_body = to_bytes(proxy_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let proxy_value: Value = serde_json::from_slice(&proxy_body).unwrap();
        assert_eq!(
            proxy_value["output"][0]["content"][0]["text"],
            "hello from upstream"
        );
    }

    #[tokio::test]
    async fn chat_completions_channel_accepts_gemini_openai_base_prefix() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_gemini_openai_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();

        let pool = SqlitePool::connect(&config.database_url).await.unwrap();
        sqlx::query("insert into model_routes (model_pattern, enabled, routing_strategy, cooldown_seconds) values ('gemini-2.5-pro', 1, 'weighted', 300)")
            .execute(&pool)
            .await
            .unwrap();

        let create_channel_request = Request::builder()
            .method("POST")
            .uri("/api/routes/1/channels")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "base_url": format!("http://{upstream_addr}/v1beta/openai"),
                    "api_key": "test-key",
                    "upstream_model": "gemini-2.5-pro",
                    "protocol": "chat_completions"
                }))
                .unwrap(),
            ))
            .unwrap();

        let create_response = app.clone().oneshot(create_channel_request).await.unwrap();
        assert_eq!(create_response.status(), StatusCode::CREATED);

        let proxy_request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gemini-2.5-pro",
                    "messages": [{ "role": "user", "content": "ping" }]
                }))
                .unwrap(),
            ))
            .unwrap();

        let proxy_response = app.clone().oneshot(proxy_request).await.unwrap();
        assert_eq!(proxy_response.status(), StatusCode::OK);
        let proxy_body = to_bytes(proxy_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let proxy_value: Value = serde_json::from_slice(&proxy_body).unwrap();
        assert_eq!(proxy_value["object"], "chat.completion");
        assert_eq!(
            proxy_value["choices"][0]["message"]["content"],
            "hello from gemini-compatible upstream"
        );
        assert_eq!(proxy_value["usage"]["prompt_tokens"], 9);
        assert_eq!(proxy_value["usage"]["completion_tokens"], 6);
    }

    #[tokio::test]
    async fn create_route_then_add_channels_keeps_one_route() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_a = spawn_json_upstream().await;
        let upstream_b = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();

        let create_route = Request::builder()
            .method("POST")
            .uri("/api/routes")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "route_model": "gpt-5.4",
                }))
                .unwrap(),
            ))
            .unwrap();

        let first_response = app.clone().oneshot(create_route).await.unwrap();
        assert_eq!(first_response.status(), StatusCode::CREATED);
        let first_body = to_bytes(first_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let first_value: Value = serde_json::from_slice(&first_body).unwrap();
        assert_eq!(first_value["data"]["created"], true);
        assert_eq!(first_value["data"]["route"]["model_pattern"], "gpt-5.4");
        let route_id = first_value["data"]["route"]["id"].as_i64().unwrap();

        let create_first_channel = Request::builder()
            .method("POST")
            .uri(format!("/api/routes/{route_id}/channels"))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "base_url": format!("http://{upstream_a}/v1"),
                    "api_key": "key-a",
                    "upstream_model": "gpt-5.4",
                    "protocol": "responses"
                }))
                .unwrap(),
            ))
            .unwrap();

        let first_channel_response = app.clone().oneshot(create_first_channel).await.unwrap();
        assert_eq!(first_channel_response.status(), StatusCode::CREATED);

        let create_second_channel = Request::builder()
            .method("POST")
            .uri(format!("/api/routes/{route_id}/channels"))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "base_url": format!("http://{upstream_b}/v1"),
                    "api_key": "key-b",
                    "upstream_model": "gpt-5-4",
                    "protocol": "responses",
                    "priority": 1
                }))
                .unwrap(),
            ))
            .unwrap();

        let second_response = app.clone().oneshot(create_second_channel).await.unwrap();
        assert_eq!(second_response.status(), StatusCode::CREATED);
        let second_body = to_bytes(second_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let second_value: Value = serde_json::from_slice(&second_body).unwrap();
        assert_eq!(second_value["data"]["route_id"], route_id);
        assert_eq!(second_value["data"]["protocol"], "responses");
        assert_eq!(second_value["data"]["upstream_model"], "gpt-5-4");

        let routes_request = Request::builder()
            .method("GET")
            .uri("/api/routes")
            .body(Body::empty())
            .unwrap();

        let routes_response = app.clone().oneshot(routes_request).await.unwrap();
        assert_eq!(routes_response.status(), StatusCode::OK);
        let routes_body = to_bytes(routes_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let routes_value: Value = serde_json::from_slice(&routes_body).unwrap();
        let routes = routes_value["data"].as_array().unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0]["model_pattern"], "gpt-5.4");
        assert_eq!(routes[0]["channel_count"], 2);

        let channels_request = Request::builder()
            .method("GET")
            .uri(format!("/api/routes/{route_id}/channels"))
            .body(Body::empty())
            .unwrap();

        let channels_response = app.oneshot(channels_request).await.unwrap();
        assert_eq!(channels_response.status(), StatusCode::OK);
        let channels_body = to_bytes(channels_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let channels_value: Value = serde_json::from_slice(&channels_body).unwrap();
        let channels = channels_value["data"]["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 2);
        assert!(
            channels
                .iter()
                .any(|channel| channel["upstream_model"] == "gpt-5.4")
        );
        assert!(
            channels
                .iter()
                .any(|channel| channel["upstream_model"] == "gpt-5-4")
        );
    }

    #[tokio::test]
    async fn create_route_without_channel_succeeds() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();

        let create_request = Request::builder()
            .method("POST")
            .uri("/api/routes")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "route_model": "gpt-5.4"
                }))
                .unwrap(),
            ))
            .unwrap();

        let create_response = app.clone().oneshot(create_request).await.unwrap();
        assert_eq!(create_response.status(), StatusCode::CREATED);
        let create_body = to_bytes(create_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let create_value: Value = serde_json::from_slice(&create_body).unwrap();
        assert_eq!(create_value["data"]["created"], true);
        assert_eq!(create_value["data"]["route"]["model_pattern"], "gpt-5.4");

        let routes_request = Request::builder()
            .method("GET")
            .uri("/api/routes")
            .body(Body::empty())
            .unwrap();

        let routes_response = app.oneshot(routes_request).await.unwrap();
        assert_eq!(routes_response.status(), StatusCode::OK);
        let routes_body = to_bytes(routes_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let routes_value: Value = serde_json::from_slice(&routes_body).unwrap();
        assert_eq!(routes_value["data"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn messages_protocol_proxies_messages_with_anthropic_headers() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_claude_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();

        let create_route_request = Request::builder()
            .method("POST")
            .uri("/api/routes")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "route_model": "claude-4-sonnet"
                }))
                .unwrap(),
            ))
            .unwrap();

        let create_response = app.clone().oneshot(create_route_request).await.unwrap();
        assert_eq!(create_response.status(), StatusCode::CREATED);
        let create_body = to_bytes(create_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let create_value: Value = serde_json::from_slice(&create_body).unwrap();
        let route_id = create_value["data"]["route"]["id"].as_i64().unwrap();

        let create_channel_request = Request::builder()
            .method("POST")
            .uri(format!("/api/routes/{route_id}/channels"))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "base_url": format!("http://{upstream_addr}/v1"),
                    "api_key": "test-key",
                    "upstream_model": "claude-sonnet-4",
                    "protocol": "messages"
                }))
                .unwrap(),
            ))
            .unwrap();

        let create_channel_response = app.clone().oneshot(create_channel_request).await.unwrap();
        assert_eq!(create_channel_response.status(), StatusCode::CREATED);

        let proxy_request = Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "claude-4-sonnet",
                    "messages": [{ "role": "user", "content": "ping" }],
                    "max_tokens": 8
                }))
                .unwrap(),
            ))
            .unwrap();

        let proxy_response = app.oneshot(proxy_request).await.unwrap();
        assert_eq!(proxy_response.status(), StatusCode::OK);
        let proxy_body = to_bytes(proxy_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let proxy_value: Value = serde_json::from_slice(&proxy_body).unwrap();
        assert_eq!(proxy_value["type"], "message");
        assert_eq!(
            proxy_value["content"][0]["text"],
            "hello from claude upstream"
        );
    }

    #[test]
    fn build_upstream_url_supports_openai_compatible_prefixes() {
        assert_eq!(
            super::build_upstream_url("https://api.example.com", Protocol::ChatCompletions),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            super::build_upstream_url(
                "https://generativelanguage.googleapis.com/v1beta/openai",
                Protocol::ChatCompletions
            ),
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
        );
        assert_eq!(
            super::build_upstream_url(
                "https://example.com/v1/chat/completions",
                Protocol::ChatCompletions
            ),
            "https://example.com/v1/chat/completions"
        );
    }

    #[tokio::test]
    async fn channel_actions_disable_and_reset_cooldown_update_state() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;
        seed_management_data(&config.database_url).await;

        let disable_request = Request::builder()
            .method("POST")
            .uri("/api/channels/2/disable")
            .body(Body::empty())
            .unwrap();

        let disable_response = app.clone().oneshot(disable_request).await.unwrap();
        assert_eq!(disable_response.status(), StatusCode::OK);
        let disable_body = to_bytes(disable_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let disable_value: Value = serde_json::from_slice(&disable_body).unwrap();
        assert_eq!(disable_value["data"]["channel_id"], 2);
        assert_eq!(disable_value["data"]["enabled"], false);
        assert_eq!(disable_value["data"]["state"], "disabled");
        assert_eq!(disable_value["data"]["eligible"], false);

        let reset_request = Request::builder()
            .method("POST")
            .uri("/api/channels/1/reset-cooldown")
            .body(Body::empty())
            .unwrap();

        let reset_response = app.clone().oneshot(reset_request).await.unwrap();
        assert_eq!(reset_response.status(), StatusCode::OK);
        let reset_body = to_bytes(reset_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let reset_value: Value = serde_json::from_slice(&reset_body).unwrap();
        assert_eq!(reset_value["data"]["channel_id"], 1);
        assert_eq!(reset_value["data"]["state"], "ready");
        assert_eq!(reset_value["data"]["eligible"], true);
        assert!(reset_value["data"]["cooldown_until"].is_null());
        assert_eq!(reset_value["data"]["consecutive_fail_count"], 0);

        let enable_request = Request::builder()
            .method("POST")
            .uri("/api/channels/2/enable")
            .body(Body::empty())
            .unwrap();

        let enable_response = app.oneshot(enable_request).await.unwrap();
        assert_eq!(enable_response.status(), StatusCode::OK);
        let enable_body = to_bytes(enable_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let enable_value: Value = serde_json::from_slice(&enable_body).unwrap();
        assert_eq!(enable_value["data"]["channel_id"], 2);
        assert_eq!(enable_value["data"]["enabled"], true);
        assert_eq!(enable_value["data"]["state"], "ready");
        assert_eq!(enable_value["data"]["eligible"], true);
    }

    #[tokio::test]
    async fn probe_channel_marks_success_ready() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let pool = SqlitePool::connect(&config.database_url).await.unwrap();
        sqlx::query(
            r#"
            update channels
            set manual_blocked = 1,
                consecutive_fail_count = 3,
                last_status = 401,
                last_error = 'invalid api key'
            where id = 1
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/channels/1/probe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["data"]["state"], "ready");
        assert_eq!(value["data"]["manual_blocked"], false);
        assert_eq!(value["data"]["eligible"], true);
        assert_eq!(value["data"]["last_status"], 200);
        assert!(value["data"]["last_error"].is_null());
    }

    #[tokio::test]
    async fn probe_channel_marks_failure_unavailable() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_edge_blocked_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/channels/1/probe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["data"]["state"], "manual_intervention_required");
        assert_eq!(value["data"]["manual_blocked"], true);
        assert_eq!(value["data"]["eligible"], false);
        assert_eq!(value["data"]["last_status"], 403);
        assert_eq!(value["data"]["last_error_kind"], "edge_blocked");
        assert!(value["data"]["cooldown_until"].is_null());
    }

    #[test]
    fn chat_completions_tools_are_mapped_to_responses_payload() {
        let payload = json!({
            "model": "gpt-5.4",
            "tool_choice": {
                "type": "function",
                "function": { "name": "get_weather" }
            },
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Get weather by city",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "city": { "type": "string" }
                        },
                        "required": ["city"]
                    }
                }
            }],
            "messages": [
                { "role": "user", "content": "weather?" },
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\":\"Paris\"}"
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_123",
                    "content": "{\"temp\":18}"
                }
            ]
        });

        let mapped = super::chat_completions_to_responses_payload(&payload).unwrap();
        assert_eq!(mapped["tools"][0]["type"], "function");
        assert_eq!(mapped["tools"][0]["name"], "get_weather");
        assert_eq!(mapped["tool_choice"]["type"], "function");
        assert_eq!(mapped["tool_choice"]["name"], "get_weather");
        assert_eq!(mapped["input"][0]["role"], "user");
        assert_eq!(mapped["input"][1]["type"], "function_call");
        assert_eq!(mapped["input"][1]["call_id"], "call_123");
        assert_eq!(mapped["input"][1]["name"], "get_weather");
        assert_eq!(mapped["input"][2]["type"], "function_call_output");
        assert_eq!(mapped["input"][2]["call_id"], "call_123");
        assert_eq!(mapped["input"][2]["output"], "{\"temp\":18}");
    }

    #[test]
    fn responses_function_call_maps_to_chat_completion_tool_calls() {
        let response = json!({
            "id": "resp_tool_123",
            "output": [{
                "type": "function_call",
                "id": "fc_123",
                "call_id": "call_123",
                "name": "get_weather",
                "arguments": "{\"city\":\"Paris\"}"
            }],
            "usage": {
                "input_tokens": 14,
                "output_tokens": 5,
                "total_tokens": 19
            }
        });

        let chat = super::responses_json_to_chat_completion(&response, "gpt-5.4", "req_1");
        assert_eq!(chat["choices"][0]["finish_reason"], "tool_calls");
        assert!(chat["choices"][0]["message"]["content"].is_null());
        assert_eq!(
            chat["choices"][0]["message"]["tool_calls"][0]["id"],
            "call_123"
        );
        assert_eq!(
            chat["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "get_weather"
        );
        assert_eq!(
            chat["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"city\":\"Paris\"}"
        );
    }

    #[tokio::test]
    async fn chat_completions_non_stream_maps_function_calls_from_responses() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_tool_call_json_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "tools": [{
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "parameters": {
                                "type": "object",
                                "properties": {
                                    "city": { "type": "string" }
                                }
                            }
                        }
                    }],
                    "messages": [{ "role": "user", "content": "weather?" }]
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(
            value["choices"][0]["message"]["tool_calls"][0]["id"],
            "call_123"
        );
        assert_eq!(
            value["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "get_weather"
        );
    }

    #[tokio::test]
    async fn chat_completions_stream_maps_function_calls_from_responses_sse() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let upstream_addr = spawn_tool_call_streaming_upstream().await;
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let app = app::build_app(&config).await.unwrap();
        seed_database(&config.database_url, upstream_addr).await;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "stream": true,
                    "tools": [{
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "parameters": {
                                "type": "object",
                                "properties": {
                                    "city": { "type": "string" }
                                }
                            }
                        }
                    }],
                    "messages": [{ "role": "user", "content": "weather?" }]
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("\"tool_calls\""));
        assert!(text.contains("\"id\":\"call_123\""));
        assert!(text.contains("\"name\":\"get_weather\""));
        assert!(text.contains("\"finish_reason\":\"tool_calls\""));
        assert!(text.contains("[DONE]"));
    }
}
