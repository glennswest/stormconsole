//! One TOML file: who may log in, where the upstreams are, which plugins
//! run. Fleet-discovered endpoints (per-node stormdrive) need no config.
//!
//! Two shapes are accepted. The sectioned one (`[api] bind`, `[logs]
//! db_path`, …) is the console's own. The flat one — `listen_addr` and
//! `data_dir` at top level, nothing else — is what every StormCOS node
//! service (stormdrive, stormstorage) takes, and what stormpump's golden
//! builder writes for all three without knowing which is which. Rejecting
//! it is how the console crash-looped on its first boot (issue #3).

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Flat node-service form of `api.bind`; wins when both are set.
    pub listen_addr: Option<String>,
    /// Where the console keeps state it writes (the log ring). The
    /// golden mounts its data volume at /var/lib/stormconsole.
    pub data_dir: Option<String>,
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub api: Api,
    #[serde(default)]
    pub kubernetes: Kubernetes,
    #[serde(default)]
    pub fleet: Fleet,
    #[serde(default)]
    pub logs: Logs,
    #[serde(default)]
    pub stormdrive: Stormdrive,
    #[serde(default)]
    pub stormstorage: Stormstorage,
    #[serde(default)]
    pub stormblock: Stormblock,
    #[serde(default)]
    pub sbregistry: Sbregistry,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct General {
    #[serde(default = "default_name")]
    pub name: String,
    /// Default web UI theme; a viewer's own pick overrides.
    pub theme: Option<String>,
}

impl Default for General {
    fn default() -> Self {
        Self { name: default_name(), theme: None }
    }
}

