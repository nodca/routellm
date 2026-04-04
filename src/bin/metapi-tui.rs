use std::{error::Error, io::Write, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use futures_util::StreamExt;
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use reqwest::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::time::{self, MissedTickBehavior};

#[derive(Debug, Deserialize, Clone)]
struct ApiResponse<T> {
    data: T,
}

#[derive(Debug, Deserialize, Clone)]
struct RouteSummary {
    id: i64,
    model_pattern: String,
    enabled: bool,
    routing_strategy: String,
    channel_count: i64,
    enabled_channel_count: i64,
    ready_channel_count: i64,
    cooling_channel_count: i64,
    manual_blocked_channel_count: i64,
}

#[derive(Debug, Deserialize, Clone)]
struct RouteDetailEnvelope {
    route: RouteDetail,
    channels: Vec<ChannelSummary>,
}

#[derive(Debug, Deserialize, Clone)]
struct RouteLogsEnvelope {
    logs: Vec<RequestLogSummary>,
}

#[derive(Debug, Deserialize, Clone)]
struct ChannelPrefill {
    base_url: String,
    api_key: String,
    upstream_model: String,
}

#[derive(Debug, Deserialize, Clone)]
struct RouteDetail {
    id: i64,
    model_pattern: String,
}

#[derive(Debug, Deserialize, Clone)]
struct ChannelSummary {
    channel_id: i64,
    route_id: i64,
    account_id: i64,
    site_name: String,
    site_base_url: String,
    account_label: String,
    account_status: String,
    channel_label: String,
    site_status: String,
    upstream_model: String,
    priority: i64,
    weight: i64,
    manual_blocked: bool,
    cooldown_remaining_seconds: Option<i64>,
    consecutive_fail_count: i64,
    last_status: Option<i64>,
    last_error: Option<String>,
    last_error_kind: Option<String>,
    last_error_hint: Option<String>,
    eligible: bool,
    state: String,
    reason: String,
}

#[derive(Debug, Deserialize, Clone)]
struct RequestLogSummary {
    downstream_path: String,
    site_name: String,
    upstream_model: String,
    http_status: Option<i64>,
    latency_ms: i64,
    error_message: Option<String>,
    error_kind: String,
    created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusPane {
    Routes,
    Channels,
    Logs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelAction {
    Enable,
    Disable,
    ResetCooldown,
}

impl ChannelAction {
    fn path(self) -> &'static str {
        match self {
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::ResetCooldown => "reset-cooldown",
        }
    }

    fn request_label(self) -> &'static str {
        match self {
            Self::Enable => "enable channel",
            Self::Disable => "disable channel",
            Self::ResetCooldown => "clear cooldown/manual block",
        }
    }

    fn success_message(self, label: &str, site_name: &str) -> String {
        match self {
            Self::Enable => format!("enabled {label} @ {site_name}"),
            Self::Disable => format!("disabled {label} @ {site_name}"),
            Self::ResetCooldown => {
                format!("cleared cooldown/manual block for {label} @ {site_name}")
            }
        }
    }
}

fn toggle_action_for_channel(channel: &ChannelSummary) -> ChannelAction {
    match channel.state.as_str() {
        "disabled" => ChannelAction::Enable,
        "ready" => ChannelAction::Disable,
        _ => ChannelAction::ResetCooldown,
    }
}

#[derive(Debug, Clone)]
enum AppMode {
    Browse,
    OnboardRoute(OnboardRouteForm),
    EditChannel(EditChannelForm),
    Confirm(ConfirmDialog),
    Search(SearchDialog),
    Detail(DetailDialog),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfirmDialog {
    action: PendingAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingAction {
    ChannelState {
        channel_id: i64,
        label: String,
        site_name: String,
        action: ChannelAction,
    },
    DeleteRoute {
        route_id: i64,
        route_model: String,
    },
    DeleteChannel {
        channel_id: i64,
        route_index: usize,
        channel_label: String,
        site_name: String,
    },
}

impl ConfirmDialog {
    fn title(&self) -> &'static str {
        match self.action {
            PendingAction::ChannelState { action, .. } => match action {
                ChannelAction::Enable => "Confirm Enable",
                ChannelAction::Disable => "Confirm Disable",
                ChannelAction::ResetCooldown => "Confirm Recover",
            },
            PendingAction::DeleteRoute { .. } => "Confirm Delete Route",
            PendingAction::DeleteChannel { .. } => "Confirm Delete",
        }
    }

    fn message(&self) -> String {
        match &self.action {
            PendingAction::ChannelState {
                label,
                site_name,
                action,
                ..
            } => match action {
                ChannelAction::Enable => format!("Enable {label} @ {site_name} ?"),
                ChannelAction::Disable => format!("Disable {label} @ {site_name} ?"),
                ChannelAction::ResetCooldown => {
                    format!("Recover {label} @ {site_name} and clear cooldown/block ?")
                }
            },
            PendingAction::DeleteRoute { route_model, .. } => {
                format!("Delete empty route {route_model} ? This cannot be undone.")
            }
            PendingAction::DeleteChannel {
                channel_label,
                site_name,
                ..
            } => format!("Delete {channel_label} @ {site_name} ? This cannot be undone."),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchDialog {
    query: String,
}

impl SearchDialog {
    fn new(query: String) -> Self {
        Self { query }
    }

    fn title(&self) -> &'static str {
        "Filter Routes"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LastSearch {
    query: String,
}

#[derive(Debug, Clone)]
enum DetailDialog {
    Channel(ChannelSummary),
    Log(RequestLogSummary),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FormMode {
    NewRoute,
    AddChannel { route_id: i64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnboardRouteField {
    RouteModel,
    BaseUrl,
    ApiKey,
    UpstreamModel,
    Priority,
    Weight,
}

impl OnboardRouteField {
    fn all() -> [Self; 6] {
        [
            Self::RouteModel,
            Self::BaseUrl,
            Self::ApiKey,
            Self::UpstreamModel,
            Self::Priority,
            Self::Weight,
        ]
    }

    fn label(self) -> &'static str {
        match self {
            Self::RouteModel => "Route Model",
            Self::BaseUrl => "Base URL",
            Self::ApiKey => "API Key",
            Self::UpstreamModel => "Upstream Model",
            Self::Priority => "Priority",
            Self::Weight => "Weight",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::RouteModel => "下游稳定模型名。精确匹配，例如 gpt-5.4。",
            Self::BaseUrl => "上游站点根地址。可直接填带 /v1 的兼容地址。",
            Self::ApiKey => "上游 Bearer Key。界面里会掩码显示，提交时原值会发送。",
            Self::UpstreamModel => "可留空时默认跟 route model 一样，用于适配上游不同模型名。",
            Self::Priority => "越小越优先。只会在最低 priority 可用组内选路。",
            Self::Weight => "同一 priority 组内的加权随机权重，默认 10。",
        }
    }

    fn required(self) -> bool {
        matches!(self, Self::RouteModel | Self::BaseUrl | Self::ApiKey)
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::RouteModel => "例如: gpt-5.4",
            Self::BaseUrl => "例如: https://api.example.com/v1",
            Self::ApiKey => "例如: sk-...",
            Self::UpstreamModel => "留空则使用 route model",
            Self::Priority => "默认 0",
            Self::Weight => "默认 10",
        }
    }
}

trait TextEditableForm {
    type Field: Copy + PartialEq;

    fn fields(&self) -> Vec<Self::Field>;
    fn active_field(&self) -> Self::Field;
    fn set_active_field(&mut self, field: Self::Field);
    fn active_value_mut(&mut self) -> &mut String;

    fn next_field(&mut self) {
        let fields = self.fields();
        let index = fields
            .iter()
            .position(|field| *field == self.active_field())
            .unwrap_or(0);
        self.set_active_field(fields[(index + 1) % fields.len()]);
    }

    fn previous_field(&mut self) {
        let fields = self.fields();
        let index = fields
            .iter()
            .position(|field| *field == self.active_field())
            .unwrap_or(0);
        self.set_active_field(fields[(index + fields.len() - 1) % fields.len()]);
    }

    fn push_char(&mut self, ch: char) {
        self.active_value_mut().push(ch);
    }

    fn backspace(&mut self) {
        self.active_value_mut().pop();
    }
}

#[derive(Debug, Clone, Serialize)]
struct OnboardRouteRequest {
    route_model: String,
    base_url: String,
    api_key: String,
    upstream_model: Option<String>,
    priority: Option<i64>,
    weight: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OnboardRouteForm {
    mode: FormMode,
    route_model: String,
    base_url: String,
    api_key: String,
    upstream_model: String,
    priority: String,
    weight: String,
    active_field: OnboardRouteField,
}

impl OnboardRouteForm {
    fn new_route() -> Self {
        Self {
            mode: FormMode::NewRoute,
            route_model: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            upstream_model: String::new(),
            priority: "0".to_string(),
            weight: "10".to_string(),
            active_field: OnboardRouteField::RouteModel,
        }
    }

    fn add_channel(route: &RouteSummary, prefill: Option<&ChannelPrefill>) -> Self {
        Self {
            mode: FormMode::AddChannel { route_id: route.id },
            route_model: route.model_pattern.clone(),
            base_url: prefill
                .map(|value| value.base_url.clone())
                .unwrap_or_default(),
            api_key: prefill
                .map(|value| value.api_key.clone())
                .unwrap_or_default(),
            upstream_model: prefill
                .map(|value| value.upstream_model.clone())
                .unwrap_or_else(|| route.model_pattern.clone()),
            priority: "0".to_string(),
            weight: "10".to_string(),
            active_field: OnboardRouteField::BaseUrl,
        }
    }

    fn editable_fields(&self) -> Vec<OnboardRouteField> {
        let mut fields = OnboardRouteField::all().to_vec();
        if self.route_model_locked() {
            fields.retain(|field| *field != OnboardRouteField::RouteModel);
        }
        fields
    }

    fn route_model_locked(&self) -> bool {
        matches!(self.mode, FormMode::AddChannel { .. })
    }

    fn title(&self) -> &'static str {
        match self.mode {
            FormMode::NewRoute => "New Route",
            FormMode::AddChannel { .. } => "Add Channel",
        }
    }

    fn to_request(&self) -> Result<OnboardRouteRequest, String> {
        let route_model = self.route_model.trim().to_string();
        if route_model.is_empty() {
            return Err("route_model is required".to_string());
        }

        let base_url = self.base_url.trim().to_string();
        if base_url.is_empty() {
            return Err("base_url is required".to_string());
        }

        let api_key = self.api_key.trim().to_string();
        if api_key.is_empty() {
            return Err("api_key is required".to_string());
        }

        let priority = parse_optional_i64(&self.priority, "priority")?;
        let weight = parse_optional_i64(&self.weight, "weight")?;
        let upstream_model = self.upstream_model.trim();

        Ok(OnboardRouteRequest {
            route_model,
            base_url,
            api_key,
            upstream_model: (!upstream_model.is_empty()).then(|| upstream_model.to_string()),
            priority,
            weight,
        })
    }

    fn to_submission(&self) -> Result<FormSubmission, String> {
        match self.mode {
            FormMode::NewRoute => Ok(FormSubmission::Onboard(self.to_request()?)),
            FormMode::AddChannel { route_id } => {
                let request = self.to_request()?;
                Ok(FormSubmission::AddChannel {
                    route_id,
                    request: CreateRouteChannelRequest {
                        base_url: request.base_url,
                        api_key: request.api_key,
                        upstream_model: request.upstream_model,
                        priority: request.priority,
                        weight: request.weight,
                    },
                })
            }
        }
    }
}

impl TextEditableForm for OnboardRouteForm {
    type Field = OnboardRouteField;

    fn fields(&self) -> Vec<Self::Field> {
        self.editable_fields()
    }

    fn active_field(&self) -> Self::Field {
        self.active_field
    }

    fn set_active_field(&mut self, field: Self::Field) {
        self.active_field = field;
    }

    fn active_value_mut(&mut self) -> &mut String {
        match self.active_field {
            OnboardRouteField::RouteModel => &mut self.route_model,
            OnboardRouteField::BaseUrl => &mut self.base_url,
            OnboardRouteField::ApiKey => &mut self.api_key,
            OnboardRouteField::UpstreamModel => &mut self.upstream_model,
            OnboardRouteField::Priority => &mut self.priority,
            OnboardRouteField::Weight => &mut self.weight,
        }
    }
}

enum FormSubmission {
    Onboard(OnboardRouteRequest),
    AddChannel {
        route_id: i64,
        request: CreateRouteChannelRequest,
    },
}

#[derive(Debug, Deserialize, Clone)]
struct OnboardRouteResponse {
    route_created: bool,
    route: RouteDetail,
    channel: ChannelSummary,
}

#[derive(Debug, Clone, Serialize)]
struct CreateRouteChannelRequest {
    base_url: String,
    api_key: String,
    upstream_model: Option<String>,
    priority: Option<i64>,
    weight: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
struct UpdateChannelRequest {
    base_url: String,
    api_key: String,
    upstream_model: String,
    priority: i64,
    weight: i64,
}

#[derive(Debug, Deserialize, Clone)]
struct DeleteChannelResponse {
    route_id: i64,
    route_model: String,
    channel_label: String,
    site_name: String,
    route_deleted: bool,
    deleted: bool,
}

#[derive(Debug, Deserialize, Clone)]
struct DeleteRouteResponse {
    route_model: String,
    deleted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditChannelField {
    BaseUrl,
    ApiKey,
    UpstreamModel,
    Priority,
    Weight,
}

impl EditChannelField {
    fn all() -> [Self; 5] {
        [
            Self::BaseUrl,
            Self::ApiKey,
            Self::UpstreamModel,
            Self::Priority,
            Self::Weight,
        ]
    }

    fn label(self) -> &'static str {
        match self {
            Self::BaseUrl => "Base URL",
            Self::ApiKey => "API Key",
            Self::UpstreamModel => "Upstream Model",
            Self::Priority => "Priority",
            Self::Weight => "Weight",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::BaseUrl => "上游站点根地址。可直接填带 /v1 的兼容地址。",
            Self::ApiKey => "上游 Bearer Key。界面里会掩码显示，提交时原值会发送。",
            Self::UpstreamModel => "真实发给上游的模型名。",
            Self::Priority => "越小越优先，必须 >= 0。",
            Self::Weight => "同优先级组里的权重，必须 > 0。",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EditChannelForm {
    channel_id: i64,
    route_model: String,
    channel_label: String,
    site_name: String,
    base_url: String,
    api_key: String,
    upstream_model: String,
    priority: String,
    weight: String,
    active_field: EditChannelField,
}

impl EditChannelForm {
    fn new(channel: &ChannelSummary, route: &RouteDetail, prefill: ChannelPrefill) -> Self {
        Self {
            channel_id: channel.channel_id,
            route_model: route.model_pattern.clone(),
            channel_label: channel.channel_label.clone(),
            site_name: channel.site_name.clone(),
            base_url: prefill.base_url,
            api_key: prefill.api_key,
            upstream_model: channel.upstream_model.clone(),
            priority: channel.priority.to_string(),
            weight: channel.weight.to_string(),
            active_field: EditChannelField::BaseUrl,
        }
    }

    fn to_request(&self) -> Result<UpdateChannelRequest, String> {
        let base_url = self.base_url.trim().to_string();
        if base_url.is_empty() {
            return Err("base_url is required".to_string());
        }

        let api_key = self.api_key.trim().to_string();
        if api_key.is_empty() {
            return Err("api_key is required".to_string());
        }

        let upstream_model = self.upstream_model.trim().to_string();
        if upstream_model.is_empty() {
            return Err("upstream_model is required".to_string());
        }

        let priority = parse_required_i64(&self.priority, "priority")?;
        if priority < 0 {
            return Err("priority must be >= 0".to_string());
        }

        let weight = parse_required_i64(&self.weight, "weight")?;
        if weight <= 0 {
            return Err("weight must be > 0".to_string());
        }

        Ok(UpdateChannelRequest {
            base_url,
            api_key,
            upstream_model,
            priority,
            weight,
        })
    }
}

impl TextEditableForm for EditChannelForm {
    type Field = EditChannelField;

    fn fields(&self) -> Vec<Self::Field> {
        EditChannelField::all().to_vec()
    }

    fn active_field(&self) -> Self::Field {
        self.active_field
    }

    fn set_active_field(&mut self, field: Self::Field) {
        self.active_field = field;
    }

    fn active_value_mut(&mut self) -> &mut String {
        match self.active_field {
            EditChannelField::BaseUrl => &mut self.base_url,
            EditChannelField::ApiKey => &mut self.api_key,
            EditChannelField::UpstreamModel => &mut self.upstream_model,
            EditChannelField::Priority => &mut self.priority,
            EditChannelField::Weight => &mut self.weight,
        }
    }
}

struct App {
    client: Client,
    base_url: String,
    auth_key: Option<String>,
    routes: Vec<RouteSummary>,
    selected_route: usize,
    channels: Vec<ChannelSummary>,
    logs: Vec<RequestLogSummary>,
    route_detail: Option<RouteDetail>,
    selected_channel: usize,
    selected_log: usize,
    focus: FocusPane,
    mode: AppMode,
    last_search: Option<LastSearch>,
    status: String,
    show_help: bool,
}

impl App {
    fn new(base_url: String, auth_key: Option<String>) -> Self {
        Self {
            client: Client::new(),
            base_url,
            auth_key,
            routes: Vec::new(),
            selected_route: 0,
            channels: Vec::new(),
            logs: Vec::new(),
            route_detail: None,
            selected_channel: 0,
            selected_log: 0,
            focus: FocusPane::Routes,
            mode: AppMode::Browse,
            last_search: None,
            status: "loading...".to_string(),
            show_help: false,
        }
    }

    async fn refresh_all(&mut self) {
        match self.reload_all().await {
            Ok(()) => {
                self.status = if self.routes.is_empty() {
                    "no routes found".to_string()
                } else {
                    format!(
                        "loaded {} routes, {} channels",
                        self.routes.len(),
                        self.channels.len()
                    )
                };
            }
            Err(error) => {
                self.status = error;
            }
        }
    }

    async fn reload_all(&mut self) -> Result<(), String> {
        self.routes = self.fetch_routes().await?;
        if self.routes.is_empty() {
            self.selected_route = 0;
            self.channels.clear();
            self.logs.clear();
            self.route_detail = None;
            self.selected_channel = 0;
            self.selected_log = 0;
            return Ok(());
        }

        self.selected_route = self.selected_route.min(self.routes.len() - 1);
        self.refresh_channels().await
    }

    fn close_mode(&mut self, status: &str) {
        self.mode = AppMode::Browse;
        self.status = status.to_string();
    }

    async fn refresh_channels(&mut self) -> Result<(), String> {
        let Some(route) = self.selected_route_ref() else {
            self.channels.clear();
            self.logs.clear();
            self.route_detail = None;
            self.selected_channel = 0;
            self.selected_log = 0;
            return Ok(());
        };

        let envelope = self.fetch_route_channels(route.id).await?;
        let logs_envelope = self.fetch_route_logs(route.id).await?;
        self.route_detail = Some(envelope.route);
        self.channels = envelope.channels;
        self.logs = logs_envelope.logs;
        self.selected_channel = if self.channels.is_empty() {
            0
        } else {
            self.selected_channel.min(self.channels.len() - 1)
        };
        self.selected_log = 0;
        Ok(())
    }

    async fn fetch_routes(&self) -> Result<Vec<RouteSummary>, String> {
        self.get_json("/api/routes", "load routes").await
    }

    async fn fetch_route_channels(&self, route_id: i64) -> Result<RouteDetailEnvelope, String> {
        self.get_json(
            &format!("/api/routes/{route_id}/channels"),
            "load route channels",
        )
        .await
    }

    async fn fetch_route_logs(&self, route_id: i64) -> Result<RouteLogsEnvelope, String> {
        self.get_json(
            &format!("/api/routes/{route_id}/logs?limit=12"),
            "load route logs",
        )
        .await
    }

    async fn fetch_channel_prefill(&self, channel_id: i64) -> Result<ChannelPrefill, String> {
        self.get_json(
            &format!("/api/channels/{channel_id}/prefill"),
            "load channel prefill",
        )
        .await
    }

    fn api_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    async fn decode_api_response<T: DeserializeOwned>(
        response: reqwest::Response,
        action: &str,
    ) -> Result<T, String> {
        let response = response
            .error_for_status()
            .map_err(|error| format!("{action} request failed: {error}"))?;
        let payload = response
            .json::<ApiResponse<T>>()
            .await
            .map_err(|error| format!("invalid {action} payload: {error}"))?;
        Ok(payload.data)
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str, action: &str) -> Result<T, String> {
        let mut request = self.client.get(self.api_url(path));
        if let Some(auth_key) = &self.auth_key {
            request = request.bearer_auth(auth_key);
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("failed to {action}: {error}"))?;
        Self::decode_api_response(response, action).await
    }

    async fn post_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        action: &str,
    ) -> Result<T, String> {
        let mut request = self.client.post(self.api_url(path));
        if let Some(auth_key) = &self.auth_key {
            request = request.bearer_auth(auth_key);
        }
        let response = request
            .json(body)
            .send()
            .await
            .map_err(|error| format!("failed to {action}: {error}"))?;
        Self::decode_api_response(response, action).await
    }

    async fn patch_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        action: &str,
    ) -> Result<T, String> {
        let mut request = self.client.patch(self.api_url(path));
        if let Some(auth_key) = &self.auth_key {
            request = request.bearer_auth(auth_key);
        }
        let response = request
            .json(body)
            .send()
            .await
            .map_err(|error| format!("failed to {action}: {error}"))?;
        Self::decode_api_response(response, action).await
    }

    async fn post_empty<T: DeserializeOwned>(&self, path: &str, action: &str) -> Result<T, String> {
        let mut request = self.client.post(self.api_url(path));
        if let Some(auth_key) = &self.auth_key {
            request = request.bearer_auth(auth_key);
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("failed to {action}: {error}"))?;
        Self::decode_api_response(response, action).await
    }

    async fn delete_empty<T: DeserializeOwned>(
        &self,
        path: &str,
        action: &str,
    ) -> Result<T, String> {
        let mut request = self.client.delete(self.api_url(path));
        if let Some(auth_key) = &self.auth_key {
            request = request.bearer_auth(auth_key);
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("failed to {action}: {error}"))?;
        Self::decode_api_response(response, action).await
    }

    fn selected_route_ref(&self) -> Option<&RouteSummary> {
        self.routes.get(self.selected_route)
    }

    fn selected_channel_ref(&self) -> Option<&ChannelSummary> {
        self.channels.get(self.selected_channel)
    }

    fn filtered_route_indices(&self) -> Vec<usize> {
        let Some(query) = self.route_filter_query() else {
            return (0..self.routes.len()).collect();
        };
        let lowered = query.to_ascii_lowercase();
        self.routes
            .iter()
            .enumerate()
            .filter_map(|(index, route)| {
                route_search_text(route).contains(&lowered).then_some(index)
            })
            .collect()
    }

    fn selected_route_visible_index(&self) -> Option<usize> {
        self.filtered_route_indices()
            .iter()
            .position(|index| *index == self.selected_route)
    }

    fn visible_logs(&self) -> Vec<&RequestLogSummary> {
        self.logs.iter().collect()
    }

    fn selected_log_ref(&self) -> Option<&RequestLogSummary> {
        self.visible_logs().get(self.selected_log).copied()
    }

    fn route_filter_query(&self) -> Option<&str> {
        self.last_search
            .as_ref()
            .map(|search| search.query.trim())
            .filter(|query| !query.is_empty())
    }

    async fn set_selected_route(&mut self, route_index: usize) {
        if self.selected_route == route_index {
            return;
        }
        self.selected_route = route_index;
        if let Err(error) = self.refresh_channels().await {
            self.status = error;
        }
    }

    async fn move_down(&mut self) {
        match self.focus {
            FocusPane::Routes => {
                let visible = self.filtered_route_indices();
                if let Some(position) = self.selected_route_visible_index() {
                    if position + 1 < visible.len() {
                        self.set_selected_route(visible[position + 1]).await;
                    }
                }
            }
            FocusPane::Channels => {
                if !self.channels.is_empty() && self.selected_channel + 1 < self.channels.len() {
                    self.selected_channel += 1;
                    self.selected_log = 0;
                }
            }
            FocusPane::Logs => {
                let log_count = self.visible_logs().len();
                if log_count > 0 && self.selected_log + 1 < log_count {
                    self.selected_log += 1;
                }
            }
        }
    }

    async fn move_up(&mut self) {
        match self.focus {
            FocusPane::Routes => {
                let visible = self.filtered_route_indices();
                if let Some(position) = self.selected_route_visible_index() {
                    if position > 0 {
                        self.set_selected_route(visible[position - 1]).await;
                    }
                }
            }
            FocusPane::Channels => {
                if self.selected_channel > 0 {
                    self.selected_channel -= 1;
                    self.selected_log = 0;
                }
            }
            FocusPane::Logs => {
                if self.selected_log > 0 {
                    self.selected_log -= 1;
                }
            }
        }
    }

    async fn move_to_start(&mut self) {
        match self.focus {
            FocusPane::Routes => {
                if let Some(index) = self.filtered_route_indices().first().copied() {
                    self.set_selected_route(index).await;
                }
            }
            FocusPane::Channels => {
                if !self.channels.is_empty() {
                    self.selected_channel = 0;
                    self.selected_log = 0;
                }
            }
            FocusPane::Logs => {
                if !self.logs.is_empty() {
                    self.selected_log = 0;
                }
            }
        }
    }

    async fn move_to_end(&mut self) {
        match self.focus {
            FocusPane::Routes => {
                if let Some(index) = self.filtered_route_indices().last().copied() {
                    self.set_selected_route(index).await;
                }
            }
            FocusPane::Channels => {
                if !self.channels.is_empty() {
                    self.selected_channel = self.channels.len() - 1;
                    self.selected_log = 0;
                }
            }
            FocusPane::Logs => {
                if !self.logs.is_empty() {
                    self.selected_log = self.logs.len() - 1;
                }
            }
        }
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            FocusPane::Routes => FocusPane::Channels,
            FocusPane::Channels => FocusPane::Logs,
            FocusPane::Logs => FocusPane::Routes,
        };
    }

    async fn move_page_down(&mut self) {
        for _ in 0..5 {
            self.move_down().await;
        }
    }

    async fn move_page_up(&mut self) {
        for _ in 0..5 {
            self.move_up().await;
        }
    }

    fn move_focus_left(&mut self) {
        self.focus = match self.focus {
            FocusPane::Routes => FocusPane::Routes,
            FocusPane::Channels => FocusPane::Routes,
            FocusPane::Logs => FocusPane::Channels,
        };
    }

    fn move_focus_right(&mut self) {
        self.focus = match self.focus {
            FocusPane::Routes => FocusPane::Channels,
            FocusPane::Channels => FocusPane::Logs,
            FocusPane::Logs => FocusPane::Logs,
        };
    }

    async fn open_add_channel_form(&mut self) {
        let Some(route) = self.selected_route_ref() else {
            self.open_new_route_form();
            return;
        };
        let route = route.clone();
        let prefill = if let Some(channel) = self.selected_channel_ref() {
            match self.fetch_channel_prefill(channel.channel_id).await {
                Ok(prefill) => Some(prefill),
                Err(error) => {
                    self.status = format!("prefill unavailable, fallback to empty values: {error}");
                    None
                }
            }
        } else {
            None
        };
        self.mode = AppMode::OnboardRoute(OnboardRouteForm::add_channel(&route, prefill.as_ref()));
    }

    fn open_new_route_form(&mut self) {
        self.mode = AppMode::OnboardRoute(OnboardRouteForm::new_route());
    }

    fn open_search(&mut self) {
        let query = self.route_filter_query().unwrap_or_default().to_string();
        self.mode = AppMode::Search(SearchDialog::new(query));
    }

    async fn open_edit_channel_form(&mut self) {
        let Some(channel) = self.selected_channel_ref() else {
            self.status = "no channel selected".to_string();
            return;
        };
        let channel = channel.clone();
        let Some(route) = &self.route_detail else {
            self.status = "route detail unavailable".to_string();
            return;
        };
        let route = route.clone();
        let prefill = match self.fetch_channel_prefill(channel.channel_id).await {
            Ok(prefill) => prefill,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        self.mode = AppMode::EditChannel(EditChannelForm::new(&channel, &route, prefill));
    }

    fn open_detail(&mut self) {
        match self.focus {
            FocusPane::Routes => {
                self.focus = FocusPane::Channels;
            }
            FocusPane::Channels => {
                let Some(channel) = self.selected_channel_ref() else {
                    self.status = "no channel selected".to_string();
                    return;
                };
                self.mode = AppMode::Detail(DetailDialog::Channel(channel.clone()));
            }
            FocusPane::Logs => {
                let Some(log) = self.selected_log_ref() else {
                    self.status = "no log selected".to_string();
                    return;
                };
                self.mode = AppMode::Detail(DetailDialog::Log(log.clone()));
            }
        }
    }

    async fn open_add_form(&mut self) {
        match self.focus {
            FocusPane::Routes => self.open_new_route_form(),
            FocusPane::Channels | FocusPane::Logs => self.open_add_channel_form().await,
        }
    }

    async fn submit_add_channel_form(&mut self) {
        let AppMode::OnboardRoute(form) = &self.mode else {
            return;
        };
        let form = form.clone();
        let submission = match form.to_submission() {
            Ok(request) => request,
            Err(error) => {
                self.status = error;
                return;
            }
        };

        match submission {
            FormSubmission::Onboard(request) => match self.onboard_route(&request).await {
                Ok(result) => {
                    self.mode = AppMode::Browse;
                    self.select_route_by_id(result.route.id);
                    let success_message = if result.route_created {
                        format!(
                            "created route {} and added {} @ {}",
                            result.route.model_pattern,
                            result.channel.channel_label,
                            result.channel.site_name
                        )
                    } else {
                        format!(
                            "added {} @ {} to route {}",
                            result.channel.channel_label,
                            result.channel.site_name,
                            result.route.model_pattern
                        )
                    };
                    if let Err(error) = self.reload_all().await {
                        self.status = error;
                    } else {
                        self.status = success_message;
                    }
                }
                Err(error) => {
                    self.status = error;
                }
            },
            FormSubmission::AddChannel { route_id, request } => {
                match self.create_route_channel(route_id, &request).await {
                    Ok(channel) => {
                        self.mode = AppMode::Browse;
                        self.select_route_by_id(channel.route_id);
                        let success_message = format!(
                            "added {} @ {} to route {}",
                            channel.channel_label, channel.site_name, form.route_model
                        );
                        if let Err(error) = self.reload_all().await {
                            self.status = error;
                        } else {
                            if let Some(index) = self
                                .channels
                                .iter()
                                .position(|existing| existing.channel_id == channel.channel_id)
                            {
                                self.selected_channel = index;
                            }
                            self.status = success_message;
                        }
                    }
                    Err(error) => {
                        self.status = error;
                    }
                }
            }
        }
    }

    async fn onboard_route(
        &self,
        request: &OnboardRouteRequest,
    ) -> Result<OnboardRouteResponse, String> {
        self.post_json("/api/routes", request, "onboard route")
            .await
    }

    async fn create_route_channel(
        &self,
        route_id: i64,
        request: &CreateRouteChannelRequest,
    ) -> Result<ChannelSummary, String> {
        self.post_json(
            &format!("/api/routes/{route_id}/channels"),
            request,
            "add channel",
        )
        .await
    }

    async fn update_channel_routing(
        &self,
        channel_id: i64,
        request: &UpdateChannelRequest,
    ) -> Result<ChannelSummary, String> {
        self.patch_json(
            &format!("/api/channels/{channel_id}"),
            request,
            "edit channel",
        )
        .await
    }

    async fn submit_edit_channel_form(&mut self) {
        let AppMode::EditChannel(form) = &self.mode else {
            return;
        };
        let form = form.clone();
        let request = match form.to_request() {
            Ok(request) => request,
            Err(error) => {
                self.status = error;
                return;
            }
        };

        match self.update_channel_routing(form.channel_id, &request).await {
            Ok(updated) => {
                self.mode = AppMode::Browse;
                let success_message =
                    format!("updated {} @ {}", updated.channel_label, updated.site_name);
                if let Err(error) = self.reload_all().await {
                    self.status = error;
                } else {
                    if let Some(index) = self
                        .channels
                        .iter()
                        .position(|channel| channel.channel_id == updated.channel_id)
                    {
                        self.selected_channel = index;
                    }
                    self.status = success_message;
                }
            }
            Err(error) => {
                self.status = error;
            }
        }
    }

    fn select_route_by_id(&mut self, route_id: i64) {
        if let Some(index) = self.routes.iter().position(|route| route.id == route_id) {
            self.selected_route = index;
        }
    }

    async fn enable_selected_channel(&mut self) {
        self.confirm_selected_channel_action(ChannelAction::Enable);
    }

    async fn disable_selected_channel(&mut self) {
        self.confirm_selected_channel_action(ChannelAction::Disable);
    }

    async fn reset_selected_channel_cooldown(&mut self) {
        self.confirm_selected_channel_action(ChannelAction::ResetCooldown);
    }

    fn copy_base_url(&mut self) {
        match copy_to_clipboard(&format!("{}/v1", self.base_url.trim_end_matches('/'))) {
            Ok(()) => self.status = "OK copied downstream URL to clipboard".to_string(),
            Err(error) => self.status = error,
        }
    }

    fn copy_auth_key(&mut self) {
        let Some(auth_key) = self.auth_key.as_deref() else {
            self.status = "no auth key configured".to_string();
            return;
        };

        match copy_to_clipboard(auth_key) {
            Ok(()) => self.status = "OK copied auth key to clipboard".to_string(),
            Err(error) => self.status = error,
        }
    }

    async fn delete_selected_channel(&mut self) {
        let Some(channel) = self.selected_channel_ref() else {
            self.status = "no channel selected".to_string();
            return;
        };

        self.mode = AppMode::Confirm(ConfirmDialog {
            action: PendingAction::DeleteChannel {
                channel_id: channel.channel_id,
                route_index: self.selected_route,
                channel_label: channel.channel_label.clone(),
                site_name: channel.site_name.clone(),
            },
        });
    }

    async fn delete_selected_route(&mut self) {
        let Some(route) = self.selected_route_ref() else {
            self.status = "no route selected".to_string();
            return;
        };

        if route.channel_count > 0 {
            self.status = format!(
                "route {} is not empty; delete its channels first",
                route.model_pattern
            );
            return;
        }

        self.mode = AppMode::Confirm(ConfirmDialog {
            action: PendingAction::DeleteRoute {
                route_id: route.id,
                route_model: route.model_pattern.clone(),
            },
        });
    }

    fn confirm_selected_channel_action(&mut self, action: ChannelAction) {
        let Some(channel) = self.selected_channel_ref() else {
            self.status = "no channel selected".to_string();
            return;
        };

        self.mode = AppMode::Confirm(ConfirmDialog {
            action: PendingAction::ChannelState {
                channel_id: channel.channel_id,
                label: channel.channel_label.clone(),
                site_name: channel.site_name.clone(),
                action,
            },
        });
    }

    fn toggle_selected_channel_state(&mut self) {
        let Some(channel) = self.selected_channel_ref() else {
            self.status = "no channel selected".to_string();
            return;
        };

        self.confirm_selected_channel_action(toggle_action_for_channel(channel));
    }

    async fn apply_route_filter(&mut self, query: &str) {
        let query = query.trim();
        if query.is_empty() {
            self.last_search = None;
            self.status = "cleared route filter".to_string();
            return;
        }

        let lowered = query.to_ascii_lowercase();
        let matches = self
            .routes
            .iter()
            .enumerate()
            .filter_map(|(index, route)| {
                route_search_text(route).contains(&lowered).then_some(index)
            })
            .collect::<Vec<_>>();

        if matches.is_empty() {
            self.status = format!("no route matched `{query}`");
            return;
        }

        self.last_search = Some(LastSearch {
            query: query.to_string(),
        });
        self.focus = FocusPane::Routes;
        if !matches.contains(&self.selected_route) {
            self.set_selected_route(matches[0]).await;
        }
        self.status = format!("filtered {} route(s) by `{query}`", matches.len());
    }

    async fn submit_search(&mut self) {
        let AppMode::Search(dialog) = &self.mode else {
            return;
        };
        let dialog = dialog.clone();
        self.mode = AppMode::Browse;
        self.apply_route_filter(&dialog.query).await;
    }

    async fn execute_confirmed_action(&mut self) {
        let AppMode::Confirm(dialog) = &self.mode else {
            return;
        };
        let action = dialog.action.clone();
        self.mode = AppMode::Browse;

        match action {
            PendingAction::ChannelState {
                channel_id,
                label,
                site_name,
                action,
            } => {
                self.perform_channel_action(channel_id, &label, &site_name, action)
                    .await;
            }
            PendingAction::DeleteRoute {
                route_id,
                route_model,
            } => {
                self.perform_delete_route(route_id, &route_model).await;
            }
            PendingAction::DeleteChannel {
                channel_id,
                route_index,
                ..
            } => {
                self.perform_delete_channel(channel_id, route_index).await;
            }
        }
    }

    async fn perform_delete_channel(&mut self, channel_id: i64, selected_route_index: usize) {
        match self.delete_channel_request(channel_id).await {
            Ok(deleted) => {
                if let Err(error) = self.reload_all().await {
                    self.status = error;
                } else {
                    self.selected_route =
                        selected_route_index.min(self.routes.len().saturating_sub(1));
                    if let Some(route) = self.route_detail.as_ref() {
                        if route.id == deleted.route_id && !self.channels.is_empty() {
                            self.selected_channel = self
                                .selected_channel
                                .min(self.channels.len().saturating_sub(1));
                        }
                    }
                    self.status = if deleted.deleted {
                        if deleted.route_deleted {
                            format!(
                                "deleted {} @ {} and removed empty route {}",
                                deleted.channel_label, deleted.site_name, deleted.route_model
                            )
                        } else {
                            format!("deleted {} @ {}", deleted.channel_label, deleted.site_name)
                        }
                    } else {
                        format!(
                            "delete returned unexpected state for {}",
                            deleted.channel_label
                        )
                    };
                }
            }
            Err(error) => {
                self.status = error;
            }
        }
    }

    async fn perform_delete_route(&mut self, route_id: i64, route_model: &str) {
        match self.delete_route_request(route_id).await {
            Ok(deleted) => {
                if let Err(error) = self.reload_all().await {
                    self.status = error;
                } else {
                    self.selected_route =
                        self.selected_route.min(self.routes.len().saturating_sub(1));
                    self.status = if deleted.deleted {
                        format!("deleted empty route {}", deleted.route_model)
                    } else {
                        format!("delete returned unexpected state for route {route_model}")
                    };
                }
            }
            Err(error) => {
                self.status = error;
            }
        }
    }

    async fn handle_mode_key(&mut self, key: KeyCode) -> bool {
        match key {
            KeyCode::Esc => match self.mode {
                AppMode::OnboardRoute(_) => {
                    self.close_mode("cancelled onboarding");
                    true
                }
                AppMode::EditChannel(_) => {
                    self.close_mode("cancelled channel edit");
                    true
                }
                AppMode::Confirm(_) => {
                    self.close_mode("cancelled action");
                    true
                }
                AppMode::Search(_) => {
                    self.close_mode("cancelled search");
                    true
                }
                AppMode::Detail(_) => {
                    self.close_mode("closed detail");
                    true
                }
                AppMode::Browse => match self.focus {
                    FocusPane::Routes => false,
                    FocusPane::Channels => {
                        self.focus = FocusPane::Routes;
                        true
                    }
                    FocusPane::Logs => {
                        self.focus = FocusPane::Channels;
                        true
                    }
                },
            },
            KeyCode::Enter => match self.mode {
                AppMode::OnboardRoute(_) => {
                    self.submit_add_channel_form().await;
                    true
                }
                AppMode::EditChannel(_) => {
                    self.submit_edit_channel_form().await;
                    true
                }
                AppMode::Confirm(_) => {
                    self.execute_confirmed_action().await;
                    true
                }
                AppMode::Search(_) => {
                    self.submit_search().await;
                    true
                }
                AppMode::Detail(_) => {
                    self.close_mode("closed detail");
                    true
                }
                AppMode::Browse => false,
            },
            KeyCode::Tab | KeyCode::Down => match &mut self.mode {
                AppMode::OnboardRoute(form) => {
                    form.next_field();
                    true
                }
                AppMode::EditChannel(form) => {
                    form.next_field();
                    true
                }
                AppMode::Search(_) | AppMode::Confirm(_) | AppMode::Detail(_) => false,
                AppMode::Browse => false,
            },
            KeyCode::BackTab | KeyCode::Up => match &mut self.mode {
                AppMode::OnboardRoute(form) => {
                    form.previous_field();
                    true
                }
                AppMode::EditChannel(form) => {
                    form.previous_field();
                    true
                }
                AppMode::Search(_) | AppMode::Confirm(_) | AppMode::Detail(_) => false,
                AppMode::Browse => false,
            },
            KeyCode::Backspace => match &mut self.mode {
                AppMode::OnboardRoute(form) => {
                    form.backspace();
                    true
                }
                AppMode::EditChannel(form) => {
                    form.backspace();
                    true
                }
                AppMode::Search(dialog) => {
                    dialog.query.pop();
                    true
                }
                AppMode::Confirm(_) | AppMode::Detail(_) => false,
                AppMode::Browse => false,
            },
            KeyCode::Char(ch) => match &mut self.mode {
                AppMode::OnboardRoute(form) => {
                    form.push_char(ch);
                    true
                }
                AppMode::EditChannel(form) => {
                    form.push_char(ch);
                    true
                }
                AppMode::Search(dialog) => {
                    dialog.query.push(ch);
                    true
                }
                AppMode::Confirm(_) if matches!(ch, 'y' | 'Y') => {
                    self.execute_confirmed_action().await;
                    true
                }
                AppMode::Confirm(_) if matches!(ch, 'n' | 'N') => {
                    self.close_mode("cancelled action");
                    true
                }
                AppMode::Confirm(_) | AppMode::Detail(_) => false,
                AppMode::Browse => false,
            },
            _ => false,
        }
    }

    async fn perform_channel_action(
        &mut self,
        channel_id: i64,
        label: &str,
        site_name: &str,
        action: ChannelAction,
    ) {
        match self.post_channel_action(channel_id, action).await {
            Ok(updated) => {
                let success_message = action.success_message(label, site_name);

                if let Some(existing) = self
                    .channels
                    .iter_mut()
                    .find(|candidate| candidate.channel_id == channel_id)
                {
                    *existing = updated;
                }

                if let Err(error) = self.reload_all().await {
                    self.status = error;
                } else {
                    self.status = success_message;
                }
            }
            Err(error) => {
                self.status = error;
            }
        }
    }

    async fn post_channel_action(
        &self,
        channel_id: i64,
        action: ChannelAction,
    ) -> Result<ChannelSummary, String> {
        self.post_empty(
            &format!("/api/channels/{channel_id}/{}", action.path()),
            action.request_label(),
        )
        .await
    }

    async fn delete_channel_request(
        &self,
        channel_id: i64,
    ) -> Result<DeleteChannelResponse, String> {
        self.delete_empty(&format!("/api/channels/{channel_id}"), "delete channel")
            .await
    }

    async fn delete_route_request(&self, route_id: i64) -> Result<DeleteRouteResponse, String> {
        self.delete_empty(&format!("/api/routes/{route_id}"), "delete route")
            .await
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();

    let base_url =
        std::env::var("METAPI_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    let auth_key = std::env::var("METAPI_AUTH_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let mut app = App::new(base_url, auth_key);
    app.refresh_all().await;

    let terminal = ratatui::init();

    let result = run_app(terminal, app).await;

    ratatui::restore();

    result
}

async fn run_app(mut terminal: DefaultTerminal, mut app: App) -> Result<(), Box<dyn Error>> {
    let mut events = EventStream::new();
    let mut ticker = time::interval(Duration::from_secs(5));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        terminal.draw(|frame| draw(frame, &app))?;

        tokio::select! {
            maybe_event = events.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_event {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }

                    if app.handle_mode_key(key.code).await {
                        continue;
                    }

                    match key.code {
                        KeyCode::Home => app.move_to_start().await,
                        KeyCode::End => app.move_to_end().await,
                        KeyCode::PageDown => app.move_page_down().await,
                        KeyCode::PageUp => app.move_page_up().await,
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('r') => app.refresh_all().await,
                        KeyCode::Char('/') => app.open_search(),
                        KeyCode::Char('a') => app.open_add_form().await,
                        KeyCode::Char('i') => app.open_edit_channel_form().await,
                        KeyCode::Char('x') => match app.focus {
                            FocusPane::Routes => app.delete_selected_route().await,
                            FocusPane::Channels => app.delete_selected_channel().await,
                            FocusPane::Logs => {}
                        },
                        KeyCode::Char('e') => app.enable_selected_channel().await,
                        KeyCode::Char('d') => app.disable_selected_channel().await,
                        KeyCode::Char('c') => app.reset_selected_channel_cooldown().await,
                        KeyCode::Char(' ') => app.toggle_selected_channel_state(),
                        KeyCode::Char('u') => app.copy_base_url(),
                        KeyCode::Char('K') => app.copy_auth_key(),
                        KeyCode::Char('?') => app.show_help = !app.show_help,
                        KeyCode::Tab => app.toggle_focus(),
                        KeyCode::Left => app.move_focus_left(),
                        KeyCode::Right => app.move_focus_right(),
                        KeyCode::Down => app.move_down().await,
                        KeyCode::Up => app.move_up().await,
                        KeyCode::Enter => app.open_detail(),
                        _ => {}
                    }
                }
            }
            _ = ticker.tick() => {
                app.refresh_all().await;
            }
        }
    }
}

fn draw(frame: &mut Frame, app: &App) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(4)])
        .split(frame.area());

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
        .split(layout[0]);

    draw_routes(frame, columns[0], app);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(columns[1]);
    draw_channels(frame, right[0], app);
    draw_logs(frame, right[1], app);
    draw_status(frame, layout[1], app);

    if app.show_help {
        draw_help_modal(frame);
    }

    match &app.mode {
        AppMode::OnboardRoute(form) => draw_add_channel_modal(frame, form),
        AppMode::EditChannel(form) => draw_edit_channel_modal(frame, form),
        AppMode::Confirm(dialog) => draw_confirm_modal(frame, dialog),
        AppMode::Search(dialog) => draw_search_modal(frame, dialog),
        AppMode::Detail(dialog) => draw_detail_modal(frame, dialog, app),
        AppMode::Browse => {}
    }
}

fn draw_routes(frame: &mut Frame, area: Rect, app: &App) {
    let visible = app.filtered_route_indices();
    let items: Vec<ListItem> = app
        .filtered_route_indices()
        .into_iter()
        .map(|index| &app.routes[index])
        .map(|route| {
            let (health_label, health_color) = route_health_badge(route);
            let line1 = Line::from(vec![
                Span::styled(
                    route.model_pattern.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    route_health_lane(route),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(" "),
                Span::styled(
                    health_label,
                    Style::default()
                        .fg(health_color)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            let line2 = Line::from(format!(
                "run {}/{}  stop {}  cool {}",
                route.ready_channel_count,
                route.enabled_channel_count,
                route.manual_blocked_channel_count,
                route.cooling_channel_count
            ));
            let line3 = Line::from(format!(
                "{}  total {}  off {}",
                route.routing_strategy,
                route.channel_count,
                route
                    .channel_count
                    .saturating_sub(route.enabled_channel_count)
            ));
            ListItem::new(vec![line1, line2, line3])
        })
        .collect();

    let mut state = ListState::default().with_selected(app.selected_route_visible_index());

    let route_title = pane_label("Routes", visible.len(), app.route_filter_query());
    let title = pane_title(&route_title, app.focus == FocusPane::Routes);
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().bg(Color::Blue).fg(Color::Black))
        .highlight_symbol(">> ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_channels(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .channels
        .iter()
        .map(|channel| {
            let state_color = channel_state_color(channel);
            let line1 = Line::from(vec![
                Span::styled(
                    format!("{} @ {}", channel.channel_label, channel.site_name),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    channel_state_badge(channel),
                    Style::default()
                        .fg(state_color)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            let health_hint = channel
                .last_error_kind
                .as_deref()
                .map(short_error_label)
                .unwrap_or("-");
            let line2 = Line::from(format!(
                "P{}  W{}  {}",
                channel.priority, channel.weight, channel.upstream_model
            ));
            let line3 = Line::from(if channel.eligible {
                truncate_text(&channel.site_base_url, 72)
            } else {
                format!("blocked by {health_hint}")
            });
            ListItem::new(vec![line1, line2, line3])
        })
        .collect();

    let mut state = ListState::default().with_selected(if app.channels.is_empty() {
        None
    } else {
        Some(app.selected_channel)
    });

    let channel_title = app
        .selected_route_ref()
        .map(|route| format!("Channels ({}) {}", app.channels.len(), route.model_pattern))
        .unwrap_or_else(|| format!("Channels ({})", app.channels.len()));
    let title = pane_title(&channel_title, app.focus == FocusPane::Channels);
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().bg(Color::Blue).fg(Color::Black))
        .highlight_symbol(">> ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_logs(frame: &mut Frame, area: Rect, app: &App) {
    let visible_logs = app.visible_logs();

    let items: Vec<ListItem> = if visible_logs.is_empty() {
        vec![ListItem::new("No recent logs for this route")]
    } else {
        visible_logs
            .iter()
            .map(|log| {
                let status = log
                    .http_status
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "-".to_string());
                let status_color = log_status_color(log);
                let summary = log
                    .error_message
                    .as_deref()
                    .map(|message| {
                        format!(
                            "{}: {}",
                            short_error_label(&log.error_kind),
                            truncate_text(message, 72)
                        )
                    })
                    .unwrap_or_else(|| "ok".to_string());
                ListItem::new(vec![
                    Line::from(vec![
                        Span::raw(format!("{}  ", log.created_at)),
                        Span::styled(
                            status,
                            Style::default()
                                .fg(status_color)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(format!("  {}ms", log.latency_ms)),
                    ]),
                    Line::from(format!(
                        "{} -> {} @ {}",
                        log.downstream_path, log.upstream_model, log.site_name
                    )),
                    Line::from(summary),
                ])
            })
            .collect()
    };

    let mut state = ListState::default().with_selected(if visible_logs.is_empty() {
        None
    } else {
        Some(app.selected_log.min(visible_logs.len() - 1))
    });

    let logs_title = app
        .selected_route_ref()
        .map(|route| format!("Logs ({}) {}", visible_logs.len(), route.model_pattern))
        .unwrap_or_else(|| format!("Logs ({})", visible_logs.len()));
    let title = pane_title(&logs_title, app.focus == FocusPane::Logs);
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().bg(Color::Blue).fg(Color::Black))
        .highlight_symbol(">> ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let route_hint = app
        .selected_route_ref()
        .map(|route| route.model_pattern.as_str())
        .unwrap_or("-");
    let channel_hint = app
        .selected_channel_ref()
        .map(|channel| channel.channel_label.as_str())
        .unwrap_or("-");

    let lines = vec![
        Line::from(app.status.clone()),
        Line::from(format!(
            "endpoint={}  auth={}  route={}  channel={}  pane={}  filter={}",
            app.base_url.trim_end_matches('/'),
            if app.auth_key.is_some() { "ON" } else { "OFF" },
            route_hint,
            channel_hint,
            focus_label(app.focus),
            current_search_label(app)
        )),
        Line::from(shortcut_hint(app)),
    ];

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Status"))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn draw_help_modal(frame: &mut Frame) {
    let area = centered_rect(60, 40, frame.area());
    frame.render_widget(Clear, area);
    let text = vec![
        Line::from(Span::styled(
            "metapi-tui",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Navigation"),
        Line::from("Tab     cycle focus between routes, channels and logs"),
        Line::from("/       filter routes"),
        Line::from("Left/Right move between panes"),
        Line::from("Home/End jump to top / bottom"),
        Line::from("PgUp/PgDn jump faster in long lists"),
        Line::from("Up/Down move selection"),
        Line::from(""),
        Line::from("Actions"),
        Line::from("a       add route or channel based on current pane"),
        Line::from("i       edit current channel base/url/key and routing"),
        Line::from("x       delete empty route or selected channel"),
        Line::from("Space   toggle current channel state"),
        Line::from("e       enable selected channel"),
        Line::from("d       disable selected channel"),
        Line::from("c       recover selected channel (clear cooldown/block)"),
        Line::from("u       copy downstream base url"),
        Line::from("K       copy configured auth key"),
        Line::from("Enter   drill in or open detail"),
        Line::from("Enter/y confirm current action"),
        Line::from("Esc/n   cancel current action"),
        Line::from(""),
        Line::from("App"),
        Line::from("r       refresh all data"),
        Line::from("?       toggle this help"),
        Line::from("q       quit"),
    ];
    let widget = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title("Help"))
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, area);
}

fn pane_title<'a>(label: &'a str, focused: bool) -> Line<'a> {
    if focused {
        Line::from(vec![
            Span::styled(label, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled("focused", Style::default().fg(Color::Cyan)),
        ])
    } else {
        Line::from(label)
    }
}

fn detail_line(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label}: "),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(value),
    ])
}

fn detail_section_header(label: &str) -> Line<'static> {
    Line::from(Span::styled(
        label.to_string(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))
}

fn route_health_badge(route: &RouteSummary) -> (&'static str, Color) {
    if !route.enabled {
        return ("OFF", Color::DarkGray);
    }
    if route.ready_channel_count > 0 {
        return ("RUN", Color::Green);
    }
    if route.manual_blocked_channel_count > 0 {
        return ("STOP", Color::Red);
    }
    if route.cooling_channel_count > 0 {
        return ("COOL", Color::Yellow);
    }
    ("STOP", Color::LightRed)
}

fn route_health_lane(route: &RouteSummary) -> String {
    let total = route.channel_count.max(0) as usize;
    if total == 0 {
        return "[empty]".to_string();
    }

    let run = route.ready_channel_count.max(0) as usize;
    let cool = route.cooling_channel_count.max(0) as usize;
    let stop = route.manual_blocked_channel_count.max(0) as usize;
    let off = total.saturating_sub(route.enabled_channel_count.max(0) as usize);
    let other = total.saturating_sub(run + cool + stop + off);

    let mut lane = String::from("[");
    lane.push_str(&"#".repeat(run.min(8)));
    lane.push_str(&"~".repeat(cool.min(8)));
    lane.push_str(&"!".repeat(stop.min(8)));
    lane.push_str(&"x".repeat(other.min(8)));
    lane.push_str(&".".repeat(off.min(8)));

    let visible = lane.chars().count().saturating_sub(1);
    if visible > 8 {
        lane = lane.chars().take(9).collect();
        lane.push('+');
    }

    lane.push(']');
    lane
}

fn channel_state_badge(channel: &ChannelSummary) -> String {
    match channel.state.as_str() {
        "ready" => "RUN".to_string(),
        "cooling_down" => channel
            .cooldown_remaining_seconds
            .map(|seconds| format!("COOL {seconds}s"))
            .unwrap_or_else(|| "COOL".to_string()),
        "manual_intervention_required" => "STOP".to_string(),
        "disabled" => "OFF".to_string(),
        "account_inactive" => "STOP".to_string(),
        "site_inactive" => "STOP".to_string(),
        "responses_unsupported" => "STOP".to_string(),
        other => other.to_ascii_uppercase(),
    }
}

fn channel_state_color(channel: &ChannelSummary) -> Color {
    match channel.state.as_str() {
        "ready" => Color::Green,
        "cooling_down" => Color::Yellow,
        "manual_intervention_required" => Color::Red,
        "disabled" => Color::DarkGray,
        "account_inactive" | "site_inactive" => Color::LightRed,
        "responses_unsupported" => Color::Magenta,
        _ => Color::Cyan,
    }
}

fn short_error_label(kind: &str) -> &str {
    match kind {
        "auth_error" => "auth",
        "rate_limited" => "rate-limit",
        "upstream_server_error" => "upstream-5xx",
        "transport_error" => "transport",
        "edge_blocked" => "edge-blocked",
        "upstream_path_error" => "path",
        "unknown_error" => "unknown",
        _ => kind,
    }
}

fn log_status_color(log: &RequestLogSummary) -> Color {
    match log.http_status {
        Some(status) if (200..300).contains(&status) => Color::Green,
        Some(429) => Color::Yellow,
        Some(status) if status >= 400 => Color::Red,
        None => Color::LightRed,
        _ => Color::Cyan,
    }
}

fn focus_label(focus: FocusPane) -> &'static str {
    match focus {
        FocusPane::Routes => "routes",
        FocusPane::Channels => "channels",
        FocusPane::Logs => "logs",
    }
}

fn current_search_label(app: &App) -> String {
    app.route_filter_query()
        .map(|query| format!("/{query}"))
        .unwrap_or_else(|| "-".to_string())
}

fn pane_label(base: &str, count: usize, query: Option<&str>) -> String {
    match query {
        Some(query) => format!("{base} ({count}) /{query}"),
        None => format!("{base} ({count})"),
    }
}

fn shortcut_hint(app: &App) -> &'static str {
    match app.mode {
        AppMode::OnboardRoute(_) => {
            "Actions: Tab next field  Shift-Tab prev  Enter submit  Esc cancel"
        }
        AppMode::EditChannel(_) => {
            "Actions: Tab next field  Shift-Tab prev  Enter save  Esc cancel"
        }
        AppMode::Confirm(_) => "Actions: Enter or Y confirm  Esc or N cancel",
        AppMode::Search(_) => "Actions: type filter  Enter apply  empty + Enter clears  Esc cancel",
        AppMode::Detail(_) => "Actions: Enter or Esc close",
        AppMode::Browse => match app.focus {
            FocusPane::Routes => {
                "Actions: Up/Down move  Enter select  Right channels  a add route  x delete empty  / filter"
            }
            FocusPane::Channels => {
                "Actions: Up/Down move  Enter detail  Left routes  Right logs  Space toggle  c recover  i edit  a add"
            }
            FocusPane::Logs => {
                "Actions: Up/Down move  Enter detail  Left channels  u copy URL  K copy key  r refresh"
            }
        },
    }
}

fn route_search_text(route: &RouteSummary) -> String {
    format!(
        "{} {} {} {} {}",
        route.model_pattern,
        route.routing_strategy,
        route.ready_channel_count,
        route.cooling_channel_count,
        route.manual_blocked_channel_count
    )
    .to_ascii_lowercase()
}

fn masked_secret(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "not configured".to_string();
    };

    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len().max(4));
    }

    let head = chars.iter().take(4).collect::<String>();
    let tail = chars
        .iter()
        .rev()
        .take(4)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{head}...{tail}")
}

fn copy_to_clipboard(value: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        clipboard_win::set_clipboard_string(value)
            .map_err(|error| format!("failed to copy via Windows clipboard: {error}"))?;
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        copy_to_clipboard_osc52(value)
    }
}

#[cfg(not(windows))]
fn copy_to_clipboard_osc52(value: &str) -> Result<(), String> {
    let encoded = STANDARD.encode(value);
    let mut stdout = std::io::stdout();
    write!(stdout, "\x1b]52;c;{encoded}\x07")
        .and_then(|_| stdout.flush())
        .map_err(|error| format!("failed to copy via terminal clipboard: {error}"))?;
    Ok(())
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_string();
    }

    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn draw_confirm_modal(frame: &mut Frame, dialog: &ConfirmDialog) {
    let area = centered_rect(56, 26, frame.area());
    frame.render_widget(Clear, area);
    let lines = vec![
        Line::from(Span::styled(
            dialog.title(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(dialog.message()),
        Line::from(""),
        Line::from("[Enter] Confirm    [Esc] Cancel"),
        Line::from("[Y] Confirm        [N] Cancel"),
    ];

    let widget = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Action"))
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, area);
}

fn draw_detail_modal(frame: &mut Frame, dialog: &DetailDialog, app: &App) {
    let area = centered_rect(72, 70, frame.area());
    frame.render_widget(Clear, area);

    let mut lines = vec![
        detail_section_header("Downstream Access"),
        detail_line(
            "Client Base URL",
            format!("{}/v1", app.base_url.trim_end_matches('/')),
        ),
        detail_line("Client API Key", masked_secret(app.auth_key.as_deref())),
        detail_line("Copy Base URL", "u".to_string()),
        detail_line("Copy API Key", "K".to_string()),
        Line::from(""),
    ];

    let title = match dialog {
        DetailDialog::Channel(channel) => {
            lines.push(detail_section_header("Channel"));
            lines.push(detail_line(
                "Name",
                format!("{} (#{} )", channel.channel_label, channel.channel_id),
            ));
            lines.push(detail_line("State", channel_state_badge(channel)));
            lines.push(detail_line("Why", channel.reason.clone()));
            lines.push(detail_line("Model", channel.upstream_model.clone()));
            lines.push(detail_line(
                "Route Weight",
                format!("P{} / W{}", channel.priority, channel.weight),
            ));
            lines.push(detail_line(
                "Site",
                format!("{}  {}", channel.site_name, channel.site_base_url),
            ));
            lines.push(detail_line(
                "Account",
                format!("{} (#{} )", channel.account_label, channel.account_id),
            ));
            lines.push(detail_line(
                "Health",
                format!(
                    "site={}  account={}  eligible={}",
                    channel.site_status, channel.account_status, channel.eligible
                ),
            ));
            lines.push(detail_line(
                "Cooldown",
                channel
                    .cooldown_remaining_seconds
                    .map(|seconds| format!("{seconds}s remaining"))
                    .unwrap_or_else(|| "not cooling".to_string()),
            ));
            lines.push(detail_line(
                "Fail Count",
                channel.consecutive_fail_count.to_string(),
            ));
            lines.push(detail_line(
                "Last Status",
                channel
                    .last_status
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "-".to_string()),
            ));
            lines.push(detail_line(
                "Error",
                channel
                    .last_error_kind
                    .clone()
                    .unwrap_or_else(|| "none".to_string()),
            ));
            lines.push(detail_line(
                "Hint",
                channel.last_error_hint.clone().unwrap_or_else(|| {
                    channel
                        .last_error
                        .clone()
                        .unwrap_or_else(|| "-".to_string())
                }),
            ));
            lines.push(detail_line(
                "Manual Block",
                channel.manual_blocked.to_string(),
            ));
            "Channel Detail"
        }
        DetailDialog::Log(log) => {
            lines.push(detail_section_header("Log"));
            lines.push(detail_line("When", log.created_at.clone()));
            lines.push(detail_line(
                "Flow",
                format!(
                    "{} -> {} @ {}",
                    log.downstream_path, log.upstream_model, log.site_name
                ),
            ));
            lines.push(detail_line(
                "Status",
                log.http_status
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "-".to_string()),
            ));
            lines.push(detail_line("Latency", format!("{}ms", log.latency_ms)));
            lines.push(detail_line(
                "Kind",
                short_error_label(&log.error_kind).to_string(),
            ));
            lines.push(detail_line(
                "Message",
                log.error_message
                    .clone()
                    .unwrap_or_else(|| "ok".to_string()),
            ));
            "Log Detail"
        }
    };

    lines.push(Line::from(""));
    lines.push(Line::from("[Enter] Close    [Esc] Close"));

    let widget = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, area);
}

fn draw_search_modal(frame: &mut Frame, dialog: &SearchDialog) {
    let area = centered_rect(60, 24, frame.area());
    frame.render_widget(Clear, area);
    let query = if dialog.query.is_empty() {
        "<type to search>".to_string()
    } else {
        dialog.query.clone()
    };
    let query_style = if dialog.query.is_empty() {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };
    let lines = vec![
        Line::from("Filters the left route list only."),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Route Filter: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(query, query_style),
        ]),
        Line::from(""),
        Line::from("[Enter] Apply filter"),
        Line::from("[Enter] on empty input clears the filter"),
        Line::from("[Esc] Cancel"),
    ];

    let widget = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(dialog.title()))
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, area);
}

