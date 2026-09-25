//! Projects: the top of the console (#28).
//!
//! A project is a namespace with an owner. rustkube serves OpenShift's
//! `project.openshift.io/v1` (rustkube#97): `projects` lists the namespaces
//! the caller has a RoleBinding in, `projectrequests` creates one annotated
//! `openshift.io/requester` and binds the requester `admin` in it, and the
//! ClusterRoles `admin`, `edit` and `view` are what membership is made of.
//! Everything here is asked **as the viewer**, so what a person sees and may
//! change is the apiserver's answer, not the console's.
//!
//! Where the API is not served, a project falls back to what it is made of —
//! a namespace — so the console still works against a plain apiserver; the
//! answer says which it is.
//!
//! **System namespaces** (`default`, `openshift`, `kube-*`, `openshift-*`,
//! and `[kubernetes] system_namespaces`) are never a project: not listed as
//! one, never a create target. They are the admin section's.
//!
//! **Isolation** is two NetworkPolicies, named so they can be found and
//! removed as one: `storm-isolate` lets every pod (and VM) in the namespace
//! reach every other and nothing outside reach in or be reached out to, and
//! `storm-isolate-dns` — opt-in — lets them resolve names through the
//! cluster DNS in `kube-system`. Enforcement is Cilium's; a VM is covered
//! only once it is a real pod-network endpoint (stormvm#16).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use console_core::Viewer;
use serde_json::{json, Value};

use crate::Inner;

pub const API: &str = "/apis/project.openshift.io/v1";
pub const RBAC: &str = "/apis/rbac.authorization.k8s.io/v1";
pub const NETPOL: &str = "/apis/networking.k8s.io/v1";
pub const REQUESTER: &str = "openshift.io/requester";
pub const DISPLAY: &str = "openshift.io/display-name";
pub const DESCRIPTION: &str = "openshift.io/description";
pub const ISOLATE: &str = "storm-isolate";
pub const ISOLATE_DNS: &str = "storm-isolate-dns";
/// The roles a member can hold, most to least.
pub const ROLES: &[&str] = &["admin", "edit", "view"];

fn ann<'a>(o: &'a Value, k: &str) -> Option<&'a str> {
    o.pointer("/metadata/annotations").and_then(|a| a.get(k)).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// One project as a row: from a `Project` or a `Namespace`, which carry the
/// same metadata. `isolated` is read from the watched NetworkPolicies.
pub fn row(o: &Value, system: bool, isolated: Option<bool>) -> Value {
    let name = o.pointer("/metadata/name").and_then(Value::as_str).unwrap_or_default();
    json!({
        "name": name,
        "displayName": ann(o, DISPLAY),
        "description": ann(o, DESCRIPTION),
        "requester": ann(o, REQUESTER),
        "phase": o.pointer("/status/phase").and_then(Value::as_str),
        "created": o.pointer("/metadata/creationTimestamp").and_then(Value::as_str),
        "system": system,
        "isolated": isolated.unwrap_or(false),
        "dns": isolated.is_some_and(|i| i),
    })
}

/// A DNS-1123 label, or why not — the apiserver's rule, said before asking.
pub fn check_name(n: &str) -> Result<(), String> {
    let ok = !n.is_empty()
        && n.len() <= 63
        && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && n.starts_with(|c: char| c.is_ascii_alphanumeric())
        && n.ends_with(|c: char| c.is_ascii_alphanumeric());
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{n:?} is not a project name: lowercase letters, digits and '-', at most 63, \
             starting and ending with a letter or digit"
        ))
    }
}

/// A name to offer somebody making their first project: theirs, and what
/// it is for.
pub fn suggest(user: &str, purpose: &str) -> String {
    let base: String = user
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let base = base.trim_matches('-');
    let base = if base.is_empty() { "my" } else { base };
    format!("{}-{purpose}", base.chars().take(50).collect::<String>())
}

