//! A grid for every custom resource (#348), as OpenShift's *Custom Resource
//! Definitions → Instances*.
//!
//! The kinds are the CRDs the watch cache already holds, plus whatever the
//! aggregated groups (storm.io's volumes, slabs, …) say they serve by
//! discovery. A kind's columns are its CRD's `additionalPrinterColumns`,
//! resolved exactly as rustkube's `table.rs` resolves them for `oc get`
//! (a leading `.`, dotted fields, numeric indices; a path that does not
//! resolve is an empty cell), so the console and the CLI print the same
//! values. NAME comes first, NAMESPACE where it applies, AGE last. A kind
//! that declares no columns gets its first-level `status` scalars and says
//! that it has none (`gap`).
//!
//! Everything is read **as the viewer**: the list, the live watch, the
//! object, its events. The apiserver's RBAC decides, and a refusal is said
//! in its words. Projects this viewer may not see are dropped on top.

use std::collections::{BTreeMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use console_core::Viewer;
use serde_json::{json, Value};

use crate::authz::Need;
use crate::client::RkError;
use crate::{objevents, Inner};

/// A followed grid ends here and the browser asks again, so the viewer's
/// rights are looked at again.
const FOLLOW_FOR: Duration = Duration::from_secs(300);
/// How often a grid with no watch of its own (a viewer who may list only
/// some projects) is listed again.
const RELIST: Duration = Duration::from_secs(10);
/// How long discovery of the aggregated groups is kept.
const DISCOVERY_FOR: Duration = Duration::from_secs(60);
/// One count on the CRD list may take this long before it is "—".
const COUNT_WITHIN: Duration = Duration::from_secs(5);
/// A kind with no printer columns shows this many of its status fields.
const STATUS_FIELDS: usize = 6;

/// The aggregated groups' kinds by discovery, and when they were asked.
pub type Discovered = tokio::sync::Mutex<Option<(Instant, Vec<Kind>)>>;

/// One printer column.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    pub kind: String,
    pub format: String,
    pub description: String,
    pub priority: i64,
    pub path: String,
}

impl Column {
    fn of(c: &Value) -> Option<Self> {
        let s = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let path = c.get("jsonPath").or_else(|| c.get("JSONPath")).and_then(Value::as_str)?;
        Some(Self {
            name: c.get("name").and_then(Value::as_str)?.to_string(),
            kind: Some(s("type")).filter(|t| !t.is_empty()).unwrap_or_else(|| "string".into()),
            format: s("format"),
            description: s("description"),
            priority: c.get("priority").and_then(Value::as_i64).unwrap_or(0),
            path: path.to_string(),
        })
    }

    fn json(&self, key: &str) -> Value {
        json!({
            "key": key,
            "name": self.name,
            "type": self.kind,
            "format": self.format,
            "description": self.description,
            "priority": self.priority,
            "path": self.path,
            "printer": true,
        })
    }

    /// The CRD's own Age column: a date read off `creationTimestamp`.
    fn is_age(&self) -> bool {
        self.kind == "date" && self.path.trim_start_matches('.') == "metadata.creationTimestamp"
    }
}

/// One kind the grid can list.
#[derive(Clone, Debug, PartialEq)]
pub struct Kind {
    /// `plural.group`, the CRD's own name and the key every route uses.
    pub name: String,
    pub group: String,
    pub version: String,
    pub versions: Vec<String>,
    pub plural: String,
    pub kind: String,
    pub short: Vec<String>,
    pub namespaced: bool,
    /// `None` when the apiserver keeps no conditions on its CRDs.
    pub established: Option<bool>,
    pub columns: Vec<Column>,
    /// `crd`, or `aggregated` for a kind an APIService serves.
    pub source: &'static str,
}

