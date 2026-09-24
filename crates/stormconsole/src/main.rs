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
    /// Hash a password for `password_hash` in the config, and exit.
    ///
    /// Here rather than in a separate tool because the alternative is
    /// somebody pasting a plaintext password into the config and meaning to
    /// come back to it. Reads the password from stdin so it does not end up
    /// in a shell history:
    ///
    ///     printf %s 'the password' | stormconsole --hash-password
    #[arg(long)]
    hash_password: bool,
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
    if args.hash_password {
        use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
        let mut pw = String::new();
        if std::io::Read::read_to_string(&mut std::io::stdin(), &mut pw).is_err() {
            fatal("could not read the password from stdin", EX_CONFIG);
        }
        let pw = pw.trim_end_matches(['\n', '\r']);
        if pw.is_empty() {
            fatal("no password on stdin", EX_CONFIG);
        }
        let salt = SaltString::generate(&mut OsRng);
        match argon2::Argon2::default().hash_password(pw.as_bytes(), &salt) {
            Ok(h) => {
                println!("{h}");
                std::process::exit(0);
            }
            Err(e) => fatal(&format!("could not hash: {e}"), EX_CONFIG),
        }
    }
    let config = if std::path::Path::new(&args.config).exists() {
        match config::Config::load(&args.config) {
            Ok(c) => c,
            Err(e) => fatal(&e, EX_CONFIG),
        }
    } else {
        info!(path = %args.config, "no config file — running on defaults");
        config::Config::default()
    };

    // Say it every start, not once in a release note.
    //
    // A plaintext password in the config means anyone who can read the file
    // can log in as that user, and until now the comparison was `==` on the
    // raw string, so a timing difference told a guesser when they were
    // close. Both are fixed; a config still carrying one is not.
    for u in &config.api.users {
        if u.password_hash.is_none() && u.password.is_some() {
            tracing::warn!(
                user = %u.name,
                "plaintext password in the config. Replace it with password_hash: \
                 printf %s '<password>' | stormconsole --hash-password"
            );
        }
    }
    if !config.auth_required() {
        tracing::warn!(
            "no users and no auth_token configured: every request is an \
             authenticated administrator. Anyone who can reach this port can \
             open a serial console, delete a volume, or destroy a machine."
        );
    }
    let config = Arc::new(config);

    // Every upstream defaults to this node's own daemon (the golden runs on
    // the host network), so a StormCOS node lights up with no config at all.
    let mut plugins: Vec<Arc<dyn ConsolePlugin>> = Vec::new();
    // The kubernetes plugin owns the namespace cache and the
    // authorization answer derived from it; the VM plugin shares that one
    // answer rather than asking the apiserver the same question twice.
    let mut namespace_access = None;
    if config.kubernetes.enabled {
        let k8s = Arc::new(plugin_kubernetes::KubernetesPlugin::new(
            Some(config.kubernetes_server()),
            config.kubernetes.token.clone(),
            config.kubernetes_insecure(),
        ));
        namespace_access = Some(k8s.namespace_access());
        plugins.push(k8s);
    }
    let logs = config.logs.enabled.then(|| {
        Arc::new(plugin_logs::LogsPlugin::with_retention(
            config.logs.mcast_group.clone(),
            config.logs_db_path(),
            config.logs.ring_cap,
            config.logs.retain_hours,
            config.logs.dedup,
        ))
    });
    if config.fleet.enabled {
        plugins.push(Arc::new(plugin_fleet::FleetPlugin::new(
            config.fleet.mcast_group.clone(),
            config.fleet.stormd_host.clone(),
            config.fleet.stormd_ports.clone(),
            logs.as_ref().map(|l| l.hosts()),
        )));
    }
    if let Some(logs) = logs {
        plugins.push(logs);
    }
    if config.stormdrive.enabled {
        plugins.push(Arc::new(plugin_stormdrive::plugin(&config.stormdrive_url())));
    }
    if config.stormstorage.enabled {
        plugins.push(Arc::new(plugin_stormstorage::plugin(&config.stormstorage_url())));
    }
    if config.stormblock.enabled {
        plugins.push(Arc::new(plugin_stormblock::StormblockPlugin::new(&config.stormblock_url())));
    }
    // The datastore rustkube stands on, so the relation is drawn only
    // when there is an apiserver component to draw it to.
    if config.fastetcd.enabled {
        plugins.push(Arc::new(plugin_fastetcd::FastetcdPlugin::new(
            &config.fastetcd_url(),
            &config.fastetcd_metrics_url(),
            config.kubernetes.enabled.then(|| "plugin:k8s".to_string()),
        )));
    }
    if config.sbregistry.enabled {
        plugins.push(Arc::new(plugin_sbregistry::SbregistryPlugin::new(&config.sbregistry_url())));
    }
    // Cloud images: what could be goldened, what has been, and where the
    // copies are. Beside sbregistry in the nav, because both answer "where
    // does an image come from" and a person should find one list.
    if config.vmimages.enabled {
        // With the apiserver, so an image's own events can be drawn beside it.
        plugins.push(Arc::new(plugin_vmimages::VmImagesPlugin::with_kube(
            &config.vmimages_url(),
            config.kubernetes.enabled.then(|| config.kubernetes_server()),
            config.kubernetes.token.clone(),
            config.kubernetes_insecure(),
        )));
    }
    // VMs are kube objects here — the plugin watches the same apiserver
    // with the same credential, and only reaches stormvm for the console
    // doors.
    if config.vm.enabled {
        // The same image operator the vmimages plugin browses, given to the
        // create form so a root disk is chosen from what exists rather than
        // typed from memory. Only when that plugin is enabled: pointing at an
        // operator the operator's own view is not showing would be two
        // answers about one cluster.
        let image_operator = config.vmimages.enabled.then(|| config.vmimages_url());
        plugins.push(Arc::new(plugin_vm::VmPlugin::with_images(
            config.kubernetes.enabled.then(|| config.kubernetes_server()),
            config.kubernetes.token.clone(),
            config.kubernetes_insecure(),
            Some(config.stormvm_url()),
            namespace_access.clone(),
            image_operator,
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
