//! What the agent says, as rows a person can read.
//!
//! Pure functions over the agent's JSON (flowsdn `docs/agent-api.md`, read
//! with `crates/flowsdn-agent/src/{api,health_api}.rs` at 8fa0cc8), so every
//! shape is tested without an agent. Fields the agent may add later are
//! ignored, as its stability section asks; fields it leaves out read as
//! absent, never as a guess.

use console_core::Health;
use serde::Serialize;
use serde_json::Value;

fn s<'a>(v: &'a Value, path: &str) -> Option<&'a str> {
    v.pointer(path).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Which pod network this node runs, from the release it booted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "name", content = "why", rename_all = "lowercase")]
pub enum Edition {
    Flowsdn(String),
    Cilium(String),
    /// The manifest could not be read or did not say.
    Unknown(String),
}

impl Edition {
    /// Read from the release manifest stormcos ships at
    /// `/etc/stormcos/release/manifest.json`. An `edition` (or `network`)
    /// field wins; then the release version, which a flowsdn release
    /// carries as `<n>-flowsdn`; then the components it was built from.
    pub fn from_manifest(text: &str, path: &str) -> Self {
        let Ok(doc) = serde_json::from_str::<Value>(text) else {
            return Edition::Unknown(format!("{path} is not JSON"));
        };
        for key in ["edition", "network"] {
            if let Some(e) = doc.get(key).and_then(Value::as_str) {
                let why = format!("{path} says {key} = {e}");
                return match e {
                    "flowsdn" => Edition::Flowsdn(why),
                    "cilium" => Edition::Cilium(why),
                    _ => Edition::Unknown(why),
                };
            }
        }
        if let Some(v) = doc.get("version").and_then(Value::as_str) {
            if v.ends_with("-flowsdn") {
                return Edition::Flowsdn(format!("release {v}"));
            }
        }
        if let Some(c) = doc.get("components").and_then(Value::as_object) {
            if c.keys().any(|k| k.starts_with("flowsdn")) {
                return Edition::Flowsdn(format!("{path} lists the flowsdn component"));
            }
            if c.keys().any(|k| k.contains("cilium")) {
                return Edition::Cilium(format!("{path} lists cilium and no flowsdn"));
            }
        }
        Edition::Unknown(format!("{path} names no pod network"))
    }

    pub fn is_cilium(&self) -> bool {
        matches!(self, Edition::Cilium(_))
    }
}

/// One endpoint: a pod's attachment to the flowsdn network on this node.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Endpoint {
    pub id: u64,
    pub namespace: String,
    pub pod: String,
    pub node: String,
    pub state: String,
    pub ready: bool,
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
    /// The numeric identity, once flowsdn allocates one (#291). Shown, not
    /// led with: on its own it means nothing.
    pub identity: Option<u64>,
    pub interface: String,
    pub container_interface: String,
    pub mac: String,
    pub gateways: Vec<String>,
    pub attachment: String,
    pub sandbox: String,
    pub workloads: Vec<String>,
    pub containers: Vec<String>,
    pub labels: Vec<String>,
}

