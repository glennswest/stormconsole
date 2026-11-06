//! What this build of a node carries (#285). The owner: "lets keep all builds the same. only
//! diff is on cillium builds vs flowsdn then console should only have the one that is built in."
//!
//! One image serves every build, so nothing here is a build-time switch: the console asks the
//! node. The release manifest says, and where it cannot be read an ordered list of
//! [`NetworkSource`]s does (this node's beacon, the agent's DaemonSet, the CRDs served, an agent
//! answering). A network's surfaces show only where the node is known to run it: nothing said
//! means neither, never both.
//!
//! [`Build::enabled`] is the one list of plugins. Only the network decides it today; another
//! feature switched per build later goes there, read from the node as this is.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;

/// Which pod network this node runs, and how that is known.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "name", content = "why", rename_all = "lowercase")]
pub enum Network {
    Flowsdn(String),
    Cilium(String),
    /// Nothing that could say has said.
    Unknown(String),
}

impl Network {
    /// What a release's name says: a flowsdn release is `<n>-flowsdn`, and
    /// the only other flavour is the plain `<n>`, which is cilium
    /// (stormcos docs/NETWORK-TYPE.md). A node's beacon carries this name.
    pub fn from_release(release: &str) -> Self {
        if release.ends_with("-flowsdn") {
            Network::Flowsdn(format!("release {release}"))
        } else {
            Network::Cilium(format!("release {release}"))
        }
    }

    /// Read from the release manifest stormcos ships at
    /// `/etc/stormcos/release/manifest.json`. An `edition` (or `network`)
    /// field wins; then the components it was built from; then the name of
    /// the release ([`Self::from_release`]).
    pub fn from_manifest(text: &str, path: &str) -> Self {
        let Ok(doc) = serde_json::from_str::<Value>(text) else {
            return Network::Unknown(format!("{path} is not JSON"));
        };
        for key in ["edition", "network"] {
            if let Some(e) = doc.get(key).and_then(Value::as_str) {
                let why = format!("{path} says {key} = {e}");
                return match e {
                    "flowsdn" => Network::Flowsdn(why),
                    "cilium" => Network::Cilium(why),
                    _ => Network::Unknown(why),
                };
            }
        }
        if let Some(c) = doc.get("components").and_then(Value::as_object) {
            if c.keys().any(|k| k.starts_with("flowsdn")) {
                return Network::Flowsdn(format!("{path} lists the flowsdn component"));
            }
            if c.keys().any(|k| k.contains("cilium")) {
                return Network::Cilium(format!("{path} lists cilium and no flowsdn"));
            }
        }
        if let Some(v) = doc.get("version").and_then(Value::as_str).filter(|v| !v.is_empty()) {
            return Self::from_release(v);
        }
        Network::Unknown(format!("{path} names no pod network"))
    }

    pub fn read_manifest(path: &str) -> Self {
        match std::fs::read_to_string(path) {
            Ok(t) => Self::from_manifest(&t, path),
            Err(e) => Network::Unknown(format!("{path}: {e}")),
        }
    }

    pub fn is_cilium(&self) -> bool {
        matches!(self, Network::Cilium(_))
    }

    pub fn is_flowsdn(&self) -> bool {
        matches!(self, Network::Flowsdn(_))
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Network::Unknown(_))
    }

    pub fn why(&self) -> &str {
        match self {
            Network::Flowsdn(w) | Network::Cilium(w) | Network::Unknown(w) => w,
        }
    }
}

/// One thing that can say which network this node runs, when the manifest does not.
#[async_trait]
pub trait NetworkSource: Send + Sync {
    /// Its word, or `None` when it cannot tell.
    async fn network(&self) -> Option<Network>;
}

/// What this node's build carries. Cheap to read and shared by everything that asks.
pub struct Build {
    manifest: String,
    sources: RwLock<Vec<Arc<dyn NetworkSource>>>,
    network: RwLock<Network>,
}

impl Build {
    /// Read the manifest now, so a node that has one is right from the first request.
    pub fn new(manifest: &str) -> Arc<Self> {
        Arc::new(Self {
            manifest: manifest.to_string(),
            sources: RwLock::new(Vec::new()),
            network: RwLock::new(Network::read_manifest(manifest)),
        })
    }

    /// A source, asked after those already added.
    pub fn add_source(&self, source: Arc<dyn NetworkSource>) {
        self.sources.write().unwrap_or_else(|e| e.into_inner()).push(source);
    }

