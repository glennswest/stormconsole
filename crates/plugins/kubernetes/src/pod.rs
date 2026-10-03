//! One pod, all of it (#69): what it runs, where, on which addresses,
//! behind which Services, under which policies, and what the node does
//! not report yet — said, not left blank.
//!
//! Everything here is a pure function of the pod and the watch cache, so
//! the page's answer is the same object the list row was drawn from. The
//! route in `lib.rs` adds the reads that are not in the cache (the owner
//! chain past a ReplicaSet, each Service's Endpoints, the golden behind a
//! `stormpump://` image) and the runs `logruns.rs` kept.
//!
//! What the kubelet writes was read from rustkube-node (2026-10-02):
//! `image`, `imageID` (a digest under containerd; under the stormpump
//! runtime the image string again), `restartCount`, `state` (a terminated
//! state with no reason for a regular container), `podIPs`, `hostIP`. It
//! writes no `lastState`, no time the image was resolved, no build info,
//! and nothing about the interface beyond its address — rustkube-node#130
//! and #131. Each of those is read when present, so the page fills in the
//! day the node starts writing them.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::components::Snapshot;
use crate::network;

/// The node's container-status gaps.
pub const STATUS_ISSUE: &str = "rustkube-node#130";
/// The node's network gaps: counters beyond bytes, the interface itself,
/// runs before the previous one.
pub const NETWORK_ISSUE: &str = "rustkube-node#131";

fn s<'a>(v: &'a Value, ptr: &str) -> &'a str {
    v.pointer(ptr).and_then(Value::as_str).unwrap_or("")
}

/// The `sha256:…` in an image reference or image ID, if there is one.
///
/// containerd's `imageRef` is `docker.io/library/x@sha256:…` or a bare
/// `sha256:…`; a reference written by digest carries it too. Anything
/// else — a tag, a `stormpump://` asset — has none, and saying so is the
/// answer, not a blank.
pub fn digest_of(s: &str) -> Option<String> {
    let at = s.find("sha256:")?;
    let hex: String = s[at + 7..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    (hex.len() == 64).then(|| format!("sha256:{hex}"))
}

/// What the kubelet does when the spec says nothing: `Always` for
/// `:latest` or no tag, `IfNotPresent` otherwise — Kubernetes' defaulting.
pub fn pull_policy(container: &Value) -> (String, bool) {
    if let Some(p) = container.get("imagePullPolicy").and_then(Value::as_str) {
        return (p.to_string(), false);
    }
    let image = s(container, "/image");
    if image.contains('@') {
        return ("IfNotPresent".into(), true);
    }
    let last = image.rsplit('/').next().unwrap_or(image);
    let tag = last.split_once(':').map(|(_, t)| t);
    match tag {
        None | Some("latest") => ("Always".into(), true),
        Some(_) => ("IfNotPresent".into(), true),
    }
}

/// One container state (`waiting`, `running` or `terminated`) as one shape.
pub fn state_of(st: Option<&Value>) -> Value {
    let Some(st) = st.and_then(Value::as_object) else { return Value::Null };
    let Some((kind, v)) = st.iter().next() else { return Value::Null };
    let since = v
        .get("startedAt")
        .or_else(|| v.get("finishedAt"))
        .and_then(Value::as_str)
        .unwrap_or("");
    json!({
        "kind": kind,
        "since": since,
        "startedAt": v.get("startedAt").cloned().unwrap_or(Value::Null),
        "finishedAt": v.get("finishedAt").cloned().unwrap_or(Value::Null),
        "reason": v.get("reason").cloned().unwrap_or(Value::Null),
        "message": v.get("message").cloned().unwrap_or(Value::Null),
        "exitCode": v.get("exitCode").cloned().unwrap_or(Value::Null),
    })
}

/// The QoS class: what the kubelet reported, else what the resources say.
pub fn qos_class(pod: &Value) -> String {
    if let Some(q) = pod.pointer("/status/qosClass").and_then(Value::as_str) {
        return q.to_string();
    }
    let containers = pod.pointer("/spec/containers").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut any = false;
    let mut guaranteed = !containers.is_empty();
    for c in &containers {
        let req = c.pointer("/resources/requests").and_then(Value::as_object);
        let lim = c.pointer("/resources/limits").and_then(Value::as_object);
        any |= req.is_some_and(|m| !m.is_empty()) || lim.is_some_and(|m| !m.is_empty());
        let full = ["cpu", "memory"].iter().all(|r| {
            let l = lim.and_then(|m| m.get(*r));
            let q = req.and_then(|m| m.get(*r)).or(l);
            l.is_some() && q == l
        });
        guaranteed &= full;
    }
    if guaranteed {
        "Guaranteed".into()
    } else if any {
        "Burstable".into()
    } else {
        "BestEffort".into()
    }
}

/// Where a container's image comes from: a `stormpump://` golden on the
/// node, or an OCI registry.
pub fn image_source(image: &str) -> &'static str {
    if image.starts_with("stormpump://") {
        "stormpump"
    } else {
        "oci"
    }
}