impl Endpoint {
    pub fn from_agent(v: &Value) -> Option<Self> {
        let id = v.get("id")?.as_u64()?;
        let st = v.get("status").cloned().unwrap_or(Value::Null);
        let net = st.get("pod-networks").and_then(|n| n.get("default").or_else(|| n.as_object()?.values().next()));
        let net = net.cloned().unwrap_or(Value::Null);
        let pick = |a: &str, b: &str| s(&st, a).or_else(|| s(&st, b)).unwrap_or("").to_string();
        let mut ipv4 = Vec::new();
        let mut ipv6 = Vec::new();
        for a in st.pointer("/networking/addressing").and_then(Value::as_array).into_iter().flatten() {
            if let Some(x) = s(a, "/ipv4") {
                ipv4.push(x.to_string());
            }
            if let Some(x) = s(a, "/ipv6") {
                ipv6.push(x.to_string());
            }
        }
        // An endpoint restored from before the networking block was
        // written still has its pod-networks addresses.
        if ipv4.is_empty() && ipv6.is_empty() {
            for a in net.get("ip_addresses").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                let bare = a.split('/').next().unwrap_or(a).to_string();
                if bare.contains(':') { ipv6.push(bare) } else { ipv4.push(bare) }
            }
        }
        let identity = match st.get("identity").or_else(|| st.pointer("/pod/identity")) {
            Some(Value::Number(n)) => n.as_u64(),
            Some(o @ Value::Object(_)) => o.get("id").and_then(Value::as_u64),
            _ => None,
        };
        let list = |path: &str, f: &dyn Fn(&Value) -> Option<String>| -> Vec<String> {
            st.pointer(path).and_then(Value::as_array).into_iter().flatten().filter_map(f).collect()
        };
        let state = s(&st, "/state").unwrap_or("unknown").to_string();
        Some(Endpoint {
            id,
            namespace: pick("/pod/namespace", "/external-identifiers/k8s-namespace"),
            pod: pick("/pod/pod_name", "/external-identifiers/k8s-pod-name"),
            node: s(&st, "/pod/node_name").or_else(|| s(&net, "/node")).unwrap_or("").to_string(),
            ready: state == "ready",
            state,
            ipv4,
            ipv6,
            identity,
            interface: s(&st, "/networking/interface-name").or_else(|| s(&net, "/host_interface")).unwrap_or("").into(),
            container_interface: s(&st, "/networking/container-interface-name")
                .or_else(|| s(&net, "/interface"))
                .unwrap_or("")
                .into(),
            mac: s(&net, "/mac_address").unwrap_or("").into(),
            gateways: net
                .get("gateway_ips")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|g| g.as_str().map(String::from))
                .collect(),
            attachment: s(&st, "/external-identifiers/cni-attachment-id").unwrap_or("").into(),
            sandbox: pick("/pod/container_id", "/external-identifiers/container-id"),
            workloads: list("/pod/workloads", &|w| {
                Some(format!("{} {}", s(w, "/kind").unwrap_or("owner"), s(w, "/name")?))
            }),
            containers: list("/pod/containers", &|c| {
                let n = s(c, "/name")?;
                Some(if c.get("init").and_then(Value::as_bool) == Some(true) { format!("{n} (init)") } else { n.into() })
            }),
            labels: list("/pod/labels", &|l| l.as_str().map(String::from)),
        })
    }

    /// `namespace/pod`, or what the CNI named it when it supplied no pod.
    pub fn name(&self) -> String {
        match (self.namespace.as_str(), self.pod.as_str()) {
            ("", "") if !self.attachment.is_empty() => self.attachment.clone(),
            ("", "") => format!("endpoint {}", self.id),
            (ns, pod) => format!("{ns}/{pod}"),
        }
    }
}

/// One IPAM pool and family. The agent sends every count as a decimal
/// string so an IPv6 prefix stays exact; they are u128 here for the same
/// reason.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Pool {
    pub pool: String,
    pub family: String,
    pub cidr: String,
    pub capacity: String,
    pub allocated: String,
    pub excluded: String,
    pub allocated_excluded: String,
    pub available: String,
    /// Percent of capacity still free, when the counts parse.
    pub free_pct: Option<f64>,
}

impl Pool {
    pub fn from_agent(v: &Value) -> Self {
        let c = |k: &str| v.get(k).map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).unwrap_or_default();
        let (capacity, available) = (c("capacity"), c("available"));
        let free_pct = match (capacity.parse::<u128>(), available.parse::<u128>()) {
            (Ok(cap), Ok(av)) if cap > 0 => Some(av as f64 * 100.0 / cap as f64),
            _ => None,
        };
        Pool {
            pool: c("pool"),
            family: c("family"),
            cidr: c("cidr"),
            capacity,
            allocated: c("allocated"),
            excluded: c("excluded"),
            allocated_excluded: c("allocated-excluded"),
            available,
            free_pct,
        }
    }

    pub fn health(&self) -> Health {
        match (self.available.parse::<u128>(), self.free_pct) {
            (Ok(0), _) => Health::Error,
            (_, Some(p)) if p < 10.0 => Health::Warn,
            (Ok(_), _) => Health::Ok,
            _ => Health::Unknown,
        }
    }
}

/// One row of `/v1/health/modules`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Module {
    pub id: String,
    pub level: String,
    pub message: String,
    pub error: String,
    pub updated: String,
    pub last_ok: String,
    pub count: u64,
    /// What it means for the node: a degraded module whose error is "not
    /// implemented" is flowsdn saying a controller does not exist yet, not
    /// that something broke — idle, not a warning on every node for ever.
    pub health: Health,
}

/// The zero time the agent writes for "never".
fn when(v: &Value, k: &str) -> String {
    s(v, &format!("/{k}")).filter(|t| !t.starts_with("0001-")).unwrap_or("").to_string()
}

