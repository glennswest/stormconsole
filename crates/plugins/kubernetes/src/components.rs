//! Raw kube objects → stormview components. Health comes from the same
//! conditions kubectl reads; relations wire pods to their namespace and
//! node so the grid nests the way the cluster actually hangs together.

use std::collections::HashMap;

use console_core::{Action, ComponentSummary, Health, Metric, Relation};
use serde_json::Value;

type Snapshot = HashMap<&'static str, HashMap<String, Value>>;

fn s<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str)
}

fn n(v: &Value, ptr: &str) -> i64 {
    v.pointer(ptr).and_then(Value::as_i64).unwrap_or(0)
}

fn condition<'a>(v: &'a Value, kind: &str) -> Option<&'a str> {
    v.pointer("/status/conditions")?
        .as_array()?
        .iter()
        .find(|c| s(c, "/type") == Some(kind))
        .and_then(|c| s(c, "/status"))
}

fn split_key(key: &str) -> (Option<&str>, &str) {
    match key.split_once('/') {
        Some((ns, name)) => (Some(ns), name),
        None => (None, key),
    }
}

fn base(kind: &str, key: &str, label: &str, health: Health, detail: String) -> ComponentSummary {
    ComponentSummary {
        id: format!("k8s:{kind}:{key}"),
        kind: format!("k8s-{kind}"),
        label: label.to_string(),
        health,
        detail,
        metrics: vec![],
        actions: vec![],
        relations: vec![],
        link: None,
    }
}

fn ns_relation(key: &str) -> Option<Relation> {
    let (ns, _) = split_key(key);
    ns.map(|ns| Relation::belongs_to("namespace", format!("k8s:ns:{ns}")))
}

/// The containers a pod declares, init containers first, as component ids.
///
/// Reads `spec`, not `status`: a pod that has not started yet still *has*
/// containers, and a list that appears only once the kubelet has reported is
/// a list that is empty exactly when somebody is looking to find out why.
fn container_ids(key: &str, pod: &Value) -> Vec<String> {
    CONTAINER_FIELDS
        .iter()
        .flat_map(|(spec_field, _, _)| {
            pod.pointer(&format!("/spec/{spec_field}"))
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[])
        })
        .filter_map(|c| s(c, "/name"))
        .map(|name| format!("k8s:container:{key}/{name}"))
        .collect()
}

/// `(spec field, status field, whether it is an init container)`.
///
/// Init containers are listed first because that is the order they run in,
/// and a pod stuck in `Init:0/2` is a pod whose interesting container is one
/// of these rather than the app.
const CONTAINER_FIELDS: [(&str, &str, bool); 2] =
    [("initContainers", "initContainerStatuses", true), ("containers", "containerStatuses", false)];

/// The container's state, as the three words `kubectl describe` uses, plus
/// the reason when there is one — `Waiting: CrashLoopBackOff` is the whole
/// diagnosis in most cases and it should not need a YAML tab to find.
fn container_state(status: Option<&Value>) -> (Health, String, bool) {
    let Some(st) = status else {
        // Declared, never reported: the kubelet has not got to it. Not an
        // error — a pod that is still being admitted looks exactly like this.
        return (Health::Unknown, "not started".to_string(), false);
    };
    let ready = st.pointer("/ready").and_then(Value::as_bool).unwrap_or(false);
    if let Some(run) = st.pointer("/state/running") {
        let since = s(run, "/startedAt").unwrap_or("");
        let detail = if ready { "running".to_string() } else { "running · not ready".to_string() };
        let _ = since;
        return (if ready { Health::Ok } else { Health::Warn }, detail, ready);
    }
    if let Some(w) = st.pointer("/state/waiting") {
        let reason = s(w, "/reason").unwrap_or("waiting");
        // CrashLoopBackOff is the one everybody is actually looking for.
        let health = if reason.contains("Err") || reason.contains("CrashLoop") || reason.contains("Invalid") {
            Health::Error
        } else {
            Health::Warn
        };
        return (health, format!("waiting · {reason}"), ready);
    }
    if let Some(t) = st.pointer("/state/terminated") {
        let code = n(t, "/exitCode");
        let reason = s(t, "/reason").unwrap_or("terminated");
        let health = if code == 0 { Health::Idle } else { Health::Error };
        return (health, format!("terminated · {reason} ({code})"), ready);
    }
    (Health::Unknown, "unknown".to_string(), ready)
}

