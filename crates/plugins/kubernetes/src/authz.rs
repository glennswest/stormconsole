//! What a viewer may see and do — asked of the apiserver, not decided here.
//!
//! The distinction issue #7 turns on: a filter in the UI is a display
//! choice, and this has to be an authorization result. So the console
//! never decides; it asks rustkube **as the viewer**, with the viewer's
//! own bearer token, and takes the answer.
//!
//! One question, `SelfSubjectRulesReview` (rustkube#59, #45): what may I do
//! here? Asked once with no namespace, which answers for the cluster's
//! bindings, and once per namespace the console knows of, unless the first
//! answer is already the wildcard rule. A review reads nothing and performs
//! nothing, and one answer serves every question about that namespace:
//!
//! - **What they see** ([`Rights::sees`]): `list namespaces` at cluster scope
//!   (the apiserver's own test for seeing every project), or `list pods` in
//!   the namespace, which is what a RoleBinding grants and what "can I see
//!   anything in here" means. Not `get` on the Namespace: that is
//!   cluster-scoped and no RoleBinding can grant it. Where a rules answer is
//!   marked incomplete or failed, one `SelfSubjectAccessReview` settles it.
//! - **What they are offered** ([`Rights::may`]): the actions a plugin would
//!   carry to the apiserver, so a `view` member is not offered Delete. That
//!   is a display choice, and the only one here: the apiserver still decides
//!   the request, so what cannot be determined stays offered.
//!
//! An apiserver that does not serve the review gets the older questions:
//! `GET /api/v1/namespaces`, then on a 403 `GET
//! /api/v1/namespaces/{ns}/pods?limit=1` per namespace. It says nothing about
//! actions, so all stay offered.
//!
//! Answers are cached briefly per token: a console redraws its feed every
//! two seconds and an authorization question per redraw would be a denial
//! of service on the apiserver by way of a UI.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use console_core::{Action, Viewer};
use futures_util::future::join_all;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::cache::Store;

/// How long an answer stands. Short enough that a RoleBinding takes
/// effect while somebody is still looking at the screen.
const TTL: Duration = Duration::from_secs(30);

/// How many namespaces are asked about at once.
const AT_ONCE: usize = 16;

#[derive(Clone, Debug, PartialEq)]
pub struct Allowed {
    pub namespaces: HashSet<String>,
    /// How the answer was reached, for the note the UI shows.
    pub source: Source,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The reviews answered.
    Reviewed,
    /// The apiserver serves no review, and listed them for this identity.
    Listed,
    /// The apiserver serves no review and refused the list, so each was
    /// asked about in turn.
    Probed,
    /// The apiserver could not be asked at all. Nothing is claimed and
    /// nothing is hidden — an unreachable authorizer must not silently
    /// become a permissive one, so the caller reports it as unknown.
    Unavailable,
}

/// One thing somebody might do, as RBAC spells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Need {
    pub group: String,
    /// With its subresource, as a rule writes it: `deployments/scale`.
    pub resource: String,
    pub verb: String,
    pub namespace: Option<String>,
    pub name: Option<String>,
}

impl Need {
    pub fn new(group: &str, resource: &str, verb: &str) -> Self {
        Self { group: group.into(), resource: resource.into(), verb: verb.into(), namespace: None, name: None }
    }

    pub fn within(mut self, namespace: &str) -> Self {
        self.namespace = Some(namespace.into());
        self
    }

    pub fn named(mut self, name: &str) -> Self {
        self.name = Some(name.into());
        self
    }

    /// What `verb` on an apiserver object's path needs.
    pub fn of_api_path(path: &str, verb: &str) -> Option<Self> {
        let path = path.split('?').next()?;
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let (group, rest) = match parts.as_slice() {
            ["api", _, rest @ ..] => ("", rest),
            ["apis", group, _, rest @ ..] => (*group, rest),
            _ => return None,
        };
        let (namespace, rest) = match rest {
            ["namespaces", ns, rest @ ..] if !rest.is_empty() => (Some(*ns), rest),
            _ => (None, rest),
        };
        let (resource, name) = match rest {
            [resource] => ((*resource).to_string(), None),
            [resource, name] => ((*resource).to_string(), Some(*name)),
            [resource, name, sub] => (format!("{resource}/{sub}"), Some(*name)),
            _ => return None,
        };
        Some(Self {
            group: group.into(),
            resource,
            verb: verb.into(),
            namespace: namespace.map(str::to_string),
            name: name.map(str::to_string),
        })
    }