impl Module {
    pub fn from_agent(v: &Value) -> Self {
        let parts: Vec<&str> = ["/ID/Module", "/ID/Component"]
            .iter()
            .flat_map(|p| v.pointer(p).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str))
            .collect();
        let level = s(v, "/Level").unwrap_or("").to_string();
        let error = s(v, "/Error").unwrap_or("").to_string();
        let health = match level.as_str() {
            "OK" => Health::Ok,
            "Degraded" if error == "not implemented" => Health::Idle,
            "Degraded" => Health::Warn,
            "Stopped" => Health::Error,
            _ => Health::Unknown,
        };
        Module {
            id: parts.join("."),
            message: s(v, "/Message").unwrap_or("").into(),
            updated: when(v, "Updated"),
            last_ok: when(v, "LastOK"),
            count: v.get("Count").and_then(Value::as_u64).unwrap_or(0),
            level,
            error,
            health,
        }
    }
}

/// One frontend of `/v1/service` (Kubernetes mode with `service-lb`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Service {
    pub id: u64,
    pub namespace: String,
    pub name: String,
    pub kind: String,
    pub frontend: String,
    pub scope: String,
    pub backends: Vec<String>,
    pub realized: bool,
}

fn addr(a: &Value) -> String {
    let ip = s(a, "/ip").unwrap_or("?");
    let ip = if ip.contains(':') { format!("[{ip}]") } else { ip.to_string() };
    let port = a.get("port").and_then(Value::as_u64).unwrap_or(0);
    format!("{ip}:{port}/{}", s(a, "/protocol").unwrap_or("TCP"))
}

impl Service {
    pub fn from_agent(v: &Value) -> Self {
        let spec = v.get("spec").cloned().unwrap_or(Value::Null);
        let fe = spec.get("frontend-address").cloned().unwrap_or(Value::Null);
        Service {
            id: spec.get("id").and_then(Value::as_u64).unwrap_or(0),
            namespace: s(&spec, "/flags/namespace").unwrap_or("").into(),
            name: s(&spec, "/flags/name").unwrap_or("").into(),
            kind: s(&spec, "/flags/type").or_else(|| s(&spec, "/flags/service-type")).unwrap_or("").into(),
            frontend: addr(&fe),
            scope: s(&fe, "/scope").unwrap_or("").into(),
            backends: spec
                .get("backend-addresses")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|b| match s(b, "/state") {
                    Some(st) if st != "active" => format!("{} ({st})", addr(b)),
                    _ => addr(b),
                })
                .collect(),
            realized: v.pointer("/status/realized").is_some_and(|r| !r.is_null()),
        }
    }
}

/// `/v1/healthz`, reduced to what the masthead and the agent row need.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Healthz {
    pub state: String,
    pub message: String,
    /// Present only in Kubernetes mode.
    pub kubernetes: Option<KubeHealth>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct KubeHealth {
    pub state: String,
    pub message: String,
    pub node: String,
    pub direct_routes: Option<bool>,
    pub service_lb: Option<bool>,
}

impl Healthz {
    pub fn from_agent(v: &Value) -> Self {
        Healthz {
            state: s(v, "/agent/state").unwrap_or("").into(),
            message: s(v, "/agent/msg").unwrap_or("").into(),
            kubernetes: v.get("kubernetes").filter(|k| k.is_object()).map(|k| KubeHealth {
                state: s(k, "/state").unwrap_or("").into(),
                message: s(k, "/msg").unwrap_or("").into(),
                node: s(k, "/node-name").unwrap_or("").into(),
                direct_routes: k.get("auto-direct-node-routes").and_then(Value::as_bool),
                service_lb: k.get("service-lb").and_then(Value::as_bool),
            }),
        }
    }
}

/// The agent's own health, worst first: the API, then the Kubernetes
/// view, then its modules.
pub fn agent_health(h: &Healthz, modules: &[Module]) -> (Health, String) {
    if h.state != "Ok" {
        return (Health::Error, format!("agent {}: {}", if h.state.is_empty() { "unknown" } else { &h.state }, h.message));
    }
    if let Some(m) = modules.iter().find(|m| m.health == Health::Error) {
        return (Health::Error, format!("{} stopped: {}", m.id, if m.error.is_empty() { &m.message } else { &m.error }));
    }
    if let Some(k) = h.kubernetes.as_ref().filter(|k| k.state != "Ok") {
        return (Health::Warn, format!("kubernetes {}: {}", k.state, k.message));
    }
    if let Some(m) = modules.iter().find(|m| m.health == Health::Warn) {
        return (Health::Warn, format!("{} degraded: {}", m.id, if m.error.is_empty() { &m.message } else { &m.error }));
    }
    let msg = h.kubernetes.as_ref().map(|k| k.message.clone()).filter(|m| !m.is_empty()).unwrap_or_else(|| h.message.clone());
    (Health::Ok, msg)
}

#[cfg(test)]
#[path = "model_tests.rs"]
pub(crate) mod tests;
