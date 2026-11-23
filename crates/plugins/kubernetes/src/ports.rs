//! A node's network ports (#336, stormcos#959), from stormpump's `NetworkPort` objects (stormpump#236).
//!
//! One object per physical port, cluster-scoped, named `<node>-<ifname>`. `status` is the port as the
//! node reads it (link, speed, FEC, the module, driver and firmware, the last change, the counters, an
//! `Applied` condition); `spec` is what is wanted, applied live by the node's PID 1, the port's one
//! writer. Today PID 1 applies `fec` only; the rest is accepted and said `not applied yet`
//! (stormpump#246).
//!
//! - **Read** from the watch cache, on the console's own credential (`networkports` get/list/watch,
//!   the rule stormcos copies from `deploy/rbac/stormconsole.yaml`).
//! - **A change is a merge patch of `spec`, as the signed-in person** (admin on this console, and
//!   `patch networkports` at the apiserver), never the console's credential. The values are checked
//!   against the CRD's own schema first, so a typo is refused here in words and not as a 422.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use console_core::{ComponentSummary, Health, Metric, Relation, Viewer};
use serde_json::{json, Map, Value};

use crate::client::{As, NO_IDENTITY};
use crate::Inner;

/// The cache's word for the kind (`cache::RESOURCES`).
pub const KIND: &str = "np";
pub const COLLECTION: &str = "/apis/network.storm.io/v1alpha1/networkports";

/// stormpump#236 `deploy/crds.yaml` `spec.fec`.
pub const FEC_MODES: &[&str] = &["rs", "baser", "off", "llrs", "auto"];

/// The settings PID 1 applies today (stormpump `docs/ports.md`). The rest are accepted and reported.
pub const APPLIED_TODAY: &[&str] = &["fec"];
pub const NOT_APPLIED_ISSUE: &str = "stormpump#246";

fn s<'a>(v: &'a Value, p: &str) -> Option<&'a str> {
    v.pointer(p).and_then(Value::as_str)
}

/// The object's `Applied` condition, as `{status, reason, message}`.
pub fn applied(o: &Value) -> Value {
    o.pointer("/status/conditions")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|c| c.get("type").and_then(Value::as_str) == Some("Applied")))
        .map(|c| json!({"status": c.get("status"), "reason": c.get("reason"), "message": c.get("message")}))
        .unwrap_or_else(|| json!({"status": null, "reason": null, "message": "the node has not reported this port yet"}))
}

/// The settings `spec` asks for, without the port's identity.
pub fn wanted(o: &Value) -> Value {
    let mut m = o.get("spec").and_then(Value::as_object).cloned().unwrap_or_default();
    for k in ["node", "ifname", "mac"] {
        m.remove(k);
    }
    m.retain(|_, v| !v.is_null());
    Value::Object(m)
}

/// How the port is: a word, and its health. A port down that the node has is a warning, not an
/// error: an unused port is down on purpose.
pub fn state(o: &Value) -> (Health, String) {
    let st = o.get("status");
    if st.is_none_or(Value::is_null) {
        return (Health::Unknown, "not reported yet".into());
    }
    if o.pointer("/status/present").and_then(Value::as_bool) == Some(false) {
        return (Health::Error, "gone from the node".into());
    }
    let a = applied(o);
    if a["status"] == "False" && a["reason"] == "Refused" {
        return (Health::Error, "spec refused".to_string());
    }
    match s(o, "/status/link") {
        Some("up") => (Health::Ok, "up".into()),
        Some("down") => (Health::Warn, "down".into()),
        _ => (Health::Unknown, "link unknown".into()),
    }
}

/// `25000` Mb/s → `25G`.
pub fn speed_word(mbps: Option<i64>) -> String {
    match mbps {
        Some(m) if m >= 1000 && m % 1000 == 0 => format!("{}G", m / 1000),
        Some(m) if m > 0 => format!("{m}M"),
        _ => "—".into(),
    }
}