impl Kind {
    /// A CustomResourceDefinition as a kind: its served version (the storage
    /// one when it is served, else the first served) and that version's
    /// printer columns.
    pub fn from_crd(crd: &Value) -> Option<Self> {
        let spec = crd.get("spec")?;
        let group = spec.get("group").and_then(Value::as_str)?.to_string();
        let names = spec.get("names")?;
        let plural = names.get("plural").and_then(Value::as_str)?.to_string();
        let kind = names.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
        let short = names
            .get("shortNames")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let versions: Vec<&Value> = spec.get("versions").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
        let served = |v: &&Value| v.get("served").and_then(Value::as_bool).unwrap_or(true);
        let chosen = versions
            .iter()
            .find(|v| served(v) && v.get("storage").and_then(Value::as_bool).unwrap_or(false))
            .or_else(|| versions.iter().find(|v| served(v)))
            .or_else(|| versions.first())
            .copied();
        let version = chosen
            .and_then(|v| v.get("name").and_then(Value::as_str))
            .or_else(|| spec.get("version").and_then(Value::as_str))?
            .to_string();
        let printer = chosen
            .and_then(|v| v.get("additionalPrinterColumns"))
            .or_else(|| spec.get("additionalPrinterColumns"))
            .and_then(Value::as_array);
        let established = crd.pointer("/status/conditions").and_then(Value::as_array).and_then(|cs| {
            cs.iter()
                .find(|c| c.get("type").and_then(Value::as_str) == Some("Established"))
                .map(|c| c.get("status").and_then(Value::as_str) == Some("True"))
        });
        Some(Self {
            name: crd.pointer("/metadata/name").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("{plural}.{group}")),
            versions: versions.iter().filter(|v| served(v)).filter_map(|v| v.get("name").and_then(Value::as_str)).map(str::to_string).collect(),
            group,
            version,
            plural,
            kind,
            short,
            namespaced: spec.get("scope").and_then(Value::as_str) == Some("Namespaced"),
            established,
            columns: printer.map(|a| a.iter().filter_map(Column::of).collect()).unwrap_or_default(),
            source: "crd",
        })
    }

    /// One resource of an aggregated group's discovery document.
    fn from_discovery(group: &str, version: &str, r: &Value) -> Option<Self> {
        let plural = r.get("name").and_then(Value::as_str).filter(|n| !n.contains('/'))?;
        let verbs: Vec<&str> = r.get("verbs").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        if !verbs.contains(&"list") {
            return None;
        }
        Some(Self {
            name: format!("{plural}.{group}"),
            group: group.into(),
            version: version.into(),
            versions: vec![version.into()],
            plural: plural.into(),
            kind: r.get("kind").and_then(Value::as_str).unwrap_or("").into(),
            short: r
                .get("shortNames")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
            namespaced: r.get("namespaced").and_then(Value::as_bool).unwrap_or(false),
            established: None,
            columns: vec![],
            source: "aggregated",
        })
    }

    fn base(&self) -> String {
        format!("/apis/{}/{}", self.group, self.version)
    }

    /// The collection: every project's, or one's.
    pub fn collection(&self, ns: Option<&str>) -> String {
        match ns.filter(|_| self.namespaced) {
            Some(ns) => format!("{}/namespaces/{ns}/{}", self.base(), self.plural),
            None => format!("{}/{}", self.base(), self.plural),
        }
    }

    pub fn object_path(&self, ns: &str, name: &str) -> String {
        format!("{}/{name}", self.collection(Some(ns).filter(|n| !n.is_empty())))
    }

    pub fn href(&self) -> String {
        format!("#/cr/{}", self.name)
    }

    pub fn object_href(&self, ns: &str, name: &str) -> String {
        let ns = if self.namespaced && !ns.is_empty() { ns } else { "_" };
        format!("#/cr/{}/{ns}/{name}", self.name)
    }

    pub fn summary(&self) -> Value {
        json!({
            "name": self.name,
            "group": self.group,
            "version": self.version,
            "versions": self.versions,
            "plural": self.plural,
            "kind": self.kind,
            "short": self.short,
            "scope": if self.namespaced { "Namespaced" } else { "Cluster" },
            "namespaced": self.namespaced,
            "established": self.established,
            "columns": self.columns.len(),
            "source": self.source,
            "href": self.href(),
        })
    }
}

/// Resolve one printer column's JSONPath, as rustkube's `table.rs` does:
/// a leading `.`, dotted field names, numeric indices. Anything else does
/// not resolve, and an empty cell is what upstream prints for it.
pub fn json_path<'a>(obj: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = obj;
    for seg in path.trim_start_matches('.').split('.') {
        if seg.is_empty() {
            continue;
        }
        let (field, rest) = match seg.find('[') {
            Some(i) => (&seg[..i], &seg[i..]),
            None => (seg, ""),
        };
        if !field.is_empty() {
            cur = cur.get(field)?;
        }
        for idx in rest.split('[').filter(|p| !p.is_empty()) {
            let n: usize = idx.trim_end_matches(']').parse().ok()?;
            cur = cur.get(n)?;
        }
    }
    Some(cur)
}

