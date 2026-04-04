use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::Deserialize;

use crate::error::AppError;

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_DATABASE_URL: &str = "sqlite://llmrouter-state.db";
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 90;
const DEFAULT_ROUTE_COOLDOWN_SECS: i64 = 300;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub database_url: String,
    pub request_timeout_secs: u64,
    pub master_key: Option<String>,
    pub bootstrap: Option<BootstrapConfig>,
    pub cooldown_policy: CooldownPolicy,
    pub manual_intervention_policy: ManualInterventionPolicy,
}

#[derive(Debug, Clone)]
pub struct BootstrapConfig {
    pub default_cooldown_seconds: i64,
    pub routes: Vec<ConfiguredRoute>,
}

#[derive(Debug, Clone, Default)]
pub struct CooldownPolicy {
    pub auth_error_seconds: Option<i64>,
    pub rate_limited_seconds: Option<i64>,
    pub upstream_server_error_seconds: Option<i64>,
    pub transport_error_seconds: Option<i64>,
    pub edge_blocked_seconds: Option<i64>,
    pub upstream_path_error_seconds: Option<i64>,
    pub unknown_error_seconds: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct ManualInterventionPolicy {
    pub auth_error: bool,
    pub rate_limited: bool,
    pub upstream_server_error: bool,
    pub transport_error: bool,
    pub edge_blocked: bool,
    pub upstream_path_error: bool,
    pub unknown_error: bool,
}

#[derive(Debug, Clone)]
pub struct ConfiguredRoute {
    pub model: String,
    pub cooldown_seconds: i64,
    pub channels: Vec<ConfiguredChannel>,
}

#[derive(Debug, Clone)]
pub struct ConfiguredChannel {
    pub base_url: String,
    pub api_key: String,
    pub upstream_model: String,
    pub protocol: String,
    pub priority: i64,
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
struct RawConfigFile {
    server: Option<RawServerConfig>,
    routing: Option<RawRoutingConfig>,
    #[serde(default)]
    routes: Vec<RawRouteConfig>,
}

#[derive(Debug, Deserialize)]
struct RawServerConfig {
    bind_addr: Option<String>,
    database_url: Option<String>,
    request_timeout_secs: Option<u64>,
    master_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawRoutingConfig {
    default_cooldown_seconds: Option<i64>,
    cooldowns: Option<RawCooldownPolicy>,
    manual_intervention: Option<RawManualInterventionPolicy>,
}

#[derive(Debug, Deserialize)]
struct RawCooldownPolicy {
    auth_error: Option<i64>,
    rate_limited: Option<i64>,
    upstream_server_error: Option<i64>,
    transport_error: Option<i64>,
    edge_blocked: Option<i64>,
    upstream_path_error: Option<i64>,
    unknown_error: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RawManualInterventionPolicy {
    auth_error: Option<bool>,
    rate_limited: Option<bool>,
    upstream_server_error: Option<bool>,
    transport_error: Option<bool>,
    edge_blocked: Option<bool>,
    upstream_path_error: Option<bool>,
    unknown_error: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RawRouteConfig {
    model: String,
    cooldown_seconds: Option<i64>,
    #[serde(default)]
    channels: Vec<RawChannelConfig>,
}

#[derive(Debug, Deserialize)]
struct RawChannelConfig {
    base_url: String,
    api_key: String,
    upstream_model: String,
    protocol: String,
    priority: Option<i64>,
    enabled: Option<bool>,
}

impl Config {
    pub fn from_env() -> Result<Self, AppError> {
        let bind_addr_env = std::env::var("LLMROUTER_BIND_ADDR").ok();
        let database_url_env = std::env::var("LLMROUTER_DATABASE_URL").ok();
        let request_timeout_secs_env = std::env::var("LLMROUTER_REQUEST_TIMEOUT_SECS")
            .ok()
            .map(|value| {
                value.parse::<u64>().map_err(|error| {
                    AppError::Config(format!("invalid LLMROUTER_REQUEST_TIMEOUT_SECS: {error}"))
                })
            })
            .transpose()?;
        let master_key_env = std::env::var("LLMROUTER_MASTER_KEY")
            .ok()
            .map(|value| normalize_optional_secret(value, "LLMROUTER_MASTER_KEY"))
            .transpose()?
            .flatten();
        let config_path = std::env::var("LLMROUTER_CONFIG_PATH")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from);

        let file_config = config_path.as_deref().map(load_config_file).transpose()?;

        let bind_addr = bind_addr_env
            .or_else(|| {
                file_config
                    .as_ref()
                    .and_then(|file| file.server_bind_addr())
            })
            .unwrap_or_else(|| DEFAULT_BIND_ADDR.to_string());
        let database_url = database_url_env
            .or_else(|| file_config.as_ref().and_then(|file| file.database_url()))
            .unwrap_or_else(|| DEFAULT_DATABASE_URL.to_string());
        let request_timeout_secs = request_timeout_secs_env
            .or_else(|| {
                file_config
                    .as_ref()
                    .and_then(|file| file.request_timeout_secs())
            })
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);
        let master_key =
            master_key_env.or_else(|| file_config.as_ref().and_then(|file| file.master_key()));
        let bootstrap = file_config.as_ref().map(|file| file.bootstrap.clone());
        let cooldown_policy = file_config
            .as_ref()
            .map(|file| file.cooldown_policy.clone())
            .unwrap_or_default();
        let manual_intervention_policy = file_config
            .as_ref()
            .map(|file| file.manual_intervention_policy.clone())
            .unwrap_or_default();

        Ok(Self {
            bind_addr: SocketAddr::from_str(&bind_addr)
                .map_err(|error| AppError::Config(format!("invalid LLMROUTER_BIND_ADDR: {error}")))?,
            database_url,
            request_timeout_secs,
            master_key,
            bootstrap,
            cooldown_policy,
            manual_intervention_policy,
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, AppError> {
        let file_config = load_config_file(path)?;
        let bind_addr = file_config
            .server_bind_addr()
            .unwrap_or_else(|| DEFAULT_BIND_ADDR.to_string());
        let database_url = file_config
            .database_url()
            .unwrap_or_else(|| DEFAULT_DATABASE_URL.to_string());
        let request_timeout_secs = file_config
            .request_timeout_secs()
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);

        Ok(Self {
            bind_addr: SocketAddr::from_str(&bind_addr)
                .map_err(|error| AppError::Config(format!("invalid LLMROUTER_BIND_ADDR: {error}")))?,
            database_url,
            request_timeout_secs,
            master_key: file_config.master_key(),
            bootstrap: Some(file_config.bootstrap.clone()),
            cooldown_policy: file_config.cooldown_policy.clone(),
            manual_intervention_policy: file_config.manual_intervention_policy.clone(),
        })
    }

    pub fn config_path_from_env() -> Option<PathBuf> {
        std::env::var("LLMROUTER_CONFIG_PATH")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
    }
}

#[derive(Debug)]
struct LoadedConfigFile {
    server: Option<RawServerConfig>,
    bootstrap: BootstrapConfig,
    cooldown_policy: CooldownPolicy,
    manual_intervention_policy: ManualInterventionPolicy,
}

impl LoadedConfigFile {
    fn server_bind_addr(&self) -> Option<String> {
        self.server.as_ref()?.bind_addr.clone()
    }

    fn database_url(&self) -> Option<String> {
        self.server.as_ref()?.database_url.clone()
    }

    fn request_timeout_secs(&self) -> Option<u64> {
        self.server.as_ref()?.request_timeout_secs
    }

    fn master_key(&self) -> Option<String> {
        self.server
            .as_ref()?
            .master_key
            .clone()
            .map(|value| value.trim().to_string())
    }
}

fn load_config_file(path: &Path) -> Result<LoadedConfigFile, AppError> {
    let raw = fs::read_to_string(path)
        .map_err(|error| AppError::Config(format!("failed to read config file: {error}")))?;
    parse_config_file(&raw)
}

fn parse_config_file(raw: &str) -> Result<LoadedConfigFile, AppError> {
    let parsed = toml::from_str::<RawConfigFile>(raw)
        .map_err(|error| AppError::Config(format!("invalid config file: {error}")))?;
    if let Some(server) = parsed.server.as_ref() {
        if let Some(master_key) = server.master_key.clone() {
            let _ = normalize_optional_secret(master_key, "server.master_key")?;
        }
    }

    let default_cooldown_seconds = parsed
        .routing
        .as_ref()
        .and_then(|routing| routing.default_cooldown_seconds)
        .unwrap_or(DEFAULT_ROUTE_COOLDOWN_SECS);
    validate_cooldown_seconds(default_cooldown_seconds, "routing.default_cooldown_seconds")?;
    let cooldown_policy = parse_cooldown_policy(parsed.routing.as_ref())?;
    let manual_intervention_policy = parse_manual_intervention_policy(parsed.routing.as_ref());

    let mut routes = Vec::with_capacity(parsed.routes.len());
    for (route_index, route) in parsed.routes.into_iter().enumerate() {
        let model = normalize_non_empty(route.model, &format!("routes[{route_index}].model"))?;
        let cooldown_seconds = route.cooldown_seconds.unwrap_or(default_cooldown_seconds);
        validate_cooldown_seconds(
            cooldown_seconds,
            &format!("routes[{route_index}].cooldown_seconds"),
        )?;

        let mut channels = Vec::with_capacity(route.channels.len());
        for (channel_index, channel) in route.channels.into_iter().enumerate() {
            let prefix = format!("routes[{route_index}].channels[{channel_index}]");
            let base_url = normalize_non_empty(channel.base_url, &format!("{prefix}.base_url"))?;
            if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
                return Err(AppError::Config(format!(
                    "{prefix}.base_url must start with http:// or https://"
                )));
            }

            channels.push(ConfiguredChannel {
                base_url,
                api_key: normalize_non_empty(channel.api_key, &format!("{prefix}.api_key"))?,
                upstream_model: normalize_non_empty(
                    channel.upstream_model,
                    &format!("{prefix}.upstream_model"),
                )?,
                protocol: normalize_protocol(&channel.protocol, &format!("{prefix}.protocol"))?,
                priority: validate_priority(
                    channel.priority.unwrap_or(0),
                    &format!("{prefix}.priority"),
                )?,
                enabled: channel.enabled.unwrap_or(true),
            });
        }

        routes.push(ConfiguredRoute {
            model,
            cooldown_seconds,
            channels,
        });
    }

    Ok(LoadedConfigFile {
        server: parsed.server,
        bootstrap: BootstrapConfig {
            default_cooldown_seconds,
            routes,
        },
        cooldown_policy,
        manual_intervention_policy,
    })
}

fn parse_cooldown_policy(routing: Option<&RawRoutingConfig>) -> Result<CooldownPolicy, AppError> {
    let raw = routing.and_then(|routing| routing.cooldowns.as_ref());
    let parse = |value: Option<i64>, field: &str| -> Result<Option<i64>, AppError> {
        value
            .map(|value| validate_cooldown_seconds(value, field))
            .transpose()
    };

    Ok(CooldownPolicy {
        auth_error_seconds: parse(
            raw.and_then(|value| value.auth_error),
            "routing.cooldowns.auth_error",
        )?,
        rate_limited_seconds: parse(
            raw.and_then(|value| value.rate_limited),
            "routing.cooldowns.rate_limited",
        )?,
        upstream_server_error_seconds: parse(
            raw.and_then(|value| value.upstream_server_error),
            "routing.cooldowns.upstream_server_error",
        )?,
        transport_error_seconds: parse(
            raw.and_then(|value| value.transport_error),
            "routing.cooldowns.transport_error",
        )?,
        edge_blocked_seconds: parse(
            raw.and_then(|value| value.edge_blocked),
            "routing.cooldowns.edge_blocked",
        )?,
        upstream_path_error_seconds: parse(
            raw.and_then(|value| value.upstream_path_error),
            "routing.cooldowns.upstream_path_error",
        )?,
        unknown_error_seconds: parse(
            raw.and_then(|value| value.unknown_error),
            "routing.cooldowns.unknown_error",
        )?,
    })
}

fn parse_manual_intervention_policy(
    routing: Option<&RawRoutingConfig>,
) -> ManualInterventionPolicy {
    let raw = routing.and_then(|routing| routing.manual_intervention.as_ref());

    ManualInterventionPolicy {
        auth_error: raw.and_then(|value| value.auth_error).unwrap_or(false),
        rate_limited: raw.and_then(|value| value.rate_limited).unwrap_or(false),
        upstream_server_error: raw
            .and_then(|value| value.upstream_server_error)
            .unwrap_or(false),
        transport_error: raw.and_then(|value| value.transport_error).unwrap_or(false),
        edge_blocked: raw.and_then(|value| value.edge_blocked).unwrap_or(false),
        upstream_path_error: raw
            .and_then(|value| value.upstream_path_error)
            .unwrap_or(false),
        unknown_error: raw.and_then(|value| value.unknown_error).unwrap_or(false),
    }
}

fn normalize_non_empty(value: String, field: &str) -> Result<String, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config(format!("{field} is required")));
    }
    Ok(trimmed.to_string())
}

fn normalize_optional_secret(value: String, field: &str) -> Result<Option<String>, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config(format!("{field} must not be empty")));
    }
    Ok(Some(trimmed.to_string()))
}

