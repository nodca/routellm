use std::path::PathBuf;

use metapi_rs::{app, bootstrap, config::Config};
use tokio::net::TcpListener;
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    match parse_command(std::env::args().skip(1).collect::<Vec<_>>())? {
        Command::Serve => {
            let config = Config::from_env()?;
            let state = app::build_state(&config).await?;
            if let Some(bootstrap_config) = config.bootstrap.as_ref() {
                let summary = bootstrap::sync_config(&state.store, bootstrap_config).await?;
                tracing::info!(
                    routes_created = summary.routes_created,
                    routes_updated = summary.routes_updated,
                    channels_created = summary.channels_created,
                    channels_updated = summary.channels_updated,
                    "config topology sync completed"
                );
            }
            let app = app::build_router(state);
            let listener = TcpListener::bind(config.bind_addr).await?;

            tracing::info!("listening on {}", config.bind_addr);
            axum::serve(listener, app).await?;
        }
        Command::CheckConfig(path) => {
            let config = Config::from_path(&path)?;
            let route_count = config
                .bootstrap
                .as_ref()
                .map(|bootstrap| bootstrap.routes.len())
                .unwrap_or(0);
            let channel_count = config
                .bootstrap
                .as_ref()
                .map(|bootstrap| {
                    bootstrap
                        .routes
                        .iter()
                        .map(|route| route.channels.len())
                        .sum::<usize>()
                })
                .unwrap_or(0);

            println!("config ok");
            println!("path={}", path.display());
            println!("bind_addr={}", config.bind_addr);
            println!("database_url={}", config.database_url);
            println!(
                "master_key={}",
                if config.master_key.is_some() {
                    "configured"
                } else {
                    "off"
                }
            );
            println!("routes={route_count}");
            println!("channels={channel_count}");
        }
    }

    Ok(())
}

enum Command {
    Serve,
    CheckConfig(PathBuf),
}

fn parse_command(args: Vec<String>) -> Result<Command, Box<dyn std::error::Error>> {
    match args.as_slice() {
        [] => Ok(Command::Serve),
        [cmd] if cmd == "check-config" => Ok(Command::CheckConfig(
            Config::config_path_from_env()
                .ok_or("check-config requires a path or METAPI_CONFIG_PATH")?,
        )),
        [cmd, path] if cmd == "check-config" => Ok(Command::CheckConfig(PathBuf::from(path))),
        _ => Err("usage: metapi-rs [check-config [config_path]]".into()),
    }
}
