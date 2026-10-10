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
            "no users, no auth_token and no auth_token_file configured: every request is an \
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
    // One connection to the apiserver for everything that speaks to it as
    // the console (#33): the bearer from `token_file` follows its renewals,
    // and the certificate is checked against `ca_file`.
    let kube = config.kubernetes_conn();
    if let Some(conn) = &kube {
        if !conn.verified() {
            tracing::warn!(
                server = conn.server(),
                "the apiserver's certificate is not verified; set [kubernetes] ca_file"
            );
        }
        if let Some(e) = conn.error() {
            tracing::warn!("{e}");
        }
    }
    if let Some(conn) = kube.clone() {
        let mut k8s = plugin_kubernetes::KubernetesPlugin::new(Some(conn));
        // The provenance of a `stormpump://` image on the pod page (#69).
        if let Some(url) = &config.stormcentral.url {
            let token = config.stormcentral.token_file.as_deref().and_then(|f| match std::fs::read_to_string(f) {
                Ok(t) => Some(t.trim().to_string()),
                Err(e) => {
                    tracing::warn!(file = f, "stormcentral token_file unreadable: {e}");
                    None
                }
            });
            k8s = k8s.with_stormcentral(url.clone(), token);
        }
        let k8s = Arc::new(k8s);
        k8s.namespace_access().set_system_namespaces(config.kubernetes.system_namespaces.clone());
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
    // The fleet's hosts and their addresses: the fleet plugin drills into
    // them, and the drives plugin reads each one's stormdrive (#32).
    let log_hosts = logs.as_ref().map(|l| l.hosts());
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
        plugins.push(Arc::new(plugin_stormdrive::DrivesPlugin::new(
            &config.stormdrive_url(),
            config.stormdrive.nodes.clone(),
            log_hosts.clone(),
        )));
    }
    if config.stormstorage.enabled {
        // stormstorage's write token (#53), held server-side like the
        // engine's: every action on its feed is a write.
        let token = config.stormstorage.token_file.as_deref().and_then(|f| match std::fs::read_to_string(f) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(file = f, "stormstorage token_file unreadable, its writes will be refused: {e}");
                None
            }
        });
        plugins.push(Arc::new(plugin_stormstorage::plugin(&config.stormstorage_url(), token)));
    }
    if config.stormblock.enabled {
        // A guarded engine answers every read with 401 without its token;
        // an unreadable file is said and the plugin shows the 401 in words.
        let token = config.stormblock.token_file.as_deref().and_then(|f| match std::fs::read_to_string(f) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(file = f, "stormblock token_file unreadable: {e}");
                None
            }
        });
        plugins.push(Arc::new(plugin_stormblock::StormblockPlugin::with_token(&config.stormblock_url(), token)));
    }
    // The datastore rustkube stands on, so the relation is drawn only
    // when there is an apiserver component to draw it to.
    if config.fastetcd.enabled {
        plugins.push(Arc::new(plugin_fastetcd::FastetcdPlugin::with_tls(
            &config.fastetcd_url(),
            &config.fastetcd_metrics_url(),
            config.kubernetes.enabled.then(|| "plugin:k8s".to_string()),
            config.fastetcd_tls(),
        )));
    }
    // Bare metal by service tag (#31). stormipmi's write token is read once
    // here; a missing file is said and the page stays readable — writes then
    // come back from stormipmi as 401, in words.
    if config.stormipmi.enabled {
        let token = config.stormipmi.token_file.as_deref().and_then(|f| match std::fs::read_to_string(f) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(file = f, "stormipmi token_file unreadable, writes will be refused: {e}");
                None
            }
        });
        plugins.push(Arc::new(plugin_stormipmi::StormipmiPlugin::new(&config.stormipmi_url(), token)));
    }
    // What the cluster is made of, and changing it (#63). The same shape as
    // stormipmi: reads open, writes admin only with stormcluster's token.
    if config.stormcluster.enabled {
        let token = config.stormcluster.token_file.as_deref().and_then(|f| match std::fs::read_to_string(f) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(file = f, "stormcluster token_file unreadable, writes will be refused: {e}");
                None
            }
        });
        // The objects are written and watched through the apiserver (#88).
        plugins.push(Arc::new(
            plugin_stormcluster::StormclusterPlugin::with_tls(&config.stormcluster_url(), token, config.stormcluster_tls())
                .with_kube(kube.clone()),
        ));
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
            kube.clone(),
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
            kube.clone(),
            Some(config.stormvm_url()),
            namespace_access.clone(),
            image_operator,
        )
        .with_keys_namespace(config.vm.ssh_keys_namespace.as_deref().unwrap_or("default"))));
    }
    // This node's pod network on the flowsdn edition (#83): the agent's
    // loopback API, endpoints scoped by namespace like the pods they are.
    if config.flowsdn.enabled {
        plugins.push(Arc::new(plugin_flowsdn::FlowsdnPlugin::new(
            &config.flowsdn_url(),
            &config.flowsdn_release_manifest(),
            namespace_access.clone(),
        )));
    }

    // Destructive storage is asked of the apiserver as the viewer (#82);
    // with kubernetes off there is nobody to ask, and nobody may.
    let reviewer = console_core::storage::Reviewer::new(kube.clone());
    let registry = Arc::new(Registry::new(plugins).with_reviewer(reviewer));
    let shutdown = CancellationToken::new();
    tokio::spawn(registry.clone().run(shutdown.clone()));

    let state = server::AppState {
        auth_required: config.auth_required(),
        token: Arc::new(auth::ConsoleToken::new(config.api.auth_token.clone(), config.api.auth_token_file.clone())),
        sessions: Arc::new(auth::Sessions::new()),
        registry,
        config: config.clone(),
        kube: kube.clone(),
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