/// A kind with no printer columns: its first-level `status` scalars, the
/// ones that say most first, as many as fit.
pub fn status_columns(items: &[Value]) -> Vec<Column> {
    let mut seen: BTreeMap<String, &'static str> = BTreeMap::new();
    for it in items {
        if let Some(st) = it.get("status").and_then(Value::as_object) {
            for (k, v) in st {
                let ty = match v {
                    Value::String(_) => "string",
                    Value::Bool(_) => "boolean",
                    Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
                    Value::Number(_) => "number",
                    _ => continue,
                };
                seen.entry(k.clone()).or_insert(ty);
            }
        }
    }
    const FIRST: [&str; 6] = ["phase", "state", "health", "ready", "reason", "message"];
    let mut keys: Vec<(String, &'static str)> = seen.into_iter().collect();
    keys.sort_by_key(|(k, _)| (FIRST.iter().position(|f| f == k).unwrap_or(FIRST.len()), k == "message"));
    // `message` is long: it goes last of what is shown.
    let mut out: Vec<(String, &'static str)> = keys.into_iter().take(STATUS_FIELDS).collect();
    out.sort_by_key(|(k, _)| k == "message");
    out.into_iter()
        .map(|(k, ty)| Column {
            name: title_case(&k),
            kind: ty.into(),
            format: String::new(),
            description: format!(".status.{k}"),
            priority: 0,
            path: format!(".status.{k}"),
        })
        .collect()
}

fn title_case(k: &str) -> String {
    let mut c = k.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The grid's columns: NAME, NAMESPACE (namespaced kinds), the printer
/// columns, AGE unless the CRD prints its own.
pub fn grid_columns(kind: &Kind, printer: &[Column]) -> Vec<Value> {
    let mut cols = vec![json!({"key": "name", "name": "Name", "type": "string", "priority": 0})];
    if kind.namespaced {
        cols.push(json!({"key": "namespace", "name": "Namespace", "type": "string", "priority": 0}));
    }
    for (i, c) in printer.iter().enumerate() {
        cols.push(c.json(&format!("c{i}")));
    }
    if !printer.iter().any(Column::is_age) {
        cols.push(json!({"key": "age", "name": "Age", "type": "date", "priority": 0}));
    }
    cols
}

/// One object as a grid row: its key (`ns/name` or `name`), the fixed
/// fields, and one cell per printer column — strings bare, everything else
/// as JSON, missing as empty, as the apiserver's Table prints them.
pub fn row(kind: &Kind, printer: &[Column], o: &Value) -> Value {
    let g = |p: &str| o.pointer(p).and_then(Value::as_str).unwrap_or("").to_string();
    let (name, ns) = (g("/metadata/name"), g("/metadata/namespace"));
    let mut r = serde_json::Map::new();
    r.insert("id".into(), json!(if ns.is_empty() { name.clone() } else { format!("{ns}/{name}") }));
    r.insert("name".into(), json!(name));
    r.insert("namespace".into(), json!(ns));
    r.insert("age".into(), json!(g("/metadata/creationTimestamp")));
    r.insert("href".into(), json!(kind.object_href(&ns, &name)));
    for (i, c) in printer.iter().enumerate() {
        r.insert(format!("c{i}"), json_path(o, &c.path).cloned().unwrap_or(json!("")));
    }
    // What a curated renderer needs beyond the columns: the Ready/… conditions in brief.
    if let Some(cs) = o.pointer("/status/conditions").and_then(Value::as_array) {
        let brief: Vec<Value> = cs
            .iter()
            .map(|c| json!({"type": c.get("type"), "status": c.get("status"), "reason": c.get("reason")}))
            .collect();
        r.insert("conditions".into(), Value::Array(brief));
    }
    Value::Object(r)
}

/// Which columns this kind prints, given what is in it.
fn printer_of(kind: &Kind, items: &[Value]) -> (Vec<Column>, bool) {
    if kind.columns.is_empty() {
        (status_columns(items), true)
    } else {
        (kind.columns.clone(), false)
    }
}

/// The whole grid for a list of objects.
pub fn table(kind: &Kind, items: &[Value]) -> (Value, Vec<Column>) {
    let (printer, gap) = printer_of(kind, items);
    let mut rows: Vec<Value> = items.iter().map(|o| row(kind, &printer, o)).collect();
    rows.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    (
        json!({
            "kind": kind.summary(),
            "columns": grid_columns(kind, &printer),
            "rows": rows,
            "gap": gap,
        }),
        printer,
    )
}

/// Every kind the grid can list: the CRDs, then what aggregated groups
/// serve that no CRD does.
pub async fn kinds(inner: &Inner) -> Vec<Kind> {
    let mut all: Vec<Kind> = inner.store.kind("crd").await.values().filter_map(Kind::from_crd).collect();
    let have: HashSet<String> = all.iter().map(|k| k.name.clone()).collect();
    all.extend(aggregated(inner).await.into_iter().filter(|k| !have.contains(&k.name)));
    all.sort_by(|a, b| a.name.cmp(&b.name));
    all
}

/// A group that is somebody's own, not the platform's built-in API.
pub fn is_custom_group(g: &str) -> bool {
    g.contains('.') && !g.ends_with("k8s.io") && !g.ends_with("openshift.io")
}

/// The aggregated groups' kinds, by discovery as the console (what exists
/// is not a secret; what is in it is asked as the viewer). Kept a minute.
async fn aggregated(inner: &Inner) -> Vec<Kind> {
    let mut kept = inner.discovered.lock().await;
    if let Some((at, k)) = kept.as_ref() {
        if at.elapsed() < DISCOVERY_FOR {
            return k.clone();
        }
    }
    let Some(client) = &inner.client else { return vec![] };
    let Ok(groups) = client.get("/apis").await else {
        return kept.as_ref().map(|(_, k)| k.clone()).unwrap_or_default();
    };
    let gvs: Vec<(String, String)> = groups
        .get("groups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|g| {
            let name = g.get("name").and_then(Value::as_str)?;
            let v = g.pointer("/preferredVersion/version").and_then(Value::as_str)?;
            is_custom_group(name).then(|| (name.to_string(), v.to_string()))
        })
        .collect();
    let lists = futures_util::future::join_all(gvs.iter().map(|(g, v)| async move {
        let doc = tokio::time::timeout(COUNT_WITHIN, client.get(&format!("/apis/{g}/{v}"))).await.ok()?.ok()?;
        Some(
            doc.get("resources")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|r| Kind::from_discovery(g, v, r))
                .collect::<Vec<_>>(),
        )
    }))
    .await;
    let found: Vec<Kind> = lists.into_iter().flatten().flatten().collect();
    *kept = Some((Instant::now(), found.clone()));
    found
}

pub async fn resolve(inner: &Inner, name: &str) -> Option<Kind> {
    if let Some(k) = inner.store.object("crd", name).await.as_ref().and_then(Kind::from_crd) {
        return Some(k);
    }
    aggregated(inner).await.into_iter().find(|k| k.name == name)
}

/// A refusal, in the apiserver's words where it gave some.
struct Refusal(StatusCode, String);

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

fn refusal(kind: &Kind, e: &RkError, what: &str) -> Refusal {
    match e {
        RkError::Status(s) if s.as_u16() == 403 => {
            Refusal(StatusCode::FORBIDDEN, format!("the apiserver says you may not {what} {}", kind.name))
        }
        RkError::Status(s) if s.as_u16() == 404 => {
            Refusal(StatusCode::NOT_FOUND, format!("the apiserver does not serve {} {}", kind.name, kind.version))
        }
        other => Refusal(StatusCode::BAD_GATEWAY, other.to_string()),
    }
}

fn no_apiserver() -> Refusal {
    Refusal(StatusCode::SERVICE_UNAVAILABLE, "no apiserver".into())
}

fn unknown(name: &str) -> Refusal {
    Refusal(StatusCode::NOT_FOUND, format!("no custom resource {name}: not a CRD, and no aggregated group serves it"))
}

/// What one list gave: the objects this viewer may see, and — when it was
/// one collection read in one go — where to watch it from.
struct Listed {
    items: Vec<Value>,
    watch: Option<(String, String)>,
}

/// List a kind as the viewer. A namespaced kind they may not list across
/// every project is listed in each project they may see instead.
async fn list(inner: &Inner, viewer: &Viewer, kind: &Kind, project: Option<&str>) -> Result<Listed, Refusal> {
    let client = inner.client.as_ref().ok_or_else(no_apiserver)?;
    let token = viewer.token.as_deref();
    if let (true, Some(ns)) = (kind.namespaced, project) {
        if let Some(r) = crate::refuse_hidden(inner, viewer, ns).await {
            return Err(Refusal(r.status(), format!("no project {ns}")));
        }
    }
    let path = kind.collection(project);
    let hidden = inner.access.hidden(viewer).await.map(|(h, _)| h);
    let keep = |items: Vec<Value>| -> Vec<Value> {
        match &hidden {
            Some(h) => items
                .into_iter()
                .filter(|o| o.pointer("/metadata/namespace").and_then(Value::as_str).is_none_or(|ns| !h.contains(ns)))
                .collect(),
            None => items,
        }
    };
    match client.get_as(&path, token).await {
        Ok(list) => {
            let rv = list.pointer("/metadata/resourceVersion").and_then(Value::as_str).unwrap_or("").to_string();
            Ok(Listed { items: keep(items_of(list)), watch: Some((path, rv)) })
        }
        Err(RkError::Status(s)) if s.as_u16() == 403 && kind.namespaced && project.is_none() => {
            let projects = match inner.access.visible(viewer).await {
                Some(v) => v,
                None => inner.store.namespaces().await,
            };
            let lists = futures_util::future::join_all(
                projects.iter().map(|ns| async move { client.get_as(&kind.collection(Some(ns)), token).await }),
            )
            .await;
            let mut any = false;
            let mut items = vec![];
            for l in lists.into_iter().flatten() {
                any = true;
                items.extend(items_of(l));
            }
            if !any {
                return Err(refusal(kind, &RkError::Status(s), "list"));
            }
            Ok(Listed { items: keep(items), watch: None })
        }
        Err(e) => Err(refusal(kind, &e, "list")),
    }
}

fn items_of(list: Value) -> Vec<Value> {
    match list {
        Value::Object(mut m) => match m.remove("items") {
            Some(Value::Array(a)) => a,
            _ => vec![],
        },
        _ => vec![],
    }
}

#[derive(serde::Deserialize, Default)]
pub struct Ask {
    /// The project filter: one project's instances of a namespaced kind.
    #[serde(default)]
    pub project: Option<String>,
}

impl Ask {
    fn project(&self) -> Option<&str> {
        self.project.as_deref().map(str::trim).filter(|p| !p.is_empty())
    }
}

/// `GET /crds`: every kind, with how many of it this viewer may see.
pub async fn crds(State(inner): State<Arc<Inner>>, viewer: Viewer) -> Response {
    let all = kinds(&inner).await;
    let counts = futures_util::future::join_all(all.iter().map(|k| {
        let (inner, viewer) = (&inner, &viewer);
        async move {
            match tokio::time::timeout(COUNT_WITHIN, list(inner, viewer, k, None)).await {
                Ok(Ok(l)) => json!({"count": l.items.len()}),
                Ok(Err(Refusal(s, e))) => json!({"count": null, "forbidden": s == StatusCode::FORBIDDEN, "error": e}),
                Err(_) => json!({"count": null, "error": "no answer within 5 s"}),
            }
        }
    }))
    .await;
    let items: Vec<Value> = all
        .iter()
        .zip(counts)
        .map(|(k, c)| {
            let mut s = k.summary();
            if let (Some(s), Some(c)) = (s.as_object_mut(), c.as_object()) {
                s.extend(c.clone());
            }
            s
        })
        .collect();
    Json(json!({"items": items})).into_response()
}

/// `GET /cr/{crd}`: the instances grid.
pub async fn instances(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path(name): Path<String>,
    Query(q): Query<Ask>,
) -> Response {
    let Some(kind) = resolve(&inner, &name).await else { return unknown(&name).into_response() };
    match list(&inner, &viewer, &kind, q.project()).await {
        Ok(l) => {
            let (mut t, _) = table(&kind, &l.items);
            t["live"] = json!(l.watch.is_some());
            Json(t).into_response()
        }
        Err(r) => r.into_response(),
    }
}

/// `GET /cr/{crd}/watch`: the grid, live (SSE). A `snapshot` first, then
/// one `ADDED`/`MODIFIED`/`DELETED` per change from the viewer's own watch;
/// a broken watch lists again. Ends after five minutes; the browser asks
/// again, and the viewer's rights are asked again with it.
pub async fn watch(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path(name): Path<String>,
    Query(q): Query<Ask>,
) -> Response {
    let Some(kind) = resolve(&inner, &name).await else { return unknown(&name).into_response() };
    let (tx, rx) = tokio::sync::mpsc::channel::<Value>(64);
    let project = q.project().map(str::to_string);
    tokio::spawn(async move { follow(inner, viewer, kind, project, tx).await });
    let s = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|v| (Ok::<_, Infallible>(Event::default().data(v.to_string())), rx))
    });
    Sse::new(s).keep_alive(KeepAlive::default()).into_response()
}

