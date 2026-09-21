//! The VM images plugin: what cloud images the fleet *could* have, which of
//! them it has goldened, and which nodes carry a local copy.
//!
//! Three tiers, and the console's job is to make the first one a list you
//! pick from rather than a URL you paste:
//!
//! | tier | component | where it lives |
//! |---|---|---|
//! | **catalogue** | `img:catalog:fedora:43` | the distribution's mirror — nothing here yet |
//! | **fleet golden** | `img:golden:fedora-43` | sbregistry, content-addressed `media-<sha12>` |
//! | **local copy** | `img:local:fedora-43-node1` | one node's stormblock, as `fedora-43-x86_64` |
//!
//! All of it comes from vmcloud-image-operator's REST API, which is a door
//! onto the cluster's own `CloudImage` and `CloudImagePlacement` objects —
//! so a golden made from this console is the same object `kubectl` shows,
//! and the console keeps no state of its own about images.
//!
//! **A catalogue row is a button.** `POST /api/v1/catalog/{reference}`
//! takes no body precisely so that a stormview action — a method and a
//! path, and nothing else — can be wired to it.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use console_core::value::{field, human_bytes, u64_field};
use console_core::{
    Action, ComponentSummary, ConsolePlugin, Creator, Field, Health, Metric, NavSection, Relation,
};
use serde_json::Value;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const PROXY: &str = "/api/plugins/img/proxy";

struct State {
    health: Health,
    detail: String,
    components: Vec<ComponentSummary>,
}

/// What the create forms offer, kept separately because `creators()` is
/// synchronous: the console asks for the forms on a plain HTTP request, and
/// a form whose choices are a year old is a form that offers an image the
/// catalogue no longer has.
#[derive(Default)]
struct Picks {
    references: Vec<String>,
    goldens: Vec<String>,
    nodes: Vec<String>,
}

struct Inner {
    base: String,
    client: reqwest::Client,
    state: RwLock<State>,
    picks: std::sync::RwLock<Picks>,
}

pub struct VmImagesPlugin {
    inner: Arc<Inner>,
}

impl VmImagesPlugin {
    pub fn new(url: &str) -> Self {
        Self {
            inner: Arc::new(Inner {
                base: url.trim_end_matches('/').to_string(),
                client: reqwest::Client::new(),
                state: RwLock::new(State {
                    health: Health::Unknown,
                    detail: "not yet polled".into(),
                    components: vec![],
                }),
                picks: std::sync::RwLock::new(Picks::default()),
            }),
        }
    }

    fn picks(&self) -> Picks {
        match self.inner.picks.read() {
            Ok(p) => Picks {
                references: p.references.clone(),
                goldens: p.goldens.clone(),
                nodes: p.nodes.clone(),
            },
            Err(_) => Picks::default(),
        }
    }
}