/// Every container, init and ephemeral included, with its spec beside
/// its status.
pub fn containers(pod: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for (spec_ptr, status_ptr, role) in [
        ("/spec/initContainers", "/status/initContainerStatuses", "init"),
        ("/spec/containers", "/status/containerStatuses", "container"),
        ("/spec/ephemeralContainers", "/status/ephemeralContainerStatuses", "ephemeral"),
    ] {
        let statuses: HashMap<String, Value> = pod
            .pointer(status_ptr)
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|c| (s(c, "/name").to_string(), c.clone())).collect())
            .unwrap_or_default();
        for c in pod.pointer(spec_ptr).and_then(Value::as_array).into_iter().flatten() {
            let name = s(c, "/name");
            let st = statuses.get(name);
            let image = s(c, "/image");
            let image_id = st.map(|v| s(v, "/imageID")).unwrap_or("");
            let running_image = st.map(|v| s(v, "/image")).filter(|i| !i.is_empty()).unwrap_or(image);
            let digest = digest_of(image_id).or_else(|| digest_of(running_image));
            let (policy, defaulted) = pull_policy(c);
            let ports: Vec<Value> = c
                .get("ports")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|p| {
                            json!({
                                "name": p.get("name").cloned().unwrap_or(Value::Null),
                                "containerPort": p.get("containerPort").cloned().unwrap_or(Value::Null),
                                "protocol": p.get("protocol").and_then(Value::as_str).unwrap_or("TCP"),
                                "hostPort": p.get("hostPort").cloned().unwrap_or(Value::Null),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let annotations = pod.pointer("/metadata/annotations").and_then(Value::as_object);
            // Read the day the node writes it (rustkube-node#130).
            let resolved = annotations
                .and_then(|a| a.get(&format!("storm.io/image-resolved.{name}")))
                .cloned()
                .unwrap_or(Value::Null);
            out.push(json!({
                "name": name,
                "role": role,
                "image": image,
                "runningImage": running_image,
                "imageID": image_id,
                "digest": digest,
                "source": image_source(image),
                "pullPolicy": policy,
                "pullPolicyDefaulted": defaulted,
                "imageResolved": resolved,
                "ports": ports,
                "command": c.get("command").cloned().unwrap_or(Value::Null),
                "args": c.get("args").cloned().unwrap_or(Value::Null),
                "resources": c.get("resources").cloned().unwrap_or(json!({})),
                "ready": st.and_then(|v| v.get("ready")).and_then(Value::as_bool).unwrap_or(false),
                "started": st.and_then(|v| v.get("started")).cloned().unwrap_or(Value::Null),
                "restartCount": st.and_then(|v| v.get("restartCount")).and_then(Value::as_i64).unwrap_or(0),
                "reported": st.is_some(),
                "containerID": st.map(|v| s(v, "/containerID")).unwrap_or(""),
                "state": state_of(st.and_then(|v| v.get("state"))),
                "lastState": state_of(st.and_then(|v| v.get("lastState"))),
            }));
        }
    }
    out
}

/// The pod's addresses, every one of them.
pub fn addresses(pod: &Value) -> Vec<String> {
    let mut ips: Vec<String> = pod
        .pointer("/status/podIPs")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|p| p.get("ip").and_then(Value::as_str)).map(str::to_string).collect())
        .unwrap_or_default();
    let ip = s(pod, "/status/podIP");
    if !ip.is_empty() && !ips.iter().any(|i| i == ip) {
        ips.insert(0, ip.to_string());
    }
    ips
}