async fn follow(inner: Arc<Inner>, viewer: Viewer, kind: Kind, project: Option<String>, tx: tokio::sync::mpsc::Sender<Value>) {
    let until = Instant::now() + FOLLOW_FOR;
    while Instant::now() < until && !tx.is_closed() {
        let listed = list(&inner, &viewer, &kind, project.as_deref()).await;
        let l = match listed {
            Ok(l) => l,
            Err(Refusal(s, e)) => {
                if tx.send(json!({"type": "error", "status": s.as_u16(), "error": e})).await.is_err() {
                    return;
                }
                tokio::time::sleep(RELIST).await;
                continue;
            }
        };
        let (mut t, printer) = table(&kind, &l.items);
        t["type"] = json!("snapshot");
        t["live"] = json!(l.watch.is_some());
        if tx.send(t).await.is_err() {
            return;
        }
        let Some((path, rv)) = l.watch else {
            tokio::time::sleep(RELIST).await;
            continue;
        };
        if !changes(&inner, &viewer, &kind, &printer, &path, &rv, until, &tx).await {
            return;
        }
    }
}

/// Send one viewer's watch down the stream until it ends. `false` when the
/// browser has gone.
#[allow(clippy::too_many_arguments)]
async fn changes(
    inner: &Inner,
    viewer: &Viewer,
    kind: &Kind,
    printer: &[Column],
    path: &str,
    rv: &str,
    until: Instant,
    tx: &tokio::sync::mpsc::Sender<Value>,
) -> bool {
    let Some(client) = inner.client.clone() else { return true };
    let hidden = inner.access.hidden(viewer).await.map(|(h, _)| h);
    let stop = tokio_util::sync::CancellationToken::new();
    let (etx, mut erx) = tokio::sync::mpsc::channel::<(String, Value)>(64);
    let (p, r, tok, s) = (path.to_string(), rv.to_string(), viewer.token.clone(), stop.clone());
    let w = tokio::spawn(async move { client.watch_as(&p, &r, tok.as_deref(), &s, etx).await });
    let deadline = tokio::time::sleep_until(tokio::time::Instant::from_std(until));
    tokio::pin!(deadline);
    let gone = loop {
        let ev = tokio::select! {
            ev = erx.recv() => ev,
            _ = &mut deadline => break false,
            _ = tx.closed() => break true,
        };
        let Some((ty, obj)) = ev else { break false };
        let ns = obj.pointer("/metadata/namespace").and_then(Value::as_str).unwrap_or("");
        if hidden.as_ref().is_some_and(|h| h.contains(ns)) {
            continue;
        }
        let msg = match ty.as_str() {
            "ADDED" | "MODIFIED" => json!({"type": ty, "row": row(kind, printer, &obj)}),
            "DELETED" => json!({"type": ty, "id": row(kind, printer, &obj)["id"]}),
            // An ERROR (a too-old resourceVersion) ends the watch: list again.
            "ERROR" => break false,
            _ => continue,
        };
        if tx.send(msg).await.is_err() {
            break true;
        }
    };
    stop.cancel();
    w.abort();
    !gone
}