#[async_trait]
impl ConsolePlugin for VmImagesPlugin {
    fn name(&self) -> &'static str {
        "img"
    }

    /// Under "Images", beside sbregistry's — sections with the same label
    /// merge, and a person looking for an image should find one list of
    /// places images are.
    fn nav(&self) -> Vec<NavSection> {
        vec![NavSection::new("Images", 50)
            .item("VM catalogue", "#/grid?id=img:operator&rel=catalogue")
            .item("VM goldens", "#/grid?id=img:operator&rel=goldens")
            .item("Local copies", "#/grid?id=img:operator&rel=local")]
    }

    fn creators(&self) -> Vec<Creator> {
        let p = self.picks();
        let refs: Vec<&str> = p.references.iter().map(String::as_str).collect();
        let goldens: Vec<&str> = p.goldens.iter().map(String::as_str).collect();
        let nodes: Vec<&str> = p.nodes.iter().map(String::as_str).collect();

        let mut out = vec![];
        if !refs.is_empty() {
            let mut fields = vec![
                Field::select("reference", "VM image", &refs)
                    .hint("from the catalogue this cluster knows")
                    .required(),
                Field::select("arch", "Architecture", &["x86_64", "aarch64"]),
            ];
            // Only offer a node when one is known. A select with no options
            // is a field somebody cannot fill in and cannot skip.
            if !nodes.is_empty() {
                let mut with_none = vec![""];
                with_none.extend(nodes.iter().copied());
                fields.push(
                    Field::select("node", "Also put it on", &with_none)
                        .hint("optional: a local copy on this node as soon as the golden is made"),
                );
            }
            out.push(
                Creator::form("img:golden", "VM golden", &format!("{PROXY}/api/v1/images"), fields)
                    .describe(
                        "Golden a public cloud image for the fleet: sbregistry fetches it, \
                         the engine decodes and seals it, and every node clones it from there",
                    )
                    .at(&["#/grid?id=img:operator&rel=catalogue", "#/grid?id=img:operator&rel=goldens"]),
            );
        }
        if !goldens.is_empty() && !nodes.is_empty() {
            out.push(
                Creator::form(
                    "img:local",
                    "Local copy",
                    &format!("{PROXY}/api/v1/local"),
                    vec![
                        Field::select("image", "Golden", &goldens).required(),
                        Field::select("node", "Node", &nodes).required(),
                        Field::text("size", "Grow to")
                            .hint("optional, e.g. 40Gi — a cloud image is sized for its own contents"),
                    ],
                )
                .describe("Import the golden onto one node, under the name a VM's dataVolume asks for")
                .at(&["#/grid?id=img:operator&rel=goldens", "#/grid?id=img:operator&rel=local"]),
            );
        }
        out
    }

    fn routes(&self) -> axum::Router {
        axum::Router::new().nest(
            "/proxy",
            console_core::proxy::router(self.inner.client.clone(), self.inner.base.clone()),
        )
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        self.inner.state.read().await.components.clone()
    }

    async fn health(&self) -> Health {
        self.inner.state.read().await.health
    }

    async fn detail(&self) -> String {
        let s = self.inner.state.read().await;
        console_core::upstream::detail("vm images", &self.inner.base, &s.detail)
    }

    async fn run(&self, shutdown: CancellationToken) {
        loop {
            poll(&self.inner).await;
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                _ = shutdown.cancelled() => return,
            }
        }
    }
}

async fn get(inner: &Inner, path: &str) -> Result<Value, String> {
    let resp = inner
        .client
        .get(format!("{}{path}", inner.base))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| {
            use std::error::Error as _;
            e.source().map(|s| s.to_string()).unwrap_or_else(|| e.to_string())
        })?;
    resp.json().await.map_err(|e| e.to_string())
}

async fn items(inner: &Inner, path: &str) -> Vec<Value> {
    match get(inner, path).await {
        Ok(Value::Array(a)) => a,
        Ok(v) => v.get("items").and_then(Value::as_array).cloned().unwrap_or_default(),
        Err(_) => vec![],
    }
}

async fn poll(inner: &Inner) {
    let version = match get(inner, "/api/v1/version").await {
        Ok(v) => v,
        Err(e) => {
            let mut s = inner.state.write().await;
            s.health = Health::Error;
            s.detail = format!("unreachable: {e}");
            s.components = vec![operator(Health::Error, &s.detail, vec![], &[])];
            return;
        }
    };

    let catalogue = items(inner, "/api/v1/catalog").await;
    let images = items(inner, "/api/v1/images").await;
    let local = items(inner, "/api/v1/local").await;
    let nodes = items(inner, "/api/v1/nodes").await;

    // Which references already have a golden: a catalogue row that is
    // already the fleet's says so and points at it, rather than inviting a
    // second click that does nothing.
    // The phase travels with the name, because the catalogue row reports it.
    //
    // Without it the two views of one image contradicted each other: a
    // catalogue entry was hardcoded Ok while its golden was Warn or Error,
    // so an image that was still building — or had failed — read "ready" in
    // the list somebody browses to decide what to build.
    let goldened: Vec<(String, String, String)> = images
        .iter()
        .filter_map(|i| {
            let name = field(i, &["name"])?;
            let reference = i.get("spec").and_then(|s| field(s, &["reference"]))?;
            let phase = i
                .get("status")
                .and_then(|st| field(st, &["phase"]))
                .unwrap_or_else(|| "Pending".into());
            Some((reference, name, phase))
        })
        .collect();

    let catalog_cs: Vec<_> = catalogue
        .iter()
        .map(|v| catalog_row(v, &goldened))
        .collect();
    let golden_cs: Vec<_> = images.iter().map(|v| golden(v, &local)).collect();
    let local_cs: Vec<_> = local.iter().map(placement).collect();

    let (health, detail) = verdict(&version, &golden_cs, &local_cs);

    let groups = [
        ("catalogue", catalog_cs.iter().map(|c| c.id.clone()).collect::<Vec<_>>()),
        ("goldens", golden_cs.iter().map(|c| c.id.clone()).collect()),
        ("local", local_cs.iter().map(|c| c.id.clone()).collect()),
    ];
    let building = version.get("building").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
    let metrics = vec![
        Metric::new("catalogue", catalog_cs.len().to_string()),
        Metric::new("goldens", golden_cs.len().to_string()).tone("accent"),
        Metric::new("local", local_cs.len().to_string()),
        Metric::new("building", building.to_string()).tone(if building > 0 { "warn" } else { "muted" }),
    ];

    let mut out = vec![operator(health, &detail, metrics, &groups)];
    out.extend(catalog_cs);
    out.extend(golden_cs);
    out.extend(local_cs);

    {
        let mut p = inner.picks.write().unwrap_or_else(|e| e.into_inner());
        p.references = catalogue.iter().filter_map(|v| field(v, &["reference"])).collect();
        p.goldens = images.iter().filter_map(|v| field(v, &["name"])).collect();
        p.nodes = nodes.iter().filter_map(|v| field(v, &["name"])).collect();
    }

    let mut s = inner.state.write().await;
    s.health = health;
    s.detail = detail;
    s.components = out;
}