    /// What a row's action needs, for the ones this plugin carries to the
    /// apiserver as the viewer. `None`: not the apiserver's to answer.
    pub fn of_action(a: &Action) -> Option<Self> {
        if let Some(node) = a.path.strip_prefix("#/node/") {
            // Drain is a link to the page that shows the plan first.
            return (a.id == "drain").then(|| Self::new("", "nodes", "patch").named(node));
        }
        if let Some(rest) = a.path.strip_prefix("#/attach/") {
            let (ns, _claim) = rest.split_once('/')?;
            return Some(Self::new("kubevirt.io", "virtualmachines", "patch").within(ns));
        }
        let rest = a.path.strip_prefix("/api/plugins/k8s")?;
        if let Some(api) = rest.strip_prefix("/raw") {
            return a.method.eq_ignore_ascii_case("DELETE").then(|| Self::of_api_path(api, "delete")).flatten();
        }
        let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
        match parts.as_slice() {
            ["pods", ns, name, "delete"] => Some(Self::new("", "pods", "delete").within(ns).named(name)),
            ["nodes", name, "cordon" | "uncordon"] => Some(Self::new("", "nodes", "patch").named(name)),
            ["scale", kind, ns, name] => {
                let plural = crate::dayto::scalable(kind)?;
                Some(Self::new("apps", &format!("{plural}/scale"), "patch").within(ns).named(name))
            }
            _ => None,
        }
    }
}

/// One rules review's answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    rules: Vec<Value>,
    /// The apiserver could not read everything: there may be more rules.
    incomplete: bool,
}

impl Rules {
    fn from_review(v: &Value) -> Self {
        Self {
            rules: v.pointer("/status/resourceRules").and_then(Value::as_array).cloned().unwrap_or_default(),
            incomplete: v.pointer("/status/incomplete").and_then(Value::as_bool).unwrap_or(false),
        }
    }

    fn permits(&self, need: &Need) -> bool {
        self.rules.iter().any(|r| rule_matches(r, need))
    }

    /// A cluster-admin's answer: nothing further needs asking.
    fn everything(&self) -> bool {
        let star = |r: &Value, f: &str| r[f].as_array().is_some_and(|l| l.iter().any(|x| x == "*"));
        self.rules.iter().any(|r| {
            star(r, "verbs")
                && star(r, "apiGroups")
                && star(r, "resources")
                && r["resourceNames"].as_array().is_none_or(Vec::is_empty)
        })
    }
}

/// Reads a rule exactly as rustkube's `rule_matches` does (`rbac_engine.rs`),
/// so the answer here is the one the request would get. An empty or absent
/// group or resource list grants nothing; `pods/*` is every subresource of
/// pods and not pods; `resourceNames` narrows only a request that has a name.
fn rule_matches(rule: &Value, need: &Need) -> bool {
    let list = |f: &str| rule[f].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    if !list("apiGroups").iter().map(text).any(|g| g == "*" || g == need.group) {
        return false;
    }
    let resource = |r: String| {
        r == "*" || r == need.resource || (r.ends_with("/*") && need.resource.starts_with(&r[..r.len() - 1]))
    };
    if !list("resources").iter().map(text).any(resource) {
        return false;
    }
    if rule["verbs"].is_array() && !list("verbs").iter().map(text).any(|v| v == "*" || v == need.verb) {
        return false;
    }
    match &need.name {
        Some(name) if !list("resourceNames").is_empty() => list("resourceNames").iter().any(|n| n.as_str() == Some(name)),
        _ => true,
    }
}

/// Everything one token may do, as far as the apiserver said.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rights {
    cluster: Rules,
    /// `None`: that namespace's review failed.
    namespaces: HashMap<String, Option<Rules>>,
}

impl Rights {
    /// Did the apiserver say yes?
    fn granted(&self, need: &Need) -> bool {
        self.cluster.permits(need)
            || need
                .namespace
                .as_ref()
                .and_then(|ns| self.namespaces.get(ns))
                .and_then(Option::as_ref)
                .is_some_and(|r| r.permits(need))
    }