/// The object a route names: `_` is no project (a cluster-scoped kind).
fn ns_of(ns: &str) -> &str {
    if ns == "_" {
        ""
    } else {
        ns
    }
}

/// An owner reference as a link to where the console shows that owner.
pub fn owner_link(o: &Value, ns: &str, crds: &[Kind]) -> Value {
    let g = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or("");
    let (api, kind, name) = (g("apiVersion"), g("kind"), g("name"));
    let group = api.split_once('/').map(|(g, _)| g).unwrap_or("");
    let href = match (group, kind) {
        ("", "Pod") => Some(format!("#/pod/{ns}/{name}")),
        ("", "Node") => Some(format!("#/node/{name}")),
        ("kubevirt.io", "VirtualMachine" | "VirtualMachineInstance") => Some(format!("#/vm/{ns}/{name}")),
        _ => crds.iter().find(|k| k.group == group && k.kind == kind).map(|k| k.object_href(ns, name)).or_else(|| {
            crate::cache::RESOURCES
                .iter()
                .find(|r| r.api_kind == kind && !r.list_path.contains("apiextensions"))
                .map(|r| {
                    let key = if r.namespaced { format!("{ns}/{name}") } else { name.to_string() };
                    format!("#/grid?id=k8s:{}:{key}", r.kind)
                })
        }),
    };
    json!({
        "kind": kind,
        "name": name,
        "apiVersion": api,
        "controller": o.get("controller").and_then(Value::as_bool).unwrap_or(false),
        "href": href,
    })
}

