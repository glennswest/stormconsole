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

/// Exit status for a config the console cannot run on (sysexits EX_CONFIG).
/// A restart does not fix a config file, and a supervisor reading the
/// code should be able to tell this from a port that was busy.
const EX_CONFIG: i32 = 78;

/// One line on stderr naming what could not be done, then exit. Under
/// stormd that line is the whole of the evidence in the archived run log,
/// so it says the thing itself rather than a Debug dump of an error chain.
fn fatal(what: &str, code: i32) -> ! {
    eprintln!("stormconsole: fatal: {what}");
    std::process::exit(code)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let config = if std::path::Path::new(&args.config).exists() {
        match config::Config::load(&args.config) {
            Ok(c) => c,
            Err(e) => fatal(&e, EX_CONFIG),
        }
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
            config.logs_db_path(),
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

    let bind = config.bind();
    let listener = match tokio::net::TcpListener::bind(bind).await {
        Ok(l) => l,
        Err(e) => fatal(&format!("cannot listen on {bind}: {e}"), 1),
    };
    info!(bind, "stormconsole serving");
    let served = axum::serve(listener, server::router(state))
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            shutdown.cancel();
        })
        .await;
    if let Err(e) = served {
        fatal(&format!("server on {bind} stopped: {e}"), 1);
    }
}
