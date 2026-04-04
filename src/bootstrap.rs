use crate::{config::BootstrapConfig, error::AppError, store::SqliteStore};

#[derive(Debug, Default, Clone, Copy)]
pub struct BootstrapSummary {
    pub routes_created: usize,
    pub routes_updated: usize,
    pub channels_created: usize,
    pub channels_updated: usize,
}

pub async fn sync_config(
    store: &SqliteStore,
    config: &BootstrapConfig,
) -> Result<BootstrapSummary, AppError> {
    let mut summary = BootstrapSummary::default();

    for route in &config.routes {
        let (route_row, route_created) = store
            .upsert_route(&route.model, route.cooldown_seconds)
            .await?;
        if route_created {
            summary.routes_created += 1;
        } else {
            summary.routes_updated += 1;
        }

        for channel in &route.channels {
            let (_, channel_created) = store
                .sync_channel_for_route(
                    &route_row,
                    &channel.base_url,
                    &channel.api_key,
                    &channel.upstream_model,
                    &channel.protocol,
                    channel.priority,
                    channel.enabled,
                )
                .await?;
            if channel_created {
                summary.channels_created += 1;
            } else {
                summary.channels_updated += 1;
            }
        }
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use crate::{
        config::{BootstrapConfig, Config, ConfiguredChannel, ConfiguredRoute},
        store::SqliteStore,
    };

    use super::sync_config;

    fn database_url(path: &std::path::Path) -> String {
        format!("sqlite://{}", path.display())
    }

    #[tokio::test]
    async fn sync_config_creates_and_updates_routes_and_channels() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("llmrouter.db");
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url(&db_path),
            request_timeout_secs: 30,
            master_key: None,
            bootstrap: None,
            cooldown_policy: Default::default(),
            manual_intervention_policy: Default::default(),
        };
        let store = SqliteStore::connect(&config).await.unwrap();

        let bootstrap = BootstrapConfig {
            default_cooldown_seconds: 300,
            routes: vec![ConfiguredRoute {
                model: "gpt-5.4".to_string(),
                cooldown_seconds: 300,
                channels: vec![ConfiguredChannel {
                    base_url: "https://api.example.com/v1".to_string(),
                    api_key: "sk-1".to_string(),
                    upstream_model: "gpt-5.4".to_string(),
                    protocol: "responses".to_string(),
                    priority: 0,
                    enabled: true,
                }],
            }],
        };

        let first = sync_config(&store, &bootstrap).await.unwrap();
        assert_eq!(first.routes_created, 1);
        assert_eq!(first.channels_created, 1);

        let updated = BootstrapConfig {
            default_cooldown_seconds: 300,
            routes: vec![ConfiguredRoute {
                model: "gpt-5.4".to_string(),
                cooldown_seconds: 600,
                channels: vec![ConfiguredChannel {
                    base_url: "https://api.example.com/v1".to_string(),
                    api_key: "sk-1".to_string(),
                    upstream_model: "gpt-5.4".to_string(),
                    protocol: "messages".to_string(),
                    priority: 2,
                    enabled: false,
                }],
            }],
        };

        let second = sync_config(&store, &updated).await.unwrap();
        assert_eq!(second.routes_updated, 1);
        assert_eq!(second.channels_updated, 1);

        let route = store.get_route(1).await.unwrap();
        assert_eq!(route.cooldown_seconds, 600);

        let channel = store.load_channel(1).await.unwrap();
        assert_eq!(channel.priority, 2);
        assert_eq!(channel.protocol, "messages");
        assert_eq!(channel.enabled, 0);
    }
}