/// `status.conditions` as a table's rows.
pub fn conditions(o: &Value) -> Vec<Value> {
    o.pointer("/status/conditions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| {
            let s = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("");
            json!({
                "type": s("type"),
                "status": s("status"),
                "reason": s("reason"),
                "message": s("message"),
                "lastTransitionTime": s("lastTransitionTime"),
            })
        })
        .collect()
}

/// `GET /cr/{crd}/object/{ns|_}/{name}`: one object, all of it.
pub async fn object(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((name, ns, obj)): Path<(String, String, String)>,
) -> Response {
    let Some(kind) = resolve(&inner, &name).await else { return unknown(&name).into_response() };
    let Some(client) = &inner.client else { return no_apiserver().into_response() };
    let ns = ns_of(&ns);
    if !ns.is_empty() {
        if let Some(r) = crate::refuse_hidden(&inner, &viewer, ns).await {
            return r;
        }
    }
    let path = kind.object_path(ns, &obj);
    let token = viewer.token.as_deref();
    let o = match client.get_as(&path, token).await {
        Ok(o) => o,
        Err(RkError::Status(s)) if s.as_u16() == 404 => {
            return Refusal(StatusCode::NOT_FOUND, format!("no {} {obj}", kind.kind)).into_response()
        }
        Err(e) => return refusal(&kind, &e, "read").into_response(),
    };
    let (printer, _) = printer_of(&kind, std::slice::from_ref(&o));
    let printed: Vec<Value> = printer
        .iter()
        .map(|c| json!({"name": c.name, "type": c.kind, "path": c.path, "priority": c.priority, "description": c.description, "value": json_path(&o, &c.path)}))
        .collect();
    // Events: the project's (or the cluster's), as the viewer, about this object.
    let events_path = if ns.is_empty() { "/api/v1/events".to_string() } else { format!("/api/v1/namespaces/{ns}/events") };
    let events: Value = match client.get_as(&events_path, token).await {
        Ok(list) => Value::Array(
            items_of(list)
                .iter()
                .filter(|e| objevents::about(e, &kind.kind, ns, &obj))
                .map(crate::event_row)
                .collect(),
        ),
        Err(_) => Value::Null,
    };
    let all = kinds(&inner).await;
    let owners: Vec<Value> = o
        .pointer("/metadata/ownerReferences")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|r| owner_link(r, ns, &all))
        .collect();
    let may = |verb: &'static str| {
        let (inner, viewer, path) = (&inner, &viewer, &path);
        async move {
            viewer.may_write()
                && match Need::of_api_path(path, verb) {
                    Some(n) => inner.access.may(viewer, &n).await,
                    None => true,
                }
        }
    };
    Json(json!({
        "kind": kind.summary(),
        "name": obj,
        "namespace": ns,
        "object": o,
        "yaml": crate::to_yaml(&o),
        "columns": printed,
        "conditions": conditions(&o),
        "owners": owners,
        "events": events,
        "editable": may("update").await,
        "deletable": may("delete").await,
    }))
    .into_response()
}

fn apiserver_words(status: reqwest::StatusCode, body: &Value) -> Response {
    let msg = body
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("apiserver returned {}", status.as_u16()));
    let code = match status.as_u16() {
        403 => StatusCode::FORBIDDEN,
        404 => StatusCode::NOT_FOUND,
        409 => StatusCode::CONFLICT,
        400 | 422 => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::BAD_GATEWAY,
    };
    (code, Json(json!({"error": msg}))).into_response()
}