/// One port as the page shows it.
pub fn row(o: &Value) -> Value {
    let (health, word) = state(o);
    let status = o.get("status").cloned().unwrap_or(Value::Null);
    let generation = o.pointer("/metadata/generation").and_then(Value::as_i64);
    let observed = o.pointer("/status/observedGeneration").and_then(Value::as_i64);
    json!({
        "name": o.pointer("/metadata/name"),
        "node": o.pointer("/spec/node"),
        "ifname": o.pointer("/spec/ifname"),
        "mac": o.pointer("/spec/mac"),
        "health": health,
        "state": word,
        "present": status.get("present"),
        "link": status.get("link"),
        "speed": status.get("speed"),
        "supportedSpeed": status.get("supportedSpeed"),
        "speedWord": speed_word(status.get("speed").and_then(Value::as_i64)),
        "fec": status.get("fec").cloned().unwrap_or_else(|| json!({})),
        "pause": status.get("pause"),
        "module": status.get("module"),
        "driver": status.get("driver"),
        "lastChange": status.get("lastChange"),
        "counters": status.get("counters").cloned().unwrap_or_else(|| json!({})),
        "applied": applied(o),
        "spec": wanted(o),
        "generation": generation,
        "observedGeneration": observed,
        // The node has not read the latest spec yet.
        "pending": generation.is_some() && observed != generation,
    })
}

/// A node's ports, by interface name.
pub fn rows_for<'a>(objects: impl Iterator<Item = &'a Value>, node: &str) -> Vec<Value> {
    let mut out: Vec<&Value> = objects.filter(|o| s(o, "/spec/node") == Some(node)).collect();
    out.sort_by_key(|o| s(o, "/spec/ifname").unwrap_or_default().to_string());
    out.into_iter().map(row).collect()
}

/// The `spec` merge patch for a change, checked against the CRD's schema (stormpump#236
/// `deploy/crds.yaml`). A key set to `null` takes the setting out: the port is left as it has it.
pub fn spec_patch(change: &Value) -> Result<Value, String> {
    let Some(change) = change.as_object() else { return Err("a change is an object of settings".into()) };
    if change.is_empty() {
        return Err("nothing to change".into());
    }
    let int = |k: &str, v: &Value, lo: i64, hi: i64| -> Result<(), String> {
        match v.as_i64() {
            Some(n) if (lo..=hi).contains(&n) => Ok(()),
            _ => Err(format!("{k}: a whole number from {lo} to {hi}")),
        }
    };
    let pair = |k: &str, v: &Value, check: &dyn Fn(&str, &Value) -> Result<(), String>| -> Result<(), String> {
        let Some(m) = v.as_object() else { return Err(format!("{k}: {{rx, tx}}")) };
        for (side, x) in m {
            if !matches!(side.as_str(), "rx" | "tx") {
                return Err(format!("{k}.{side}: only rx and tx"));
            }
            if !x.is_null() {
                check(&format!("{k}.{side}"), x)?;
            }
        }
        Ok(())
    };
    let boolean = |k: &str, v: &Value| if v.is_boolean() { Ok(()) } else { Err(format!("{k}: on or off")) };
    let mut spec = Map::new();
    for (k, v) in change {
        if !v.is_null() {
            match k.as_str() {
                "fec" => {
                    if !v.as_str().is_some_and(|f| FEC_MODES.contains(&f)) {
                        return Err(format!("fec: one of {}", FEC_MODES.join(", ")));
                    }
                }
                "autoneg" => boolean(k, v)?,
                "speed" => int(k, v, 10, 1_600_000)?,
                "mtu" => int(k, v, 68, 65535)?,
                "pause" => pair(k, v, &boolean)?,
                "rings" => pair(k, v, &|k: &str, v: &Value| int(k, v, 1, 1 << 20))?,
                "offloads" => {
                    let Some(m) = v.as_object() else { return Err("offloads: feature name to on or off".into()) };
                    for (f, x) in m {
                        let named = !f.is_empty() && f.len() <= 64 && f.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
                        if !named {
                            return Err(format!("offloads: {f:?} is not a feature name"));
                        }
                        if !x.is_null() {
                            boolean(&format!("offloads.{f}"), x)?;
                        }
                    }
                }
                "node" | "ifname" | "mac" => return Err(format!("{k} is the port's identity, written by the node")),
                other => return Err(format!("{other:?} is not a setting of a NetworkPort")),
            }
        } else if matches!(k.as_str(), "node" | "ifname" | "mac") {
            return Err(format!("{k} is the port's identity, written by the node"));
        } else if !matches!(k.as_str(), "fec" | "autoneg" | "speed" | "mtu" | "pause" | "rings" | "offloads") {
            return Err(format!("{k:?} is not a setting of a NetworkPort"));
        }
        spec.insert(k.clone(), v.clone());
    }
    Ok(json!({"spec": spec}))
}

/// Why this viewer may not change a port, or `None` when it is the apiserver's to say.
pub fn refusal(admin: bool, identity: bool) -> Option<&'static str> {
    if !admin {
        return Some("Changing a port's settings needs the admin role on this console.");
    }
    if !identity {
        return Some("A port's settings are changed as you, and this sign-in has no kubernetes identity. Sign in as a user with one.");
    }
    None
}