/// The members of a project: every RoleBinding in it to one of the
/// project roles, flattened to (binding, role, subject).
pub fn members(bindings: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for b in bindings["items"].as_array().into_iter().flatten() {
        let rr = &b["roleRef"];
        let role = rr["name"].as_str().unwrap_or_default();
        if rr["kind"] != "ClusterRole" || !ROLES.contains(&role) {
            continue;
        }
        for s in b["subjects"].as_array().into_iter().flatten() {
            out.push(json!({
                "binding": b.pointer("/metadata/name"),
                "role": role,
                "kind": s["kind"],
                "name": s["name"],
                "namespace": s.get("namespace"),
            }));
        }
    }
    out.sort_by(|a, b| {
        let r = |v: &Value| ROLES.iter().position(|x| *x == v["role"].as_str().unwrap_or("")).unwrap_or(9);
        r(a).cmp(&r(b)).then(a["name"].as_str().cmp(&b["name"].as_str()))
    });
    out
}

/// The RoleBinding that makes `who` a `role` in `ns`. `system:serviceaccount:
/// <ns>:<name>` is a ServiceAccount; anything else is a User.
pub fn binding(ns: &str, who: &str, role: &str) -> Result<Value, String> {
    if !ROLES.contains(&role) {
        return Err(format!("{role:?} is not a project role: admin, edit or view"));
    }
    let who = who.trim();
    if who.is_empty() {
        return Err("who? A user name, or system:serviceaccount:<namespace>:<name>".into());
    }
    let subject = match who.strip_prefix("system:serviceaccount:").and_then(|r| r.split_once(':')) {
        Some((sns, name)) => json!({"kind": "ServiceAccount", "name": name, "namespace": sns}),
        None => json!({"kind": "User", "name": who, "apiGroup": "rbac.authorization.k8s.io"}),
    };
    let slug: String = who
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    Ok(json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "RoleBinding",
        "metadata": {"name": format!("{role}-{}", slug.trim_matches('-')), "namespace": ns,
                     "labels": {"storm.io/project-member": "true"}},
        "roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": "ClusterRole", "name": role},
        "subjects": [subject],
    }))
}

/// The two isolation policies. Every pod in the namespace talks to every
/// other; nothing else in or out; DNS to kube-system when asked for.
pub fn isolation(ns: &str, dns: bool) -> Vec<Value> {
    let meta = |name: &str| json!({"name": name, "namespace": ns, "labels": {"storm.io/isolation": "true"}});
    let mut out = vec![json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": meta(ISOLATE),
        "spec": {
            "podSelector": {},
            "policyTypes": ["Ingress", "Egress"],
            "ingress": [{"from": [{"podSelector": {}}]}],
            "egress": [{"to": [{"podSelector": {}}]}],
        }
    })];
    if dns {
        out.push(json!({
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": meta(ISOLATE_DNS),
            "spec": {
                "podSelector": {},
                "policyTypes": ["Egress"],
                "egress": [{
                    "to": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "kube-system"}}}],
                    "ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}],
                }],
            }
        }));
    }
    out
}

// ---- routes ---------------------------------------------------------------

fn err(code: StatusCode, e: impl Into<String>) -> Response {
    (code, Json(json!({"error": e.into()}))).into_response()
}

/// The apiserver refused: a 403 stays a 403, with its reason, because "you
/// may not" is the answer and a 502 says the console broke.
fn refused(status: reqwest::StatusCode, body: &Value) -> Response {
    let code = match status.as_u16() {
        403 => StatusCode::FORBIDDEN,
        404 => StatusCode::NOT_FOUND,
        409 => StatusCode::CONFLICT,
        422 | 400 => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_GATEWAY,
    };
    err(code, apiserver_says(status, body))
}

fn apiserver_says(status: reqwest::StatusCode, body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("apiserver returned {}", status.as_u16()))
}