/// The operator's own line.
///
/// A golden that failed is the thing worth saying on the card: everything
/// else about this plugin is a list, and a list does not raise its hand.
fn verdict(version: &Value, goldens: &[ComponentSummary], local: &[ComponentSummary]) -> (Health, String) {
    let v = field(version, &["version"]).unwrap_or_default();
    let failed = goldens.iter().chain(local).filter(|c| c.health == Health::Error).count();
    let building = version.get("building").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
    let built = goldens.iter().filter(|c| c.health == Health::Ok).count();
    match (failed, building) {
        (0, 0) => (Health::Ok, format!("v{v} · {built} golden(s), {} local", local.len())),
        (0, n) => (Health::Ok, format!("v{v} · building {n}")),
        (f, _) => (
            Health::Warn,
            format!("v{v} · {f} failed · {built} golden(s), {} local", local.len()),
        ),
    }
}

fn operator(health: Health, detail: &str, metrics: Vec<Metric>, groups: &[(&str, Vec<String>)]) -> ComponentSummary {
    ComponentSummary {
        id: "img:operator".into(),
        kind: "operator".into(),
        label: "vm images".into(),
        health,
        detail: detail.to_string(),
        metrics,
        actions: vec![],
        relations: groups
            .iter()
            .filter(|(_, ids)| !ids.is_empty())
            .map(|(name, ids)| Relation::has_many(name, ids.clone()))
            .collect(),
        link: Some("#/grid?id=img:operator&rel=goldens".into()),
    }
}

