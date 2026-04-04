use serde::Serialize;
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow)]
pub struct ModelRouteRow {
    pub id: i64,
    pub model_pattern: String,
    pub enabled: i64,
    pub routing_strategy: String,
    pub cooldown_seconds: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct AdminRouteRow {
    pub id: i64,
    pub model_pattern: String,
    pub enabled: i64,
    pub routing_strategy: String,
    pub cooldown_seconds: i64,
    pub channel_count: i64,
    pub enabled_channel_count: i64,
    pub ready_channel_count: i64,
    pub cooling_channel_count: i64,
    pub manual_blocked_channel_count: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct ChannelRow {
    pub channel_id: i64,
    pub route_id: i64,
    pub account_id: i64,
    pub account_label: String,
    pub account_api_key: String,
    pub account_status: String,
    pub site_name: String,
    pub site_base_url: String,
    pub site_status: String,
    pub channel_label: String,
    pub upstream_model: String,
    pub protocol: String,
    pub enabled: i64,
    pub priority: i64,
    pub cooldown_until: Option<i64>,
    pub manual_blocked: i64,
    pub consecutive_fail_count: i64,
    pub last_status: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct RequestLogRow {
    pub id: i64,
    pub request_id: String,
    pub downstream_path: String,
    pub upstream_path: String,
    pub model_requested: String,
    pub channel_id: Option<i64>,
    pub http_status: Option<i64>,
    pub latency_ms: i64,
    pub error_message: Option<String>,
    pub created_at: String,
    pub channel_label: String,
    pub site_name: String,
    pub upstream_model: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CandidateView {
    pub channel_id: i64,
    pub account_id: i64,
    pub site_name: String,
    pub label: String,
    pub protocol: String,
    pub priority: i64,
    pub eligible: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RouteDecisionView {
    pub requested_model: String,
    pub route_id: i64,
    pub routing_strategy: String,
    pub selected_channel_id: i64,
    pub selected_account_id: i64,
    pub selected_label: String,
    pub candidates: Vec<CandidateView>,
}

#[derive(Debug, Clone)]
pub struct RequestLogWrite {
    pub request_id: String,
    pub downstream_path: String,
    pub upstream_path: String,
    pub model_requested: String,
    pub channel_id: i64,
    pub http_status: Option<i64>,
    pub latency_ms: i64,
    pub error_message: Option<String>,
}