fn draw_add_channel_modal(frame: &mut Frame, form: &OnboardRouteForm) {
    let area = centered_rect(78, 68, frame.area());
    frame.render_widget(Clear, area);
    let masked_api_key = mask_api_key(&form.api_key);
    let mode = match form.mode {
        FormMode::NewRoute => "create route + add first channel",
        FormMode::AddChannel { .. } => "append channel to current route",
    };
    let submit_ready = form.to_request().is_ok();
    let preview_upstream = if form.upstream_model.trim().is_empty() {
        "<same as route model>"
    } else {
        form.upstream_model.trim()
    };

    let fields = [
        (OnboardRouteField::RouteModel, form.route_model.as_str()),
        (OnboardRouteField::BaseUrl, form.base_url.as_str()),
        (OnboardRouteField::ApiKey, masked_api_key.as_str()),
        (
            OnboardRouteField::UpstreamModel,
            form.upstream_model.as_str(),
        ),
        (OnboardRouteField::Priority, form.priority.as_str()),
        (OnboardRouteField::Weight, form.weight.as_str()),
    ];

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Mode: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(mode),
        ]),
        Line::from(vec![
            Span::styled("Preview: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(
                "{} -> {} @ {}",
                empty_as_placeholder(form.route_model.trim(), "<route model>"),
                preview_upstream,
                empty_as_placeholder(form.base_url.trim(), "<base url>")
            )),
        ]),
        Line::from(vec![
            Span::styled("Probe: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("submit 前会真实请求一次 /v1/responses，失败不会保存。"),
        ]),
        Line::from(""),
    ];

    for (field, value) in fields {
        let marker = if form.active_field == field { ">" } else { " " };
        let display_value = if value.is_empty() {
            field.placeholder().to_string()
        } else {
            value.to_string()
        };
        let value_style = if value.is_empty() {
            Style::default().fg(Color::DarkGray)
        } else if field == OnboardRouteField::RouteModel && form.route_model_locked() {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };
        let required_tag = if field.required() { "*" } else { "" };
        let locked_tag = if field == OnboardRouteField::RouteModel && form.route_model_locked() {
            " [locked]"
        } else {
            ""
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(
                    "{marker} {:<14}{}{}",
                    field.label(),
                    required_tag,
                    locked_tag
                ),
                Style::default().add_modifier(if form.active_field == field {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::styled(display_value, value_style),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "Field Help: ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(form.active_field.hint()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Defaults: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("priority=0, weight=10。"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Status: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(if submit_ready {
            "required fields look ready"
        } else {
            "route_model / base_url / api_key are required"
        }),
    ]));
    lines.push(Line::from(
        "Type to edit. Tab/Shift-Tab switch fields. Enter submit. Esc cancel.",
    ));

    let widget = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(form.title()))
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, area);
}