/// A row per port in the feed (`k8s:np:<name>`), so a port that goes down or whose spec is refused
/// is seen without opening its node.
pub fn components(of_np: &HashMap<String, Value>, of_node: &HashMap<String, Value>) -> Vec<ComponentSummary> {
    let mut out = Vec::new();
    for (key, o) in of_np {
        let (health, word) = state(o);
        let node = s(o, "/spec/node").unwrap_or_default();
        let ifname = s(o, "/spec/ifname").unwrap_or(key.as_str());
        let speed = speed_word(o.pointer("/status/speed").and_then(Value::as_i64));
        let fec = s(o, "/status/fec/active").unwrap_or("—");
        let mut c = ComponentSummary {
            id: format!("k8s:{KIND}:{key}"),
            kind: format!("k8s-{KIND}"),
            label: format!("{node} {ifname}"),
            health,
            detail: format!("{word} · {speed} · fec {fec}"),
            metrics: vec![],
            actions: vec![],
            relations: vec![],
            link: Some(format!("#/node/{node}/network?port={key}")),
        };
        c.metrics.push(Metric::new("link", word));
        c.metrics.push(Metric::new("speed", speed));
        c.metrics.push(Metric::new("fec", fec));
        if let Some(part) = s(o, "/status/module/part") {
            c.metrics.push(Metric::new("module", part).tone("muted"));
        }
        let a = applied(o);
        if let Some(r) = a["reason"].as_str() {
            c.metrics.push(Metric::new("applied", r).tone("muted"));
        }
        if of_node.contains_key(node) {
            c.relations.push(Relation::belongs_to("node", format!("k8s:node:{node}")));
        }
        out.push(c);
    }
    out
}

/// One setting of a change, in words: `fec rs`, `mtu left to the node`.
fn said(k: &str, v: &Value) -> String {
    match v {
        Value::Null => format!("{k} left to the node"),
        Value::String(s) => format!("{k} {s}"),
        v => format!("{k} {v}"),
    }
}

fn answer(code: StatusCode, why: impl Into<String>) -> Response {
    (code, Json(json!({"error": why.into()}))).into_response()
}

/// `GET /nodes/{name}/ports`: the node's ports, and what this viewer may do about them.
pub async fn list(State(inner): State<Arc<Inner>>, viewer: Viewer, Path(node): Path<String>) -> Response {
    let why = refusal(viewer.has_role("admin"), viewer.may_act_upstream());
    let base = json!({
        "node": node,
        "may": why.is_none(),
        "why": why,
        "fecModes": FEC_MODES,
        "appliedToday": APPLIED_TODAY,
        "notAppliedIssue": NOT_APPLIED_ISSUE,
    });
    let with = |mut v: Value, k: &str, x: Value| {
        v[k] = x;
        v
    };
    if inner.client.is_none() {
        return Json(with(with(base, "ports", json!([])), "note", json!("no apiserver"))).into_response();
    }
    match inner.store.kind_if_present(KIND).await {
        Some(objs) => {
            let rows = rows_for(objs.values(), &node);
            let note = if rows.is_empty() {
                json!(format!("{node} has no NetworkPort objects: its stormport has not made them, or is not running"))
            } else {
                Value::Null
            };
            Json(with(with(base, "ports", json!(rows)), "note", note)).into_response()
        }
        None => {
            let note = if inner.store.is_absent(KIND).await {
                "the apiserver does not serve NetworkPort (network.storm.io): the release has no stormport yet"
            } else {
                "the ports have not been read yet"
            };
            Json(with(with(base, "ports", json!([])), "note", json!(note))).into_response()
        }
    }
}

