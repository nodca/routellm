use std::{path::PathBuf, time::Duration};

use llmrouter::{app, bootstrap, config::Config};
use tokio::net::TcpListener;
use tokio::time::{self, MissedTickBehavior};
use tracing_subscriber::fmt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    fmt().init();

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
            let recovery_state = state.clone();
            tokio::spawn(async move {
                let mut ticker = time::interval(Duration::from_secs(15));
                ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                loop {
                    ticker.tick().await;
                    match llmrouter::http::run_background_recovery_cycle(recovery_state.clone())
                        .await
                    {
                        Ok(recovered) if recovered > 0 => {
                            tracing::info!(
                                recovered,
                                "background recovery probe restored channel(s)"
                            );
                        }
                        Ok(_) => {}
                        Err(error) => {
                            tracing::warn!("background recovery cycle failed: {error}");
                        }
                    }
                }
            });

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
                .ok_or("check-config requires a path or LLMROUTER_CONFIG_PATH")?,
        )),
        [cmd, path] if cmd == "check-config" => Ok(Command::CheckConfig(PathBuf::from(path))),
        _ => Err("usage: llmrouter [check-config [config_path]]".into()),
    }
}