fn draw_edit_channel_modal(frame: &mut Frame, form: &EditChannelForm) {
    let area = centered_rect(78, 62, frame.area());
    frame.render_widget(Clear, area);
    let submit_ready = form.to_request().is_ok();
    let masked_api_key = mask_api_key(&form.api_key);

    let fields = [
        (EditChannelField::BaseUrl, form.base_url.as_str()),
        (EditChannelField::ApiKey, masked_api_key.as_str()),
        (
            EditChannelField::UpstreamModel,
            form.upstream_model.as_str(),
        ),
        (EditChannelField::Priority, form.priority.as_str()),
        (EditChannelField::Weight, form.weight.as_str()),
    ];

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Channel: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(
                "{} @ {} (#{} )",
                form.channel_label, form.site_name, form.channel_id
            )),
        ]),
        Line::from(vec![
            Span::styled("Route: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(form.route_model.clone()),
        ]),
        Line::from(vec![
            Span::styled("Preview: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(
                "{} -> {} @ {}",
                form.route_model,
                empty_as_placeholder(form.upstream_model.trim(), "<upstream model>"),
                empty_as_placeholder(form.base_url.trim(), "<base url>")
            )),
        ]),
        Line::from(""),
    ];

    for (field, value) in fields {
        let marker = if form.active_field == field { ">" } else { " " };
        let display_value = if value.is_empty() {
            match field {
                EditChannelField::BaseUrl => "<base url>".to_string(),
                EditChannelField::ApiKey => "<api key>".to_string(),
                EditChannelField::UpstreamModel => "<upstream model>".to_string(),
                EditChannelField::Priority => "<priority>".to_string(),
                EditChannelField::Weight => "<weight>".to_string(),
            }
        } else {
            value.to_string()
        };
        let value_style = if value.is_empty() {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{marker} {:<14}", field.label()),
                Style::default().add_modifier(if form.active_field == field {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::styled(display_value, value_style),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "Field Help: ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(form.active_field.hint()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Status: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(if submit_ready {
            "ready to submit"
        } else {
            "base_url / api_key / upstream_model required; priority >= 0; weight > 0"
        }),
    ]));
    lines.push(Line::from(
        "Type to edit. Tab/Shift-Tab switch fields. Enter submit. Esc cancel.",
    ));

    let widget = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Edit Channel"))
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, area);
}

