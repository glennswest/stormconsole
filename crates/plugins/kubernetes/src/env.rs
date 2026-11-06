//! A container's environment, resolved (#12): what the spec comes to, where `describe` prints references.
//!
//! `valueFrom` is most of what is interesting and none of it is in the spec. So a field reference is
//! read off the pod, a ConfigMap's value and an `envFrom`'s keys are read as the viewer, and later entries
//! override earlier as the kubelet orders them: `envFrom` in turn, then `env`.
//!
//! **A Secret's value is never read out**, here or into a `$(VAR)` that names it. A key is said to be
//! there or not. Whoever may exec can run `env`, and that goes through `pods/exec`, not this page.
//!
//! This is the spec's environment. The image's own `ENV` and the Service variables the node adds are not
//! in it, and the page says so.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use console_core::Viewer;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::Inner;

/// A ConfigMap's `data`, or a Secret's key names, or why it could not be read.
pub type Read = Result<BTreeMap<String, String>, String>;

/// One variable as it comes out.
#[derive(Debug, Clone, PartialEq)]
pub struct Var {
    pub name: String,
    /// `None` when it is not shown or has no value.
    pub value: Option<String>,
    pub source: String,
    pub hidden: bool,
    pub problem: Option<String>,
}

fn text<'a>(v: &'a Value, ptr: &str) -> &'a str {
    v.pointer(ptr).and_then(Value::as_str).unwrap_or("")
}

/// Which ConfigMaps and Secrets a container names.
pub fn references(container: &Value) -> (Vec<String>, Vec<String>) {
    let (mut maps, mut secrets) = (Vec::new(), Vec::new());
    let add = |list: &mut Vec<String>, n: &str| {
        if !n.is_empty() && !list.iter().any(|x| x == n) {
            list.push(n.to_string());
        }
    };
    for e in container.get("envFrom").and_then(Value::as_array).into_iter().flatten() {
        add(&mut maps, text(e, "/configMapRef/name"));
        add(&mut secrets, text(e, "/secretRef/name"));
    }
    for e in container.get("env").and_then(Value::as_array).into_iter().flatten() {
        add(&mut maps, text(e, "/valueFrom/configMapKeyRef/name"));
        add(&mut secrets, text(e, "/valueFrom/secretKeyRef/name"));
    }
    (maps, secrets)
}

/// `metadata.labels['x']` and the rest of what the downward API serves.
fn field(pod: &Value, path: &str) -> Option<String> {
    if let Some(key) = path.strip_prefix("metadata.labels['").and_then(|k| k.strip_suffix("']")) {
        return pod.pointer("/metadata/labels").and_then(|l| l.get(key)).and_then(Value::as_str).map(str::to_string);
    }
    if let Some(key) = path.strip_prefix("metadata.annotations['").and_then(|k| k.strip_suffix("']")) {
        return pod.pointer("/metadata/annotations").and_then(|l| l.get(key)).and_then(Value::as_str).map(str::to_string);
    }
    let plain = |p: &str| pod.pointer(p).and_then(Value::as_str).map(str::to_string);
    let ips = |p: &str| {
        pod.pointer(p).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.get("ip").and_then(Value::as_str)).collect::<Vec<_>>().join(","))
    };
    match path {
        "metadata.name" => plain("/metadata/name"),
        "metadata.namespace" => plain("/metadata/namespace"),
        "metadata.uid" => plain("/metadata/uid"),
        "spec.nodeName" => plain("/spec/nodeName"),
        "spec.serviceAccountName" => plain("/spec/serviceAccountName"),
        "status.hostIP" => plain("/status/hostIP"),
        "status.podIP" => plain("/status/podIP"),
        "status.podIPs" => ips("/status/podIPs"),
        "status.hostIPs" => ips("/status/hostIPs"),
        _ => None,
    }
}

