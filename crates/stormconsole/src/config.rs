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
    #[serde(default)]
    pub vm: Vm,
    #[serde(default)]
    pub vmimages: VmImages,
    #[serde(default)]
    pub fastetcd: Fastetcd,
    #[serde(default)]
    pub stormipmi: Stormipmi,
    #[serde(default)]
    pub stormcluster: Stormcluster,
    #[serde(default)]
    pub stormcentral: Stormcentral,
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
#[serde(deny_unknown_fields)]
pub struct User {
    pub name: String,
    /// An argon2 PHC string — `$argon2id$v=19$m=...$...`.
    ///
    /// Generate one with `stormconsole --hash-password`. argon2 rather than a
    /// digest because a password store is the one place where being slow is
    /// the feature: SHA-256 is fast enough that a leaked config is a list of
    /// passwords by the afternoon.
    #[serde(default)]
    pub password_hash: Option<String>,
    /// A plaintext password, still read so existing configs keep working.
    ///
    /// **Deprecated and warned about at startup.** It was the only option
    /// and the comparison was `==` on the raw string, so anyone who could
    /// read the config could log in as anyone, and a timing difference told
    /// them when they were close.
    #[serde(default)]
    pub password: Option<String>,
    /// What this user may do. Empty means `viewer`: a config that predates
    /// roles should not silently grant more than it used to.
    #[serde(default)]
    pub roles: Vec<String>,
    /// This user's SSH public keys.
    ///
    /// So a machine they create is one they can log into, without pasting a
    /// key into a form every time — and, more importantly, without the habit
    /// that grows in its place: a password in the cloud-init seed. That
    /// happened here, on a VM that turned out to be reachable on the real
    /// network, and the seed is readable by anyone who can read the VMI.
    ///
    /// Several, because people have more than one machine, and a key that
    /// has to be replaced should not mean a VM that cannot be reached.
    #[serde(default)]
    pub ssh_keys: Vec<String>,
    /// This user's own kubernetes identity — a ServiceAccount token or
    /// any bearer rustkube's RBAC knows. When set, what they see of the
    /// cluster is what the apiserver says they may see, asked as them
    /// (issue #7). Without one there is no identity to authorize against
    /// and the console says so rather than implying a check.
    pub kube_token: Option<String>,
}