/// Does a Service's selector pick these labels? An empty or absent
/// selector selects nothing: such a Service's Endpoints are written by
/// hand, not chosen by labels.
pub fn service_selects(svc: &Value, labels: &serde_json::Map<String, Value>) -> bool {
    let Some(sel) = svc.pointer("/spec/selector").and_then(Value::as_object) else { return false };
    !sel.is_empty() && sel.iter().all(|(k, v)| labels.get(k) == Some(v))
}

/// The Services in the pod's namespace that select it.
pub fn services(snap: &Snapshot, ns: &str, pod: &Value) -> Vec<Value> {
    let empty = serde_json::Map::new();
    let labels = pod.pointer("/metadata/labels").and_then(Value::as_object).unwrap_or(&empty);
    let prefix = format!("{ns}/");
    let mut out: Vec<Value> = snap
        .get("svc")
        .into_iter()
        .flatten()
        .filter(|(k, svc)| k.starts_with(&prefix) && service_selects(svc, labels))
        .map(|(k, svc)| {
            let mut ips: Vec<Value> = svc
                .pointer("/spec/clusterIPs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if ips.is_empty() {
                if let Some(ip) = svc.pointer("/spec/clusterIP") {
                    ips.push(ip.clone());
                }
            }
            json!({
                "id": format!("k8s:svc:{k}"),
                "name": s(svc, "/metadata/name"),
                "type": svc.pointer("/spec/type").and_then(Value::as_str).unwrap_or("ClusterIP"),
                "clusterIPs": ips,
                "ports": svc.pointer("/spec/ports").cloned().unwrap_or(json!([])),
            })
        })
        .collect();
    out.sort_by(|a, b| s(a, "/name").cmp(s(b, "/name")));
    out
}

/// One Service's Endpoints, and whether this pod is among the ready ones.
///
/// A Service selecting a pod and the pod receiving its traffic are two
/// different facts: the second needs the pod in the ready addresses,
/// which is exactly what a failing readiness probe takes it out of.
pub fn endpoints_summary(eps: &Value, pod_name: &str, pod_ips: &[String]) -> Value {
    let mut ready = 0;
    let mut not_ready = 0;
    let mut this = "absent";
    let is_this = |a: &Value| {
        a.pointer("/targetRef/name").and_then(Value::as_str) == Some(pod_name)
            || a.get("ip").and_then(Value::as_str).is_some_and(|ip| pod_ips.iter().any(|p| p == ip))
    };
    for subset in eps.get("subsets").and_then(Value::as_array).into_iter().flatten() {
        for a in subset.get("addresses").and_then(Value::as_array).into_iter().flatten() {
            ready += 1;
            if is_this(a) {
                this = "ready";
            }
        }
        for a in subset.get("notReadyAddresses").and_then(Value::as_array).into_iter().flatten() {
            not_ready += 1;
            if is_this(a) && this != "ready" {
                this = "notReady";
            }
        }
    }
    json!({"ready": ready, "notReady": not_ready, "thisPod": this})
}

/// What Cilium knows of this pod: its endpoint, its identity, and the
/// policies that select it (the same evaluation the pod row uses).
pub fn cilium(snap: &Snapshot, key: &str) -> Value {
    let empty = HashMap::new();
    let of = |kind: &str| snap.get(kind).unwrap_or(&empty);
    let Some(cep) = of("cep").get(key) else { return Value::Null };
    let ns = key.split_once('/').map(|(n, _)| n).unwrap_or("");
    let id = cep.pointer("/status/identity/id").and_then(Value::as_i64);
    let cid = id.and_then(|i| of("cid").get(&i.to_string()));
    let addressing: Vec<Value> = cep
        .pointer("/status/networking/addressing")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    json!({
        "endpoint": format!("k8s:cep:{key}"),
        "state": s(cep, "/status/state"),
        "identity": id,
        "identityLabels": cid.map(network::identity_labels).unwrap_or_default(),
        "policies": network::selecting(snap, ns, cid),
        "node": s(cep, "/status/networking/node"),
        "addressing": addressing,
    })
}