fn default_name() -> String {
    "stormconsole".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Api {
    #[serde(default = "default_bind")]
    pub bind: String,
    /// Machine credential: Authorization: Bearer <token>.
    pub auth_token: Option<String>,
    /// Named users for the login screen. Any user or the token being set
    /// turns authentication on, stormd-style.
    #[serde(default)]
    pub users: Vec<User>,
}

impl Default for Api {
    fn default() -> Self {
        Self { bind: default_bind(), auth_token: None, users: Vec::new() }
    }
}

fn default_bind() -> String {
    "0.0.0.0:9094".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub name: String,
    pub password: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Kubernetes {
    #[serde(default = "on")]
    pub enabled: bool,
    /// rustkube apiserver, e.g. "https://192.168.8.150:6443".
    pub server: Option<String>,
    /// Bearer token (ServiceAccount JWT).
    pub token: Option<String>,
    /// Accept the apiserver's self-signed cert.
    #[serde(default)]
    pub insecure_skip_tls_verify: bool,
}

impl Default for Kubernetes {
    fn default() -> Self {
        Self { enabled: true, server: None, token: None, insecure_skip_tls_verify: false }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fleet {
    #[serde(default = "on")]
    pub enabled: bool,
    /// stormcast multicast group nodes announce on.
    #[serde(default = "default_group")]
    pub mcast_group: String,
    /// Local ports probed for stormd instances — this node's services.
    /// The StormCOS layout: control plane 9081–9085, node services at
    /// their port + 100 (stormdrive 9192, stormstorage 9193, console 9194).
    #[serde(default = "default_stormd_ports")]
    pub stormd_ports: Vec<u16>,
}

impl Default for Fleet {
    fn default() -> Self {
        Self { enabled: true, mcast_group: default_group(), stormd_ports: default_stormd_ports() }
    }
}

fn default_stormd_ports() -> Vec<u16> {
    (9080..=9089).chain(9180..=9199).collect()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Logs {
    #[serde(default = "on")]
    pub enabled: bool,
    #[serde(default = "default_group")]
    pub mcast_group: String,
    /// SQLite ring store path; defaults to `<data_dir>/logs.db`.
    pub db_path: Option<String>,
}

impl Default for Logs {
    fn default() -> Self {
        Self { enabled: true, mcast_group: default_group(), db_path: None }
    }
}

fn default_group() -> String {
    "239.255.42.1:5514".to_string()
}

pub const DEFAULT_DATA_DIR: &str = "/var/lib/stormconsole";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stormdrive {
    #[serde(default = "on")]
    pub enabled: bool,
    /// This node's stormdrive, e.g. "http://127.0.0.1:9092".
    pub url: Option<String>,
}

impl Default for Stormdrive {
    fn default() -> Self {
        Self { enabled: true, url: None }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stormstorage {
    #[serde(default = "on")]
    pub enabled: bool,
    /// The storage control plane, e.g. "http://127.0.0.1:9093".
    pub url: Option<String>,
}

impl Default for Stormstorage {
    fn default() -> Self {
        Self { enabled: true, url: None }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stormblock {
    #[serde(default = "on")]
    pub enabled: bool,
    /// Block engine management API, e.g. "http://192.168.8.150:9090".
    pub url: Option<String>,
}

impl Default for Stormblock {
    fn default() -> Self {
        Self { enabled: true, url: None }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sbregistry {
    #[serde(default = "on")]
    pub enabled: bool,
    /// sbregistry base URL.
    pub url: Option<String>,
}

impl Default for Sbregistry {
    fn default() -> Self {
        Self { enabled: true, url: None }
    }
}

fn on() -> bool {
    true
}

impl Config {
    /// Read and validate a config file. The error is one line that names
    /// the file and what is wrong with it — a supervisor's log is the only
    /// place it will ever be read.
    pub fn load(path: &str) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read config {path}: {e}"))?;
        Self::parse(&text).map_err(|e| format!("config {path}: {e}"))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let config: Config = toml::from_str(text).map_err(|e| {
            // toml's Display is multi-line with a source excerpt; the
            // message alone says which key or value is wrong.
            let msg = e.message().trim().to_string();
            match e.span() {
                Some(span) => {
                    let line = text[..span.start].matches('\n').count() + 1;
                    format!("line {line}: {msg}")
                }
                None => msg,
            }
        })?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        let bind = self.bind();
        bind.parse::<std::net::SocketAddr>()
            .map_err(|e| format!("listen address {bind:?} is not host:port: {e}"))?;
        Ok(())
    }

    /// The address the console serves on.
    pub fn bind(&self) -> &str {
        self.listen_addr.as_deref().unwrap_or(&self.api.bind)
    }

    pub fn data_dir(&self) -> &str {
        self.data_dir.as_deref().unwrap_or(DEFAULT_DATA_DIR)
    }

    /// rustkube: configured, or this node's own apiserver. The golden runs
    /// on the host network, and a StormCOS sno apiserver is on :6443.
    pub fn kubernetes_server(&self) -> String {
        self.kubernetes.server.clone().unwrap_or_else(|| "https://127.0.0.1:6443".to_string())
    }

    /// The node's apiserver serves a stormcert self-signed certificate and
    /// the console golden mounts no CA, so the local default is accepted
    /// unverified; a configured server is verified unless told otherwise.
    pub fn kubernetes_insecure(&self) -> bool {
        self.kubernetes.insecure_skip_tls_verify || self.kubernetes.server.is_none()
    }

    pub fn stormblock_url(&self) -> String {
        self.stormblock.url.clone().unwrap_or_else(|| "http://127.0.0.1:9090".to_string())
    }

    pub fn sbregistry_url(&self) -> String {
        self.sbregistry.url.clone().unwrap_or_else(|| "http://127.0.0.1:5100".to_string())
    }

    pub fn stormdrive_url(&self) -> String {
        self.stormdrive.url.clone().unwrap_or_else(|| "http://127.0.0.1:9092".to_string())
    }

    pub fn stormstorage_url(&self) -> String {
        self.stormstorage.url.clone().unwrap_or_else(|| "http://127.0.0.1:9093".to_string())
    }

    /// The log ring's SQLite file.
    pub fn logs_db_path(&self) -> String {
        match &self.logs.db_path {
            Some(p) => p.clone(),
            None => format!("{}/logs.db", self.data_dir().trim_end_matches('/')),
        }
    }

    /// Auth is on the moment any credential is configured.
    pub fn auth_required(&self) -> bool {
        !self.api.users.is_empty() || self.api.auth_token.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim what stormpump's build-goldens.sh writes to
    /// /etc/stormconsole/stormconsole.toml — the file that crashed #3.
    const STORMPUMP_GOLDEN: &str = "# stormconsole under stormd, in a golden.
listen_addr = \"0.0.0.0:9094\"
data_dir    = \"/var/lib/stormconsole\"
";

    #[test]
    fn stormpump_flat_shape_is_accepted() {
        let c = Config::parse(STORMPUMP_GOLDEN).unwrap();
        assert_eq!(c.bind(), "0.0.0.0:9094");
        assert_eq!(c.data_dir(), "/var/lib/stormconsole");
        assert_eq!(c.logs_db_path(), "/var/lib/stormconsole/logs.db");
        assert!(c.logs.enabled && c.kubernetes.enabled);
        assert!(!c.auth_required());
    }

    #[test]
    fn example_config_is_accepted() {
        let c = Config::parse(include_str!("../../../config/config.toml")).unwrap();
        assert_eq!(c.bind(), "0.0.0.0:9094");
        assert_eq!(c.logs_db_path(), "/var/lib/stormconsole/logs.db");
    }

    #[test]
    fn defaults_without_a_file() {
        let c = Config::default();
        assert_eq!(c.bind(), "0.0.0.0:9094");
        assert_eq!(c.logs_db_path(), "/var/lib/stormconsole/logs.db");
        assert_eq!(c.data_dir(), DEFAULT_DATA_DIR);
    }

    #[test]
    fn node_local_defaults_light_every_plugin() {
        let c = Config::parse(STORMPUMP_GOLDEN).unwrap();
        assert_eq!(c.kubernetes_server(), "https://127.0.0.1:6443");
        assert!(c.kubernetes_insecure());
        assert_eq!(c.stormblock_url(), "http://127.0.0.1:9090");
        assert_eq!(c.sbregistry_url(), "http://127.0.0.1:5100");
        assert_eq!(c.stormdrive_url(), "http://127.0.0.1:9092");
        assert_eq!(c.stormstorage_url(), "http://127.0.0.1:9093");
        assert!(c.fleet.stormd_ports.contains(&9085) && c.fleet.stormd_ports.contains(&9194));
    }

    #[test]
    fn a_configured_server_is_verified_unless_told_otherwise() {
        let c = Config::parse("[kubernetes]\nserver = \"https://k.example:6443\"\n").unwrap();
        assert_eq!(c.kubernetes_server(), "https://k.example:6443");
        assert!(!c.kubernetes_insecure());
    }

    #[test]
    fn flat_listen_addr_wins_over_api_bind() {
        let c = Config::parse("listen_addr = \"127.0.0.1:1\"\n[api]\nbind = \"0.0.0.0:2\"\n")
            .unwrap();
        assert_eq!(c.bind(), "127.0.0.1:1");
    }

    #[test]
    fn explicit_db_path_wins_over_data_dir() {
        let c = Config::parse("data_dir = \"/d\"\n[logs]\ndb_path = \"/x/ring.db\"\n").unwrap();
        assert_eq!(c.logs_db_path(), "/x/ring.db");
    }

    #[test]
    fn unknown_key_is_named_with_its_line() {
        let e = Config::parse("listen_addr = \"0.0.0.0:9094\"\nport = 9094\n").unwrap_err();
        assert!(e.contains("line 2"), "{e}");
        assert!(e.contains("port"), "{e}");
    }

    #[test]
    fn bad_listen_address_is_a_config_error() {
        let e = Config::parse("listen_addr = \"9094\"\n").unwrap_err();
        assert!(e.contains("listen address"), "{e}");
    }
}