/// Isolation per namespace, from the watched NetworkPolicies: `None` when
/// not isolated, `Some(dns)` when it is.
async fn isolation_state(inner: &Inner) -> BTreeMap<String, bool> {
    let pols = inner.store.kind("netpol").await;
    let mut out = BTreeMap::new();
    for key in pols.keys() {
        if let Some((ns, name)) = key.split_once('/') {
            if name == ISOLATE {
                out.entry(ns.to_string()).or_insert(false);
            }
        }
    }
    for key in pols.keys() {
        if let Some((ns, name)) = key.split_once('/') {
            if name == ISOLATE_DNS && out.contains_key(ns) {
                out.insert(ns.to_string(), true);
            }
        }
    }
    out
}

fn with_isolation(mut r: Value, iso: &BTreeMap<String, bool>) -> Value {
    let name = r["name"].as_str().unwrap_or_default().to_string();
    r["isolated"] = json!(iso.contains_key(&name));
    r["dns"] = json!(iso.get(&name).copied().unwrap_or(false));
    r
}

/// The viewer's projects, and — for somebody who may see them — the system
/// namespaces, separately.
pub(crate) async fn list(State(inner): State<Arc<Inner>>, viewer: Viewer) -> Response {
    let Some(client) = &inner.client else { return err(StatusCode::SERVICE_UNAVAILABLE, "no apiserver") };
    let iso = isolation_state(&inner).await;
    let token = viewer.token.as_deref();
    let (served, objects): (bool, Vec<Value>) = match client.get_as(&format!("{API}/projects"), token).await {
        Ok(v) => (true, v["items"].as_array().cloned().unwrap_or_default()),
        Err(crate::RkError::Status(s)) if s.as_u16() == 404 => {
            // A plain apiserver: namespaces, filtered the way the feed is.
            let hidden = inner.access.hidden(&viewer).await.map(|(h, _)| h).unwrap_or_default();
            let all = inner.store.kind("ns").await;
            (false, all.into_values().filter(|o| !hidden.contains(o["metadata"]["name"].as_str().unwrap_or(""))).collect())
        }
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("could not list projects: {e}")),
    };
    let mut projects = Vec::new();
    let mut system = Vec::new();
    for o in &objects {
        let name = o["metadata"]["name"].as_str().unwrap_or_default();
        if inner.access.is_system(name) {
            system.push(with_isolation(row(o, true, None), &iso));
        } else {
            projects.push(with_isolation(row(o, false, None), &iso));
        }
    }
    let by_name = |a: &Value, b: &Value| a["name"].as_str().cmp(&b["name"].as_str());
    projects.sort_by(by_name);
    system.sort_by(by_name);
    Json(json!({
        "served": served,
        "projects": projects,
        // Only for somebody who may change them; everybody else is never
        // offered one, so there is no reason to list them.
        "system": if viewer.has_role("admin") { json!(system) } else { json!([]) },
        "suggested": suggest(viewer.user.as_deref().unwrap_or("my"), "work"),
        "write": viewer.may_write(),
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
pub struct New {
    pub name: String,
    #[serde(default, rename = "displayName")]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
}

/// New project: a `ProjectRequest` as the viewer, so the requester is who
/// asked and they are its admin. On a plain apiserver, a Namespace carrying
/// the same annotations, naming the console user.
pub(crate) async fn create(State(inner): State<Arc<Inner>>, viewer: Viewer, Json(n): Json<New>) -> Response {
    let Some(client) = &inner.client else { return err(StatusCode::SERVICE_UNAVAILABLE, "no apiserver") };
    let name = n.name.trim().to_string();
    if let Err(e) = check_name(&name) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    if inner.access.is_system(&name) {
        return err(StatusCode::BAD_REQUEST, format!("{name} is a system namespace, not a project name"));
    }
    let token = viewer.token.as_deref();
    let mut req = json!({
        "apiVersion": "project.openshift.io/v1",
        "kind": "ProjectRequest",
        "metadata": {"name": name},
    });
    if !n.display_name.trim().is_empty() {
        req["displayName"] = json!(n.display_name.trim());
    }
    if !n.description.trim().is_empty() {
        req["description"] = json!(n.description.trim());
    }
    let (status, body) = match client.post_json_as(&format!("{API}/projectrequests"), &req, token).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_GATEWAY, e.to_string()),
    };
    if status.is_success() {
        inner.access.forget().await;
        return Json(json!({"message": format!("project {name} created"), "name": name})).into_response();
    }
    if status.as_u16() != 404 {
        return refused(status, &body);
    }
    // Not served: the namespace it would have made.
    let mut annotations = json!({ REQUESTER: viewer.user.clone().unwrap_or_else(|| "admin".into()) });
    if !n.display_name.trim().is_empty() {
        annotations[DISPLAY] = json!(n.display_name.trim());
    }
    if !n.description.trim().is_empty() {
        annotations[DESCRIPTION] = json!(n.description.trim());
    }
    let ns = json!({"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": name, "annotations": annotations}});
    match client.post_json_as("/api/v1/namespaces", &ns, token).await {
        Ok((s, _)) if s.is_success() => Json(json!({
            "message": format!("namespace {name} created — this apiserver serves no projects, so nobody was made its admin"),
            "name": name,
        }))
        .into_response(),
        Ok((s, b)) => refused(s, &b),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

/// Delete a project, and with it everything in it — the namespace
/// controller's cascade. System namespaces are refused here outright.
pub(crate) async fn remove(State(inner): State<Arc<Inner>>, viewer: Viewer, Path(name): Path<String>) -> Response {
    let Some(client) = &inner.client else { return err(StatusCode::SERVICE_UNAVAILABLE, "no apiserver") };
    if inner.access.is_system(&name) {
        return err(StatusCode::FORBIDDEN, format!("{name} is a system namespace and is not deleted from here"));
    }
    let token = viewer.token.as_deref();
    let status = match client.delete(&format!("{API}/projects/{name}"), token).await {
        Ok(s) if s.as_u16() == 404 => client.delete(&format!("/api/v1/namespaces/{name}"), token).await,
        other => other,
    };
    match status {
        Ok(s) if s.is_success() => {
            Json(json!({"message": format!("project {name} is being deleted, with everything in it")})).into_response()
        }
        Ok(s) if s.as_u16() == 404 => err(StatusCode::NOT_FOUND, format!("no project {name}")),
        Ok(s) if s.as_u16() == 403 => err(StatusCode::FORBIDDEN, format!("the apiserver says you may not delete {name}")),
        Ok(s) => err(StatusCode::BAD_GATEWAY, format!("apiserver returned {}", s.as_u16())),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

/// One project: its row, its members and whether the viewer may manage them.
pub(crate) async fn detail(State(inner): State<Arc<Inner>>, viewer: Viewer, Path(name): Path<String>) -> Response {
    let Some(client) = &inner.client else { return err(StatusCode::SERVICE_UNAVAILABLE, "no apiserver") };
    if let Some(r) = crate::refuse_hidden(&inner, &viewer, &name).await {
        return r;
    }
    let token = viewer.token.as_deref();
    let object = match client.get_as(&format!("{API}/projects/{name}"), token).await {
        Ok(v) => v,
        Err(_) => match inner.store.object("ns", &name).await {
            Some(v) => v,
            None => return err(StatusCode::NOT_FOUND, format!("no project {name}")),
        },
    };
    let iso = isolation_state(&inner).await;
    let (members, members_error) = match client.get_as(&format!("{RBAC}/namespaces/{name}/rolebindings"), token).await {
        Ok(b) => (members(&b), None),
        Err(e) => (vec![], Some(format!("could not read who is in {name}: {e}"))),
    };
    Json(json!({
        "project": with_isolation(row(&object, inner.access.is_system(&name), None), &iso),
        "members": members,
        "membersError": members_error,
        "roles": ROLES,
        "write": viewer.may_write(),
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
pub struct Member {
    pub who: String,
    pub role: String,
}

pub(crate) async fn add_member(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path(name): Path<String>,
    Json(m): Json<Member>,
) -> Response {
    let Some(client) = &inner.client else { return err(StatusCode::SERVICE_UNAVAILABLE, "no apiserver") };
    if let Some(r) = crate::refuse_hidden(&inner, &viewer, &name).await {
        return r;
    }
    let body = match binding(&name, &m.who, &m.role) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    match client.post_json_as(&format!("{RBAC}/namespaces/{name}/rolebindings"), &body, viewer.token.as_deref()).await {
        Ok((s, _)) if s.is_success() => {
            // Whoever was just let in should see it now, not in 30 seconds.
            inner.access.forget().await;
            Json(json!({"message": format!("{} is {} in {name}", m.who.trim(), m.role)})).into_response()
        }
        Ok((s, b)) => refused(s, &b),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

pub(crate) async fn remove_member(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((name, binding)): Path<(String, String)>,
) -> Response {
    let Some(client) = &inner.client else { return err(StatusCode::SERVICE_UNAVAILABLE, "no apiserver") };
    if let Some(r) = crate::refuse_hidden(&inner, &viewer, &name).await {
        return r;
    }
    match client.delete(&format!("{RBAC}/namespaces/{name}/rolebindings/{binding}"), viewer.token.as_deref()).await {
        Ok(s) if s.is_success() => {
            inner.access.forget().await;
            Json(json!({"message": format!("binding {binding} removed from {name}")})).into_response()
        }
        Ok(s) if s.as_u16() == 403 => err(StatusCode::FORBIDDEN, format!("the apiserver says you may not change who is in {name}")),
        Ok(s) => err(StatusCode::BAD_GATEWAY, format!("apiserver returned {}", s.as_u16())),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

#[derive(serde::Deserialize)]
pub struct Isolate {
    #[serde(default)]
    pub dns: bool,
}

/// Isolate a project: its pods and machines talk to each other and nothing
/// else. Re-isolating replaces the policies, so DNS can be turned on or off.
pub(crate) async fn isolate(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path(name): Path<String>,
    Json(i): Json<Isolate>,
) -> Response {
    let Some(client) = &inner.client else { return err(StatusCode::SERVICE_UNAVAILABLE, "no apiserver") };
    if let Some(r) = crate::refuse_hidden(&inner, &viewer, &name).await {
        return r;
    }
    if inner.access.is_system(&name) {
        return err(StatusCode::FORBIDDEN, format!("{name} is a system namespace; isolating it would cut the system off"));
    }
    let token = viewer.token.as_deref();
    let base = format!("{NETPOL}/namespaces/{name}/networkpolicies");
    // Replace, not add to: the DNS half comes and goes with the choice.
    for p in [ISOLATE, ISOLATE_DNS] {
        let _ = client.delete(&format!("{base}/{p}"), token).await;
    }
    for p in isolation(&name, i.dns) {
        match client.post_json_as(&base, &p, token).await {
            Ok((s, _)) if s.is_success() => {}
            Ok((s, b)) => return refused(s, &b),
            Err(e) => return err(StatusCode::BAD_GATEWAY, e.to_string()),
        }
    }
    Json(json!({"message": format!(
        "{name} is isolated: its pods and machines reach each other and nothing else{}. \
         Cilium enforces it; a VM is covered once it is on the pod network (stormvm#16)",
        if i.dns { ", and may resolve names through the cluster DNS" } else { " — not even DNS" }
    )}))
    .into_response()
}

pub(crate) async fn unisolate(State(inner): State<Arc<Inner>>, viewer: Viewer, Path(name): Path<String>) -> Response {
    let Some(client) = &inner.client else { return err(StatusCode::SERVICE_UNAVAILABLE, "no apiserver") };
    if let Some(r) = crate::refuse_hidden(&inner, &viewer, &name).await {
        return r;
    }
    let token = viewer.token.as_deref();
    let base = format!("{NETPOL}/namespaces/{name}/networkpolicies");
    let mut removed = 0;
    for p in [ISOLATE, ISOLATE_DNS] {
        if let Ok(s) = client.delete(&format!("{base}/{p}"), token).await {
            if s.is_success() {
                removed += 1;
            }
        }
    }
    if removed == 0 {
        return err(StatusCode::NOT_FOUND, format!("{name} is not isolated"));
    }
    Json(json!({"message": format!("{name} is no longer isolated")})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_row_carries_its_owner() {
        let ns = json!({"metadata": {"name": "gw-work", "annotations": {
            REQUESTER: "gw", DISPLAY: "GW's work", DESCRIPTION: ""}}, "status": {"phase": "Active"}});
        let r = row(&ns, false, None);
        assert_eq!(r["requester"], "gw");
        assert_eq!(r["displayName"], "GW's work");
        assert_eq!(r["description"], Value::Null, "an empty annotation is no description");
        assert_eq!(r["phase"], "Active");
    }

    #[test]
    fn names_and_suggestions() {
        assert!(check_name("gw-work").is_ok());
        assert!(check_name("GW").is_err());
        assert!(check_name("-x").is_err());
        assert_eq!(suggest("Glenn West", "vms"), "glenn-west-vms");
        assert_eq!(suggest("", "work"), "my-work");
    }

    #[test]
    fn members_are_bindings_to_project_roles_only() {
        let b = json!({"items": [
            {"metadata": {"name": "admin"}, "roleRef": {"kind": "ClusterRole", "name": "admin"},
             "subjects": [{"kind": "User", "name": "alice"}]},
            {"metadata": {"name": "view-bob"}, "roleRef": {"kind": "ClusterRole", "name": "view"},
             "subjects": [{"kind": "User", "name": "bob"}]},
            {"metadata": {"name": "sa"}, "roleRef": {"kind": "Role", "name": "pod-reader"},
             "subjects": [{"kind": "User", "name": "carol"}]},
        ]});
        let m = members(&b);
        assert_eq!(m.len(), 2, "a namespace Role is not project membership");
        assert_eq!(m[0]["name"], "alice");
        assert_eq!(m[0]["role"], "admin");
        assert_eq!(m[1]["role"], "view");
    }

    #[test]
    fn a_binding_is_rbacs_shape() {
        let b = binding("demo", "bob", "edit").unwrap();
        assert_eq!(b["metadata"]["name"], "edit-bob");
        assert_eq!(b["roleRef"]["name"], "edit");
        assert_eq!(b["subjects"][0]["kind"], "User");
        let sa = binding("demo", "system:serviceaccount:ci:runner", "view").unwrap();
        assert_eq!(sa["subjects"][0], json!({"kind": "ServiceAccount", "name": "runner", "namespace": "ci"}));
        assert!(binding("demo", "bob", "cluster-admin").unwrap_err().contains("not a project role"));
        assert!(binding("demo", " ", "view").is_err());
    }

    #[test]
    fn isolation_is_within_the_namespace_and_dns_is_opt_in() {
        let p = isolation("demo", false);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["metadata"]["name"], ISOLATE);
        assert_eq!(p[0]["spec"]["policyTypes"], json!(["Ingress", "Egress"]));
        assert_eq!(p[0]["spec"]["ingress"][0]["from"][0], json!({"podSelector": {}}));
        assert_eq!(p[0]["spec"]["egress"][0]["to"][0], json!({"podSelector": {}}));
        let d = isolation("demo", true);
        assert_eq!(d.len(), 2);
        assert_eq!(d[1]["spec"]["egress"][0]["ports"][0]["port"], 53);
    }

    #[test]
    fn system_namespaces_are_never_projects() {
        let extra = vec!["cilium".to_string()];
        for ns in ["default", "kube-system", "kube-node-lease", "openshift", "openshift-infra", "cilium"] {
            assert!(crate::authz::is_system_namespace(ns, &extra), "{ns}");
        }
        for ns in ["gw-work", "web", "defaults", "kubefoo"] {
            assert!(!crate::authz::is_system_namespace(ns, &extra), "{ns}");
        }
    }
}