fn validate_cooldown_seconds(value: i64, field: &str) -> Result<i64, AppError> {
    if value < 0 {
        return Err(AppError::Config(format!("{field} must be >= 0")));
    }
    Ok(value)
}

fn validate_priority(value: i64, field: &str) -> Result<i64, AppError> {
    if value < 0 {
        return Err(AppError::Config(format!("{field} must be >= 0")));
    }
    Ok(value)
}

fn normalize_protocol(value: &str, field: &str) -> Result<String, AppError> {
    match value.trim() {
        "responses" | "chat_completions" => Ok(value.trim().to_string()),
        "claude" | "messages" => Ok("claude".to_string()),
        _ => Err(AppError::Config(format!(
            "{field} must be one of responses, chat_completions, claude"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_ROUTE_COOLDOWN_SECS, parse_config_file};

    #[test]
    fn parse_config_file_builds_bootstrap_topology() {
        let parsed = parse_config_file(
            r#"
            [server]
            bind_addr = "127.0.0.1:18080"
            master_key = "sk-llmrouter-test"

            [routing]
            default_cooldown_seconds = 450

            [[routes]]
            model = "gpt-5.4"

            [[routes.channels]]
            base_url = "https://api.example.com/v1"
            api_key = "sk-1"
            upstream_model = "gpt-5-4"
            protocol = "responses"
            "#,
        )
        .unwrap();

        assert_eq!(
            parsed.server_bind_addr().as_deref(),
            Some("127.0.0.1:18080")
        );
        assert_eq!(parsed.master_key().as_deref(), Some("sk-llmrouter-test"));
        assert_eq!(parsed.bootstrap.default_cooldown_seconds, 450);
        assert_eq!(parsed.bootstrap.routes.len(), 1);
        assert_eq!(parsed.bootstrap.routes[0].cooldown_seconds, 450);
        assert_eq!(parsed.bootstrap.routes[0].channels[0].protocol, "responses");
        assert!(parsed.bootstrap.routes[0].channels[0].enabled);
    }

    #[test]
    fn parse_config_file_requires_channel_protocol() {
        let error = parse_config_file(
            r#"
            [[routes]]
            model = "gpt-5.4"

            [[routes.channels]]
            base_url = "https://api.example.com"
            api_key = "sk-1"
            upstream_model = "gpt-5.4"
            "#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("missing field `protocol`"));
    }

    #[test]
    fn parse_config_file_rejects_invalid_channel_protocol() {
        let error = parse_config_file(
            r#"
            [[routes]]
            model = "gpt-5.4"

            [[routes.channels]]
            base_url = "https://api.example.com"
            api_key = "sk-1"
            upstream_model = "gpt-5.4"
            protocol = "invalid"
            "#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("protocol must be one of"));
    }

    #[test]
    fn parse_config_file_uses_default_cooldown() {
        let parsed = parse_config_file(
            r#"
            [[routes]]
            model = "gpt-5.4"

            [[routes.channels]]
            base_url = "https://api.example.com"
            api_key = "sk-1"
            upstream_model = "gpt-5.4"
            protocol = "responses"
            "#,
        )
        .unwrap();

        assert_eq!(
            parsed.bootstrap.routes[0].cooldown_seconds,
            DEFAULT_ROUTE_COOLDOWN_SECS
        );
    }

    #[test]
    fn parse_config_file_reads_cooldown_policy_overrides() {
        let parsed = parse_config_file(
            r#"
            [routing.cooldowns]
            auth_error = 1800
            rate_limited = 45
            transport_error = 20

            [[routes]]
            model = "gpt-5.4"

            [[routes.channels]]
            base_url = "https://api.example.com"
            api_key = "sk-1"
            upstream_model = "gpt-5.4"
            protocol = "responses"
            "#,
        )
        .unwrap();

        assert_eq!(parsed.cooldown_policy.auth_error_seconds, Some(1800));
        assert_eq!(parsed.cooldown_policy.rate_limited_seconds, Some(45));
        assert_eq!(parsed.cooldown_policy.transport_error_seconds, Some(20));
        assert_eq!(parsed.cooldown_policy.unknown_error_seconds, None);
    }

    #[test]
    fn parse_config_file_reads_manual_intervention_policy() {
        let parsed = parse_config_file(
            r#"
            [routing.manual_intervention]
            auth_error = true
            upstream_path_error = true

            [[routes]]
            model = "gpt-5.4"

            [[routes.channels]]
            base_url = "https://api.example.com"
            api_key = "sk-1"
            upstream_model = "gpt-5.4"
            protocol = "responses"
            "#,
        )
        .unwrap();

        assert!(parsed.manual_intervention_policy.auth_error);
        assert!(parsed.manual_intervention_policy.upstream_path_error);
        assert!(!parsed.manual_intervention_policy.rate_limited);
    }

    #[test]
    fn parse_config_file_rejects_empty_master_key() {
        let error = parse_config_file(
            r#"
            [server]
            master_key = "   "

            [[routes]]
            model = "gpt-5.4"

            [[routes.channels]]
            base_url = "https://api.example.com"
            api_key = "sk-1"
            upstream_model = "gpt-5.4"
            protocol = "responses"
            "#,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("server.master_key must not be empty")
        );
    }
}