/// One image the fleet could have.
///
/// `provisioning` is on the row rather than buried: an Ignition image
/// ignores a cloud-init seed completely — it boots, ignores everything it
/// was told, and has no login — so what a guest reads on first boot is part
/// of choosing it. `rolling` is the other one: the distribution publishes
/// only `latest` in that path, so the same reference is different bytes
/// next month.
fn catalog_row(v: &Value, goldened: &[(String, String, String)]) -> ComponentSummary {
    let reference = field(v, &["reference"]).unwrap_or_default();
    let arch = field(v, &["arch"]).unwrap_or_default();
    let provisioning = field(v, &["provisioning"]).unwrap_or_default();
    let rolling = v.get("rolling").and_then(Value::as_bool).unwrap_or(false);
    let source = field(v, &["source"]).unwrap_or_default();
    let note = field(v, &["note"]).unwrap_or_default();
    let existing = goldened
        .iter()
        .find(|(r, _, _)| *r == reference)
        .map(|(_, n, p)| (n.clone(), p.clone()));

    let mut metrics = vec![
        Metric::new("arch", arch),
        Metric::new("format", field(v, &["format"]).unwrap_or_default()),
        Metric::new("provisioning", provisioning.clone())
            .tone(if provisioning == "cloud-init" { "ok" } else { "accent" }),
    ];
    if rolling {
        metrics.push(Metric::new("rolling", "yes").tone("warn"));
    }
    if source == "overlay" {
        metrics.push(Metric::new("source", "overlay").tone("accent"));
    }

    let mut relations = vec![Relation::belongs_to("operator", "img:operator")];
    if let Some((name, _)) = &existing {
        relations.push(Relation::has_one("golden", format!("img:golden:{name}")));
    }

    ComponentSummary {
        id: format!("img:catalog:{reference}"),
        kind: "cloudimage".into(),
        label: reference.clone(),
        // The golden's own state when there is one, and Idle when there is
        // not. Hardcoded Ok said "ready" about an image nobody had built and
        // about one whose build had failed, which are the two things a
        // person reads this list to tell apart.
        health: match &existing {
            Some((_, phase)) => health_of(phase),
            None => Health::Idle,
        },
        detail: match &existing {
            Some((_, phase)) if phase == "Available" => "in the fleet".to_string(),
            Some((_, phase)) => phase.to_lowercase(),
            None => note,
        },
        metrics,
        // What it is, in as few words as carry meaning.
        //
        // This read "goldened as fedora-43-x86_64 · cleanest cloud-init story
        // and the newest kernel; ~13 months of support per release" — the
        // golden name is a column, and the distribution's own blurb is the
        // same for every version of it. What a person scanning this list
        // wants is whether the fleet has it, and if it is being built, how
        // far along.
        actions: catalog_actions(&reference, &existing),
        relations,
        link: None,
    }
}

/// What can be done to a catalogue entry, given what the fleet has of it.
///
/// **"Golden again" was offered on every row, including ones already built.**
/// Re-taking an image that is Available achieves nothing unless it is a
/// rolling tag, and it costs a download and a seal. So the build is offered
/// when there is nothing, offered again only when the last attempt failed,
/// and replaced by Delete once it is there — which frees the entry to be
/// built again, and is the honest way to say "start over".
fn catalog_actions(reference: &str, existing: &Option<(String, String)>) -> Vec<Action> {
    let build = |label: &str, tone: Option<&str>| Action {
        id: "golden".into(),
        label: label.into(),
        method: "POST".into(),
        path: format!("{PROXY}/api/v1/catalog/{reference}"),
        enabled: true,
        danger: false,
        tone: tone.map(str::to_string),
    };
    match existing {
        None => vec![build("Make golden", None)],
        Some((name, phase)) if phase == "Available" => vec![Action {
            // Deleting the CloudImage is what re-enables the build. The
            // golden's bytes are not touched: a VM cloned from it keeps
            // working, which is why this is not as destructive as it reads.
            id: "delete".into(),
            label: "Delete golden".into(),
            method: "DELETE".into(),
            path: format!("{PROXY}/api/v1/images/{name}"),
            enabled: true,
            danger: true,
            tone: None,
        }],
        Some((_, phase)) if phase == "Failed" => vec![build("Retry", Some("warn"))],
        // Building or Resolving: it is already happening, and a second POST
        // while the first is downloading is how two imports race.
        Some(_) => vec![Action {
            id: "golden".into(),
            label: "Building…".into(),
            method: "POST".into(),
            path: format!("{PROXY}/api/v1/catalog/{reference}"),
            enabled: false,
            danger: false,
            tone: Some("warn".into()),
        }],
    }
}

/// One golden the fleet has.        relations,
        link: None,
    }
}