fn system_namespaces() -> Vec<String> {
    vec!["cilium".into()]
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Kubernetes {
    #[serde(default = "on")]
    pub enabled: bool,
    /// rustkube apiserver, e.g. "https://192.168.8.150:6443".
    pub server: Option<String>,
    /// Bearer token (ServiceAccount JWT), inline.
    pub token: Option<String>,
    /// A file holding the bearer — what stormcert writes and renews in place
    /// (stormcert#27). Re-read whenever it changes. Not with `token`.
    pub token_file: Option<String>,
    /// PEM CA the apiserver's certificate is checked against — only this
    /// CA is trusted. Re-read when it changes. Not with
    /// `insecure_skip_tls_verify`.
    pub ca_file: Option<String>,
    /// Accept the apiserver's self-signed cert.
    #[serde(default)]
    pub insecure_skip_tls_verify: bool,
    /// Namespaces beyond `default`, `openshift`, `kube-*` and `openshift-*`
    /// that hold the system's own things and are never a project (#28).
    #[serde(default = "system_namespaces")]
    pub system_namespaces: Vec<String>,
}

impl Default for Kubernetes {
    fn default() -> Self {
        Self {
            enabled: true,
            server: None,
            token: None,
            token_file: None,
            ca_file: None,
            insecure_skip_tls_verify: false,
            system_namespaces: system_namespaces(),
        }
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
    /// Where those ports are probed; loopback on a node. Set to a node's
    /// address to run the console elsewhere and look at that node.
    #[serde(default = "default_stormd_host")]
    pub stormd_host: String,
}

impl Default for Fleet {
    fn default() -> Self {
        Self {
            enabled: true,
            mcast_group: default_group(),
            stormd_ports: default_stormd_ports(),
            stormd_host: default_stormd_host(),
        }
    }
}

fn default_stormd_host() -> String {
    "127.0.0.1".to_string()
}

/// Where this node's stormd APIs can be: the control plane's 9081–9085, a
/// service golden's port + 100 (9180–9199), and the services whose ports
/// are outside that range — stormlb (80 → 180), nextnfs (8080 → 8180),
/// stormimds (8169 → 8269), minismbd (8445 → 8545), stormrdp (9101 → 9201)
/// and stormcluster (9102 → 9202), per stormcos `deploy/build-goldens.sh`
/// and stormcentral's component registry (#61).
fn default_stormd_ports() -> Vec<u16> {
    (9080..=9089).chain(9180..=9199).chain([9201, 9202, 180, 8180, 8269, 8545]).collect()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Logs {
    #[serde(default = "on")]
    pub enabled: bool,
    #[serde(default = "default_group")]
    pub mcast_group: String,
    /// redb ring store path; defaults to `<data_dir>/logs.redb`.
    pub db_path: Option<String>,
    /// Most distinct entries the ring keeps. The oldest go first.
    #[serde(default = "default_ring_cap")]
    pub ring_cap: u64,
    /// Drop an entry this long after it was last seen. 0 disables the age
    /// bound and leaves `ring_cap` as the only one.
    #[serde(default = "default_retain_hours")]
    pub retain_hours: u64,
    /// Collapse repeats of the same host/app/severity/message into one
    /// entry with a count. Off stores every arrival separately.
    #[serde(default = "on")]
    pub dedup: bool,
}

impl Default for Logs {
    fn default() -> Self {
        Self {
            enabled: true,
            mcast_group: default_group(),
            db_path: None,
            ring_cap: default_ring_cap(),
            retain_hours: default_retain_hours(),
            dedup: true,
        }
    }
}

fn default_ring_cap() -> u64 {
    200_000
}

fn default_retain_hours() -> u64 {
    168
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
    /// Other nodes' stormdrives by host name, beside the ones found through
    /// the fleet (each host heard from, at its address on :9092) — for a
    /// node that does not log to this segment, or a stormdrive elsewhere.
    #[serde(default)]
    pub nodes: std::collections::BTreeMap<String, String>,
}

impl Default for Stormdrive {
    fn default() -> Self {
        Self { enabled: true, url: None, nodes: Default::default() }
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
    /// The engine's API token file (its `<data_dir>/api_token`). An engine
    /// that guards its API answers 401 to every read without it.
    pub token_file: Option<String>,
}

impl Default for Stormblock {
    fn default() -> Self {
        Self { enabled: true, url: None, token_file: None }
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vm {
    #[serde(default = "on")]
    pub enabled: bool,
    /// This node's stormvm, for the serial and framebuffer consoles only
    /// — the VM objects come from the apiserver, because a VM here is a
    /// KubeVirt object the kubelet reconciles (stormvm docs/kube.md).
    /// Unset means no console doors, said plainly rather than shown as a
    /// terminal that never prints.
    pub url: Option<String>,
    /// Where each user's SSH-key Secret (`<user>-ssh-keys`) lives. Copies
    /// are kept in the namespaces their machines are in, because KubeVirt's
    /// `accessCredentials` can only name a Secret in the machine's own.
    #[serde(default)]
    pub ssh_keys_namespace: Option<String>,
}

impl Default for Vm {
    fn default() -> Self {
        Self { enabled: true, url: None, ssh_keys_namespace: None }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmImages {
    #[serde(default = "on")]
    pub enabled: bool,
    /// vmcloud-image-operator's API — the cloud-image catalogue, the
    /// fleet's goldens, and which nodes carry a local copy. On a node it
    /// is the operator beside the control plane; elsewhere, name it.
    pub url: Option<String>,
}

impl Default for VmImages {
    fn default() -> Self {
        Self { enabled: true, url: None }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fastetcd {
    #[serde(default = "on")]
    pub enabled: bool,
    /// The client port, e.g. "http://127.0.0.1:2379": `/health`, and
    /// etcd's v3 JSON gateway (fastetcd v1.8.0 and later, fastetcd#28).
    pub url: Option<String>,
    /// The metrics listener. fastetcd binds it to loopback :2381 by
    /// default, which is why the console reads it from the node.
    pub metrics_url: Option<String>,
    /// Mutual TLS on the client port (#47): the CA fastetcd's certificate
    /// is verified against — only this one, no built-in roots — and the
    /// pair the console presents. On a node: the stormcert node CA and the
    /// `stormconsole-etcd` client pair under `/data/stormcert`, with `url =
    /// "https://127.0.0.1:2379"`. `cert_file` and `key_file` go together.
    pub ca_file: Option<String>,
    pub cert_file: Option<String>,
    pub key_file: Option<String>,
}

impl Default for Fastetcd {
    fn default() -> Self {
        Self { enabled: true, url: None, metrics_url: None, ca_file: None, cert_file: None, key_file: None }
    }
}

/// The Machines page (#31): stormipmi's Machines API, wherever the one
/// stormipmi runs — a bastion, usually, not every node.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stormipmi {
    #[serde(default = "on")]
    pub enabled: bool,
    /// e.g. "http://bastion:9097". Unset means this node's :9097.
    pub url: Option<String>,
    /// stormipmi's `api.tokenFile`, when it has one: every write it takes
    /// needs this bearer. The console holds it; the browser never sees it.
    pub token_file: Option<String>,
}

impl Default for Stormipmi {
    fn default() -> Self {
        Self { enabled: true, url: None, token_file: None }
    }
}

/// The Cluster page (#63): stormcluster, which runs on every node and
/// answers for the whole cluster — any node's will do, so this node's.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stormcluster {
    #[serde(default = "on")]
    pub enabled: bool,
    /// e.g. "http://10.0.0.1:9102". Unset means this node's :9102.
    pub url: Option<String>,
    /// stormcluster's `token_file`, when it has one: every write it takes
    /// needs this bearer. The console holds it; the browser never sees it.
    pub token_file: Option<String>,
    /// :9102 is TLS only since stormcluster#5 (#89): the node CA its
    /// serving certificate is checked against (only this CA), and the
    /// console's client pair (`stormcert-agent client --name
    /// stormconsole-client --cn stormconsole`). Re-read when they change.
    /// With a CA the default url is `https://127.0.0.1:9102`.
    pub ca_file: Option<String>,
    pub cert_file: Option<String>,
    pub key_file: Option<String>,
}

impl Default for Stormcluster {
    fn default() -> Self {
        Self { enabled: true, url: None, token_file: None, ca_file: None, cert_file: None, key_file: None }
    }
}

/// stormcentral, where a `stormpump://` golden's provenance is recorded
/// (#69): component, commit, build id, built at, built by. Off unless a
/// url is set — its golden list is authenticated, and a node holds no
/// stormcentral token by default.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Stormcentral {
    /// e.g. "http://stormcentral.g8.lo".
    pub url: Option<String>,
    /// A file holding a bearer stormcentral accepts (an agent token).
    pub token_file: Option<String>,
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
        let k = &self.kubernetes;
        if k.token.is_some() && k.token_file.is_some() {
            return Err("[kubernetes] token and token_file are both set: use one".into());
        }
        if k.ca_file.is_some() && k.insecure_skip_tls_verify {
            return Err("[kubernetes] ca_file and insecure_skip_tls_verify are both set: \
                        a CA to check against, or no check — not both"
                .into());
        }
        if k.ca_file.is_some() && !self.kubernetes_server().starts_with("https://") {
            return Err(format!(
                "[kubernetes] ca_file is set but server is {:?}: a CA is only used over https://",
                self.kubernetes_server()
            ));
        }
        let sc = &self.stormcluster;
        if sc.cert_file.is_some() != sc.key_file.is_some() {
            return Err("[stormcluster] cert_file and key_file go together: set both, or neither".into());
        }
        if (sc.ca_file.is_some() || sc.cert_file.is_some()) && !self.stormcluster_url().starts_with("https://") {
            return Err(format!(
                "[stormcluster] ca_file/cert_file are set but url is {:?}: certificates are only used over https://",
                self.stormcluster_url()
            ));
        }
        let f = &self.fastetcd;
        if f.cert_file.is_some() != f.key_file.is_some() {
            return Err("[fastetcd] cert_file and key_file go together: set both, or neither".into());
        }
        if (f.ca_file.is_some() || f.cert_file.is_some()) && !self.fastetcd_url().starts_with("https://") {
            return Err(format!(
                "[fastetcd] ca_file/cert_file are set but url is {:?}: certificates are only used over https://",
                self.fastetcd_url()
            ));
        }
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

    /// Whether the apiserver's certificate goes unchecked. With `ca_file`
    /// it is always checked (#33). Without one, a configured server is
    /// checked against the system roots unless told otherwise, and the
    /// zero-config loopback default — a stormcert certificate no system
    /// root vouches for — is not: that is the one case the console warns
    /// about at start, and stormcos closes it by setting `ca_file`
    /// (stormcos#76).
    pub fn kubernetes_insecure(&self) -> bool {
        self.kubernetes.ca_file.is_none()
            && (self.kubernetes.insecure_skip_tls_verify || self.kubernetes.server.is_none())
    }

    /// The one apiserver connection every caller shares (#33), or `None`
    /// when the kubernetes plugin is off.
    pub fn kubernetes_conn(&self) -> Option<std::sync::Arc<console_core::apiserver::Conn>> {
        use console_core::apiserver::{Bearer, Conn, Trust};
        if !self.kubernetes.enabled {
            return None;
        }
        let k = &self.kubernetes;
        let bearer = match (&k.token_file, &k.token) {
            (Some(f), _) => Bearer::File(f.into()),
            (None, Some(t)) => Bearer::Inline(t.clone()),
            (None, None) => Bearer::None,
        };
        let trust = match &k.ca_file {
            Some(ca) => Trust::Ca(ca.into()),
            None if self.kubernetes_insecure() => Trust::Unverified,
            None => Trust::System,
        };
        Some(Conn::new(&self.kubernetes_server(), bearer, trust))
    }

    pub fn stormblock_url(&self) -> String {
        self.stormblock.url.clone().unwrap_or_else(|| "http://127.0.0.1:9090".to_string())
    }

    pub fn vmimages_url(&self) -> String {
        self.vmimages.url.clone().unwrap_or_else(|| "http://127.0.0.1:9099".to_string())
    }

    pub fn sbregistry_url(&self) -> String {
        self.sbregistry.url.clone().unwrap_or_else(|| "http://127.0.0.1:5100".to_string())
    }

    pub fn stormipmi_url(&self) -> String {
        self.stormipmi.url.clone().unwrap_or_else(|| "http://127.0.0.1:9097".to_string())
    }

    /// This node's :9102 — over https when a CA is set (stormcluster#5),
    /// plain for a stormcluster from before it.
    pub fn stormcluster_url(&self) -> String {
        self.stormcluster.url.clone().unwrap_or_else(|| {
            let scheme = if self.stormcluster.ca_file.is_some() { "https" } else { "http" };
            format!("{scheme}://127.0.0.1:9102")
        })
    }

    /// The `[stormcluster]` TLS files, for the plugin (#89).
    pub fn stormcluster_tls(&self) -> console_core::tls::TlsFiles {
        let f = &self.stormcluster;
        console_core::tls::TlsFiles {
            ca: f.ca_file.as_ref().map(Into::into),
            cert: f.cert_file.as_ref().map(Into::into),
            key: f.key_file.as_ref().map(Into::into),
        }
    }

    pub fn fastetcd_url(&self) -> String {
        self.fastetcd.url.clone().unwrap_or_else(|| "http://127.0.0.1:2379".to_string())
    }

    /// The `[fastetcd]` TLS files, for the plugin.
    pub fn fastetcd_tls(&self) -> plugin_fastetcd::tls::TlsFiles {
        let f = &self.fastetcd;
        plugin_fastetcd::tls::TlsFiles {
            ca: f.ca_file.as_ref().map(Into::into),
            cert: f.cert_file.as_ref().map(Into::into),
            key: f.key_file.as_ref().map(Into::into),
        }
    }

    pub fn fastetcd_metrics_url(&self) -> String {
        self.fastetcd.metrics_url.clone().unwrap_or_else(|| "http://127.0.0.1:2381".to_string())
    }

    pub fn stormdrive_url(&self) -> String {
        self.stormdrive.url.clone().unwrap_or_else(|| "http://127.0.0.1:9092".to_string())
    }

    pub fn stormstorage_url(&self) -> String {
        self.stormstorage.url.clone().unwrap_or_else(|| "http://127.0.0.1:9093".to_string())
    }

    /// stormvm's console service on this node. Defaulted like every
    /// other upstream, since the golden runs on the host network.
    pub fn stormvm_url(&self) -> String {
        self.vm.url.clone().unwrap_or_else(|| "http://127.0.0.1:9095".to_string())
    }

    /// The log ring's redb file (`<data_dir>/logs.redb` unless set).
    pub fn logs_db_path(&self) -> String {
        match &self.logs.db_path {
            Some(p) => p.clone(),
            None => format!("{}/logs.redb", self.data_dir().trim_end_matches('/')),
        }
    }

    /// Auth is on the moment any credential is configured.
    pub fn auth_required(&self) -> bool {
        !self.api.users.is_empty() || self.api.auth_token.is_some()
    }

    /// A named user's SSH public keys.
    ///
    /// Read by the VM create form so a machine defaults to its owner's key:
    /// the person creating it is the person who will need to log in.
    pub fn ssh_keys_for(&self, user: &str) -> Vec<String> {
        self.api
            .users
            .iter()
            .find(|u| u.name == user)
            .map(|u| u.ssh_keys.clone())
            .unwrap_or_default()
    }

    /// What a named user may do, defaulting to `viewer`.
    pub fn roles_for(&self, user: &str) -> Vec<String> {
        self.api
            .users
            .iter()
            .find(|u| u.name == user)
            .map(|u| {
                if u.roles.is_empty() {
                    vec!["viewer".to_string()]
                } else {
                    u.roles.clone()
                }
            })
            .unwrap_or_default()
    }

    /// A named user's kubernetes bearer, if they have one.
    pub fn kube_token_for(&self, user: &str) -> Option<String> {
        self.api.users.iter().find(|u| u.name == user).and_then(|u| u.kube_token.clone())
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
        assert_eq!(c.logs_db_path(), "/var/lib/stormconsole/logs.redb");
        assert!(c.logs.enabled && c.kubernetes.enabled);
        assert!(!c.auth_required());
    }

    #[test]
    fn example_config_is_accepted() {
        let c = Config::parse(include_str!("../../../config/config.toml")).unwrap();
        assert_eq!(c.bind(), "0.0.0.0:9094");
        assert_eq!(c.logs_db_path(), "/var/lib/stormconsole/logs.redb");
    }

    #[test]
    fn defaults_without_a_file() {
        let c = Config::default();
        assert_eq!(c.bind(), "0.0.0.0:9094");
        assert_eq!(c.logs_db_path(), "/var/lib/stormconsole/logs.redb");
        assert_eq!(c.data_dir(), DEFAULT_DATA_DIR);
    }

    #[test]
    fn node_local_defaults_light_every_plugin() {
        let c = Config::parse(STORMPUMP_GOLDEN).unwrap();
        assert_eq!(c.kubernetes_server(), "https://127.0.0.1:6443");
        assert!(c.kubernetes_insecure());
        assert_eq!(c.stormblock_url(), "http://127.0.0.1:9090");
        assert_eq!(c.sbregistry_url(), "http://127.0.0.1:5100");
        assert_eq!(c.fastetcd_url(), "http://127.0.0.1:2379");
        assert_eq!(c.fastetcd_metrics_url(), "http://127.0.0.1:2381");
        assert_eq!(c.vmimages_url(), "http://127.0.0.1:9099");
        assert_eq!(c.stormdrive_url(), "http://127.0.0.1:9092");
        assert_eq!(c.stormstorage_url(), "http://127.0.0.1:9093");
        assert_eq!(c.stormvm_url(), "http://127.0.0.1:9095");
        assert_eq!(c.stormcluster_url(), "http://127.0.0.1:9102");
        assert!(c.stormcluster.enabled && c.stormcluster.token_file.is_none());
        assert!(c.fleet.stormd_ports.contains(&9085) && c.fleet.stormd_ports.contains(&9194));
        for p in [9201, 9202, 8180, 8545] {
            assert!(c.fleet.stormd_ports.contains(&p), "{p} missing from the default stormd ports");
        }
    }

    /// stormcert's token file and the node CA (#33).
    #[test]
    fn a_token_file_and_a_ca_file_are_used() {
        use console_core::apiserver::Trust;
        let c = Config::parse(
            "[kubernetes]\ntoken_file = \"/etc/stormcert/stormconsole.token\"\nca_file = \"/etc/stormcert/ca.crt\"\n",
        )
        .unwrap();
        // The loopback default with a CA is checked.
        assert!(!c.kubernetes_insecure());
        let conn = c.kubernetes_conn().unwrap();
        assert_eq!(conn.trust(), &Trust::Ca("/etc/stormcert/ca.crt".into()));
        assert!(conn.error().unwrap().contains("/etc/stormcert/"), "the missing files are named");
        // Without one, the loopback default is the one unchecked case.
        assert_eq!(Config::default().kubernetes_conn().unwrap().trust(), &Trust::Unverified);
        let off = Config::parse("[kubernetes]\nenabled = false\n").unwrap();
        assert!(off.kubernetes_conn().is_none());
    }

    #[test]
    fn contradictory_kubernetes_credentials_are_refused() {
        let e = Config::parse("[kubernetes]\ntoken = \"t\"\ntoken_file = \"f\"\n").unwrap_err();
        assert!(e.contains("token and token_file"), "{e}");
        let e = Config::parse("[kubernetes]\nca_file = \"ca\"\ninsecure_skip_tls_verify = true\n").unwrap_err();
        assert!(e.contains("ca_file and insecure_skip_tls_verify"), "{e}");
        let e = Config::parse("[kubernetes]\nserver = \"http://k:8080\"\nca_file = \"ca\"\n").unwrap_err();
        assert!(e.contains("only used over https://"), "{e}");
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
    fn a_user_may_carry_a_kube_identity_and_need_not() {
        let c = Config::parse(
            "[[api.users]]\nname = \"gw\"\npassword = \"p\"\nkube_token = \"jwt\"\n\n[[api.users]]\nname = \"ro\"\npassword = \"q\"\n",
        )
        .unwrap();
        assert!(c.auth_required());
        assert_eq!(c.kube_token_for("gw").as_deref(), Some("jwt"));
        assert_eq!(c.kube_token_for("ro"), None);
        assert_eq!(c.kube_token_for("nobody"), None);
    }

    #[test]
    fn bad_listen_address_is_a_config_error() {
        let e = Config::parse("listen_addr = \"9094\"\n").unwrap_err();
        assert!(e.contains("listen address"), "{e}");
    }

    /// #47: the node's mutual-TLS shape is accepted and reaches the plugin;
    /// half a pair, or certificates with a plaintext url, are config errors.
    #[test]
    /// stormcluster's :9102 is TLS only (#89, stormcluster#5).
    #[test]
    fn stormcluster_tls_files() {
        let c = Config::parse(
            "[stormcluster]\nca_file = \"/data/stormcert/ca.crt\"\n\
             cert_file = \"/data/stormcert/stormconsole-client.crt\"\nkey_file = \"/data/stormcert/stormconsole-client.key\"\n",
        )
        .unwrap();
        // A CA makes the default https.
        assert_eq!(c.stormcluster_url(), "https://127.0.0.1:9102");
        let t = c.stormcluster_tls();
        assert_eq!(t.ca.as_deref(), Some(std::path::Path::new("/data/stormcert/ca.crt")));
        assert!(t.cert.is_some() && t.key.is_some());
        // None: the plain default, for a stormcluster from before #5.
        let plain = Config::parse("").unwrap();
        assert_eq!(plain.stormcluster_url(), "http://127.0.0.1:9102");
        assert!(plain.stormcluster_tls().is_empty());
        let e = Config::parse("[stormcluster]\ncert_file = \"c\"\n").unwrap_err();
        assert!(e.contains("[stormcluster] cert_file and key_file go together"), "{e}");
        let e = Config::parse("[stormcluster]\nurl = \"http://b1:9102\"\nca_file = \"ca\"\n").unwrap_err();
        assert!(e.contains("only used over https://"), "{e}");
    }

    #[test]
    fn fastetcd_tls_files() {
        let c = Config::parse(
            "[fastetcd]\nurl = \"https://127.0.0.1:2379\"\nca_file = \"/data/stormcert/ca.crt\"\n\
             cert_file = \"/data/stormcert/stormconsole-etcd.crt\"\nkey_file = \"/data/stormcert/stormconsole-etcd.key\"\n",
        )
        .unwrap();
        let t = c.fastetcd_tls();
        assert_eq!(t.ca.as_deref(), Some(std::path::Path::new("/data/stormcert/ca.crt")));
        assert!(t.cert.is_some() && t.key.is_some());
        assert!(Config::parse("").unwrap().fastetcd_tls().is_empty());

        let e = Config::parse("[fastetcd]\nurl = \"https://e:2379\"\ncert_file = \"c\"\n").unwrap_err();
        assert!(e.contains("go together"), "{e}");
        let e = Config::parse("[fastetcd]\nca_file = \"ca\"\n").unwrap_err();
        assert!(e.contains("https://"), "{e}");
    }
}