/// The DNS the pod was given: the policy, and any explicit config.
pub fn dns(pod: &Value) -> Value {
    let host_network = pod.pointer("/spec/hostNetwork").and_then(Value::as_bool).unwrap_or(false);
    let policy = pod
        .pointer("/spec/dnsPolicy")
        .and_then(Value::as_str)
        .unwrap_or("ClusterFirst")
        .to_string();
    // ClusterFirst on the host network falls back to the node's resolver
    // — Kubernetes' rule, and the one surprise worth saying.
    let effective = if host_network && policy == "ClusterFirst" { "Default (host network)" } else { &policy };
    json!({
        "policy": policy,
        "effective": effective,
        "nameservers": pod.pointer("/spec/dnsConfig/nameservers").cloned().unwrap_or(json!([])),
        "searches": pod.pointer("/spec/dnsConfig/searches").cloned().unwrap_or(json!([])),
        "options": pod.pointer("/spec/dnsConfig/options").cloned().unwrap_or(json!([])),
        "hostname": pod.pointer("/spec/hostname").cloned().unwrap_or(Value::Null),
        "subdomain": pod.pointer("/spec/subdomain").cloned().unwrap_or(Value::Null),
    })
}

/// What the page cannot show and why — one sentence each, with the issue
/// that would close it, so a blank never reads as "nothing there".
pub fn gaps(pod: &Value, containers: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    let gap = |what: &str, why: String, issue: &str| json!({"what": what, "why": why, "issue": issue});
    let no_digest: Vec<&str> = containers
        .iter()
        .filter(|c| c["reported"].as_bool() == Some(true) && c["digest"].is_null())
        .map(|c| c["name"].as_str().unwrap_or(""))
        .collect();
    if !no_digest.is_empty() {
        out.push(gap(
            "image digest",
            format!(
                "the node reports no sha256 for {} — under the stormpump runtime its imageID repeats the image name",
                no_digest.join(", ")
            ),
            STATUS_ISSUE,
        ));
    }
    if containers.iter().all(|c| c["imageResolved"].is_null()) {
        out.push(gap(
            "image last checked",
            "the node does not record when it last resolved an image".into(),
            STATUS_ISSUE,
        ));
    }
    out.push(gap(
        "build info",
        "the node does not read the image's OCI config, so its build date and org.opencontainers labels are not reported".into(),
        STATUS_ISSUE,
    ));
    let restarted_without_last = containers
        .iter()
        .any(|c| c["restartCount"].as_i64().unwrap_or(0) > 0 && c["lastState"].is_null());
    if restarted_without_last {
        out.push(gap(
            "last termination",
            "the node writes no lastState, so a restarted container's previous exit code and reason are only in the runs kept below".into(),
            STATUS_ISSUE,
        ));
    }
    let annotated = pod
        .pointer("/metadata/annotations/k8s.v1.cni.cncf.io~1network-status")
        .is_some();
    if !annotated {
        out.push(gap(
            "interface detail",
            "MTU, prefix, gateway, routes and the CNI that wired the interface are recorded nowhere".into(),
            NETWORK_ISSUE,
        ));
    }
    out
}

/// The CNI's own account of the interfaces, when it wrote one: the
/// standard `k8s.v1.cni.cncf.io/network-status` annotation (a JSON list
/// of `{name, interface, ips, mac, mtu, default, dns, gateway}`).
pub fn network_status(pod: &Value) -> Value {
    pod.pointer("/metadata/annotations/k8s.v1.cni.cncf.io~1network-status")
        .and_then(Value::as_str)
        .and_then(|t| serde_json::from_str::<Value>(t).ok())
        .unwrap_or(json!([]))
}