    /// Should this be offered? Yes unless the apiserver's whole answer says
    /// no: a button that gets a 403 is the lesser wrong, against one a person
    /// should have and cannot find, and the request is decided regardless.
    pub fn may(&self, need: &Need) -> bool {
        if self.granted(need) || self.cluster.incomplete {
            return true;
        }
        let Some(ns) = &need.namespace else { return false };
        match self.namespaces.get(ns) {
            Some(Some(rules)) => rules.incomplete,
            // Its review failed, or it is newer than this answer.
            _ => true,
        }
    }

    /// May they see this namespace, on what the rules say? `None` when the
    /// rules cannot settle a no.
    fn sees(&self, ns: &str) -> Option<bool> {
        if self.cluster.permits(&Need::new("", "namespaces", "list"))
            || self.granted(&Need::new("", "pods", "list").within(ns))
        {
            return Some(true);
        }
        let settled = !self.cluster.incomplete && matches!(self.namespaces.get(ns), Some(Some(r)) if !r.incomplete);
        settled.then_some(false)
    }
}

/// One token's answer: what it sees, and what it may do where the reviews
/// could be asked.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub allowed: Allowed,
    pub rights: Option<Arc<Rights>>,
}

struct Entry {
    at: Instant,
    answer: Answer,
}

#[derive(Default)]
pub struct Authorizer {
    cache: RwLock<HashMap<String, Entry>>,
}

impl Authorizer {
    /// Drop every cached answer. Called when the console itself changed who
    /// may see what — a member added, a project made — so the person just
    /// let in is not shut out for the rest of the cache window.
    pub async fn forget(&self) {
        self.cache.write().await.clear();
    }

    /// The answer for this token, out of `known` (the console's own
    /// cluster-wide cache of namespaces).
    pub async fn answer(&self, base: &str, http: &reqwest::Client, token: &str, known: &[String]) -> Answer {
        if let Some(e) = self.cache.read().await.get(token) {
            if e.at.elapsed() < TTL {
                return e.answer.clone();
            }
        }
        let answer = ask(base, http, token, known).await;
        let mut cache = self.cache.write().await;
        cache.retain(|_, e| e.at.elapsed() < TTL);
        cache.insert(token.to_string(), Entry { at: Instant::now(), answer: answer.clone() });
        answer
    }
}

enum Review {
    Rules(Rules),
    /// It answered, and not with rules.
    NotAnswered,
    Unreachable,
}

async fn rules_review(base: &str, http: &reqwest::Client, token: &str, ns: Option<&str>) -> Review {
    let spec = ns.map_or_else(|| json!({}), |ns| json!({"namespace": ns}));
    let body = json!({"apiVersion": "authorization.k8s.io/v1", "kind": "SelfSubjectRulesReview", "spec": spec});
    let resp = http
        .post(format!("{base}/apis/authorization.k8s.io/v1/selfsubjectrulesreviews"))
        .bearer_auth(token)
        .json(&body)
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    match resp {
        Ok(r) if r.status().is_success() => match r.json::<Value>().await {
            Ok(v) => Review::Rules(Rules::from_review(&v)),
            Err(_) => Review::NotAnswered,
        },
        Ok(_) => Review::NotAnswered,
        Err(_) => Review::Unreachable,
    }
}

/// `SelfSubjectAccessReview` for seeing into `ns`: the one question, where a
/// rules answer did not settle it. Anything but a yes is a no.
async fn access_review(base: &str, http: &reqwest::Client, token: &str, ns: &str) -> bool {
    let body = json!({
        "apiVersion": "authorization.k8s.io/v1",
        "kind": "SelfSubjectAccessReview",
        "spec": {"resourceAttributes": {"group": "", "resource": "pods", "verb": "list", "namespace": ns}},
    });
    let resp = http
        .post(format!("{base}/apis/authorization.k8s.io/v1/selfsubjectaccessreviews"))
        .bearer_auth(token)
        .json(&body)
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    match resp {
        Ok(r) if r.status().is_success() => {
            r.json::<Value>().await.ok().and_then(|v| v.pointer("/status/allowed").and_then(Value::as_bool)) == Some(true)
        }
        _ => false,
    }
}