fn parse_optional_i64(value: &str, field_name: &str) -> Result<Option<i64>, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<i64>()
        .map(Some)
        .map_err(|error| format!("invalid {field_name}: {error}"))
}

fn parse_required_i64(value: &str, field_name: &str) -> Result<i64, String> {
    value
        .trim()
        .parse::<i64>()
        .map_err(|error| format!("invalid {field_name}: {error}"))
}

fn mask_api_key(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    "*".repeat(value.chars().count().max(8))
}

fn empty_as_placeholder<'a>(value: &'a str, placeholder: &'a str) -> &'a str {
    if value.is_empty() { placeholder } else { value }
}

fn centered_rect(horizontal_percent: u16, vertical_percent: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - vertical_percent) / 2),
            Constraint::Percentage(vertical_percent),
            Constraint::Percentage((100 - vertical_percent) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - horizontal_percent) / 2),
            Constraint::Percentage(horizontal_percent),
            Constraint::Percentage((100 - horizontal_percent) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::{ChannelAction, ChannelSummary, masked_secret, toggle_action_for_channel};

    #[test]
    fn masked_secret_keeps_head_and_tail() {
        assert_eq!(masked_secret(Some("sk-metapi-secret")), "sk-m...cret");
    }

    #[test]
    fn toggle_action_follows_channel_state() {
        let mut channel = ChannelSummary {
            channel_id: 1,
            route_id: 1,
            account_id: 1,
            site_name: "test".to_string(),
            site_base_url: "https://example.com/v1".to_string(),
            account_label: "account".to_string(),
            account_status: "active".to_string(),
            channel_label: "chan".to_string(),
            site_status: "active".to_string(),
            upstream_model: "gpt-5.4".to_string(),
            priority: 0,
            weight: 10,
            manual_blocked: false,
            cooldown_remaining_seconds: None,
            consecutive_fail_count: 0,
            last_status: None,
            last_error: None,
            last_error_kind: None,
            last_error_hint: None,
            eligible: true,
            state: "ready".to_string(),
            reason: "ok".to_string(),
        };

        assert_eq!(toggle_action_for_channel(&channel), ChannelAction::Disable);

        channel.state = "disabled".to_string();
        assert_eq!(toggle_action_for_channel(&channel), ChannelAction::Enable);

        channel.state = "cooling_down".to_string();
        assert_eq!(
            toggle_action_for_channel(&channel),
            ChannelAction::ResetCooldown
        );
    }
}