/// `PATCH /ports/{name}` with the settings to change (`{"fec": "rs"}`; `null` takes one out).
pub async fn change(State(inner): State<Arc<Inner>>, viewer: Viewer, Path(name): Path<String>, Json(body): Json<Value>) -> Response {
    if let Some(why) = refusal(viewer.has_role("admin"), viewer.may_act_upstream()) {
        return answer(StatusCode::FORBIDDEN, why);
    }
    let patch = match spec_patch(&body) {
        Ok(p) => p,
        Err(why) => return answer(StatusCode::BAD_REQUEST, why),
    };
    if inner.store.object(KIND, &name).await.is_none() {
        return answer(StatusCode::NOT_FOUND, format!("no NetworkPort {name}"));
    }
    let Some(client) = &inner.client else { return answer(StatusCode::SERVICE_UNAVAILABLE, "no apiserver") };
    let Some(who) = As::own(&viewer) else { return answer(StatusCode::FORBIDDEN, NO_IDENTITY) };
    match client.patch_merge(&format!("{COLLECTION}/{name}"), &patch, who).await {
        Ok((status, saved)) if status.is_success() => {
            // The page re-reads at once; the watch would catch up a moment later.
            inner.store.observe(KIND, saved.clone()).await;
            let said: Vec<String> = patch["spec"]
                .as_object()
                .map(|m| m.iter().map(|(k, v)| said(k, v)).collect())
                .unwrap_or_default();
            Json(json!({"message": format!("{name}: {}", said.join(", ")), "port": row(&saved)})).into_response()
        }
        Ok((status, body)) => {
            let code = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let said = body.get("message").and_then(Value::as_str).map_or_else(|| format!("the apiserver answered {status}"), str::to_string);
            answer(code, said)
        }
        Err(e) => answer(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As stormport `status_of` writes it (stormpump 43b3e23), for forge1's 25G port.
    fn port(link: &str, fec_active: &str, reason: &str) -> Value {
        json!({
            "apiVersion": "network.storm.io/v1alpha1", "kind": "NetworkPort",
            "metadata": {"name": "forge1-ens1f0np0", "generation": 4,
                         "labels": {"network.storm.io/node": "forge1", "network.storm.io/ifname": "ens1f0np0"}},
            "spec": {"node": "forge1", "ifname": "ens1f0np0", "mac": "e8:eb:d3:94:a2:80", "fec": "rs", "mtu": null},
            "status": {
                "observedGeneration": 4, "present": true, "link": link, "speed": 25000, "supportedSpeed": 25000,
                "fec": {"want": "rs", "source": "spec", "configured": "rs", "active": fec_active},
                "pause": {"rx": true, "tx": true},
                "module": {"present": true, "kind": "SFP", "identifier": 3, "vendor": "Mellanox", "part": "MCP2M00-A002",
                           "serial": null, "compliance": ["25GBASE-CR CA-N", "passive copper"], "lengthM": 2,
                           "temperatureC": 35.5, "rxPowerMw": [0.5], "txPowerMw": [0.6]},
                "driver": {"name": "mlx5_core", "version": "6.12", "firmware": "14.32.1010", "bus": "0000:03:00.0"},
                "lastChange": {"at": "2026-11-13T18:00:00.000Z", "setting": "fec", "from": "baser", "to": "rs",
                               "source": "spec", "by": "networkport/forge1-ens1f0np0 generation 4", "error": null},
                "counters": {"rxPackets": 10, "fecCorrectedBlocks": 7, "fecUncorrectableBlocks": 0},
                "conditions": [{"type": "Applied", "status": if reason == "Refused" { "False" } else { "True" }, "reason": reason, "message": "m"}]
            }
        })
    }

    #[test]
    fn a_port_row_says_link_fec_module_and_whether_its_spec_is_in_force() {
        let r = row(&port("up", "rs", "Applied"));
        assert_eq!(r["ifname"], "ens1f0np0");
        assert_eq!(r["state"], "up");
        assert_eq!(r["speedWord"], "25G");
        assert_eq!(r["fec"]["active"], "rs");
        assert_eq!(r["module"]["part"], "MCP2M00-A002");
        assert_eq!(r["applied"]["reason"], "Applied");
        assert_eq!(r["spec"], json!({"fec": "rs"}), "identity and nulls are not settings");
        assert_eq!(r["pending"], false);
        assert_eq!(r["counters"]["fecCorrectedBlocks"], 7);
    }

    #[test]
    fn a_spec_the_node_has_not_read_is_pending() {
        let mut o = port("down", "off", "Applied");
        o["metadata"]["generation"] = json!(5);
        assert_eq!(row(&o)["pending"], true);
    }

    #[test]
    fn health_of_a_port() {
        assert_eq!(state(&port("up", "rs", "Applied")).0, Health::Ok);
        assert_eq!(state(&port("down", "off", "Applied")).0, Health::Warn, "an unused port is down on purpose");
        assert_eq!(state(&port("up", "rs", "Refused")), (Health::Error, "spec refused".to_string()));
        let mut gone = port("down", "off", "Absent");
        gone["status"]["present"] = json!(false);
        assert_eq!(state(&gone).0, Health::Error);
        let mut fresh = port("up", "rs", "Applied");
        fresh.as_object_mut().unwrap().remove("status");
        assert_eq!(state(&fresh).0, Health::Unknown);
        assert_eq!(row(&fresh)["applied"]["message"], "the node has not reported this port yet");
    }

    #[test]
    fn a_nodes_ports_only_and_by_name() {
        let mut b = port("up", "rs", "Applied");
        b["spec"]["ifname"] = json!("eno1");
        let mut other = port("up", "rs", "Applied");
        other["spec"]["node"] = json!("forge2");
        let all = [port("up", "rs", "Applied"), b, other];
        let rows = rows_for(all.iter(), "forge1");
        assert_eq!(rows.iter().map(|r| r["ifname"].as_str().unwrap()).collect::<Vec<_>>(), ["eno1", "ens1f0np0"]);
    }

    #[test]
    fn speeds_in_words() {
        assert_eq!(speed_word(Some(25000)), "25G");
        assert_eq!(speed_word(Some(1000)), "1G");
        assert_eq!(speed_word(Some(100)), "100M");
        assert_eq!(speed_word(Some(2500)), "2500M");
        assert_eq!(speed_word(None), "—");
        assert_eq!(speed_word(Some(-1)), "—", "sysfs speed with no link");
    }

    #[test]
    fn a_change_is_a_spec_patch_checked_against_the_crd() {
        assert_eq!(spec_patch(&json!({"fec": "rs"})).unwrap(), json!({"spec": {"fec": "rs"}}));
        assert_eq!(spec_patch(&json!({"fec": null})).unwrap(), json!({"spec": {"fec": null}}), "null hands it back to the node");
        let all = json!({"fec": "baser", "autoneg": false, "speed": 25000, "mtu": 9000, "pause": {"rx": true, "tx": null},
                         "rings": {"rx": 4096}, "offloads": {"tso": true, "rx-checksum": false}});
        assert_eq!(spec_patch(&all).unwrap()["spec"], all);
        for (bad, says) in [
            (json!({"fec": "fast"}), "fec: one of rs"),
            (json!({"fec": 1}), "fec: one of"),
            (json!({"mtu": 67}), "mtu: a whole number from 68"),
            (json!({"mtu": 65536}), "mtu"),
            (json!({"mtu": "9000"}), "mtu"),
            (json!({"speed": 5}), "speed"),
            (json!({"autoneg": "yes"}), "autoneg: on or off"),
            (json!({"pause": {"rx": 1}}), "pause.rx: on or off"),
            (json!({"pause": {"up": true}}), "only rx and tx"),
            (json!({"rings": {"rx": 0}}), "rings.rx"),
            (json!({"offloads": {"a b": true}}), "not a feature name"),
            (json!({"offloads": {"tso": "on"}}), "offloads.tso"),
            (json!({"node": "forge2"}), "identity"),
            (json!({"mac": null}), "identity"),
            (json!({"ifname": "x"}), "identity"),
            (json!({"colour": "red"}), "not a setting"),
            (json!({"colour": null}), "not a setting"),
            (json!({}), "nothing to change"),
            (json!("rs"), "an object"),
        ] {
            let e = spec_patch(&bad).unwrap_err();
            assert!(e.contains(says), "{bad}: {e}");
        }
    }

    #[test]
    fn a_change_in_words() {
        assert_eq!(said("fec", &json!("rs")), "fec rs");
        assert_eq!(said("mtu", &json!(null)), "mtu left to the node");
        assert_eq!(said("mtu", &json!(9000)), "mtu 9000");
    }

    #[test]
    fn who_may_change_a_port() {
        assert!(refusal(false, true).unwrap().contains("admin"));
        assert!(refusal(true, false).unwrap().contains("identity"), "never the console's credential");
        assert_eq!(refusal(true, true), None);
    }

    #[test]
    fn a_port_in_the_feed_links_to_its_node_page() {
        let np = HashMap::from([("forge1-ens1f0np0".to_string(), port("down", "baser", "Applied"))]);
        let nodes = HashMap::from([("forge1".to_string(), json!({}))]);
        let c = &components(&np, &nodes)[0];
        assert_eq!(c.id, "k8s:np:forge1-ens1f0np0");
        assert_eq!(c.label, "forge1 ens1f0np0");
        assert_eq!(c.health, Health::Warn);
        assert_eq!(c.detail, "down · 25G · fec baser");
        assert_eq!(c.link.as_deref(), Some("#/node/forge1/network?port=forge1-ens1f0np0"));
        assert_eq!(c.relations.len(), 1);
    }
}