/// Labels, annotations (minus last-applied), owner references,
/// placement and the rest of what `kubectl describe` prints at the top.
pub fn metadata(pod: &Value) -> Value {
    const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";
    let annotations: serde_json::Map<String, Value> = pod
        .pointer("/metadata/annotations")
        .and_then(Value::as_object)
        .map(|m| m.iter().filter(|(k, _)| k.as_str() != LAST_APPLIED).map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    json!({
        "uid": s(pod, "/metadata/uid"),
        "created": s(pod, "/metadata/creationTimestamp"),
        "labels": pod.pointer("/metadata/labels").cloned().unwrap_or(json!({})),
        "annotations": annotations,
        "ownerReferences": pod.pointer("/metadata/ownerReferences").cloned().unwrap_or(json!([])),
        "node": s(pod, "/spec/nodeName"),
        "serviceAccount": pod
            .pointer("/spec/serviceAccountName")
            .or_else(|| pod.pointer("/spec/serviceAccount"))
            .cloned()
            .unwrap_or(Value::Null),
        "priority": pod.pointer("/spec/priority").cloned().unwrap_or(Value::Null),
        "priorityClassName": pod.pointer("/spec/priorityClassName").cloned().unwrap_or(Value::Null),
        "restartPolicy": pod.pointer("/spec/restartPolicy").and_then(Value::as_str).unwrap_or("Always"),
        "qosClass": qos_class(pod),
        "startTime": pod.pointer("/status/startTime").cloned().unwrap_or(Value::Null),
        "phase": s(pod, "/status/phase"),
        "reason": pod.pointer("/status/reason").cloned().unwrap_or(Value::Null),
        "message": pod.pointer("/status/message").cloned().unwrap_or(Value::Null),
        "conditions": pod.pointer("/status/conditions").cloned().unwrap_or(json!([])),
        "mirror": pod.pointer("/metadata/annotations/kubernetes.io~1config.mirror").is_some()
            || pod.pointer("/metadata/annotations/storm.io~1mirror").is_some(),
    })
}

/// The page's answer, less what needs the network to fill in.
pub fn detail(snap: &Snapshot, ns: &str, pod: &Value) -> Value {
    let name = s(pod, "/metadata/name");
    let key = format!("{ns}/{name}");
    let containers = containers(pod);
    let ips = addresses(pod);
    let host_network = pod.pointer("/spec/hostNetwork").and_then(Value::as_bool).unwrap_or(false);
    // A mirror pod is written with an empty hostIP; the node's own
    // InternalIP is the same address and is in the cache.
    let mut host_ip = s(pod, "/status/hostIP").to_string();
    if host_ip.is_empty() {
        host_ip = node_address(snap, s(pod, "/spec/nodeName")).unwrap_or_default();
    }
    json!({
        "name": name,
        "namespace": ns,
        "metadata": metadata(pod),
        "containers": containers,
        "network": {
            "podIPs": ips,
            "hostIP": host_ip,
            "hostNetwork": host_network,
            "dns": dns(pod),
            "services": services(snap, ns, pod),
            "cilium": cilium(snap, &key),
            "interfaces": network_status(pod),
        },
        "gaps": gaps(pod, &containers),
    })
}

/// A node's InternalIP, from the cache.
pub fn node_address(snap: &Snapshot, node: &str) -> Option<String> {
    let n = snap.get("node")?.get(node)?;
    let addrs = n.pointer("/status/addresses").and_then(Value::as_array)?;
    addrs
        .iter()
        .find(|a| a.get("type").and_then(Value::as_str) == Some("InternalIP"))
        .or_else(|| addrs.first())
        .and_then(|a| a.get("address").and_then(Value::as_str))
        .map(str::to_string)
}

/// Where an owner reference leads in this console.
pub fn owner_href(kind: &str, ns: &str, name: &str) -> Option<String> {
    let short = match kind {
        "Deployment" => "deploy",
        "StatefulSet" => "sts",
        "DaemonSet" => "ds",
        "Job" => "job",
        "CronJob" => "cronjob",
        "Node" => return Some(format!("#/grid?id=k8s:node:{name}")),
        "VirtualMachineInstance" | "VirtualMachine" => return Some(format!("#/vm/{ns}/{name}")),
        _ => return None,
    };
    Some(format!("#/grid?id=k8s:{short}:{ns}/{name}"))
}

/// The cache's kind for an owner kind, where the console watches it.
pub fn cached_kind(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "Deployment" => "deploy",
        "StatefulSet" => "sts",
        "DaemonSet" => "ds",
        "Job" => "job",
        "CronJob" => "cronjob",
        _ => return None,
    })
}

