use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde_json::Value as JsonValue;
use sqlx::{Row, SqlitePool};
use toml::Value as TomlValue;

use crate::{
    config::Config,
    error::AppError,
    store::SqliteStore,
};

const IMPORT_ROUTE_COOLDOWN_SECS: i64 = 300;
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.4";
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-opus-4-6";
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-3.1-pro-preview";

#[derive(Debug, Clone)]
pub struct ImportCcSwitchSummary {
    pub source_path: PathBuf,
    pub imported: usize,
    pub created_routes: usize,
    pub created_channels: usize,
    pub updated_channels: usize,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ImportCandidate {
    pub route_model: String,
    pub upstream_model: String,
    pub base_url: String,
    pub api_key: String,
    pub protocol: String,
}

#[derive(Debug, Clone)]
pub struct LoadCcSwitchSummary {
    pub source_path: PathBuf,
    pub channels: Vec<ImportCandidate>,
    pub skipped: Vec<String>,
}

pub async fn load_cc_switch_import(
    source_path: Option<&Path>,
) -> Result<LoadCcSwitchSummary, AppError> {
    let source_path = source_path
        .map(Path::to_path_buf)
        .unwrap_or_else(default_cc_switch_db_path);
    if !source_path.exists() {
        return Err(AppError::Config(format!(
            "cc-switch db not found: {}",
            source_path.display()
        )));
    }

    let candidates = load_candidates(&source_path).await?;
    Ok(LoadCcSwitchSummary {
        source_path,
        channels: candidates.candidates,
        skipped: candidates.skipped,
    })
}

pub async fn import_cc_switch(
    config: &Config,
    source_path: Option<&Path>,
) -> Result<ImportCcSwitchSummary, AppError> {
    let source_path = source_path
        .map(Path::to_path_buf)
        .unwrap_or_else(default_cc_switch_db_path);
    if !source_path.exists() {
        return Err(AppError::Config(format!(
            "cc-switch db not found: {}",
            source_path.display()
        )));
    }

    let candidates = load_candidates(&source_path).await?;
    let store = SqliteStore::connect(config).await?;

    let mut created_routes = 0;
    let mut created_channels = 0;
    let mut updated_channels = 0;
    let mut skipped = Vec::new();
    let mut imported = 0;

    for route_model in [
        DEFAULT_CODEX_MODEL,
        DEFAULT_CLAUDE_MODEL,
        DEFAULT_GEMINI_MODEL,
    ] {
        let (_, created) = store
            .upsert_route(route_model, IMPORT_ROUTE_COOLDOWN_SECS)
            .await?;
        if created {
            created_routes += 1;
        }
    }

    for candidate in candidates.candidates {
        let (route, route_created) = store
            .upsert_route(&candidate.route_model, IMPORT_ROUTE_COOLDOWN_SECS)
            .await?;
        debug_assert!(!route_created);

        let (_, channel_created) = store
            .sync_channel_for_route(
                &route,
                &candidate.base_url,
                &candidate.api_key,
                &candidate.upstream_model,
                &candidate.protocol,
                0,
                true,
            )
            .await?;
        imported += 1;
        if channel_created {
            created_channels += 1;
        } else {
            updated_channels += 1;
        }
    }

    skipped.extend(candidates.skipped);

    Ok(ImportCcSwitchSummary {
        source_path,
        imported,
        created_routes,
        created_channels,
        updated_channels,
        skipped,
    })
}

#[derive(Debug, Default)]
struct LoadedCandidates {
    candidates: Vec<ImportCandidate>,
    skipped: Vec<String>,
}

async fn load_candidates(path: &Path) -> Result<LoadedCandidates, AppError> {
    let database_url = format!("sqlite://{}", path.display());
    let pool = SqlitePool::connect(&database_url).await?;

    let endpoint_rows = sqlx::query(
        "select provider_id, app_type, url from provider_endpoints order by id asc",
    )
    .fetch_all(&pool)
    .await?;
    let mut endpoint_map: HashMap<(String, String), String> = HashMap::new();
    for row in endpoint_rows {
        let provider_id: String = row.try_get("provider_id")?;
        let app_type: String = row.try_get("app_type")?;
        let url: String = row.try_get("url")?;
        endpoint_map.entry((provider_id, app_type)).or_insert(url);
    }

    let provider_rows = sqlx::query(
        r#"
        select id, app_type, name, settings_config
        from providers
        where app_type in ('codex', 'claude', 'gemini')
        order by app_type asc, coalesce(sort_index, 0) asc, created_at asc
        "#,
    )
    .fetch_all(&pool)
    .await?;

    let mut loaded = LoadedCandidates::default();
    for row in provider_rows {
        let provider_id: String = row.try_get("id")?;
        let app_type: String = row.try_get("app_type")?;
        let name: String = row.try_get("name")?;
        let settings_config: String = row.try_get("settings_config")?;
        let endpoint_url = endpoint_map
            .get(&(provider_id.clone(), app_type.clone()))
            .cloned();

        match build_candidate(&app_type, &name, &settings_config, endpoint_url.as_deref()) {
            Ok(Some(candidate)) => loaded.candidates.push(candidate),
            Ok(None) => loaded
                .skipped
                .push(format!("{app_type}:{name} skipped by importer")),
            Err(error) => loaded
                .skipped
                .push(format!("{app_type}:{name} skipped: {error}")),
        }
    }

    Ok(loaded)
}

fn build_candidate(
    app_type: &str,
    name: &str,
    settings_config: &str,
    endpoint_url: Option<&str>,
) -> Result<Option<ImportCandidate>, String> {
    let settings = serde_json::from_str::<JsonValue>(settings_config)
        .map_err(|error| format!("invalid settings_config json: {error}"))?;

    match app_type {
        "codex" => build_codex_candidate(name, &settings, endpoint_url),
        "claude" => build_claude_candidate(name, &settings, endpoint_url),
        "gemini" => build_gemini_candidate(name, &settings, endpoint_url),
        _ => Ok(None),
    }
}

fn build_codex_candidate(
    _name: &str,
    settings: &JsonValue,
    endpoint_url: Option<&str>,
) -> Result<Option<ImportCandidate>, String> {
    let auth = settings
        .get("auth")
        .and_then(JsonValue::as_object)
        .cloned()
        .unwrap_or_default();
    let api_key = auth
        .get("OPENAI_API_KEY")
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if api_key.is_empty() {
        return Err("missing OPENAI_API_KEY".to_string());
    }

    let config_text = settings
        .get("config")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    let toml = if config_text.trim().is_empty() {
        TomlValue::Table(Default::default())
    } else {
        toml::from_str::<TomlValue>(config_text)
            .map_err(|error| format!("invalid codex config toml: {error}"))?
    };

    let upstream_model = toml
        .get("model")
        .and_then(TomlValue::as_str)
        .unwrap_or(DEFAULT_CODEX_MODEL)
        .trim()
        .to_string();
    let provider = toml
        .get("model_providers")
        .and_then(TomlValue::as_table)
        .and_then(|table| table.get("custom"))
        .and_then(TomlValue::as_table);
    let base_url = provider
        .and_then(|table| table.get("base_url"))
        .and_then(TomlValue::as_str)
        .or(endpoint_url)
        .unwrap_or_default()
        .trim()
        .to_string();
    if base_url.is_empty() {
        return Err("missing base_url".to_string());
    }
    let protocol = provider
        .and_then(|table| table.get("wire_api"))
        .and_then(TomlValue::as_str)
        .unwrap_or("responses");
    let protocol = match protocol {
        "responses" => "responses",
        "chat_completions" => "chat_completions",
        "messages" => "messages",
        other => return Err(format!("unsupported wire_api `{other}`")),
    };

    Ok(Some(ImportCandidate {
        route_model: DEFAULT_CODEX_MODEL.to_string(),
        upstream_model,
        base_url,
        api_key,
        protocol: protocol.to_string(),
    }))
}

fn build_claude_candidate(
    _name: &str,
    settings: &JsonValue,
    endpoint_url: Option<&str>,
) -> Result<Option<ImportCandidate>, String> {
    let env = settings
        .get("env")
        .and_then(JsonValue::as_object)
        .cloned()
        .unwrap_or_default();
    let api_key = env
        .get("ANTHROPIC_AUTH_TOKEN")
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if api_key.is_empty() {
        return Err("missing ANTHROPIC_AUTH_TOKEN".to_string());
    }
    let base_url = env
        .get("ANTHROPIC_BASE_URL")
        .and_then(JsonValue::as_str)
        .or(endpoint_url)
        .unwrap_or_default()
        .trim()
        .to_string();
    if base_url.is_empty() {
        return Err("missing ANTHROPIC_BASE_URL".to_string());
    }
    let upstream_model = env
        .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
        .and_then(JsonValue::as_str)
        .or_else(|| env.get("ANTHROPIC_MODEL").and_then(JsonValue::as_str))
        .unwrap_or(DEFAULT_CLAUDE_MODEL)
        .trim()
        .to_string();

    Ok(Some(ImportCandidate {
        route_model: DEFAULT_CLAUDE_MODEL.to_string(),
        upstream_model,
        base_url,
        api_key,
        protocol: "messages".to_string(),
    }))
}

fn build_gemini_candidate(
    _name: &str,
    settings: &JsonValue,
    endpoint_url: Option<&str>,
) -> Result<Option<ImportCandidate>, String> {
    let env = settings
        .get("env")
        .and_then(JsonValue::as_object)
        .or_else(|| {
            settings
                .get("config")
                .and_then(JsonValue::as_object)
                .and_then(|cfg| cfg.get("env"))
                .and_then(JsonValue::as_object)
        })
        .cloned()
        .unwrap_or_default();
    let api_key = env
        .get("GEMINI_API_KEY")
        .and_then(JsonValue::as_str)
        .or_else(|| env.get("GOOGLE_API_KEY").and_then(JsonValue::as_str))
        .unwrap_or_default()
        .trim()
        .to_string();
    if api_key.is_empty() {
        return Err("missing GEMINI_API_KEY".to_string());
    }
    let base_url = env
        .get("GOOGLE_GEMINI_BASE_URL")
        .and_then(JsonValue::as_str)
        .or_else(|| env.get("GEMINI_BASE_URL").and_then(JsonValue::as_str))
        .or(endpoint_url)
        .unwrap_or_default()
        .trim()
        .to_string();
    if base_url.is_empty() {
        return Err("missing gemini base url".to_string());
    }
    let upstream_model = env
        .get("GOOGLE_GEMINI_MODEL")
        .and_then(JsonValue::as_str)
        .or_else(|| env.get("GEMINI_MODEL").and_then(JsonValue::as_str))
        .unwrap_or(DEFAULT_GEMINI_MODEL)
        .trim()
        .to_string();

    Ok(Some(ImportCandidate {
        route_model: DEFAULT_GEMINI_MODEL.to_string(),
        upstream_model,
        base_url,
        api_key,
        protocol: "chat_completions".to_string(),
    }))
}

fn default_cc_switch_db_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".cc-switch").join("cc-switch.db")
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_CLAUDE_MODEL, DEFAULT_GEMINI_MODEL, build_candidate};

    #[test]
    fn build_claude_candidate_uses_default_model() {
        let settings = r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"sk-x","ANTHROPIC_BASE_URL":"https://api.example.com"}}"#;
        let candidate = build_candidate("claude", "demo", settings, None)
            .unwrap()
            .unwrap();
        assert_eq!(candidate.protocol, "messages");
        assert_eq!(candidate.route_model, DEFAULT_CLAUDE_MODEL);
    }

    #[test]
    fn build_gemini_candidate_uses_default_model() {
        let settings = r#"{"env":{"GEMINI_API_KEY":"sk-x","GOOGLE_GEMINI_BASE_URL":"https://example.com/gemini"}}"#;
        let candidate = build_candidate("gemini", "demo", settings, None)
            .unwrap()
            .unwrap();
        assert_eq!(candidate.protocol, "chat_completions");
        assert_eq!(candidate.route_model, DEFAULT_GEMINI_MODEL);
    }

    #[test]
    fn build_codex_candidate_reads_wire_api() {
        let settings = serde_json::json!({
            "auth": { "OPENAI_API_KEY": "sk-x" },
            "config": "model = \"gpt-5.4\"\n[model_providers.custom]\nbase_url = \"https://api.example.com/v1\"\nwire_api = \"responses\"\n"
        })
        .to_string();
        let candidate = build_candidate("codex", "demo", &settings, None)
            .unwrap()
            .unwrap();
        assert_eq!(candidate.protocol, "responses");
        assert_eq!(candidate.route_model, "gpt-5.4");
    }
}