/// One component per container in a pod.
///
/// A container is not a kubernetes object — there is no `/api/v1/containers`
/// — but it is the thing a person means when they open a pod, and the feed
/// is a view of what is running rather than a mirror of the API's nouns. It
/// gets an id of its own so the table can nest it, the grid can select it,
/// and a future logs/terminal tab has something to be scoped to.
fn containers_of(key: &str, pod: &Value) -> Vec<ComponentSummary> {
    let mut out = Vec::new();
    for (spec_field, status_field, is_init) in CONTAINER_FIELDS {
        let specs = pod
            .pointer(&format!("/spec/{spec_field}"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let statuses = pod
            .pointer(&format!("/status/{status_field}"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for spec in specs {
            let Some(name) = s(spec, "/name") else { continue };
            // Matched by name, not by position: the kubelet does not promise
            // the two arrays are in the same order, and pairing a container
            // with somebody else's state is worse than showing none.
            let status = statuses.iter().find(|st| s(st, "/name") == Some(name));
            let (health, state, _ready) = container_state(status);
            let image = s(spec, "/image").unwrap_or("");
            let detail = if is_init { format!("init · {state}") } else { state };
            let mut c = base("container", &format!("{key}/{name}"), name, health, detail);
            // The image in full. It is the first thing anybody checks when a
            // pod is running the wrong thing, and truncating it in the detail
            // line is what makes that check impossible.
            if !image.is_empty() {
                c.metrics.push(Metric::new("image", image.to_string()).tone("muted"));
            }
            if let Some(st) = status {
                let restarts = n(st, "/restartCount");
                c.metrics.push(
                    Metric::new("restarts", restarts.to_string())
                        .tone(if restarts > 0 { "warn" } else { "muted" }),
                );
                if let Some(id) = s(st, "/imageID") {
                    // The digest actually running, which is not always what
                    // the tag in `image` resolves to any more.
                    c.metrics.push(Metric::new("imageID", id.to_string()).tone("muted"));
                }
            }
            // What it asked for and what it is allowed.
            //
            // Requests are what the scheduler placed it on and limits are
            // what kills it, and a container OOMKilled at 128Mi is only
            // explicable next to the number. Both, because the pair is the
            // fact: a request with no limit and a limit equal to the request
            // are different machines to operate.
            for (field, label) in [("requests", "requests"), ("limits", "limits")] {
                let r = spec.pointer(&format!("/resources/{field}"));
                let cpu = r.and_then(|v| s(v, "/cpu"));
                let mem = r.and_then(|v| s(v, "/memory"));
                let text = match (cpu, mem) {
                    (Some(c), Some(m)) => format!("{c} cpu · {m}"),
                    (Some(c), None) => format!("{c} cpu"),
                    (None, Some(m)) => m.to_string(),
                    (None, None) => continue,
                };
                c.metrics.push(Metric::new(label, text).tone("muted"));
            }
            if let Some(ports) = spec.pointer("/ports").and_then(Value::as_array) {
                let list: Vec<String> = ports
                    .iter()
                    .filter_map(|p| p.pointer("/containerPort").and_then(Value::as_i64))
                    .map(|p| p.to_string())
                    .collect();
                if !list.is_empty() {
                    c.metrics.push(Metric::new("ports", list.join(", ")).tone("muted"));
                }
            }
            c.relations.push(Relation::belongs_to("pod", format!("k8s:pod:{key}")));
            out.push(c);
        }
    }
    out
}

/// What the Cilium agent's own health server said, when it could be
/// asked. `None` means the console is not on a node that runs one.
pub type AgentState = Option<(Health, String)>;

pub fn map(snap: &Snapshot, agent: AgentState) -> Vec<ComponentSummary> {
    let mut out = Vec::new();
    let empty = HashMap::new();
    let of = |kind: &str| snap.get(kind).unwrap_or(&empty);

    // Namespaces first, carrying has_many edges per workload kind.
    for (key, obj) in of("ns") {
        let health = match s(obj, "/status/phase") {
            Some("Active") | None => Health::Ok,
            Some("Terminating") => Health::Warn,
            Some(_) => Health::Unknown,
        };
        let pods = of("pod").keys().filter(|k| k.starts_with(&format!("{key}/"))).count();
        let mut c = base("ns", key, key, health, format!("{pods} pods"));
        for (kind, name) in [
            ("pod", "pods"),
            ("deploy", "deployments"),
            ("sts", "statefulsets"),
            ("ds", "daemonsets"),
            ("job", "jobs"),
            ("cronjob", "cronjobs"),
            ("svc", "services"),
            ("pvc", "pvcs"),
        ] {
            let targets: Vec<String> = of(kind)
                .keys()
                .filter(|k| k.starts_with(&format!("{key}/")))
                .map(|k| format!("k8s:{kind}:{k}"))
                .collect();
            if !targets.is_empty() {
                c.relations.push(Relation::has_many(name, targets));
            }
        }
        c.metrics.push(Metric::new("pods", pods.to_string()));
        c.link = Some(format!("#/k8s/ns/{key}"));
        out.push(c);
    }

    for (key, obj) in of("node") {
        let ready = condition(obj, "Ready") == Some("True");
        let unschedulable = obj.pointer("/spec/unschedulable").and_then(Value::as_bool)
            == Some(true);
        let health = match (ready, unschedulable) {
            (true, false) => Health::Ok,
            (true, true) => Health::Warn,
            (false, _) => Health::Error,
        };
        let kubelet = s(obj, "/status/nodeInfo/kubeletVersion").unwrap_or("?");
        let pods_on: Vec<String> = of("pod")
            .iter()
            .filter(|(_, p)| s(p, "/spec/nodeName") == Some(key))
            .map(|(k, _)| format!("k8s:pod:{k}"))
            .collect();
        let detail = format!(
            "{}{} · kubelet {kubelet}",
            if ready { "Ready" } else { "NotReady" },
            if unschedulable { " · cordoned" } else { "" }
        );
        let mut c = base("node", key, key, health, detail);
        c.metrics.push(Metric::new("pods", pods_on.len().to_string()));
        if !pods_on.is_empty() {
            c.relations.push(Relation::has_many("pods", pods_on));
        }
        c.link = Some(format!("#/grid?id=k8s:node:{key}"));
        out.push(c);
    }

    for (key, obj) in of("pod") {
        let (_, name) = split_key(key);
        let phase = s(obj, "/status/phase").unwrap_or("Unknown");
        let ready = condition(obj, "Ready") == Some("True");
        let health = match phase {
            "Running" if ready => Health::Ok,
            "Running" => Health::Warn,
            "Succeeded" => Health::Idle,
            "Pending" => Health::Warn,
            "Failed" => Health::Error,
            _ => Health::Unknown,
        };
        let restarts: i64 = obj
            .pointer("/status/containerStatuses")
            .and_then(Value::as_array)
            .map(|cs| cs.iter().map(|c| n(c, "/restartCount")).sum())
            .unwrap_or(0);
        let node = s(obj, "/spec/nodeName");
        // The phase, and only the phase.
        //
        // This used to read "<namespace> · <phase> · <node>", from before the
        // table could show either as a column. Both are `belongs_to` edges and
        // the table now renders them as sortable columns, so repeating them
        // here spends the one line a card has on facts already on screen —
        // and a sentence cannot be sorted or compared down a list, which is
        // the whole reason to want them as columns.
        let detail = phase.to_string();
        // The name without the node it is pinned to.
        //
        // A static pod's mirror is named `<name>-<node>` — upstream's
        // convention, and load-bearing: pod names are unique per namespace,
        // so twenty nodes each running `stormconsole` need twenty distinct
        // names. The *object* therefore keeps the suffix. What a person reads
        // does not need it, because the node is its own column now, and
        // "stormconsole-storm-06f96d" is mostly a node id repeated on every
        // row of a column that already says it.
        let label = short_label(name, node);
        let mut c = base("pod", key, name, health, detail);
        c.label = label;
        // How many of its containers are up, before anything else.
        //
        // "Running" with 0/1 ready is the commonest way a pod lies: the phase
        // is correct — a container in CrashLoopBackOff does not change it —
        // and the only thing that says so is this ratio.
        if let Some(cs) = obj.pointer("/status/containerStatuses").and_then(Value::as_array) {
            let ready = cs
                .iter()
                .filter(|c| c.pointer("/ready").and_then(Value::as_bool).unwrap_or(false))
                .count();
            c.metrics.push(
                Metric::new("ready", format!("{ready}/{}", cs.len()))
                    .tone(if ready == cs.len() { "muted" } else { "warn" }),
            );
        }
        c.metrics.push(
            Metric::new("restarts", restarts.to_string())
                .tone(if restarts > 0 { "warn" } else { "muted" }),
        );
        // The address, which is the one fact you cannot get anywhere else in
        // the console and the first thing wanted to reach the thing directly.
        if let Some(ip) = s(obj, "/status/podIP") {
            c.metrics.push(Metric::new("IP", ip.to_string()).tone("muted"));
        }
        // What the scheduler promised it. Burstable and BestEffort are the
        // two that get evicted first under pressure, so it is worth seeing
        // without opening YAML.
        if let Some(qos) = s(obj, "/status/qosClass") {
            c.metrics.push(
                Metric::new("QoS", qos.to_string())
                    .tone(if qos == "Guaranteed" { "muted" } else { "accent" }),
            );
        }
        // The pod's totals, summed across its containers — what it costs the
        // node, which is not derivable by eye from a list of containers.
        let (cpu_req, mem_req) = pod_requests(obj);
        if !cpu_req.is_empty() || !mem_req.is_empty() {
            let text = match (cpu_req.is_empty(), mem_req.is_empty()) {
                (false, false) => format!("{cpu_req} cpu · {mem_req}"),
                (false, true) => format!("{cpu_req} cpu"),
                _ => mem_req.clone(),
            };
            c.metrics.push(Metric::new("requests", text).tone("muted"));
        }
        c.relations.extend(ns_relation(key));
        if let Some(nd) = node {
            // **belongs_to, not has_one.** A pod does not own its node — it is
            // placed on one — and the direction is what the table reads to
            // decide what nests inside what. As `has_one` it expanded a pod
            // into its node, and the node's `has_many pods` expanded straight
            // back: open a pod, find a node, find the pods again, and the one
            // thing a pod actually contains was nowhere in it.
            c.relations.push(Relation::belongs_to("node", format!("k8s:node:{nd}")));
        }
        // What a pod *is*. Until this, `containerStatuses` was read only to
        // sum restarts, so the containers — their images, what state each is
        // in, which one is crash-looping — were not in the feed at all.
        let ids = container_ids(key, obj);
        if !ids.is_empty() {
            c.relations.push(Relation::has_many("containers", ids));
        }
        c.actions.push(Action {
            id: "delete".into(),
            label: "Delete".into(),
            method: "POST".into(),
            path: format!("/api/plugins/k8s/pods/{key}/delete"),
            enabled: true,
            danger: true,
            tone: None,
        });
        out.push(c);
        out.extend(containers_of(key, obj));
    }

    for (key, obj) in of("deploy") {
        out.push(workload("deploy", key, n(obj, "/spec/replicas"), n(obj, "/status/readyReplicas")));
    }
    for (key, obj) in of("sts") {
        out.push(workload("sts", key, n(obj, "/spec/replicas"), n(obj, "/status/readyReplicas")));
    }
    for (key, obj) in of("ds") {
        out.push(workload(
            "ds",
            key,
            n(obj, "/status/desiredNumberScheduled"),
            n(obj, "/status/numberReady"),
        ));
    }

    for (key, obj) in of("job") {
        let (_, name) = split_key(key);
        let (active, succeeded, failed) =
            (n(obj, "/status/active"), n(obj, "/status/succeeded"), n(obj, "/status/failed"));
        let (health, detail) = if failed > 0 {
            (Health::Error, format!("{failed} failed"))
        } else if active > 0 {
            (Health::Ok, format!("{active} active"))
        } else if succeeded > 0 {
            (Health::Idle, "completed".to_string())
        } else {
            (Health::Unknown, "pending".to_string())
        };
        let mut c = base("job", key, name, health, detail);
        c.relations.extend(ns_relation(key));
        out.push(c);
    }

    for (key, obj) in of("cronjob") {
        let (_, name) = split_key(key);
        let schedule = s(obj, "/spec/schedule").unwrap_or("?");
        let suspended = obj.pointer("/spec/suspend").and_then(Value::as_bool) == Some(true);
        let health = if suspended { Health::Idle } else { Health::Ok };
        let detail =
            format!("{schedule}{}", if suspended { " · suspended" } else { "" });
        let mut c = base("cronjob", key, name, health, detail);
        c.relations.extend(ns_relation(key));
        out.push(c);
    }

    for (key, obj) in of("svc") {
        let (_, name) = split_key(key);
        let svc_type = s(obj, "/spec/type").unwrap_or("ClusterIP");
        let ip = s(obj, "/spec/clusterIP").unwrap_or("-");
        let mut c = base("svc", key, name, Health::Ok, format!("{svc_type} · {ip}"));
        c.relations.extend(ns_relation(key));
        out.push(c);
    }

    for (key, obj) in of("pvc") {
        let (_, name) = split_key(key);
        let phase = s(obj, "/status/phase").unwrap_or("Unknown");
        let health = match phase {
            "Bound" => Health::Ok,
            "Pending" => Health::Warn,
            "Lost" => Health::Error,
            _ => Health::Unknown,
        };
        let size = s(obj, "/status/capacity/storage")
            .or_else(|| s(obj, "/spec/resources/requests/storage"))
            .unwrap_or("?");
        let mut c = base("pvc", key, name, health, format!("{phase} · {size}"));
        c.relations.extend(ns_relation(key));
        out.push(c);
    }

    cilium(snap, agent, &mut out);
    out
}

/// A DELETE against the apiserver path, through the console.
fn delete_action(path: &str) -> console_core::Action {
    console_core::Action {
        id: "delete".into(),
        label: "Delete".into(),
        method: "DELETE".into(),
        path: format!("/api/plugins/k8s/raw{path}"),
        enabled: true,
        danger: true,
        tone: None,
    }
}

/// `k=v, k=v` from a matchLabels map, Cilium's `k8s:` prefixes dropped.
fn labels_summary(v: Option<&Value>, max: usize) -> String {
    let Some(map) = v.and_then(Value::as_object) else { return String::new() };
    let mut parts: Vec<String> = map
        .iter()
        .filter(|(k, _)| !k.starts_with("k8s:io.cilium") && !k.starts_with("k8s:io.kubernetes.pod.namespace"))
        .map(|(k, v)| format!("{}={}", k.trim_start_matches("k8s:"), v.as_str().unwrap_or("")))
        .collect();
    parts.sort();
    let more = parts.len().saturating_sub(max);
    parts.truncate(max);
    let mut out = parts.join(", ");
    if more > 0 {
        out.push_str(&format!(" +{more}"));
    }
    out
}

/// One line for a Cilium or core network policy: whom it selects, and how
/// many ingress/egress rules it carries (`specs` counted too).
fn policy_summary(obj: &Value) -> String {
    let mut specs: Vec<&Value> = Vec::new();
    if let Some(sp) = obj.get("spec") {
        specs.push(sp);
    }
    if let Some(arr) = obj.get("specs").and_then(Value::as_array) {
        specs.extend(arr.iter());
    }
    let mut ingress = 0;
    let mut egress = 0;
    let mut selector = String::new();
    for sp in &specs {
        ingress += sp.get("ingress").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        ingress += sp.get("ingressDeny").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        egress += sp.get("egress").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        egress += sp.get("egressDeny").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        if selector.is_empty() {
            selector = labels_summary(
                sp.pointer("/endpointSelector/matchLabels")
                    .or_else(|| sp.pointer("/podSelector/matchLabels"))
                    .or_else(|| sp.pointer("/nodeSelector/matchLabels")),
                3,
            );
        }
    }
    let types = obj
        .pointer("/spec/policyTypes")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("/"))
        .unwrap_or_default();
    format!(
        "{} · {ingress} ingress · {egress} egress{}",
        if selector.is_empty() { "all endpoints".to_string() } else { format!("selects {selector}") },
        if types.is_empty() { String::new() } else { format!(" · {types}") }
    )
}

/// Cilium through its CRDs: endpoints (one per pod, with its identity and
/// address), nodes, identities, and policies — plus the core
/// NetworkPolicy — under one `k8s:cilium` card.
fn cilium(snap: &Snapshot, agent: AgentState, out: &mut Vec<ComponentSummary>) {
    let empty = HashMap::new();
    let of = |kind: &str| snap.get(kind).unwrap_or(&empty);

    let mut ready = 0usize;
    let mut endpoint_ids = Vec::new();
    for (key, obj) in of("cep") {
        let state = s(obj, "/status/state").unwrap_or("");
        let health = match state {
            "ready" => Health::Ok,
            "waiting-for-identity" | "waiting-to-regenerate" | "regenerating" | "restoring" | "creating" => Health::Warn,
            "" => Health::Unknown,
            _ => Health::Error,
        };
        if health == Health::Ok {
            ready += 1;
        }
        let ipv4 = obj.pointer("/status/networking/addressing/0/ipv4").and_then(Value::as_str).unwrap_or("?");
        let identity = obj.pointer("/status/identity/id").and_then(Value::as_i64);
        let (_, name) = split_key(key);
        let mut c = base(
            "cep",
            key,
            name,
            health,
            format!(
                "{ipv4} · identity {} · {}",
                identity.map(|i| i.to_string()).unwrap_or_else(|| "?".into()),
                if state.is_empty() { "no state" } else { state }
            ),
        );
        c.metrics.push(Metric::new("ipv4", ipv4));
        if let Some(id) = identity {
            c.metrics.push(Metric::new("identity", id.to_string()).tone("muted"));
            if of("cid").contains_key(&id.to_string()) {
                c.relations.push(Relation::has_one("identity", format!("k8s:cid:{id}")));
            }
        }
        if let Some(r) = ns_relation(key) {
            c.relations.push(r);
        }
        if of("pod").contains_key(key) {
            c.relations.push(Relation::has_one("pod", format!("k8s:pod:{key}")));
        }
        endpoint_ids.push(c.id.clone());
        out.push(c);
    }

    let mut node_ids = Vec::new();
    for (key, obj) in of("cn") {
        let ip = obj
            .pointer("/spec/addresses")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|x| s(x, "/type") == Some("InternalIP")))
            .and_then(|x| s(x, "/ip"))
            .unwrap_or("?");
        let cidr = obj
            .pointer("/spec/ipam/podCIDRs/0")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let mut c = base("cn", key, key, Health::Ok, format!("{ip} · pod CIDR {cidr}"));
        c.metrics.push(Metric::new("ip", ip));
        c.metrics.push(Metric::new("pod cidr", cidr).tone("muted"));
        if of("node").contains_key(key) {
            c.relations.push(Relation::has_one("node", format!("k8s:node:{key}")));
        }
        node_ids.push(c.id.clone());
        out.push(c);
    }

    let mut identity_ids = Vec::new();
    for (key, obj) in of("cid") {
        let labels = obj.get("security-labels");
        let ns = labels
            .and_then(|l| l.get("k8s:io.kubernetes.pod.namespace"))
            .and_then(Value::as_str);
        let summary = labels_summary(labels, 3);
        let detail = match ns {
            Some(ns) => format!("{ns} · {summary}"),
            None => summary,
        };
        let mut c = base("cid", key, key, Health::Ok, detail);
        if let Some(ns) = ns {
            c.relations.push(Relation::belongs_to("namespace", format!("k8s:ns:{ns}")));
        }
        identity_ids.push(c.id.clone());
        out.push(c);
    }

    let mut policy_ids = Vec::new();
    for (key, obj) in of("cnp") {
        let (ns, name) = split_key(key);
        let mut c = base("cnp", key, name, Health::Ok, policy_summary(obj));
        if let Some(r) = ns_relation(key) {
            c.relations.push(r);
        }
        c.actions.push(delete_action(&format!(
            "/apis/cilium.io/v2/namespaces/{}/ciliumnetworkpolicies/{name}",
            ns.unwrap_or("default")
        )));
        policy_ids.push(c.id.clone());
        out.push(c);
    }
    for (key, obj) in of("ccnp") {
        let mut c = base("ccnp", key, key, Health::Ok, format!("clusterwide · {}", policy_summary(obj)));
        c.actions.push(delete_action(&format!("/apis/cilium.io/v2/ciliumclusterwidenetworkpolicies/{key}")));
        policy_ids.push(c.id.clone());
        out.push(c);
    }
    for (key, obj) in of("netpol") {
        let (ns, name) = split_key(key);
        let mut c = base("netpol", key, name, Health::Ok, policy_summary(obj));
        if let Some(r) = ns_relation(key) {
            c.relations.push(r);
        }
        c.actions.push(delete_action(&format!(
            "/apis/networking.k8s.io/v1/namespaces/{}/networkpolicies/{name}",
            ns.unwrap_or("default")
        )));
        out.push(c);
    }

    // The card: only once Cilium's CRDs are being served at all, or the
    // agent on this node *answered* — a node whose agent is up but whose
    // CRDs are not installed is a real state worth showing.
    //
    // "Answered" means it responded, not that the probe ran. A connection
    // refused on :9879 is what a machine with no Cilium looks like, and
    // turning that into a red Cilium card would put a failure on the
    // overview of every node that was never meant to run it.
    let agent_answered =
        matches!(&agent, Some((Health::Ok, _)) | Some((Health::Warn, _)));
    if !snap.contains_key("cep") && !snap.contains_key("cn") && !agent_answered {
        return;
    }
    let eps = endpoint_ids.len();
    let (mut health, mut detail) = if eps == 0 && node_ids.is_empty() {
        (Health::Idle, "no Cilium objects — agent not running or CRDs not installed".to_string())
    } else if eps > 0 && ready == 0 {
        (Health::Error, format!("0/{eps} endpoints ready"))
    } else if ready < eps {
        (Health::Warn, format!("{ready}/{eps} endpoints ready"))
    } else {
        (Health::Ok, format!("{ready}/{eps} endpoints ready"))
    };
    // The CRDs say what the cluster believes; the agent's own health
    // server says whether the dataplane on *this* node is actually up.
    // They disagree exactly when it matters, so the worse one wins.
    let agent_metric = match &agent {
        Some((Health::Ok, _)) => Some(Metric::new("agent", "up").tone("ok")),
        Some((Health::Unknown, _)) | None => None,
        Some((h, d)) => {
            if severity(*h) < severity(health) {
                health = *h;
            }
            detail = format!("{detail} · agent {d}");
            Some(Metric::new("agent", "down").tone("error"))
        }
    };
    let mut c = base(
        "cilium",
        "cilium",
        "Cilium",
        health,
        format!(
            "{detail} · {} identities · {} nodes · {} policies",
            identity_ids.len(),
            node_ids.len(),
            policy_ids.len() + of("netpol").len()
        ),
    );
    c.kind = "cni".into();
    c.id = "k8s:cilium".into();
    c.metrics = agent_metric.into_iter().collect();
    c.metrics.extend(vec![
        Metric::new("endpoints", format!("{ready}/{eps}")).tone(match health {
            Health::Ok => "ok",
            Health::Warn => "warn",
            Health::Error => "error",
            _ => "muted",
        }),
        Metric::new("identities", identity_ids.len().to_string()),
        Metric::new("nodes", node_ids.len().to_string()),
        Metric::new("policies", (policy_ids.len() + of("netpol").len()).to_string()),
    ]);
    for (name, ids) in [("endpoints", endpoint_ids), ("nodes", node_ids), ("identities", identity_ids), ("policies", policy_ids)] {
        if !ids.is_empty() {
            c.relations.push(Relation::has_many(name, ids));
        }
    }
    c.link = Some("#/k8s/cep".into());
    out.push(c);
}

/// Broken first — the same ordering console-core sorts health by.
fn severity(h: Health) -> u8 {
    match h {
        Health::Error => 0,
        Health::Warn => 1,
        Health::Ok => 2,
        Health::Idle => 3,
        Health::Unknown => 4,
    }
}

fn workload(kind: &'static str, key: &str, desired: i64, ready: i64) -> ComponentSummary {
    let (_, name) = split_key(key);
    let health = if desired == 0 {
        Health::Idle
    } else if ready >= desired {
        Health::Ok
    } else if ready == 0 {
        Health::Error
    } else {
        Health::Warn
    };
    let mut c = base(kind, key, name, health, format!("{ready}/{desired} ready"));
    c.metrics.push(Metric::new("ready", format!("{ready}/{desired}")));
    c.relations.extend(ns_relation(key));
    c
}

/// Trim the node suffix a mirror pod carries, for display only.
///
/// Only when it is exactly `-<node>` at the end: a pod genuinely named after
/// a machine, or one whose ReplicaSet hash happens to look like one, keeps
/// what it was called. Trimming by guesswork would rename real pods.
/// A pod's summed CPU and memory requests, rendered the way they were written.
///
/// Kubernetes quantities are not numbers — `100m`, `1`, `256Mi`, `1Gi` — so
/// they are parsed into a common unit to add and then rendered back. Summed
/// because the pod is what the scheduler places and what the node pays for;
/// a list of per-container numbers is not something anyone adds up by eye.
///
/// Init containers are deliberately **not** summed with the rest. Kubernetes
/// takes the *maximum* of the init containers and the sum of the app
/// containers, because inits run before the app ones and their resources are
/// released — adding them would overstate what the pod actually holds.
fn pod_requests(pod: &Value) -> (String, String) {
    let sum = |field: &str| -> i64 {
        pod.pointer("/spec/containers")
            .and_then(Value::as_array)
            .map(|cs| {
                cs.iter()
                    .filter_map(|c| c.pointer(&format!("/resources/requests/{field}")))
                    .filter_map(|q| q.as_str())
                    .map(parse_quantity)
                    .sum()
            })
            .unwrap_or(0)
    };
    let cpu = sum("cpu");
    let mem = sum("memory");
    (
        if cpu > 0 { render_cpu(cpu) } else { String::new() },
        if mem > 0 { render_mem(mem) } else { String::new() },
    )
}

/// A Kubernetes quantity as an integer: millicores for CPU, bytes for memory.
fn parse_quantity(q: &str) -> i64 {
    let q = q.trim();
    // CPU's `m` suffix is millicores; every other suffix here is a byte scale.
    for (suffix, scale) in [
        ("Ki", 1024_i64),
        ("Mi", 1024 * 1024),
        ("Gi", 1024 * 1024 * 1024),
        ("Ti", 1024_i64.pow(4)),
        ("k", 1000),
        ("M", 1_000_000),
        ("G", 1_000_000_000),
    ] {
        if let Some(head) = q.strip_suffix(suffix) {
            return head.trim().parse::<f64>().map(|n| (n * scale as f64) as i64).unwrap_or(0);
        }
    }
    if let Some(head) = q.strip_suffix('m') {
        return head.trim().parse::<f64>().map(|n| n as i64).unwrap_or(0);
    }
    // A bare CPU value is whole cores; a bare memory value is bytes. The
    // caller knows which it asked for, and both scale the same way here
    // because CPU is kept in millicores.
    q.parse::<f64>().map(|n| n as i64).unwrap_or(0)
}

fn render_cpu(millis: i64) -> String {
    // A bare CPU request parses to whole cores, so anything under 1000 that
    // came from an `m` suffix stays milli and anything else is cores.
    if millis >= 1000 && millis % 1000 == 0 {
        format!("{}", millis / 1000)
    } else if millis >= 1000 {
        format!("{:.1}", millis as f64 / 1000.0)
    } else {
        format!("{millis}m")
    }
}

fn render_mem(bytes: i64) -> String {
    const UNIT: [(i64, &str); 3] =
        [(1024 * 1024 * 1024, "Gi"), (1024 * 1024, "Mi"), (1024, "Ki")];
    for (scale, name) in UNIT {
        if bytes >= scale {
            let v = bytes as f64 / scale as f64;
            return if v.fract() == 0.0 {
                format!("{}{name}", v as i64)
            } else {
                format!("{v:.1}{name}")
            };
        }
    }
    format!("{bytes}")
}

fn short_label(name: &str, node: Option<&str>) -> String {
    match node {
        Some(nd) if !nd.is_empty() => match name.strip_suffix(nd) {
            Some(head) if head.ends_with('-') && head.len() > 1 => {
                head.trim_end_matches('-').to_string()
            }
            _ => name.to_string(),
        },
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snap_with(kind: &'static str, key: &str, obj: Value) -> Snapshot {
        let mut m = HashMap::new();
        m.insert(kind, HashMap::from([(key.to_string(), obj)]));
        m
    }

    /// A pod with one container, the shape every test below starts from.
    fn pod_snap() -> Snapshot {
        snap_with(
            "pod",
            "default/web",
            json!({
                "metadata": {"name": "web", "namespace": "default"},
                "spec": {"nodeName": "n1", "containers": [
                    {"name": "app", "image": "nginx:1.27", "ports": [{"containerPort": 8080}]}
                ]},
                "status": {
                    "phase": "Running",
                    "podIP": "10.0.0.5",
                    "conditions": [{"type": "Ready", "status": "True"}],
                    "containerStatuses": [{
                        "name": "app", "restartCount": 2, "ready": true,
                        "imageID": "docker.io/library/nginx@sha256:abc",
                        "state": {"running": {"startedAt": "2026-09-20T22:04:46Z"}}
                    }]
                }
            }),
        )
    }

    fn find<'a>(out: &'a [ComponentSummary], id: &str) -> &'a ComponentSummary {
        out.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("no component {id} in {:?}", out.iter().map(|c| &c.id).collect::<Vec<_>>()))
    }

    #[test]
    fn running_ready_pod_is_ok() {
        let out = map(&pod_snap(), None);
        let pod = find(&out, "k8s:pod:default/web");
        assert_eq!(pod.health, Health::Ok);
        // The phase, and only the phase. Namespace and node were in this
        // string until they became sortable columns; leaving them here would
        // spend a card's one line repeating what is already on screen.
        assert_eq!(pod.detail, "Running");
        // They still have to be *somewhere*, and they are — as the edges the
        // table builds those columns from. A pod whose namespace appears
        // nowhere cannot be told apart from the identically-named pod next
        // door, which is what the old assertion was protecting.
        let edge = |name: &str| {
            pod.relations
                .iter()
                .find(|r| r.name == name)
                .map(|r| r.targets.join(","))
                .unwrap_or_default()
        };
        assert_eq!(edge("namespace"), "k8s:ns:default");
        assert_eq!(edge("node"), "k8s:node:n1");
        // By label, not by position: a metric added in front of this one
        // should not fail a test about restarts, and one just was.
        let metric = |label: &str| {
            pod.metrics.iter().find(|m| m.label == label).map(|m| m.value.clone())
        };
        assert_eq!(metric("restarts"), Some("2".into()));
        // The pod's own facts, which were not in the feed at all before.
        assert_eq!(metric("ready"), Some("1/1".into()));
        assert_eq!(metric("IP"), Some("10.0.0.5".into()));
        assert!(pod.actions.iter().any(|a| a.id == "delete"));
    }

    #[test]
    fn a_pod_carries_its_containers() {
        // What a pod *is*. Before this, `containerStatuses` was read only to
        // sum restarts and the containers were not in the feed at all.
        let out = map(&pod_snap(), None);
        let pod = find(&out, "k8s:pod:default/web");
        let rel = pod
            .relations
            .iter()
            .find(|r| r.name == "containers")
            .expect("the pod has a containers relation");
        assert_eq!(rel.targets, vec!["k8s:container:default/web/app"]);

        let c = find(&out, "k8s:container:default/web/app");
        assert_eq!(c.label, "app");
        assert_eq!(c.health, Health::Ok);
        assert_eq!(c.detail, "running");
        // The image in full, because a truncated image is exactly the thing
        // you opened the pod to check.
        let m = |name: &str| c.metrics.iter().find(|m| m.label == name).map(|m| m.value.clone());
        assert_eq!(m("image").as_deref(), Some("nginx:1.27"));
        assert_eq!(m("restarts").as_deref(), Some("2"));
        assert_eq!(m("ports").as_deref(), Some("8080"));
        assert!(m("imageID").is_some());
        // And it points back up, so the table nests it under the pod rather
        // than expanding the pod into it.
        assert!(c
            .relations
            .iter()
            .any(|r| r.name == "pod" && r.targets == vec!["k8s:pod:default/web"]));
    }

    #[test]
    fn a_pod_belongs_to_its_node_rather_than_owning_it() {
        // The loop this fixes: as `has_one`, the table expanded a pod into
        // its node, and the node's `has_many pods` expanded straight back —
        // so a pod opened into a node, which opened into the pods again, and
        // the containers were nowhere.
        let out = map(&pod_snap(), None);
        let pod = find(&out, "k8s:pod:default/web");
        let node = pod.relations.iter().find(|r| r.name == "node").expect("a node edge");
        assert_eq!(node.kind, console_core::RelationKind::BelongsTo);
        // The only thing that nests inside a pod is its containers.
        let downward: Vec<&str> = pod
            .relations
            .iter()
            .filter(|r| r.kind != console_core::RelationKind::BelongsTo)
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(downward, vec!["containers"]);
    }

    #[test]
    fn a_crash_looping_container_says_so_and_is_an_error() {
        // `Waiting · CrashLoopBackOff` is the whole diagnosis in most cases,
        // and it should not need the YAML tab to find.
        let snap = snap_with(
            "pod",
            "default/web",
            json!({
                "metadata": {"name": "web", "namespace": "default"},
                "spec": {"containers": [{"name": "app", "image": "nginx"}]},
                "status": {
                    "phase": "Running",
                    "containerStatuses": [{
                        "name": "app", "restartCount": 7, "ready": false,
                        "state": {"waiting": {"reason": "CrashLoopBackOff"}}
                    }]
                }
            }),
        );
        let out = map(&snap, None);
        let c = find(&out, "k8s:container:default/web/app");
        assert_eq!(c.health, Health::Error);
        assert_eq!(c.detail, "waiting · CrashLoopBackOff");
    }

    #[test]
    fn an_init_container_is_listed_first_and_marked() {
        let snap = snap_with(
            "pod",
            "default/web",
            json!({
                "metadata": {"name": "web", "namespace": "default"},
                "spec": {
                    "initContainers": [{"name": "setup", "image": "busybox"}],
                    "containers": [{"name": "app", "image": "nginx"}]
                },
                "status": {
                    "phase": "Running",
                    "initContainerStatuses": [{
                        "name": "setup", "restartCount": 0,
                        "state": {"terminated": {"exitCode": 0, "reason": "Completed"}}
                    }],
                    "containerStatuses": [{
                        "name": "app", "restartCount": 0, "ready": true,
                        "state": {"running": {}}
                    }]
                }
            }),
        );
        let out = map(&snap, None);
        let pod = find(&out, "k8s:pod:default/web");
        let rel = pod.relations.iter().find(|r| r.name == "containers").unwrap();
        // Init first, because that is the order they run in and a pod stuck
        // at Init:0/1 is a pod whose interesting container is one of these.
        assert_eq!(
            rel.targets,
            vec!["k8s:container:default/web/setup", "k8s:container:default/web/app"]
        );
        let init = find(&out, "k8s:container:default/web/setup");
        assert_eq!(init.detail, "init · terminated · Completed (0)");
        assert_eq!(init.health, Health::Idle);
    }

    #[test]
    fn a_container_state_is_matched_by_name_not_by_position() {
        // The kubelet does not promise the spec and status arrays are in the
        // same order, and pairing a container with somebody else's state is
        // worse than showing none.
        let snap = snap_with(
            "pod",
            "default/web",
            json!({
                "metadata": {"name": "web", "namespace": "default"},
                "spec": {"containers": [
                    {"name": "app", "image": "nginx"},
                    {"name": "sidecar", "image": "envoy"}
                ]},
                "status": {
                    "phase": "Running",
                    "containerStatuses": [
                        {"name": "sidecar", "restartCount": 5, "ready": true, "state": {"running": {}}},
                        {"name": "app", "restartCount": 0, "ready": true, "state": {"running": {}}}
                    ]
                }
            }),
        );
        let out = map(&snap, None);
        let restarts = |id: &str| {
            find(&out, id).metrics.iter().find(|m| m.label == "restarts").unwrap().value.clone()
        };
        assert_eq!(restarts("k8s:container:default/web/app"), "0");
        assert_eq!(restarts("k8s:container:default/web/sidecar"), "5");
    }

    #[test]
    fn a_container_the_kubelet_has_not_reported_is_still_listed() {
        // Read from `spec`, not `status`: a pod that has not started yet
        // still *has* containers, and a list that appears only once the
        // kubelet reports is empty exactly when somebody is looking to find
        // out why.
        let snap = snap_with(
            "pod",
            "default/web",
            json!({
                "metadata": {"name": "web", "namespace": "default"},
                "spec": {"containers": [{"name": "app", "image": "nginx"}]},
                "status": {"phase": "Pending"}
            }),
        );
        let out = map(&snap, None);
        let c = find(&out, "k8s:container:default/web/app");
        assert_eq!(c.health, Health::Unknown);
        assert_eq!(c.detail, "not started");
    }

    #[test]
    fn degraded_deployment_is_warn() {
        let snap = snap_with(
            "deploy",
            "default/api",
            json!({
                "metadata": {"name": "api", "namespace": "default"},
                "spec": {"replicas": 3},
                "status": {"readyReplicas": 1}
            }),
        );
        let out = map(&snap, None);
        assert_eq!(out[0].health, Health::Warn);
        assert_eq!(out[0].detail, "1/3 ready");
    }

    #[test]
    fn notready_node_is_error_and_namespace_links_pods() {
        let mut snap = snap_with(
            "node",
            "n1",
            json!({
                "metadata": {"name": "n1"},
                "status": {"conditions": [{"type": "Ready", "status": "False"}],
                            "nodeInfo": {"kubeletVersion": "v0.2.3"}}
            }),
        );
        snap.insert(
            "ns",
            HashMap::from([(
                "default".to_string(),
                json!({"metadata": {"name": "default"}, "status": {"phase": "Active"}}),
            )]),
        );
        snap.insert(
            "pod",
            HashMap::from([(
                "default/web".to_string(),
                json!({"metadata": {"name": "web", "namespace": "default"},
                        "spec": {"nodeName": "n1"}, "status": {"phase": "Running"}}),
            )]),
        );
        let out = map(&snap, None);
        let node = out.iter().find(|c| c.id == "k8s:node:n1").unwrap();
        assert_eq!(node.health, Health::Error);
        assert_eq!(node.relations[0].targets, vec!["k8s:pod:default/web"]);
        let ns = out.iter().find(|c| c.id == "k8s:ns:default").unwrap();
        assert!(ns.relations.iter().any(|r| r.name == "pods"));
        // A namespace opens its own page, not a generic grid (#6).
        assert_eq!(ns.link.as_deref(), Some("#/k8s/ns/default"));
    }

    #[test]
    fn cilium_endpoint_and_card() {
        let mut snap: Snapshot = HashMap::new();
        snap.insert("cep", HashMap::from([(
            "kube-system/coredns-1".to_string(),
            json!({"status": {"state": "ready", "identity": {"id": 42},
                "networking": {"addressing": [{"ipv4": "10.0.0.5"}]}}}),
        )]));
        snap.insert("cid", HashMap::from([(
            "42".to_string(),
            json!({"security-labels": {"k8s:io.kubernetes.pod.namespace": "kube-system", "k8s:k8s-app": "kube-dns", "k8s:io.cilium.k8s.policy.cluster": "default"}}),
        )]));
        snap.insert("cn", HashMap::from([(
            "storm-1".to_string(),
            json!({"spec": {"addresses": [{"type": "InternalIP", "ip": "192.168.8.106"}], "ipam": {"podCIDRs": ["10.0.0.0/24"]}}}),
        )]));
        snap.insert("cnp", HashMap::from([(
            "default/allow-dns".to_string(),
            json!({"spec": {"endpointSelector": {"matchLabels": {"app": "web"}}, "egress": [{}, {}]}}),
        )]));
        let out = map(&snap, None);
        let ep = out.iter().find(|c| c.id == "k8s:cep:kube-system/coredns-1").unwrap();
        assert_eq!(ep.health, Health::Ok);
        assert!(ep.detail.starts_with("10.0.0.5 · identity 42 · ready"), "{}", ep.detail);
        assert!(ep.relations.iter().any(|r| r.targets == vec!["k8s:cid:42"]));
        let id = out.iter().find(|c| c.id == "k8s:cid:42").unwrap();
        assert_eq!(id.detail, "kube-system · k8s-app=kube-dns");
        let pol = out.iter().find(|c| c.id == "k8s:cnp:default/allow-dns").unwrap();
        assert_eq!(pol.detail, "selects app=web · 0 ingress · 2 egress");
        assert_eq!(pol.actions[0].method, "DELETE");
        assert_eq!(pol.actions[0].path, "/api/plugins/k8s/raw/apis/cilium.io/v2/namespaces/default/ciliumnetworkpolicies/allow-dns");
        let card = out.iter().find(|c| c.id == "k8s:cilium").unwrap();
        assert_eq!(card.health, Health::Ok);
        assert!(card.detail.starts_with("1/1 endpoints ready · 1 identities · 1 nodes · 1 policies"), "{}", card.detail);
    }

    #[test]
    fn a_down_agent_outranks_a_healthy_crd_view() {
        let mut snap: Snapshot = HashMap::new();
        snap.insert("cep", HashMap::from([(
            "default/web".to_string(),
            json!({"status": {"state": "ready", "networking": {"addressing": [{"ipv4": "10.0.0.5"}]}}}),
        )]));
        // Every endpoint is ready and the dataplane on this node is not.
        let out = map(&snap, Some((Health::Error, "unreachable: connection refused".into())));
        let card = out.iter().find(|c| c.id == "k8s:cilium").unwrap();
        assert_eq!(card.health, Health::Error);
        assert!(card.detail.contains("agent unreachable"), "{}", card.detail);
        assert_eq!(card.metrics[0].label, "agent");
        assert_eq!(card.metrics[0].value, "down");

        let up = map(&snap, Some((Health::Ok, "reachable · 200 OK".into())));
        let card = up.iter().find(|c| c.id == "k8s:cilium").unwrap();
        assert_eq!(card.health, Health::Ok);
        assert_eq!(card.metrics[0].value, "up");
    }

    #[test]
    fn an_agent_on_a_node_with_no_crds_still_gets_a_card() {
        let snap: Snapshot = HashMap::new();
        assert!(map(&snap, None).iter().all(|c| c.id != "k8s:cilium"));
        let out = map(&snap, Some((Health::Ok, "reachable · 200 OK".into())));
        let card = out.iter().find(|c| c.id == "k8s:cilium").unwrap();
        assert_eq!(card.metrics[0].value, "up");
    }

    #[test]
    fn a_machine_that_never_ran_cilium_gets_no_cilium_card() {
        // Connection refused on :9879 is what "no Cilium here" looks like.
        // Rendering that as a failed Cilium would put a red card on the
        // overview of every node that was never meant to run one.
        let snap: Snapshot = HashMap::new();
        let out = map(&snap, Some((Health::Error, "unreachable: connection refused".into())));
        assert!(out.iter().all(|c| c.id != "k8s:cilium"), "no CRDs and no agent means no card");
    }

    #[test]
    fn no_cilium_means_no_card() {
        let snap: Snapshot = HashMap::new();
        assert!(map(&snap, None).iter().all(|c| c.id != "k8s:cilium"));
    }

    #[test]
    fn a_mirror_pod_shows_without_the_node_it_repeats() {
        assert_eq!(short_label("stormconsole-storm-06f96d", Some("storm-06f96d")), "stormconsole");
    }

    #[test]
    fn an_ordinary_pod_keeps_its_name() {
        assert_eq!(short_label("coredns-aa22fcb9f6-3f2ed", Some("storm-06f96d")), "coredns-aa22fcb9f6-3f2ed");
        assert_eq!(short_label("web-1", None), "web-1");
    }

    #[test]
    fn a_pod_actually_named_after_the_node_is_not_emptied() {
        // The suffix is the whole name: trimming it would leave nothing.
        assert_eq!(short_label("storm-06f96d", Some("storm-06f96d")), "storm-06f96d");
    }

    #[test]
    fn a_partial_match_is_not_trimmed() {
        // "…-storm-06f96" is not the node, and a name is not a place to guess.
        assert_eq!(short_label("thing-storm-06f96", Some("storm-06f96d")), "thing-storm-06f96");
    }

    #[test]
    fn quantities_parse_the_way_kubernetes_writes_them() {
        assert_eq!(parse_quantity("100m"), 100);
        assert_eq!(parse_quantity("1"), 1);
        assert_eq!(parse_quantity("256Mi"), 256 * 1024 * 1024);
        assert_eq!(parse_quantity("1Gi"), 1024 * 1024 * 1024);
        assert_eq!(parse_quantity("512k"), 512_000);
        assert_eq!(parse_quantity(""), 0);
        assert_eq!(parse_quantity("nonsense"), 0);
    }

    #[test]
    fn memory_renders_in_the_unit_it_was_written_in() {
        assert_eq!(render_mem(256 * 1024 * 1024), "256Mi");
        assert_eq!(render_mem(1024 * 1024 * 1024), "1Gi");
        assert_eq!(render_mem(1536 * 1024 * 1024), "1.5Gi");
    }

    #[test]
    fn cpu_renders_as_millis_or_cores() {
        assert_eq!(render_cpu(100), "100m");
        assert_eq!(render_cpu(2000), "2");
        assert_eq!(render_cpu(1500), "1.5");
    }

    #[test]
    fn a_pods_requests_are_its_app_containers_summed() {
        let pod = json!({"spec": {"containers": [
            {"name": "a", "resources": {"requests": {"cpu": "100m", "memory": "128Mi"}}},
            {"name": "b", "resources": {"requests": {"cpu": "150m", "memory": "128Mi"}}}
        ]}});
        assert_eq!(pod_requests(&pod), ("250m".to_string(), "256Mi".to_string()));
    }

    #[test]
    fn init_containers_are_not_added_to_the_total() {
        // Kubernetes takes the max of the inits and the sum of the app
        // containers, because inits run first and release what they held.
        // Adding them would overstate what the pod actually holds.
        let pod = json!({"spec": {
            "initContainers": [
                {"name": "setup", "resources": {"requests": {"cpu": "900m", "memory": "2Gi"}}}
            ],
            "containers": [
                {"name": "a", "resources": {"requests": {"cpu": "100m", "memory": "128Mi"}}}
            ]
        }});
        assert_eq!(pod_requests(&pod), ("100m".to_string(), "128Mi".to_string()));
    }

    #[test]
    fn a_pod_with_no_requests_reports_none_rather_than_zero() {
        let pod = json!({"spec": {"containers": [{"name": "a"}]}});
        assert_eq!(pod_requests(&pod), (String::new(), String::new()));
    }
}