async fn ask(base: &str, http: &reqwest::Client, token: &str, known: &[String]) -> Answer {
    match rules_review(base, http, token, None).await {
        Review::Rules(cluster) => reviewed(base, http, token, known, cluster).await,
        Review::NotAnswered => Answer { allowed: unreviewed(base, http, token, known).await, rights: None },
        Review::Unreachable => {
            Answer { allowed: Allowed { namespaces: HashSet::new(), source: Source::Unavailable }, rights: None }
        }
    }
}

async fn namespace_rules(base: &str, http: &reqwest::Client, token: &str, ns: &str) -> (String, Option<Rules>) {
    let rules = match rules_review(base, http, token, Some(ns)).await {
        Review::Rules(r) => Some(r),
        _ => None,
    };
    (ns.to_string(), rules)
}

async fn seen(base: &str, http: &reqwest::Client, token: &str, ns: &str) -> Option<String> {
    access_review(base, http, token, ns).await.then(|| ns.to_string())
}

async fn reviewed(base: &str, http: &reqwest::Client, token: &str, known: &[String], cluster: Rules) -> Answer {
    let mut namespaces = HashMap::new();
    if !cluster.everything() {
        for some in known.chunks(AT_ONCE) {
            let asks: Vec<_> = some.iter().map(|ns| namespace_rules(base, http, token, ns)).collect();
            namespaces.extend(join_all(asks).await);
        }
    }
    let rights = Rights { cluster, namespaces };
    let mut sees = HashSet::new();
    let mut unsettled = Vec::new();
    for ns in known {
        match rights.sees(ns) {
            Some(true) => {
                sees.insert(ns.clone());
            }
            Some(false) => {}
            None => unsettled.push(ns),
        }
    }
    for some in unsettled.chunks(AT_ONCE) {
        let asks: Vec<_> = some.iter().map(|ns| seen(base, http, token, ns)).collect();
        sees.extend(join_all(asks).await.into_iter().flatten());
    }
    Answer {
        allowed: Allowed { namespaces: sees, source: Source::Reviewed },
        rights: Some(Arc::new(rights)),
    }
}

/// For an apiserver that serves no review.
async fn unreviewed(base: &str, http: &reqwest::Client, token: &str, known: &[String]) -> Allowed {
    let list = http
        .get(format!("{base}/api/v1/namespaces"))
        .bearer_auth(token)
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    match list {
        Ok(resp) if resp.status().is_success() => {
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            Allowed { namespaces: names(&body), source: Source::Listed }
        }
        // The identity itself was not accepted: no namespace will say otherwise.
        Ok(resp) if resp.status().as_u16() == 401 => Allowed { namespaces: HashSet::new(), source: Source::Probed },
        Ok(resp) if resp.status().as_u16() == 403 => {
            let mut ok = HashSet::new();
            for ns in known {
                let r = http
                    .get(format!("{base}{}", probe_path(ns)))
                    .bearer_auth(token)
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await;
                if matches!(r, Ok(ref resp) if resp.status().is_success()) {
                    ok.insert(ns.clone());
                }
            }
            Allowed { namespaces: ok, source: Source::Probed }
        }
        _ => Allowed { namespaces: HashSet::new(), source: Source::Unavailable },
    }
}

/// `limit=1` because the answer is the status code, not the body.
fn probe_path(ns: &str) -> String {
    format!("/api/v1/namespaces/{ns}/pods?limit=1")
}