/// `$(VAR)` from the variables before it; `$$(VAR)` is the text `$(VAR)`; a name with no shown value is
/// left as it is, which is also what the kubelet does with one it does not know.
fn expand(value: &str, known: &[Var]) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        if let Some(escaped) = after.strip_prefix("$(") {
            out.push_str("$(");
            rest = escaped;
        } else if let Some((name, tail)) = after.strip_prefix('(').and_then(|a| a.split_once(')')) {
            match known.iter().find(|v| v.name == name && !v.hidden).and_then(|v| v.value.as_deref()) {
                Some(v) => out.push_str(v),
                None => {
                    out.push_str("$(");
                    out.push_str(name);
                    out.push(')');
                }
            }
            rest = tail;
        } else {
            out.push('$');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn set(vars: &mut Vec<Var>, var: Var) {
    match vars.iter_mut().find(|v| v.name == var.name) {
        Some(old) => *old = var,
        None => vars.push(var),
    }
}

/// The container's variables, from the pod and what the viewer could read.
pub fn resolve(pod: &Value, container: &Value, maps: &BTreeMap<String, Read>, secrets: &BTreeMap<String, Read>) -> Vec<Var> {
    let mut vars: Vec<Var> = Vec::new();
    let optional = |r: &Value| r.get("optional").and_then(Value::as_bool).unwrap_or(false);
    for e in container.get("envFrom").and_then(Value::as_array).into_iter().flatten() {
        let prefix = text(e, "/prefix");
        let (from, word, secret) = if e.get("secretRef").is_some() {
            (text(e, "/secretRef/name"), "Secret", true)
        } else {
            (text(e, "/configMapRef/name"), "ConfigMap", false)
        };
        let read = if secret { secrets.get(from) } else { maps.get(from) };
        match read {
            Some(Ok(data)) => {
                for (k, v) in data {
                    set(&mut vars, Var {
                        name: format!("{prefix}{k}"),
                        value: (!secret).then(|| v.clone()),
                        source: format!("every key of {word} {from}"),
                        hidden: secret,
                        problem: None,
                    });
                }
            }
            Some(Err(why)) => set(&mut vars, Var {
                name: format!("{prefix}*"),
                value: None,
                source: format!("every key of {word} {from}"),
                hidden: secret,
                problem: Some(why.clone()),
            }),
            None => {}
        }
    }
    for e in container.get("env").and_then(Value::as_array).into_iter().flatten() {
        let name = text(e, "/name").to_string();
        let from = e.get("valueFrom");
        let reference = |p: &str| from.and_then(|f| f.pointer(p));
        let key_of = |read: Option<&Read>, r: &Value, what: &str| -> (Option<String>, Option<String>) {
            let (n, k) = (text(r, "/name"), text(r, "/key"));
            match read {
                Some(Ok(d)) => match d.get(k) {
                    Some(v) => (Some(v.clone()), None),
                    None if optional(r) => (None, Some(format!("no key {k}: optional, so not set"))),
                    None => (None, Some(format!("{what} {n} has no key {k}: the container will not start"))),
                },
                Some(Err(why)) => (None, Some(why.clone())),
                None => (None, Some(format!("{what} {n} was not read"))),
            }
        };
        let var = if let Some(r) = reference("/secretKeyRef") {
            let (_, problem) = key_of(secrets.get(text(r, "/name")), r, "Secret");
            Var { name, value: None, source: format!("Secret {} key {}", text(r, "/name"), text(r, "/key")), hidden: true, problem }
        } else if let Some(r) = reference("/configMapKeyRef") {
            let (value, problem) = key_of(maps.get(text(r, "/name")), r, "ConfigMap");
            Var { name, value, source: format!("ConfigMap {} key {}", text(r, "/name"), text(r, "/key")), hidden: false, problem }
        } else if let Some(r) = reference("/fieldRef") {
            let path = text(r, "/fieldPath");
            let value = field(pod, path);
            let problem = value.is_none().then(|| "not set on this pod yet".to_string());
            Var { name, value, source: format!("field {path}"), hidden: false, problem }
        } else if let Some(r) = reference("/resourceFieldRef") {
            let res = text(r, "/resource");
            let value = container.pointer(&format!("/resources/{}", res.replace('.', "/"))).and_then(Value::as_str).map(str::to_string);
            let divisor = r.get("divisor").and_then(Value::as_str).filter(|d| !d.is_empty() && *d != "0" && *d != "1");
            let source = match divisor {
                Some(d) => format!("resource {res}, in units of {d}"),
                None => format!("resource {res}"),
            };
            let problem = value.is_none().then(|| "not set, so the node's allocatable".to_string());
            Var { name, value, source, hidden: false, problem }
        } else {
            let raw = e.get("value").and_then(Value::as_str).unwrap_or("");
            Var { name, value: Some(expand(raw, &vars)), source: "the spec".to_string(), hidden: false, problem: None }
        };
        set(&mut vars, var);
    }
    vars
}

#[derive(Deserialize)]
pub struct EnvQuery {
    container: String,
}

/// `GET /pods/{ns}/{name}/env?container=`
pub async fn env(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
    Query(q): Query<EnvQuery>,
) -> Response {
    let pod = match crate::podpage::the_pod(&inner, &viewer, &ns, &name).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let found = ["containers", "initContainers", "ephemeralContainers"].iter().find_map(|k| {
        pod.pointer(&format!("/spec/{k}"))?.as_array()?.iter().find(|c| c.get("name").and_then(Value::as_str) == Some(q.container.as_str()))
    });
    let Some(container) = found else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("no container {:?} in this pod", q.container)}))).into_response();
    };
    let (map_names, secret_names) = references(container);
    let mut maps = BTreeMap::new();
    for n in map_names {
        let read = read_object(&inner, &ns, "configmaps", &n, viewer.token.as_deref()).await;
        maps.insert(n, read);
    }
    // Asked only as the person, or root: with no identity the console's own credential would read a Secret.
    let mut secrets = BTreeMap::new();
    for n in secret_names {
        let read = if viewer.may_act_upstream() {
            read_object(&inner, &ns, "secrets", &n, viewer.token.as_deref()).await
        } else {
            Err("not asked: this sign-in has no kubernetes identity".to_string())
        };
        secrets.insert(n, read);
    }
    let vars: Vec<Value> = resolve(&pod, container, &maps, &secrets)
        .into_iter()
        .map(|v| json!({"name": v.name, "value": v.value, "source": v.source, "hidden": v.hidden, "problem": v.problem}))
        .collect();
    Json(json!({"container": q.container, "vars": vars})).into_response()
}

