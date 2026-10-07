//! The lifecycle as objects (#88, stormcluster#12).
//!
//! stormcluster's HTTP writes are gone: a cluster is formed, joined,
//! promoted, demoted, drained, given storage and released by writing
//! `cluster.storm.io/v1alpha1` `Cluster` and `ClusterMember` objects to the
//! apiserver, and stormcluster reconciles them. So the console writes them
//! **as the viewer**, like every other kube write here — the apiserver's
//! RBAC decides who may change what the cluster is made of, not a role
//! check in this plugin — and reads both kinds back through a watch.
//!
//! What a write means, from stormcluster's `docs/api.md`:
//! - `Cluster <name>` on an SNO's apiserver forms a cluster **seeded on that
//!   node**; the `ClusterMember`s already there join in the same operation,
//!   so a form writes the members first and the `Cluster` last. Deleting it
//!   dissolves the cluster.
//! - `ClusterMember <node>`: `spec.role` joins, promotes or demotes;
//!   `spec.drain` drains or uncordons; `spec.storage` serves the node's
//!   drives; deleting it **releases** the node — drained, its data erased,
//!   a new SNO. There is no keeping the data (stormcluster#14).
//! - Refusals are status (`blockers[]`, phase `Blocked`), planned again on
//!   every pass; a failed operation resumes by itself with a growing pause.
//!   Nothing here resumes anything.

use serde_json::{json, Value};

use plugin_kubernetes::ResourceSpec;

pub const API: &str = "/apis/cluster.storm.io/v1alpha1";

/// Both kinds, cluster-scoped, optional: a node whose apiserver does not
/// serve them yet (stormcluster installs them itself, `kube.install_crds`)
/// has a page that says so rather than a failed watch.
pub const RESOURCES: &[ResourceSpec] = &[
    ResourceSpec {
        kind: "scluster",
        api_kind: "Cluster",
        title: "Clusters",
        list_path: "/apis/cluster.storm.io/v1alpha1/clusters",
        namespaced: false,
        optional: true,
        inventory: false,
    },
    ResourceSpec {
        kind: "smember",
        api_kind: "ClusterMember",
        title: "Cluster members",
        list_path: "/apis/cluster.storm.io/v1alpha1/clustermembers",
        namespaced: false,
        optional: true,
        inventory: false,
    },
];

/// A node or cluster name the apiserver will take (DNS-1123 subdomain).
pub fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 253
        && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        && n.starts_with(|c: char| c.is_ascii_alphanumeric())
        && n.ends_with(|c: char| c.is_ascii_alphanumeric())
}

fn role_ok(r: &str) -> Result<(), String> {
    match r {
        "master" | "worker" => Ok(()),
        other => Err(format!("{other:?} is not a role: master or worker")),
    }
}

pub fn member(name: &str, role: &str, storage: bool) -> Value {
    let mut spec = json!({"role": role});
    if storage {
        spec["storage"] = json!(true);
    }
    json!({
        "apiVersion": "cluster.storm.io/v1alpha1",
        "kind": "ClusterMember",
        "metadata": {"name": name},
        "spec": spec,
    })
}

pub fn cluster(name: &str, forge: bool) -> Value {
    let mut o = json!({
        "apiVersion": "cluster.storm.io/v1alpha1",
        "kind": "Cluster",
        "metadata": {"name": name},
    });
    if forge {
        o["spec"] = json!({"forge": true});
    }
    o
}

/// `POST /form`'s body, which is also stormcluster's dry-run request.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default)]
pub struct Form {
    pub name: String,
    #[serde(default)]
    pub masters: Vec<String>,
    #[serde(default)]
    pub workers: Vec<String>,
    #[serde(default)]
    pub storage: Vec<String>,
    #[serde(default)]
    pub forge: bool,
}

/// The objects a form writes, in order: every member, then the `Cluster`.
/// `seed` is the node whose apiserver this is — the one a `Cluster` here
/// seeds on — so it must be one of the masters.
pub fn form_objects(f: &Form, seed: &str) -> Result<Vec<Value>, String> {
    let name = f.name.trim();
    if !valid_name(name) {
        return Err(format!("{name:?} is not a name the apiserver takes: lowercase letters, digits, '-' and '.'"));
    }
    if !f.masters.iter().any(|m| m == seed) {
        return Err(format!(
            "a Cluster here is seeded on {seed}, the node whose apiserver holds it: {seed} must be one of the masters"
        ));
    }
    if let Some(n) = f.masters.iter().find(|m| f.workers.contains(m)) {
        return Err(format!("{n} is asked to be both a master and a worker"));
    }
    let mut out = Vec::new();
    for (role, nodes) in [("master", &f.masters), ("worker", &f.workers)] {
        for n in nodes {
            if !valid_name(n) {
                return Err(format!("{n:?} is not a node name"));
            }
            out.push(member(n, role, f.storage.contains(n)));
        }
    }
    out.push(cluster(name, f.forge));
    Ok(out)
}