/// `metadata.name` of every item in a list response.
fn names(list: &Value) -> HashSet<String> {
    list.get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|o| o.pointer("/metadata/name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The line the UI shows about what it is not showing. Silence would read
/// as a short list, which reads as a broken console.
pub fn note(hidden: usize, source: Source) -> String {
    match source {
        Source::Unavailable => {
            "the apiserver could not be asked what you may see".to_string()
        }
        _ if hidden == 1 => "1 project you cannot view".to_string(),
        _ => format!("{hidden} projects you cannot view"),
    }
}

/// The shared answer to "what may this viewer see and do".
///
/// Two plugins ask it — kubernetes and vm — and they must not ask
/// separately: two Authorizers would mean two sets of reviews per viewer
/// per cache window against the same apiserver for the same answer, and
/// they could disagree for thirty seconds at a time. One instance is
/// built beside the kubernetes plugin's watch cache (which already holds
/// the namespace list) and handed to whoever else needs it.
pub struct NamespaceAccess {
    /// The apiserver, asked as the viewer — through the console's checked
    /// connection, never its bearer (#33).
    conn: Option<Arc<console_core::apiserver::Conn>>,
    store: Arc<Store>,
    authz: Authorizer,
    /// Namespaces beyond rustkube's reserved set that hold the system's own
    /// things — the node's services (`cilium`) — and so are never a target
    /// for somebody's workload (#28).
    system: std::sync::RwLock<Vec<String>>,
}

/// Projects the system owns (#28, #162): rustkube's reserved set
/// (`default`, `openshift`, `kube-*`, `openshift-*`), the platform's own
/// (`storm-*`, which stormcos and stormcentral make for their work, and
/// `cilium`, `cilium-secrets`), and `extra`, where a trailing `*` matches a
/// prefix. User workloads are never created in one, and the Projects page
/// marks each.
pub fn is_system_namespace(ns: &str, extra: &[String]) -> bool {
    ns == "default"
        || ns == "openshift"
        || ns == "cilium"
        || ns == "cilium-secrets"
        || ns.starts_with("kube-")
        || ns.starts_with("openshift-")
        || ns.starts_with("storm-")
        || extra.iter().any(|e| match e.strip_suffix('*') {
            Some(prefix) => ns.starts_with(prefix),
            None => e == ns,
        })
}

/// What a viewer may not see: the namespaces, and the line to say about
/// them. `None` means nothing is being withheld.
pub type Hidden = Option<(HashSet<String>, String)>;

impl NamespaceAccess {
    pub fn new(conn: Option<Arc<console_core::apiserver::Conn>>, store: Arc<Store>) -> Arc<Self> {
        Arc::new(Self {
            conn,
            store,
            authz: Authorizer::default(),
            system: std::sync::RwLock::new(vec![]),
        })
    }

    /// Replace the extra system namespaces (`[kubernetes] system_namespaces`).
    pub fn set_system_namespaces(&self, extra: Vec<String>) {
        if let Ok(mut s) = self.system.write() {
            *s = extra;
        }
    }

    pub fn system_namespaces(&self) -> Vec<String> {
        self.system.read().map(|s| s.clone()).unwrap_or_default()
    }

    /// What this viewer's requests may do at the apiserver, or `None` when
    /// that cannot be asked. Someone with no kubernetes identity of their own
    /// acts on the console's credential, so that is whose rights these are.
    pub async fn rights(&self, viewer: &Viewer) -> Option<Arc<Rights>> {
        let conn = self.conn.as_ref()?;
        let token = viewer.token.clone().or_else(|| conn.token())?;
        let known = self.store.namespaces().await;
        self.authz.answer(conn.server(), &conn.http(), &token, &known).await.rights
    }

    /// Should this be offered to this viewer? See [`Rights::may`].
    pub async fn may(&self, viewer: &Viewer, need: &Need) -> bool {
        self.rights(viewer).await.is_none_or(|r| r.may(need))
    }

    /// See [`Authorizer::forget`].
    pub async fn forget(&self) {
        self.authz.forget().await;
    }

    pub fn is_system(&self, ns: &str) -> bool {
        is_system_namespace(ns, &self.system_namespaces())
    }

    /// The projects this viewer may see, or `None` for every one (the same answer as
    /// [`hidden`](Self::hidden), for a query that has to name them, #46).
    pub async fn visible(&self, viewer: &Viewer) -> Option<Vec<String>> {
        let (hidden, _) = self.hidden(viewer).await?;
        if hidden.is_empty() {
            return None;
        }
        Some(self.store.namespaces().await.into_iter().filter(|n| !hidden.contains(n)).collect())
    }

    /// `None` when nothing is hidden — no identity to authorize against,
    /// no apiserver, or a viewer who may see everything.
    pub async fn hidden(&self, viewer: &Viewer) -> Hidden {
        let (Some(token), Some(conn)) = (&viewer.token, &self.conn) else { return None };
        let known = self.store.namespaces().await;
        let allowed = self.authz.answer(conn.server(), &conn.http(), token, &known).await.allowed;
        if allowed.source == Source::Unavailable {
            // An authorizer that cannot be reached must not quietly become
            // a permissive one, and must not blank the console either:
            // nothing is hidden, and the note says why.
            return Some((HashSet::new(), note(0, Source::Unavailable)));
        }
        let hidden: HashSet<String> =
            known.into_iter().filter(|n| !allowed.namespaces.contains(n)).collect();
        if hidden.is_empty() {
            return None;
        }
        let note = note(hidden.len(), allowed.source);
        Some((hidden, note))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in apiserver: rules per namespace (`""` for none), recording
    /// what it was asked. `reviews: false` is one that does not serve them.
    #[derive(Clone, Default)]
    struct Stub {
        rules: Arc<HashMap<String, Value>>,
        reviews: bool,
        pods_allowed: Arc<HashSet<String>>,
        asked: Arc<std::sync::Mutex<Vec<String>>>,
    }

    async fn stub(s: Stub) -> String {
        use axum::extract::{Json, Path, State};
        use axum::http::StatusCode;
        use axum::routing::{get, post};
        async fn rules(State(s): State<Stub>, Json(b): Json<Value>) -> Result<Json<Value>, StatusCode> {
            let ns = b["spec"]["namespace"].as_str().unwrap_or("").to_string();
            s.asked.lock().unwrap().push(format!("rules {ns}"));
            if !s.reviews {
                return Err(StatusCode::NOT_FOUND);
            }
            Ok(Json(s.rules.get(&ns).cloned().unwrap_or_else(|| json!({"status": {"resourceRules": []}}))))
        }
        async fn access(State(s): State<Stub>, Json(b): Json<Value>) -> Json<Value> {
            let ns = b["spec"]["resourceAttributes"]["namespace"].as_str().unwrap_or("").to_string();
            s.asked.lock().unwrap().push(format!("access {ns}"));
            Json(json!({"status": {"allowed": s.pods_allowed.contains(&ns)}}))
        }
        async fn list(State(s): State<Stub>) -> StatusCode {
            s.asked.lock().unwrap().push("list".into());
            StatusCode::FORBIDDEN
        }
        async fn pods(State(s): State<Stub>, Path(ns): Path<String>) -> StatusCode {
            s.asked.lock().unwrap().push(format!("pods {ns}"));
            if s.pods_allowed.contains(&ns) { StatusCode::OK } else { StatusCode::FORBIDDEN }
        }
        let app = axum::Router::new()
            .route("/apis/authorization.k8s.io/v1/selfsubjectrulesreviews", post(rules))
            .route("/apis/authorization.k8s.io/v1/selfsubjectaccessreviews", post(access))
            .route("/api/v1/namespaces", get(list))
            .route("/api/v1/namespaces/{ns}/pods", get(pods))
            .with_state(s);
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn view_rules() -> Value {
        json!({"status": {"resourceRules": [{"apiGroups": [""], "resources": ["pods", "services"], "verbs": ["get", "list", "watch"]}]}})
    }

    fn known() -> Vec<String> {
        vec!["mine".to_string(), "theirs".to_string()]
    }

    #[tokio::test]
    async fn the_reviews_answer_and_nothing_is_read_from_the_namespaces() {
        let s = Stub {
            reviews: true,
            rules: Arc::new(HashMap::from([("mine".to_string(), view_rules())])),
            ..Default::default()
        };
        let base = stub(s.clone()).await;
        let a = ask(&base, &reqwest::Client::new(), "t", &known()).await;
        assert_eq!(a.allowed.source, Source::Reviewed);
        assert_eq!(a.allowed.namespaces, HashSet::from(["mine".to_string()]));
        let asked = s.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 3, "the cluster's, then one per namespace: {asked:?}");
        assert!(asked.iter().all(|q| q.starts_with("rules ")), "{asked:?}");
        let r = a.rights.unwrap();
        assert!(!r.may(&Need::new("", "pods", "delete").within("mine")), "a view member is not offered delete");
        assert!(r.may(&Need::new("", "pods", "get").within("mine")));
    }

    #[tokio::test]
    async fn a_cluster_admin_costs_one_question() {
        let s = Stub {
            reviews: true,
            rules: Arc::new(HashMap::from([(
                String::new(),
                json!({"status": {"resourceRules": [{"apiGroups": ["*"], "resources": ["*"], "verbs": ["*"]}]}}),
            )])),
            ..Default::default()
        };
        let base = stub(s.clone()).await;
        let a = ask(&base, &reqwest::Client::new(), "t", &known()).await;
        assert_eq!(a.allowed.namespaces.len(), 2);
        assert_eq!(s.asked.lock().unwrap().len(), 1);
        let r = a.rights.unwrap();
        assert!(r.may(&Need::new("", "nodes", "patch")));
        assert!(r.may(&Need::new("", "pods", "delete").within("a-new-one")));
    }

    #[tokio::test]
    async fn an_incomplete_answer_is_settled_by_an_access_review_and_keeps_actions() {
        let s = Stub {
            reviews: true,
            rules: Arc::new(HashMap::from([(
                "mine".to_string(),
                json!({"status": {"resourceRules": [], "incomplete": true}}),
            )])),
            pods_allowed: Arc::new(HashSet::from(["mine".to_string()])),
            ..Default::default()
        };
        let base = stub(s.clone()).await;
        let a = ask(&base, &reqwest::Client::new(), "t", &known()).await;
        assert_eq!(a.allowed.namespaces, HashSet::from(["mine".to_string()]));
        assert!(s.asked.lock().unwrap().contains(&"access mine".to_string()));
        assert!(!s.asked.lock().unwrap().contains(&"access theirs".to_string()), "a complete no is a no");
        let r = a.rights.unwrap();
        assert!(r.may(&Need::new("", "pods", "delete").within("mine")), "may be among the rules it could not read");
        assert!(!r.may(&Need::new("", "pods", "delete").within("theirs")));
    }

    #[tokio::test]
    async fn an_apiserver_without_the_review_is_still_probed_and_offers_everything() {
        let s = Stub { pods_allowed: Arc::new(HashSet::from(["mine".to_string()])), ..Default::default() };
        let base = stub(s.clone()).await;
        let a = ask(&base, &reqwest::Client::new(), "t", &known()).await;
        assert_eq!(a.allowed.source, Source::Probed);
        assert_eq!(a.allowed.namespaces, HashSet::from(["mine".to_string()]));
        assert_eq!(a.rights, None);
        assert!(s.asked.lock().unwrap().contains(&"pods theirs".to_string()));
    }

    #[test]
    fn a_rule_reads_as_rustkube_reads_it() {
        let pods = Need::new("", "pods", "get").within("n");
        let rule = |v: Value| rule_matches(&v, &pods);
        assert!(rule(json!({"apiGroups": [""], "resources": ["pods"], "verbs": ["get"]})));
        assert!(rule(json!({"apiGroups": ["*"], "resources": ["*"], "verbs": ["*"]})));
        assert!(!rule(json!({"apiGroups": ["apps"], "resources": ["pods"], "verbs": ["get"]})));
        assert!(!rule(json!({"resources": ["pods"], "verbs": ["get"]})), "no groups grants nothing");
        assert!(!rule(json!({"apiGroups": [""], "verbs": ["get"]})), "no resources grants nothing");
        assert!(!rule(json!({"apiGroups": [""], "resources": ["pods/*"], "verbs": ["get"]})), "pods/* is not pods");
        let scale = Need::new("apps", "deployments/scale", "patch");
        assert!(rule_matches(&json!({"apiGroups": ["apps"], "resources": ["deployments/*"], "verbs": ["patch"]}), &scale));
        assert!(!rule_matches(&json!({"apiGroups": ["apps"], "resources": ["deployments"], "verbs": ["patch"]}), &scale));
        let named = json!({"apiGroups": [""], "resources": ["pods"], "verbs": ["delete"], "resourceNames": ["a"]});
        assert!(rule_matches(&named, &Need::new("", "pods", "delete").named("a")));
        assert!(!rule_matches(&named, &Need::new("", "pods", "delete").named("b")));
        assert!(rule_matches(&named, &Need::new("", "pods", "delete")), "a request with no name is not narrowed");
    }

    #[test]
    fn an_api_path_says_what_it_needs() {
        let n = Need::of_api_path("/apis/cilium.io/v2/namespaces/web/ciliumnetworkpolicies/p", "delete").unwrap();
        assert_eq!((n.group.as_str(), n.resource.as_str(), n.namespace.as_deref(), n.name.as_deref()), ("cilium.io", "ciliumnetworkpolicies", Some("web"), Some("p")));
        let n = Need::of_api_path("/apis/storm.io/v1alpha1/cloudimages/c", "delete").unwrap();
        assert_eq!((n.resource.as_str(), n.namespace, n.name.as_deref()), ("cloudimages", None, Some("c")));
        let n = Need::of_api_path("/api/v1/namespaces/x", "update").unwrap();
        assert_eq!((n.group.as_str(), n.resource.as_str(), n.namespace, n.name.as_deref()), ("", "namespaces", None, Some("x")));
        let n = Need::of_api_path("/api/v1/namespaces/x/pods/p", "update").unwrap();
        assert_eq!((n.resource.as_str(), n.namespace.as_deref()), ("pods", Some("x")));
        assert_eq!(Need::of_api_path("/healthz", "get"), None);
    }

    #[test]
    fn a_rows_action_says_what_it_needs() {
        let act = |id: &str, method: &str, path: &str| Action {
            id: id.into(), label: id.into(), method: method.into(), path: path.into(), enabled: true, danger: false, tone: None,
        };
        assert_eq!(Need::of_action(&act("delete", "POST", "/api/plugins/k8s/pods/n/p/delete")), Some(Need::new("", "pods", "delete").within("n").named("p")));
        assert_eq!(Need::of_action(&act("cordon", "POST", "/api/plugins/k8s/nodes/b1/cordon")), Some(Need::new("", "nodes", "patch").named("b1")));
        assert_eq!(Need::of_action(&act("drain", "GET", "#/node/b1")), Some(Need::new("", "nodes", "patch").named("b1")));
        assert_eq!(Need::of_action(&act("scale", "POST", "/api/plugins/k8s/scale/deploy/n/w")), Some(Need::new("apps", "deployments/scale", "patch").within("n").named("w")));
        assert_eq!(Need::of_action(&act("attach", "GET", "#/attach/n/c")), Some(Need::new("kubevirt.io", "virtualmachines", "patch").within("n")));
        let raw = Need::of_action(&act("delete", "DELETE", "/api/plugins/k8s/raw/apis/cilium.io/v2/namespaces/n/ciliumnetworkpolicies/p")).unwrap();
        assert_eq!((raw.verb.as_str(), raw.namespace.as_deref()), ("delete", Some("n")));
        assert_eq!(Need::of_action(&act("logs", "GET", "#/pod/n/p?tab=Logs")), None);
        assert_eq!(Need::of_action(&act("other", "POST", "/api/plugins/sb/proxy/api/v1/volumes")), None);
    }

    #[test]
    fn names_come_out_of_a_list_response() {
        let v = json!({"items": [
            {"metadata": {"name": "default"}},
            {"metadata": {"name": "kube-system"}},
            {"metadata": {}}
        ]});
        let got = names(&v);
        assert_eq!(got.len(), 2);
        assert!(got.contains("default") && got.contains("kube-system"));
        assert!(names(&json!({})).is_empty());
    }

    #[test]
    fn the_probe_asks_about_a_namespaced_resource_not_the_namespace_itself() {
        // A Namespace is cluster-scoped: a RoleBinding cannot grant `get`
        // on one, so probing it would report every ordinary project
        // member as having access to nothing.
        let p = probe_path("team-a");
        assert!(p.starts_with("/api/v1/namespaces/team-a/"), "{p}");
        assert!(p.contains("/pods"), "{p}");
        assert!(p.contains("limit=1"), "{p}");
    }

    #[test]
    fn the_note_counts_and_says_when_it_could_not_ask() {
        assert_eq!(note(1, Source::Listed), "1 project you cannot view");
        assert_eq!(note(3, Source::Reviewed), "3 projects you cannot view");
        assert_eq!(
            note(0, Source::Unavailable),
            "the apiserver could not be asked what you may see"
        );
    }

    #[tokio::test]
    async fn an_answer_is_cached_so_a_two_second_redraw_is_not_a_flood() {
        let a = Authorizer::default();
        // No apiserver at this address: the first ask fails and is cached
        // as Unavailable, and the second returns without dialling again.
        let http = reqwest::Client::new();
        let first = a.answer("http://127.0.0.1:1", &http, "t", &[]).await;
        assert_eq!(first.allowed.source, Source::Unavailable);
        let started = Instant::now();
        let second = a.answer("http://127.0.0.1:1", &http, "t", &[]).await;
        assert_eq!(second, first);
        assert!(started.elapsed() < Duration::from_millis(50), "second ask was not cached");
    }
}