/// An object's `data`, with a Secret's values gone before anything else can see them.
async fn read_object(inner: &Inner, ns: &str, kind: &str, name: &str, token: Option<&str>) -> Read {
    let Some(client) = &inner.client else { return Err("no apiserver".to_string()) };
    let v = client.get_as(&format!("/api/v1/namespaces/{ns}/{kind}/{name}"), token).await.map_err(|e| format!("could not be read: {e}"))?;
    let secret = kind == "secrets";
    Ok(v.get("data")
        .and_then(Value::as_object)
        .map(|d| d.iter().map(|(k, x)| (k.clone(), if secret { String::new() } else { x.as_str().unwrap_or("").to_string() })).collect())
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found<'a>(vars: &'a [Var], name: &str) -> &'a Var {
        vars.iter().find(|v| v.name == name).unwrap_or_else(|| panic!("no {name} in {vars:?}"))
    }

    fn data(pairs: &[(&str, &str)]) -> Read {
        Ok(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
    }

    fn pod() -> Value {
        json!({
            "metadata": {"name": "web", "namespace": "shop", "uid": "u1", "labels": {"app": "x"}},
            "spec": {"nodeName": "n1"},
            "status": {"podIP": "10.0.0.1"}
        })
    }

    #[test]
    fn what_the_spec_comes_to() {
        let c = json!({
            "resources": {"limits": {"cpu": "2"}},
            "envFrom": [{"configMapRef": {"name": "conf"}, "prefix": "C_"}, {"secretRef": {"name": "db"}}],
            "env": [
                {"name": "PLAIN", "value": "1"},
                {"name": "NODE", "valueFrom": {"fieldRef": {"fieldPath": "spec.nodeName"}}},
                {"name": "APP", "valueFrom": {"fieldRef": {"fieldPath": "metadata.labels['app']"}}},
                {"name": "CPUS", "valueFrom": {"resourceFieldRef": {"resource": "limits.cpu"}}},
                {"name": "MEM", "valueFrom": {"resourceFieldRef": {"resource": "limits.memory"}}},
                {"name": "URL", "valueFrom": {"configMapKeyRef": {"name": "conf", "key": "url"}}},
                {"name": "PW", "valueFrom": {"secretKeyRef": {"name": "db", "key": "password"}}},
                {"name": "GONE", "valueFrom": {"configMapKeyRef": {"name": "conf", "key": "gone"}}},
                {"name": "MAYBE", "valueFrom": {"configMapKeyRef": {"name": "conf", "key": "gone", "optional": true}}},
                {"name": "JOINED", "value": "$(NODE)-$(PW)-$$(NODE)-$(NOPE)"}
            ]
        });
        let maps = BTreeMap::from([("conf".to_string(), data(&[("url", "http://x")]))]);
        let secrets = BTreeMap::from([("db".to_string(), data(&[("password", ""), ("user", "")]))]);
        let v = resolve(&pod(), &c, &maps, &secrets);
        assert_eq!(found(&v, "PLAIN").value.as_deref(), Some("1"));
        assert_eq!(found(&v, "NODE").value.as_deref(), Some("n1"));
        assert_eq!(found(&v, "APP").value.as_deref(), Some("x"));
        assert_eq!(found(&v, "CPUS").value.as_deref(), Some("2"));
        assert!(found(&v, "MEM").problem.as_deref().unwrap().contains("allocatable"));
        assert_eq!(found(&v, "URL").value.as_deref(), Some("http://x"));
        assert_eq!(found(&v, "C_url").value.as_deref(), Some("http://x"), "envFrom with its prefix");
        assert_eq!(found(&v, "user").source, "every key of Secret db");
        assert!(found(&v, "GONE").problem.as_deref().unwrap().contains("will not start"));
        assert!(found(&v, "MAYBE").problem.as_deref().unwrap().contains("optional"));
        assert_eq!(found(&v, "JOINED").value.as_deref(), Some("n1-$(PW)-$(NODE)-$(NOPE)"), "a Secret never into another variable");
    }

    #[test]
    fn a_secret_is_a_name_and_a_key() {
        let c = json!({"envFrom": [{"secretRef": {"name": "db"}}],
            "env": [{"name": "PW", "valueFrom": {"secretKeyRef": {"name": "db", "key": "password"}}}]});
        let secrets = BTreeMap::from([("db".to_string(), data(&[("password", "hunter2"), ("user", "root")]))]);
        for v in resolve(&pod(), &c, &BTreeMap::new(), &secrets) {
            assert_eq!(v.value, None, "{v:?}");
            assert!(v.hidden);
        }
        let refused = BTreeMap::from([("db".to_string(), Err::<BTreeMap<String, String>, _>("could not be read: 403".to_string()))]);
        let v = resolve(&pod(), &c, &BTreeMap::new(), &refused);
        assert!(found(&v, "PW").problem.as_deref().unwrap().contains("403"));
        assert!(found(&v, "*").problem.is_some(), "what envFrom would have held is said to be unread");
    }

    #[test]
    fn later_overrides_earlier() {
        let c = json!({"envFrom": [{"configMapRef": {"name": "a"}}, {"configMapRef": {"name": "b"}}], "env": [{"name": "Z", "value": "env"}]});
        let maps = BTreeMap::from([
            ("a".to_string(), data(&[("X", "a"), ("Z", "a")])),
            ("b".to_string(), data(&[("X", "b")])),
        ]);
        let v = resolve(&pod(), &c, &maps, &BTreeMap::new());
        assert_eq!(found(&v, "X").value.as_deref(), Some("b"));
        assert_eq!(found(&v, "Z").value.as_deref(), Some("env"));
    }

    #[test]
    fn what_a_container_names() {
        let c = json!({"envFrom": [{"configMapRef": {"name": "a"}}, {"secretRef": {"name": "s"}}],
            "env": [{"name": "X", "valueFrom": {"configMapKeyRef": {"name": "a", "key": "k"}}}, {"name": "Y", "valueFrom": {"secretKeyRef": {"name": "t", "key": "k"}}}]});
        assert_eq!(references(&c), (vec!["a".to_string()], vec!["s".to_string(), "t".to_string()]));
    }
}