    pub fn network(&self) -> Network {
        self.network.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The one list: whether this plugin is part of this node's build.
    pub fn enabled(&self, plugin: &str) -> bool {
        match plugin {
            "flowsdn" => self.network().is_flowsdn(),
            _ => true,
        }
    }

    /// Ask again: the manifest, then each source in order. The first that says wins.
    pub async fn refresh(&self) {
        let mut found = Network::read_manifest(&self.manifest);
        if found.is_unknown() {
            let sources = self.sources.read().unwrap_or_else(|e| e.into_inner()).clone();
            for s in sources {
                if let Some(n) = s.network().await.filter(|n| !n.is_unknown()) {
                    found = n;
                    break;
                }
            }
        }
        let mut kept = self.network.write().unwrap_or_else(|e| e.into_inner());
        if *kept != found {
            tracing::info!(from = kept.why(), to = found.why(), "this node's pod network");
            *kept = found;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Says(Option<Network>);

    #[async_trait]
    impl NetworkSource for Says {
        async fn network(&self) -> Option<Network> {
            self.0.clone()
        }
    }

    fn manifest(name: &str, text: &str) -> String {
        let dir = std::env::temp_dir().join(format!("sc-build-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("manifest.json");
        std::fs::write(&p, text).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn the_network_comes_from_the_release() {
        let p = "/etc/stormcos/release/manifest.json";
        assert!(Network::from_manifest(r#"{"edition":"flowsdn"}"#, p).is_flowsdn());
        assert!(Network::from_manifest(r#"{"network":"cilium"}"#, p).is_cilium());
        assert!(Network::from_manifest(r#"{"version":"12.13-flowsdn","components":{}}"#, p).is_flowsdn());
        assert!(Network::from_manifest(r#"{"version":"12.13","components":{"flowsdn":{"commit":"b02c59f"}}}"#, p).is_flowsdn());
        assert!(Network::from_manifest(r#"{"version":"12.13","components":{"stormcos-cilium":{},"stormd":{}}}"#, p).is_cilium());
        assert!(Network::from_manifest(r#"{"version":"12.13","components":{"stormd":{}}}"#, p).is_cilium());
        assert!(Network::from_manifest(r#"{"components":{"stormd":{}}}"#, p).is_unknown());
        assert!(Network::from_release("12.61").is_cilium());
        assert!(Network::from_release("12.61-flowsdn").is_flowsdn());
        assert!(Network::from_manifest("not json", p).is_unknown());
    }

    #[tokio::test]
    async fn the_manifest_wins_over_every_source() {
        let b = Build::new(&manifest("wins", r#"{"version":"12.61-flowsdn"}"#));
        assert!(b.network().is_flowsdn(), "right before anything has been asked");
        b.add_source(Arc::new(Says(Some(Network::Cilium("an agent answered".into())))));
        b.refresh().await;
        assert!(b.network().is_flowsdn());
    }

    #[tokio::test]
    async fn with_no_manifest_the_first_source_that_says_wins() {
        let b = Build::new("/nonexistent/manifest.json");
        assert!(b.network().is_unknown());
        b.add_source(Arc::new(Says(None)));
        b.add_source(Arc::new(Says(Some(Network::Flowsdn("first".into())))));
        b.add_source(Arc::new(Says(Some(Network::Cilium("second".into())))));
        b.refresh().await;
        assert_eq!(b.network(), Network::Flowsdn("first".into()));
    }

    #[tokio::test]
    async fn nothing_saying_is_unknown_and_names_the_manifest() {
        let b = Build::new("/nonexistent/manifest.json");
        b.refresh().await;
        assert!(b.network().is_unknown(), "nothing says");
        assert!(b.network().why().contains("/nonexistent/manifest.json"), "{}", b.network().why());
    }

    #[test]
    fn flowsdn_is_on_only_where_the_node_runs_it_and_every_other_plugin_is_on() {
        let f = Build::new(&manifest("f", r#"{"version":"12.61-flowsdn"}"#));
        let c = Build::new(&manifest("c", r#"{"version":"12.61"}"#));
        let u = Build::new("/nonexistent/manifest.json");
        assert!(f.enabled("flowsdn") && !c.enabled("flowsdn") && !u.enabled("flowsdn"));
        for b in [&f, &c, &u] {
            assert!(b.enabled("k8s") && b.enabled("logs"));
        }
    }
}