/// `POST /members`: join these nodes with this role.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Join {
    pub nodes: Vec<String>,
    pub role: String,
    #[serde(default)]
    pub storage: bool,
}

pub fn join_objects(j: &Join) -> Result<Vec<Value>, String> {
    role_ok(&j.role)?;
    if j.nodes.is_empty() {
        return Err("name the nodes to join".into());
    }
    j.nodes
        .iter()
        .map(|n| if valid_name(n) { Ok(member(n, &j.role, j.storage)) } else { Err(format!("{n:?} is not a node name")) })
        .collect()
}

/// `PATCH /members/{node}`: one change to a member's spec.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct Change {
    pub role: Option<String>,
    pub drain: Option<bool>,
    pub storage: Option<bool>,
}

pub fn change_patch(c: &Change) -> Result<Value, String> {
    let mut spec = serde_json::Map::new();
    if let Some(r) = &c.role {
        role_ok(r)?;
        spec.insert("role".into(), json!(r));
    }
    if let Some(d) = c.drain {
        spec.insert("drain".into(), json!(d));
    }
    if let Some(s) = c.storage {
        spec.insert("storage".into(), json!(s));
    }
    if spec.is_empty() {
        return Err("nothing to change: role, drain or storage".into());
    }
    Ok(json!({"spec": spec}))
}

/// The dry-run request a change is previewed with — stormcluster's
/// `POST /api/v1/plan` body for the same thing.
pub fn change_plan(node: &str, c: &Change, current_role: Option<&str>) -> Option<Value> {
    if let Some(r) = &c.role {
        return Some(match (r.as_str(), current_role) {
            ("master", Some("worker")) => json!({"op": "promote", "nodes": [node]}),
            ("worker", Some("master")) => json!({"op": "demote", "node": node}),
            (role, _) => json!({"op": "join", "nodes": [node], "role": role}),
        });
    }
    if let Some(d) = c.drain {
        return Some(json!({"op": if d { "drain" } else { "uncordon" }, "node": node}));
    }
    c.storage.map(|s| json!({"op": "storage", "node": node, "storage": s}))
}

/// What the page reads of an object: its name, spec, status, and whether
/// a delete is under way (the finalizer holds it until the release ends).
pub fn summary(o: &Value) -> Value {
    json!({
        "name": o.pointer("/metadata/name"),
        "spec": o.get("spec").cloned().unwrap_or(json!({})),
        "status": o.get("status").cloned().unwrap_or(json!({})),
        "deleting": o.pointer("/metadata/deletionTimestamp").is_some(),
    })
}

/// stormcluster's dry-run answer, for the page: each step given its
/// `descriptions[i]` (stormcluster's own sentence — the copied
/// `Step::describe` is gone with stormcluster#11), and a refusal's reasons
/// carried as `error` too, so any button says them.
pub fn plan_answer(ok: bool, mut v: Value) -> Value {
    // Forwarded to the coordinator: the coordinator's answer is the answer.
    if let Some(o) = v.as_object_mut() {
        if o.contains_key("coordinator") && o.contains_key("response") {
            let coordinator = o.remove("coordinator").unwrap_or(Value::Null);
            let mut inner = o.remove("response").unwrap_or(json!({}));
            if let Some(io) = inner.as_object_mut() {
                io.insert("coordinator".into(), coordinator);
            }
            v = inner;
        }
    }
    let Some(o) = v.as_object_mut() else { return v };
    let descriptions: Vec<Value> = o.get("descriptions").and_then(Value::as_array).cloned().unwrap_or_default();
    if let Some(Value::Array(steps)) = o.get_mut("plan").and_then(|p| p.get_mut("steps")) {
        for (i, s) in steps.iter_mut().enumerate() {
            if let Value::Object(so) = s {
                if !so.contains_key("description") {
                    let d = descriptions.get(i).cloned().unwrap_or_else(|| {
                        // Never dropped: a step without a sentence is named.
                        json!(so.get("step").and_then(Value::as_str).unwrap_or("a step"))
                    });
                    so.insert("description".into(), d);
                }
            }
        }
    }
    if let Some(reasons) = o.get("refused").and_then(Value::as_array) {
        if !o.contains_key("error") {
            let said: Vec<&str> = reasons.iter().filter_map(Value::as_str).collect();
            let mut e = format!("refused: {}", said.join("; "));
            if let Some(c) = o.get("coordinator").and_then(Value::as_str) {
                e.push_str(&format!(" (by {c}, which coordinates this)"));
            }
            o.insert("error".into(), json!(e));
        }
    }
    if !ok && !o.contains_key("error") {
        o.insert("error".into(), json!("stormcluster could not plan this"));
    }
    v
}