/// The `rx`/`tx` byte counters for one pod out of a kubelet's
/// `/metrics/cadvisor`, per interface.
///
/// cAdvisor's names, which the kubelet keeps; the errors, packets and
/// drops families are read too when a kubelet exports them
/// (rustkube-node#131).
pub fn traffic(metrics: &str, ns: &str, pod: &str) -> Vec<Value> {
    const FAMILIES: &[(&str, &str)] = &[
        ("container_network_receive_bytes_total", "rxBytes"),
        ("container_network_transmit_bytes_total", "txBytes"),
        ("container_network_receive_packets_total", "rxPackets"),
        ("container_network_transmit_packets_total", "txPackets"),
        ("container_network_receive_errors_total", "rxErrors"),
        ("container_network_transmit_errors_total", "txErrors"),
        ("container_network_receive_packets_dropped_total", "rxDropped"),
        ("container_network_transmit_packets_dropped_total", "txDropped"),
    ];
    let mut by_iface: std::collections::BTreeMap<String, serde_json::Map<String, Value>> = Default::default();
    for line in metrics.lines() {
        if line.starts_with('#') {
            continue;
        }
        let Some((name, rest)) = line.split_once('{') else { continue };
        let Some((_, field)) = FAMILIES.iter().find(|(f, _)| *f == name) else { continue };
        let Some((labels, value)) = rest.rsplit_once('}') else { continue };
        let labels = parse_labels(labels);
        if labels.get("namespace").map(String::as_str) != Some(ns) || labels.get("pod").map(String::as_str) != Some(pod)
        {
            continue;
        }
        let Some(v) = value.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()) else { continue };
        let iface = labels.get("interface").cloned().unwrap_or_default();
        by_iface.entry(iface).or_default().insert((*field).to_string(), json!(v as u64));
    }
    by_iface
        .into_iter()
        .map(|(iface, mut m)| {
            m.insert("interface".into(), json!(iface));
            Value::Object(m)
        })
        .collect()
}

