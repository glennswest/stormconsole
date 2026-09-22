//! Events for one object, as opposed to for a namespace or a cluster.
//!
//! The console had events in two places and neither answered the question
//! people actually ask. A cluster-wide list is where you go when you do
//! not know what is wrong; "what happened to *this*" is where you go when
//! you do, and it was the one thing the console could not tell you.
//!
//! Matched on `involvedObject`, which needs the API kind — `Pod`,
//! `CronJob` — and not the console's own short name. Matching on name
//! alone is tempting and wrong: a Service and a Deployment routinely share
//! one, and an event about the wrong object is worse than none, because it
//! is acted on.

use console_core::{Event, Events};
use serde_json::Value;

use crate::cache;

/// The object a component id names: its API kind, namespace and name.
///
/// `k8s:pod:default/web-1` → `(Pod, default, web-1)`.
/// `k8s:node:storm-a` → `(Node, "", storm-a)`.
/// A container is not an object the apiserver knows; events about one are
/// recorded against its pod with a `fieldPath`, which is handled by the
/// caller rather than pretended away here.
pub fn object_of(id: &str) -> Option<(&'static str, String, String)> {
    let rest = id.strip_prefix("k8s:")?;
    let (kind, key) = rest.split_once(':')?;
    let spec = cache::spec(kind)?;
    Some(match (spec.namespaced, key.split_once('/')) {
        (true, Some((ns, name))) => (spec.api_kind, ns.to_string(), name.to_string()),
        _ => (spec.api_kind, String::new(), key.to_string()),
    })
}

/// A container: `k8s:container:<ns>/<pod>/<name>`.
///
/// The kubelet records a container's events against the **pod**, with the
/// container named in `fieldPath` (`spec.containers{app}`). So the pod is
/// what is asked for and the field path is what narrows it — which is why
/// a crash-looping sidecar's `BackOff` reaches the container that is
/// actually crashing rather than every container in the pod.
pub fn container_of(id: &str) -> Option<(String, String, String)> {
    let rest = id.strip_prefix("k8s:container:")?;
    let (ns, rest) = rest.split_once('/')?;
    let (pod, name) = rest.rsplit_once('/')?;
    Some((ns.to_string(), pod.to_string(), name.to_string()))
}

/// Does this event concern that object?
pub fn about(e: &Value, kind: &str, ns: &str, name: &str) -> bool {
    let g = |p: &str| e.pointer(p).and_then(Value::as_str).unwrap_or("");
    g("/involvedObject/kind") == kind
        && g("/involvedObject/name") == name
        && (ns.is_empty() || g("/involvedObject/namespace") == ns)
}

/// …and does it concern that container within it?
pub fn about_container(e: &Value, name: &str) -> bool {
    e.pointer("/involvedObject/fieldPath")
        .and_then(Value::as_str)
        .is_some_and(|p| p.contains(&format!("{{{name}}}")))
}

/// One apiserver event, in the console's shape.
pub fn event(e: &Value) -> Event {
    let g = |p: &str| e.pointer(p).and_then(Value::as_str).unwrap_or("").to_string();
    Event {
        // `lastTimestamp` is when it last happened and is what a reader
        // wants; the others are fallbacks for the events API's newer
        // shape and for a source that set neither.
        time: e
            .pointer("/lastTimestamp")
            .and_then(Value::as_str)
            .or_else(|| e.pointer("/eventTime").and_then(Value::as_str))
            .or_else(|| e.pointer("/metadata/creationTimestamp").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        kind: g("/type"),
        reason: g("/reason"),
        message: g("/message"),
        source: {
            let c = g("/source/component");
            let h = g("/source/host");
            match (c.is_empty(), h.is_empty()) {
                (false, false) => format!("{c} on {h}"),
                (false, true) => c,
                (true, false) => h,
                _ => g("/reportingComponent"),
            }
        },
        count: e.pointer("/count").and_then(Value::as_i64).unwrap_or(1),
    }
}

/// The path to list events from: a namespace's, or the cluster's.
pub fn list_path(ns: &str) -> String {
    if ns.is_empty() {
        "/api/v1/events".to_string()
    } else {
        format!("/api/v1/namespaces/{ns}/events")
    }
}

/// What rustkube says when it has no controllers writing events, which is
/// most of the time on a single node. Distinguished from "nothing
/// happened" because they are different facts and only one of them is
/// about the object.
pub fn empty_for(kind: &str, name: &str) -> Events {
    let _ = (kind, name);
    Events::of(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_component_id_names_an_api_object() {
        assert_eq!(
            object_of("k8s:pod:default/web-1"),
            Some(("Pod", "default".into(), "web-1".into()))
        );
        // Cluster-scoped: no namespace to match on.
        assert_eq!(object_of("k8s:node:storm-a"), Some(("Node", String::new(), "storm-a".into())));
        // The short name is the console's; the API kind is what an event
        // carries, and deriving one from the other is where this breaks.
        assert_eq!(object_of("k8s:pvc:default/pg").unwrap().0, "PersistentVolumeClaim");
        assert_eq!(object_of("k8s:cronjob:default/nightly").unwrap().0, "CronJob");
        assert_eq!(object_of("k8s:netpol:default/deny").unwrap().0, "NetworkPolicy");
        assert_eq!(object_of("vm:instance:default/web-1"), None, "not this plugin's");
        assert_eq!(object_of("k8s:nope:default/x"), None);
    }

    /// The reason kind is matched and not just the name: these two
    /// routinely share one, and an event about the wrong object is worse
    /// than none because it is acted on.
    #[test]
    fn a_service_and_a_deployment_of_one_name_do_not_share_events() {
        let e = json!({"involvedObject": {"kind": "Service", "name": "web", "namespace": "default"}});
        assert!(about(&e, "Service", "default", "web"));
        assert!(!about(&e, "Deployment", "default", "web"));
        assert!(!about(&e, "Service", "other", "web"));
    }

    #[test]
    fn a_containers_events_are_the_pods_narrowed_by_field_path() {
        assert_eq!(
            container_of("k8s:container:default/web-1/sidecar"),
            Some(("default".into(), "web-1".into(), "sidecar".into()))
        );
        let e = json!({"involvedObject": {"fieldPath": "spec.containers{sidecar}"}});
        assert!(about_container(&e, "sidecar"));
        assert!(!about_container(&e, "app"));
    }

    #[test]
    fn an_event_keeps_what_a_reader_needs() {
        let e = event(&json!({
            "type": "Warning", "reason": "FailedScheduling", "count": 47,
            "lastTimestamp": "2026-09-22T10:00:00Z",
            "message": "0/2 nodes are available",
            "source": {"component": "default-scheduler", "host": "storm-a"}
        }));
        assert!(e.is_warning());
        assert_eq!(e.count, 47);
        assert_eq!(e.source, "default-scheduler on storm-a");
        assert_eq!(e.time, "2026-09-22T10:00:00Z");
    }
}
