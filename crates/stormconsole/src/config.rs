//! One TOML file: who may log in, where the upstreams are, which plugins
//! run. Fleet-discovered endpoints (per-node stormdrive) need no config.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
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
}

impl Default for Fleet {
    fn default() -> Self {
        Self { enabled: true, mcast_group: default_group() }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Logs {
    #[serde(default = "on")]
    pub enabled: bool,
    #[serde(default = "default_group")]
    pub mcast_group: String,
    /// SQLite ring store path.
    #[serde(default = "default_db")]
    pub db_path: String,
}

impl Default for Logs {
    fn default() -> Self {
        Self { enabled: true, mcast_group: default_group(), db_path: default_db() }
    }
}

fn default_group() -> String {
    "239.255.42.1:5514".to_string()
}

fn default_db() -> String {
    "/var/stormconsole/logs.db".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stormdrive {
    #[serde(default = "on")]
    pub enabled: bool,
}

impl Default for Stormdrive {
    fn default() -> Self {
        Self { enabled: true }
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
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
    }

    /// Auth is on the moment any credential is configured.
    pub fn auth_required(&self) -> bool {
        !self.api.users.is_empty() || self.api.auth_token.is_some()
    }
}