fn parse_labels(s: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = s;
    while let Some((k, after)) = rest.split_once("=\"") {
        let mut val = String::new();
        let mut chars = after.char_indices();
        let mut end = after.len();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => {
                    if let Some((_, n)) = chars.next() {
                        val.push(match n {
                            'n' => '\n',
                            o => o,
                        });
                    }
                }
                '"' => {
                    end = i + 1;
                    break;
                }
                o => val.push(o),
            }
        }
        out.insert(k.trim_start_matches(',').trim().to_string(), val);
        rest = &after[end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap_with(kind: &'static str, items: Vec<(&str, Value)>) -> Snapshot {
        let mut snap = Snapshot::new();
        snap.insert(kind, items.into_iter().map(|(k, v)| (k.to_string(), v)).collect());
        snap
    }

    #[test]
    fn a_digest_is_found_wherever_it_is_written() {
        let hex = "a".repeat(64);
        assert_eq!(digest_of(&format!("docker.io/x@sha256:{hex}")), Some(format!("sha256:{hex}")));
        assert_eq!(digest_of(&format!("sha256:{hex}")), Some(format!("sha256:{hex}")));
        assert_eq!(digest_of("stormpump://cilium"), None);
        assert_eq!(digest_of("sha256:abc"), None);
    }

    #[test]
    fn pull_policy_defaults_as_kubernetes_does() {
        assert_eq!(pull_policy(&json!({"image": "nginx"})), ("Always".into(), true));
        assert_eq!(pull_policy(&json!({"image": "nginx:latest"})), ("Always".into(), true));
        assert_eq!(pull_policy(&json!({"image": "reg:5000/nginx:1.2"})), ("IfNotPresent".into(), true));
        assert_eq!(pull_policy(&json!({"image": "reg:5000/nginx"})), ("Always".into(), true));
        assert_eq!(pull_policy(&json!({"image": "x", "imagePullPolicy": "Never"})), ("Never".into(), false));
    }

    #[test]
    fn qos_is_computed_when_not_reported() {
        let g = json!({"spec":{"containers":[{"resources":{"limits":{"cpu":"1","memory":"1Gi"}}}]}});
        assert_eq!(qos_class(&g), "Guaranteed");
        let b = json!({"spec":{"containers":[{"resources":{"requests":{"cpu":"1"}}}]}});
        assert_eq!(qos_class(&b), "Burstable");
        let e = json!({"spec":{"containers":[{"name":"a"}]}});
        assert_eq!(qos_class(&e), "BestEffort");
        let r = json!({"spec":{"containers":[]},"status":{"qosClass":"Burstable"}});
        assert_eq!(qos_class(&r), "Burstable");
    }

    fn crashing_pod() -> Value {
        json!({
            "metadata": {"name": "web-1", "namespace": "shop", "labels": {"app": "web", "tier": "fe"},
                         "annotations": {"kubectl.kubernetes.io/last-applied-configuration": "{…}", "note": "x"}},
            "spec": {
                "nodeName": "n1",
                "initContainers": [{"name": "init", "image": "busybox:1.36"}],
                "containers": [
                    {"name": "app", "image": "reg/web:2", "ports": [{"containerPort": 8080}]},
                    {"name": "agent", "image": "stormpump://cilium"}
                ]
            },
            "status": {
                "phase": "Running",
                "podIP": "10.0.0.5", "podIPs": [{"ip": "10.0.0.5"}, {"ip": "fd00::5"}],
                "hostIP": "",
                "initContainerStatuses": [{"name": "init", "image": "busybox:1.36", "imageID": format!("docker.io/library/busybox@sha256:{}", "b".repeat(64)),
                    "restartCount": 0, "state": {"terminated": {"exitCode": 0, "reason": "Completed"}}}],
                "containerStatuses": [
                    {"name": "app", "image": "reg/web:2", "imageID": format!("sha256:{}", "c".repeat(64)), "restartCount": 3, "ready": false,
                     "state": {"waiting": {"reason": "CrashLoopBackOff"}}},
                    {"name": "agent", "image": "stormpump://cilium", "imageID": "stormpump://cilium", "restartCount": 0, "ready": true,
                     "state": {"running": {"startedAt": "2026-10-02T10:00:00Z"}}}
                ]
            }
        })
    }

    #[test]
    fn containers_carry_spec_and_status_side_by_side() {
        let c = containers(&crashing_pod());
        assert_eq!(c.len(), 3);
        assert_eq!(c[0]["role"], "init");
        assert_eq!(c[0]["digest"], format!("sha256:{}", "b".repeat(64)));
        assert_eq!(c[1]["name"], "app");
        assert_eq!(c[1]["restartCount"], 3);
        assert_eq!(c[1]["state"]["kind"], "waiting");
        assert_eq!(c[1]["state"]["reason"], "CrashLoopBackOff");
        assert_eq!(c[1]["ports"][0]["protocol"], "TCP");
        assert!(c[1]["lastState"].is_null());
        assert_eq!(c[2]["source"], "stormpump");
        assert!(c[2]["digest"].is_null());
    }

    #[test]
    fn gaps_name_what_the_node_does_not_report() {
        let pod = crashing_pod();
        let g = gaps(&pod, &containers(&pod));
        let whats: Vec<&str> = g.iter().map(|g| g["what"].as_str().unwrap()).collect();
        assert!(whats.contains(&"image digest"));
        assert!(g[0]["why"].as_str().unwrap().contains("agent"));
        assert!(whats.contains(&"last termination"));
        assert!(whats.contains(&"interface detail"));
        assert!(g.iter().all(|g| g["issue"].as_str().unwrap().starts_with("rustkube-node#")));
    }

    #[test]
    fn services_that_select_the_pod_and_only_those() {
        let snap = snap_with(
            "svc",
            vec![
                ("shop/web", json!({"metadata":{"name":"web"},"spec":{"selector":{"app":"web"},"clusterIP":"10.96.0.10","ports":[{"port":80,"targetPort":8080}]}})),
                ("shop/db", json!({"metadata":{"name":"db"},"spec":{"selector":{"app":"db"}}})),
                ("shop/manual", json!({"metadata":{"name":"manual"},"spec":{}})),
                ("other/web", json!({"metadata":{"name":"web"},"spec":{"selector":{"app":"web"}}})),
            ],
        );
        let s = services(&snap, "shop", &crashing_pod());
        assert_eq!(s.len(), 1);
        assert_eq!(s[0]["id"], "k8s:svc:shop/web");
        assert_eq!(s[0]["clusterIPs"][0], "10.96.0.10");
    }

    #[test]
    fn endpoints_say_whether_this_pod_takes_traffic() {
        let eps = json!({"subsets":[{"addresses":[{"ip":"10.0.0.9"}],"notReadyAddresses":[{"ip":"10.0.0.5","targetRef":{"name":"web-1"}}]}]});
        let e = endpoints_summary(&eps, "web-1", &["10.0.0.5".into()]);
        assert_eq!(e, json!({"ready":1,"notReady":1,"thisPod":"notReady"}));
        let e = endpoints_summary(&json!({}), "web-1", &[]);
        assert_eq!(e["thisPod"], "absent");
    }

    #[test]
    fn detail_fills_a_mirror_pods_host_ip_from_its_node() {
        let mut snap = snap_with(
            "node",
            vec![("n1", json!({"status":{"addresses":[{"type":"Hostname","address":"n1"},{"type":"InternalIP","address":"192.168.8.106"}]}}))],
        );
        snap.insert("svc", HashMap::new());
        let d = detail(&snap, "shop", &crashing_pod());
        assert_eq!(d["network"]["hostIP"], "192.168.8.106");
        assert_eq!(d["network"]["podIPs"], json!(["10.0.0.5", "fd00::5"]));
        assert_eq!(d["network"]["dns"]["effective"], "ClusterFirst");
        assert!(d["metadata"]["annotations"].get("kubectl.kubernetes.io/last-applied-configuration").is_none());
        assert_eq!(d["metadata"]["qosClass"], "BestEffort");
    }

    #[test]
    fn host_network_dns_falls_back_to_the_node() {
        let d = dns(&json!({"spec":{"hostNetwork":true}}));
        assert_eq!(d["effective"], "Default (host network)");
    }

    #[test]
    fn traffic_is_read_per_interface_for_this_pod_only() {
        let text = r#"# HELP container_network_receive_bytes_total Cumulative count of bytes received.
# TYPE container_network_receive_bytes_total counter
container_network_receive_bytes_total{container="",id="abc",interface="eth0",namespace="shop",pod="web-1"} 1234
container_network_receive_bytes_total{container="",id="def",interface="eth0",namespace="shop",pod="web-2"} 9
container_network_transmit_bytes_total{container="",id="abc",interface="eth0",namespace="shop",pod="web-1"} 5.678e3
container_network_transmit_bytes_total{container="",id="abc",interface="net1",namespace="shop",pod="web-1"} 7
container_memory_working_set_bytes{container="app",id="abc",namespace="shop",pod="web-1"} 1
"#;
        let t = traffic(text, "shop", "web-1");
        assert_eq!(t.len(), 2);
        assert_eq!(t[0]["interface"], "eth0");
        assert_eq!(t[0]["rxBytes"], 1234);
        assert_eq!(t[0]["txBytes"], 5678);
        assert_eq!(t[1]["interface"], "net1");
        assert!(t[1].get("rxBytes").is_none());
    }

    #[test]
    fn label_values_unescape() {
        let l = parse_labels(r#"a="x\"y",b="",c="z""#);
        assert_eq!(l["a"], "x\"y");
        assert_eq!(l["b"], "");
        assert_eq!(l["c"], "z");
    }

    #[test]
    fn owners_link_where_the_console_has_a_page() {
        assert_eq!(owner_href("Deployment", "shop", "web").unwrap(), "#/grid?id=k8s:deploy:shop/web");
        assert_eq!(owner_href("VirtualMachineInstance", "shop", "vm1").unwrap(), "#/vm/shop/vm1");
        assert!(owner_href("ReplicaSet", "shop", "web-abc").is_none());
    }
}
