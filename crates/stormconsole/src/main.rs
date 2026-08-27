//! stormconsole — the StormCOS console. Assembles the enabled plugins,
//! hands them to the console-core registry, and serves the UI and API on
//! one port.

mod auth;
mod config;
mod server;

use std::sync::Arc;

use clap::Parser;
use console_core::{ConsolePlugin, Registry};
use tokio_util::sync::CancellationToken;
use tracing::info;

#[derive(Parser)]
#[command(version, about = "The StormCOS console")]
struct Args {
    /// Path to config.toml; defaults apply if the file is absent.
    #[arg(long, default_value = "/etc/stormconsole/config.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let config = if std::path::Path::new(&args.config).exists() {
        config::Config::load(&args.config)?
    } else {
        info!(path = %args.config, "no config file — running on defaults");
        config::Config::default()
    };
    let config = Arc::new(config);

    let mut plugins: Vec<Arc<dyn ConsolePlugin>> = Vec::new();
    if config.kubernetes.enabled {
        plugins.push(Arc::new(plugin_kubernetes::KubernetesPlugin::new(
            config.kubernetes.server.clone(),
            config.kubernetes.token.clone(),
            config.kubernetes.insecure_skip_tls_verify,
        )));
    }
    if config.fleet.enabled {
        plugins.push(Arc::new(plugin_fleet::FleetPlugin::new(config.fleet.mcast_group.clone())));
    }
    if config.logs.enabled {
        plugins.push(Arc::new(plugin_logs::LogsPlugin::new(
            config.logs.mcast_group.clone(),
            config.logs.db_path.clone(),
        )));
    }
    if config.stormdrive.enabled {
        plugins.push(Arc::new(plugin_stormdrive::StormdrivePlugin::new()));
    }
    if config.stormblock.enabled {
        plugins.push(Arc::new(plugin_stormblock::StormblockPlugin::new(
            config.stormblock.url.clone(),
        )));
    }
    if config.sbregistry.enabled {
        plugins.push(Arc::new(plugin_sbregistry::SbregistryPlugin::new(
            config.sbregistry.url.clone(),
        )));
    }

    let registry = Arc::new(Registry::new(plugins));
    let shutdown = CancellationToken::new();
    tokio::spawn(registry.clone().run(shutdown.clone()));

    let state = server::AppState {
        auth_required: config.auth_required(),
        sessions: Arc::new(auth::Sessions::new()),
        registry,
        config: config.clone(),
    };

    let listener = tokio::net::TcpListener::bind(&config.api.bind).await?;
    info!(bind = %config.api.bind, "stormconsole serving");
    axum::serve(listener, server::router(state))
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            shutdown.cancel();
        })
        .await?;
    Ok(())
}
