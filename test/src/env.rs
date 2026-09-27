//! What the runner hands the container (stormcentral `docs/test-standard.md`),
//! and the few knobs these suites add.

use std::time::{Duration, Instant};

/// Where the kubelet mounts the Job's ServiceAccount.
pub const SA_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";

pub struct Env {
    pub suite: String,
    pub run_id: String,
    pub namespace: String,
    /// `STORM_API`: the apiserver, where the suites make what the console
    /// should then show.
    pub api: String,
    /// `STORM_NODE`: the node under test.
    pub node: String,
    /// The console. `STORMCONSOLE_URL`, else `http://STORM_NODE:9094`.
    pub console: String,
    /// The console's `auth_token`, when it has one: `STORMCONSOLE_TOKEN`. A
    /// console with authentication on and no token here is reported as a
    /// skip, not a pass — nothing can be checked through it.
    pub console_token: Option<String>,
    /// The ServiceAccount's token, or `STORM_TOKEN` for a run outside a pod.
    pub token: Option<String>,
    pub ca: Option<Vec<u8>>,
    pub timeout: Duration,
    pub started: Instant,
    /// How long something made in the cluster may take to show in the
    /// console's feed. `STORMCONSOLE_TEST_SEEN` (seconds, default 30).
    pub seen_wait: Duration,
    /// Objects per long-suite wave, overriding the size read from the node.
    /// `STORMCONSOLE_TEST_WAVE`.
    pub wave: Option<usize>,
}

impl Env {
    pub fn read(suite_arg: Option<String>) -> Env {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let sa = |f: &str| {
            std::fs::read_to_string(format!("{SA_DIR}/{f}")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
        };
        let suite = suite_arg.or_else(|| var("STORM_SUITE")).unwrap_or_else(|| "short".into());
        let node = var("STORM_NODE").unwrap_or_default();
        let budget = match suite.as_str() {
            "medium" => 1800,
            "long" => 8 * 3600,
            _ => 120,
        };
        Env {
            run_id: var("STORM_RUN_ID").unwrap_or_default(),
            namespace: var("STORM_NAMESPACE").or_else(|| sa("namespace")).unwrap_or_default(),
            api: var("STORM_API").unwrap_or_default(),
            console: var("STORMCONSOLE_URL").unwrap_or_else(|| format!("http://{}", join(&node, 9094))),
            console_token: var("STORMCONSOLE_TOKEN"),
            token: var("STORM_TOKEN").or_else(|| sa("token")),
            ca: std::fs::read(format!("{SA_DIR}/ca.crt")).ok(),
            timeout: Duration::from_secs(var("STORM_TIMEOUT").and_then(|v| v.parse().ok()).unwrap_or(budget)),
            started: Instant::now(),
            seen_wait: Duration::from_secs(var("STORMCONSOLE_TEST_SEEN").and_then(|v| v.parse().ok()).unwrap_or(30)),
            wave: var("STORMCONSOLE_TEST_WAVE").and_then(|v| v.parse().ok()),
            node,
            suite,
        }
    }

    /// What the runner must set and did not.
    pub fn missing(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.api.is_empty() {
            out.push("STORM_API");
        }
        if self.namespace.is_empty() {
            out.push("STORM_NAMESPACE");
        }
        if self.run_id.is_empty() {
            out.push("STORM_RUN_ID");
        }
        if self.node.is_empty() && std::env::var("STORMCONSOLE_URL").map(|v| v.is_empty()).unwrap_or(true) {
            out.push("STORM_NODE");
        }
        out
    }

    pub fn remaining(&self) -> Duration {
        self.timeout.saturating_sub(self.started.elapsed())
    }

    /// A DNS-safe name for something this run makes: `sct-<run>-<what>`.
    pub fn name(&self, what: &str) -> String {
        let run: String = self
            .run_id
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let run = run.trim_matches('-');
        let s = format!("sct-{}-{what}", &run[..run.len().min(20)]);
        s.chars().take(63).collect::<String>().trim_end_matches('-').to_string()
    }
}

/// `host:port`, bracketing an IPv6 address.
pub fn join(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_dns_safe() {
        let mut e = Env::read(Some("short".into()));
        e.run_id = "RUN_42.x".into();
        assert_eq!(e.name("svc"), "sct-run-42-x-svc");
        assert!(e.name(&"w".repeat(80)).len() <= 63);
    }

    #[test]
    fn ipv6_nodes_are_bracketed() {
        assert_eq!(join("fd00::5", 9094), "[fd00::5]:9094");
        assert_eq!(join("10.0.0.5", 9094), "10.0.0.5:9094");
    }
}