/// One golden the fleet has.
fn golden(v: &Value, local: &[Value]) -> ComponentSummary {
    let name = field(v, &["name"]).unwrap_or_default();
    let status = v.get("status").cloned().unwrap_or(Value::Null);
    let phase = field(&status, &["phase"]).unwrap_or_else(|| "Pending".into());
    let local_name = field(&status, &["localName"]).unwrap_or_default();
    let message = field(&status, &["message"]).unwrap_or_default();

    let mut metrics = vec![Metric::new("phase", phase.clone()).tone(tone(&phase))];
    if let Some(size) = u64_field(&status, "virtualSize") {
        metrics.push(Metric::new("size", human_bytes(size)));
    }
    if let Some(written) = u64_field(&status, "writtenBytes") {
        metrics.push(Metric::new("on disk", human_bytes(written)));
    }
    if let Some(src) = field(&status, &["digestSource"]) {
        metrics.push(Metric::new("digest", src));
    }
    if status.get("rolling").and_then(Value::as_bool).unwrap_or(false) {
        metrics.push(Metric::new("rolling", "yes").tone("warn"));
    }

    let mut relations = vec![Relation::belongs_to("operator", "img:operator")];
    let copies: Vec<String> = local
        .iter()
        .filter(|p| p.get("spec").and_then(|s| field(s, &["image"])).as_deref() == Some(&name))
        .filter_map(|p| field(p, &["name"]).map(|n| format!("img:local:{n}")))
        .collect();
    if !copies.is_empty() {
        relations.push(Relation::has_many("local", copies));
    }
    if let Some(reference) = v.get("spec").and_then(|s| field(s, &["reference"])) {
        relations.push(Relation::has_one("catalogue", format!("img:catalog:{reference}")));
    }

    ComponentSummary {
        id: format!("img:golden:{name}"),
        kind: "golden".into(),
        label: name,
        health: health_of(&phase),
        // The local name is the one a VM's dataVolume asks for, and the one
        // nobody can derive from the object's own name.
        detail: if message.is_empty() {
            format!(
                "{} · {}{}",
                field(&status, &["golden"]).unwrap_or_else(|| "not built yet".into()),
                if local_name.is_empty() { String::new() } else { format!("clones as {local_name}") },
                field(&status, &["repository"]).map(|r| format!(" · {r}")).unwrap_or_default()
            )
        } else {
            message
        },
        metrics,
        actions: vec![],
        relations,
        link: None,
    }
}

/// One node's copy.
fn placement(v: &Value) -> ComponentSummary {
    let name = field(v, &["name"]).unwrap_or_default();
    let spec = v.get("spec").cloned().unwrap_or(Value::Null);
    let status = v.get("status").cloned().unwrap_or(Value::Null);
    let image = field(&spec, &["image"]).unwrap_or_default();
    let node = field(&spec, &["node"]).unwrap_or_default();
    let phase = field(&status, &["phase"]).unwrap_or_else(|| "Pending".into());
    let message = field(&status, &["message"]).unwrap_or_default();
    let golden_name = field(&status, &["golden"]).unwrap_or_default();

    let mut metrics = vec![Metric::new("phase", phase.clone()).tone(tone(&phase))];
    if let Some(size) = u64_field(&status, "virtualSize") {
        metrics.push(Metric::new("size", human_bytes(size)));
    }
    if let Some(written) = u64_field(&status, "writtenBytes") {
        metrics.push(Metric::new("written", human_bytes(written)));
    }

    let mut relations = vec![
        Relation::belongs_to("golden", format!("img:golden:{image}")),
        Relation::has_one("node", format!("k8s:node:{node}")),
    ];
    // The volume is stormblock's, and the stormblock plugin already shows
    // it — so this edge crosses to it rather than describing it twice.
    if let Some(vol) = field(&status, &["volumeId"]) {
        relations.push(Relation::has_one("volume", format!("sb:volume:{vol}")));
    }

    ComponentSummary {
        id: format!("img:local:{name}"),
        kind: "localimage".into(),
        label: format!("{golden_name} on {node}"),
        health: health_of(&phase),
        detail: if message.is_empty() {
            format!("{image} on {node}")
        } else {
            message
        },
        metrics,
        // Stops managing the copy. The sealed golden on the node stays —
        // VMs are cloned from it, and a clone outlives the placement that
        // caused the golden to exist.
        actions: vec![Action {
            id: "unmanage".into(),
            label: "Stop managing".into(),
            method: "DELETE".into(),
            path: format!("{PROXY}/api/v1/local/{name}"),
            enabled: true,
            danger: true,
            tone: None,
        }],
        relations,
        link: None,
    }
}

fn health_of(phase: &str) -> Health {
    match phase {
        "Available" => Health::Ok,
        "Failed" => Health::Error,
        "" => Health::Unknown,
        _ => Health::Warn,
    }
}

fn tone(phase: &str) -> &'static str {
    match phase {
        "Available" => "ok",
        "Failed" => "error",
        _ => "warn",
    }
}