/// `PUT /cr/{crd}/object/{ns|_}/{name}`: save the edited YAML, as the viewer.
pub async fn save(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((name, ns, obj)): Path<(String, String, String)>,
    body: String,
) -> Response {
    let Some(kind) = resolve(&inner, &name).await else { return unknown(&name).into_response() };
    let Some(client) = &inner.client else { return no_apiserver().into_response() };
    let ns = ns_of(&ns);
    if !ns.is_empty() {
        if let Some(r) = crate::refuse_hidden(&inner, &viewer, ns).await {
            return r;
        }
    }
    let docs = match crate::apply::parse_documents(&body) {
        Ok(d) => d,
        Err(e) => return Refusal(StatusCode::BAD_REQUEST, e).into_response(),
    };
    let [doc] = &docs[..] else {
        return Refusal(StatusCode::BAD_REQUEST, "an edit is one document — use Import YAML for several".into()).into_response();
    };
    let named = doc.pointer("/metadata/name").and_then(Value::as_str).unwrap_or("");
    if named != obj {
        return Refusal(
            StatusCode::BAD_REQUEST,
            format!("this edits {obj}; the document names {named}. Rename with Import YAML and delete the old one."),
        )
        .into_response();
    }
    let who = match crate::own(&viewer) {
        Ok(w) => w,
        Err(r) => return r,
    };
    match client.put_json(&kind.object_path(ns, &obj), doc, who).await {
        Ok((s, _)) if s.is_success() => Json(json!({"message": format!("{} {obj} saved", kind.kind)})).into_response(),
        Ok((s, b)) => apiserver_words(s, &b),
        Err(e) => Refusal(StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

/// `DELETE /cr/{crd}/object/{ns|_}/{name}`, as the viewer.
pub async fn remove(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((name, ns, obj)): Path<(String, String, String)>,
) -> Response {
    let Some(kind) = resolve(&inner, &name).await else { return unknown(&name).into_response() };
    let Some(client) = &inner.client else { return no_apiserver().into_response() };
    let ns = ns_of(&ns);
    if !ns.is_empty() {
        if let Some(r) = crate::refuse_hidden(&inner, &viewer, ns).await {
            return r;
        }
    }
    let who = match crate::own(&viewer) {
        Ok(w) => w,
        Err(r) => return r,
    };
    match client.delete_json(&kind.object_path(ns, &obj), who).await {
        Ok((s, _)) if s.is_success() => Json(json!({"deleted": format!("{} {obj}", kind.kind)})).into_response(),
        Ok((s, b)) => apiserver_words(s, &b),
        Err(e) => Refusal(StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raidsets() -> Value {
        json!({
            "metadata": {"name": "raidsets.storage.storm.io"},
            "spec": {
                "group": "storage.storm.io",
                "names": {"plural": "raidsets", "kind": "RaidSet", "shortNames": ["rs"]},
                "scope": "Cluster",
                "versions": [
                    {"name": "v1alpha0", "served": false, "storage": false},
                    {"name": "v1alpha1", "served": true, "storage": true, "additionalPrinterColumns": [
                        {"jsonPath": ".spec.node", "name": "Node", "type": "string"},
                        {"jsonPath": ".spec.parity", "name": "Parity", "type": "integer"},
                        {"jsonPath": ".status.phase", "name": "Phase", "type": "string"},
                        {"jsonPath": ".status.message", "name": "Message", "type": "string", "priority": 1}
                    ]}
                ]
            },
            "status": {"conditions": [{"type": "Established", "status": "True"}]}
        })
    }

    fn a_set() -> Value {
        json!({
            "metadata": {"name": "md0", "creationTimestamp": "2026-11-18T10:00:00Z"},
            "spec": {"node": "forge1", "parity": 2, "drives": [{"wwn": "a"}]},
            "status": {"phase": "Rebuilding", "conditions": [{"type": "Degraded", "status": "True", "reason": "MemberMissing"}]}
        })
    }

    #[test]
    fn a_crd_is_its_served_storage_version_and_that_versions_columns() {
        let k = Kind::from_crd(&raidsets()).unwrap();
        assert_eq!((k.name.as_str(), k.version.as_str(), k.namespaced), ("raidsets.storage.storm.io", "v1alpha1", false));
        assert_eq!(k.versions, vec!["v1alpha1"]);
        assert_eq!(k.established, Some(true));
        assert_eq!(k.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Node", "Parity", "Phase", "Message"]);
        assert_eq!(k.columns[3].priority, 1);
        assert_eq!(k.collection(None), "/apis/storage.storm.io/v1alpha1/raidsets");
        assert_eq!(k.object_path("", "md0"), "/apis/storage.storm.io/v1alpha1/raidsets/md0");
        assert_eq!(k.object_href("", "md0"), "#/cr/raidsets.storage.storm.io/_/md0");
    }

    #[test]
    fn the_grid_is_name_then_the_printer_columns_then_age() {
        let k = Kind::from_crd(&raidsets()).unwrap();
        let (t, _) = table(&k, &[a_set()]);
        let names: Vec<&str> = t["columns"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Name", "Node", "Parity", "Phase", "Message", "Age"]);
        assert_eq!(t["gap"], json!(false));
        let r = &t["rows"][0];
        assert_eq!((r["id"].as_str(), r["c0"].as_str(), r["c1"].as_i64(), r["c2"].as_str()), (Some("md0"), Some("forge1"), Some(2), Some("Rebuilding")));
        // Missing is empty, as the apiserver's Table prints it.
        assert_eq!(r["c3"], json!(""));
        assert_eq!(r["conditions"][0]["reason"], json!("MemberMissing"));
    }

    #[test]
    fn a_crd_with_its_own_age_column_gets_no_second_one() {
        let mut crd = raidsets();
        crd["spec"]["versions"][1]["additionalPrinterColumns"]
            .as_array_mut()
            .unwrap()
            .push(json!({"jsonPath": ".metadata.creationTimestamp", "name": "Age", "type": "date"}));
        let k = Kind::from_crd(&crd).unwrap();
        let (t, _) = table(&k, &[a_set()]);
        let ages = t["columns"].as_array().unwrap().iter().filter(|c| c["name"] == "Age").count();
        assert_eq!(ages, 1);
    }

    #[test]
    fn paths_resolve_as_the_apiservers_table_does() {
        let o = json!({"status": {"conditions": [{"type": "Ready"}], "link": {"level": "ok"}}});
        assert_eq!(json_path(&o, ".status.link.level"), Some(&json!("ok")));
        assert_eq!(json_path(&o, ".status.conditions[0].type"), Some(&json!("Ready")));
        assert_eq!(json_path(&o, ".status.conditions[3].type"), None);
        // A filter is not allowed in a printer column, and does not resolve.
        assert_eq!(json_path(&o, ".status.conditions[?(@.type==\"Ready\")].type"), None);
    }

    #[test]
    fn a_kind_without_columns_shows_its_status_fields_and_says_so() {
        let mut crd = raidsets();
        crd["spec"]["scope"] = json!("Namespaced");
        crd["spec"]["versions"][1].as_object_mut().unwrap().remove("additionalPrinterColumns");
        let k = Kind::from_crd(&crd).unwrap();
        let o = json!({"metadata": {"name": "a", "namespace": "team"}, "status": {"message": "m", "zeta": 1, "phase": "Ready", "ok": true, "nested": {"x": 1}}});
        let (t, _) = table(&k, &[o]);
        let names: Vec<&str> = t["columns"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Name", "Namespace", "Phase", "Ok", "Zeta", "Message", "Age"]);
        assert_eq!(t["gap"], json!(true));
        assert_eq!(t["rows"][0]["id"], json!("team/a"));
        assert_eq!(t["rows"][0]["href"], json!("#/cr/raidsets.storage.storm.io/team/a"));
    }

    #[test]
    fn an_aggregated_resource_is_a_kind_and_its_subresources_are_not() {
        let k = Kind::from_discovery("storage.storm.io", "v1", &json!({"name": "volumes", "kind": "Volume", "namespaced": false, "verbs": ["get", "list"]})).unwrap();
        assert_eq!((k.name.as_str(), k.source, k.columns.len()), ("volumes.storage.storm.io", "aggregated", 0));
        assert!(Kind::from_discovery("storage.storm.io", "v1", &json!({"name": "volumes/status", "verbs": ["get", "list"]})).is_none());
        assert!(Kind::from_discovery("storage.storm.io", "v1", &json!({"name": "reviews", "verbs": ["create"]})).is_none());
    }

    #[test]
    fn only_somebodys_own_groups_are_custom() {
        assert!(is_custom_group("storage.storm.io"));
        assert!(is_custom_group("cilium.io"));
        assert!(!is_custom_group("apps"));
        assert!(!is_custom_group("rbac.authorization.k8s.io"));
        assert!(!is_custom_group("route.openshift.io"));
    }

    #[test]
    fn an_owner_is_a_link_to_where_the_console_shows_it() {
        let crds = vec![Kind::from_crd(&raidsets()).unwrap()];
        let l = owner_link(&json!({"apiVersion": "storage.storm.io/v1alpha1", "kind": "RaidSet", "name": "md0", "controller": true}), "", &crds);
        assert_eq!(l["href"], json!("#/cr/raidsets.storage.storm.io/_/md0"));
        assert_eq!(l["controller"], json!(true));
        let l = owner_link(&json!({"apiVersion": "v1", "kind": "Pod", "name": "web"}), "shop", &crds);
        assert_eq!(l["href"], json!("#/pod/shop/web"));
        let l = owner_link(&json!({"apiVersion": "apps/v1", "kind": "Deployment", "name": "web"}), "shop", &crds);
        assert_eq!(l["href"], json!("#/grid?id=k8s:deploy:shop/web"));
        let l = owner_link(&json!({"apiVersion": "x.example/v1", "kind": "Thing", "name": "t"}), "shop", &crds);
        assert_eq!(l["href"], Value::Null);
    }

    #[test]
    fn conditions_are_rows_with_every_field() {
        let rows = conditions(&json!({"status": {"conditions": [{"type": "Ready", "status": "False", "reason": "R", "message": "m", "lastTransitionTime": "t"}]}}));
        assert_eq!(rows, vec![json!({"type": "Ready", "status": "False", "reason": "R", "message": "m", "lastTransitionTime": "t"})]);
        assert!(conditions(&json!({})).is_empty());
    }
}
