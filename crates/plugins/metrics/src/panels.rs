//! What is charted, and how an answer becomes a chart. No I/O here.
//!
//! **The queries live here and nowhere else.** The page asks for a level or an object, never for a
//! query: ironprom answers anything to anyone who can reach it, and a free query would read every
//! project's series past the console's access rule. `__NS__` is where a query that names projects
//! takes the viewer's, so it cannot be forgotten: [`query`] refuses to run one without an answer.
//!
//! Series and labels are the exporters' own, read from their code (stormdrive `metrics.rs`, stormblock,
//! rustkube-node `docs/metrics.md`, rustkube `docs/metrics.md`, fastetcd `metrics.rs`, stormvm
//! `metrics.rs`); job names are stormcos's `deploy/metrics/ironprom-node.yml`.

use serde_json::{json, Value};

/// How many lines a chart draws: past this it stops being readable, and says so.
pub const MAX_SERIES: usize = 12;

pub struct Panel {
    pub id: &'static str,
    pub title: &'static str,
    /// celsius, percent, bytes, bytes_per_sec, per_sec, seconds, cores.
    pub unit: &'static str,
    /// The job that scrapes it, which is what explains an empty chart.
    pub job: &'static str,
    pub query: &'static str,
    /// The labels a line is named by.
    pub legend: &'static [&'static str],
}

pub struct Level {
    pub id: &'static str,
    pub title: &'static str,
    /// What is not here and why, shown on the level.
    pub note: &'static str,
    pub panels: &'static [Panel],
}

const CADVISOR: &str = "kubelet-cadvisor";

// The latency panel leaves out watch and connect, which stay open for as long as they are wanted.
// rustkube spells verbs in lower case and upstream in upper, so both are named.

pub const LEVELS: &[Level] = &[
    Level {
        id: "drives",
        title: "Drives",
        note: "",
        panels: &[
            Panel { id: "temperature", title: "Hottest drives", unit: "celsius", job: "stormdrive",
                query: r#"topk(12, smartctl_device_temperature{job="stormdrive",temperature_type="current"})"#, legend: &["device", "serial"] },
            Panel { id: "wear", title: "Most worn drives", unit: "percent", job: "stormdrive",
                query: r#"topk(12, smartctl_device_percentage_used{job="stormdrive"})"#, legend: &["device", "serial"] },
            Panel { id: "errors", title: "Drive IO errors", unit: "per_sec", job: "stormdrive",
                query: r#"topk(12, rate(stormdrive_drive_io_errors_total{job="stormdrive"}[5m]))"#, legend: &["device", "serial"] },
        ],
    },
    Level {
        id: "volumes",
        title: "Volumes",
        note: "The engine reports IO by operation, across every volume it exports: it has no series per volume yet (stormblock#536).",
        panels: &[
            Panel { id: "latency", title: "Volume IO latency, p99", unit: "seconds", job: "stormblock",
                query: r#"histogram_quantile(0.99, sum by (le, op) (rate(stormblock_nvmeof_io_seconds_bucket{job="stormblock"}[5m])))"#, legend: &["op"] },
            Panel { id: "rate", title: "Volume IO rate", unit: "per_sec", job: "stormblock",
                query: r#"sum by (op) (rate(stormblock_nvmeof_io_seconds_count{job="stormblock"}[5m]))"#, legend: &["op"] },
        ],
    },
    Level {
        id: "containers",
        title: "Containers",
        note: "",
        panels: &[
            Panel { id: "cpu", title: "Container CPU", unit: "cores", job: CADVISOR,
                query: r#"topk(10, sum by (namespace, pod, container) (rate(container_cpu_usage_seconds_total{job="kubelet-cadvisor",container!=""__NS__}[5m])))"#,
                legend: &["namespace", "pod", "container"] },
            Panel { id: "memory", title: "Container memory", unit: "bytes", job: CADVISOR,
                query: r#"topk(10, sum by (namespace, pod, container) (container_memory_working_set_bytes{job="kubelet-cadvisor",container!=""__NS__}))"#,
                legend: &["namespace", "pod", "container"] },
        ],
    },
    Level {
        id: "apiserver",
        title: "API server",
        note: "",
        panels: &[
            Panel { id: "rate", title: "Request rate", unit: "per_sec", job: "apiserver",
                query: r#"sum by (verb) (rate(apiserver_request_total{job="apiserver"}[5m]))"#, legend: &["verb"] },
            Panel { id: "latency", title: "Request latency, p99", unit: "seconds", job: "apiserver",
                query: r#"histogram_quantile(0.99, sum by (le, verb) (rate(apiserver_request_duration_seconds_bucket{job="apiserver",verb!~"watch|connect|WATCH|CONNECT"}[5m])))"#, legend: &["verb"] },
        ],
    },
    Level {
        id: "datastore",
        title: "Datastore",
        note: "",
        panels: &[
            Panel { id: "size", title: "Database size", unit: "bytes", job: "fastetcd",
                query: r#"etcd_mvcc_db_total_size_in_bytes{job="fastetcd"}"#, legend: &["instance"] },
            Panel { id: "puts", title: "Puts", unit: "per_sec", job: "fastetcd",
                query: r#"rate(etcd_debugging_mvcc_put_total{job="fastetcd"}[5m])"#, legend: &["instance"] },
            Panel { id: "ranges", title: "Ranges", unit: "per_sec", job: "fastetcd",
                query: r#"rate(etcd_debugging_mvcc_range_total{job="fastetcd"}[5m])"#, legend: &["instance"] },
            Panel { id: "get", title: "Single-key read, p99", unit: "seconds", job: "fastetcd",
                query: r#"histogram_quantile(0.99, sum by (le, cache) (rate(fastetcd_mvcc_get_duration_seconds_bucket{job="fastetcd"}[5m])))"#, legend: &["cache"] },
        ],
    },
    Level {
        id: "vms",
        title: "Virtual machines",
        note: "",
        panels: &[
            Panel { id: "cpu", title: "VM CPU", unit: "cores", job: "stormvm",
                query: r#"topk(10, sum by (namespace, name) (rate(kubevirt_vmi_vcpu_seconds_total{job="stormvm"__NS__}[5m])))"#, legend: &["namespace", "name"] },
            Panel { id: "read", title: "VM disk reads", unit: "bytes_per_sec", job: "stormvm",
                query: r#"topk(10, sum by (namespace, name) (rate(kubevirt_vmi_storage_read_traffic_bytes_total{job="stormvm"__NS__}[5m])))"#, legend: &["namespace", "name"] },
            Panel { id: "write", title: "VM disk writes", unit: "bytes_per_sec", job: "stormvm",
                query: r#"topk(10, sum by (namespace, name) (rate(kubevirt_vmi_storage_write_traffic_bytes_total{job="stormvm"__NS__}[5m])))"#, legend: &["namespace", "name"] },
        ],
    },
    Level {
        id: "network",
        title: "Network",
        note: "",
        panels: &[
            Panel { id: "drops", title: "Packets dropped", unit: "per_sec", job: "cilium-agent",
                query: r#"topk(10, sum by (reason, direction) (rate(cilium_drop_count_total{job="cilium-agent"}[5m])))"#, legend: &["reason", "direction"] },
        ],
    },
];

const DRIVE: &[Panel] = &[
    Panel { id: "temperature", title: "Temperature", unit: "celsius", job: "stormdrive",
        query: r#"smartctl_device_temperature{job="stormdrive",temperature_type="current",__THE__}"#, legend: &["device"] },
    Panel { id: "wear", title: "Wear", unit: "percent", job: "stormdrive",
        query: r#"smartctl_device_percentage_used{job="stormdrive",__THE__}"#, legend: &["device"] },
    Panel { id: "errors", title: "IO errors", unit: "per_sec", job: "stormdrive",
        query: r#"rate(stormdrive_drive_io_errors_total{job="stormdrive",__THE__}[5m])"#, legend: &["device"] },
];

const POD: &[Panel] = &[
    Panel { id: "cpu", title: "CPU", unit: "cores", job: CADVISOR,
        query: r#"sum by (container) (rate(container_cpu_usage_seconds_total{job="kubelet-cadvisor",container!="",__THE__}[5m]))"#, legend: &["container"] },
    Panel { id: "memory", title: "Memory", unit: "bytes", job: CADVISOR,
        query: r#"sum by (container) (container_memory_working_set_bytes{job="kubelet-cadvisor",container!="",__THE__})"#, legend: &["container"] },
    Panel { id: "rx", title: "Network received", unit: "bytes_per_sec", job: CADVISOR,
        query: r#"sum by (interface) (rate(container_network_receive_bytes_total{job="kubelet-cadvisor",__THE__}[5m]))"#, legend: &["interface"] },
    Panel { id: "tx", title: "Network sent", unit: "bytes_per_sec", job: CADVISOR,
        query: r#"sum by (interface) (rate(container_network_transmit_bytes_total{job="kubelet-cadvisor",__THE__}[5m]))"#, legend: &["interface"] },
];

const VM: &[Panel] = &[
    Panel { id: "cpu", title: "CPU", unit: "cores", job: "stormvm",
        query: r#"sum by (id) (rate(kubevirt_vmi_vcpu_seconds_total{job="stormvm",__THE__}[5m]))"#, legend: &["id"] },
    Panel { id: "memory", title: "Memory in use on the node", unit: "bytes", job: "stormvm",
        query: r#"kubevirt_vmi_memory_resident_bytes{job="stormvm",__THE__}"#, legend: &["name"] },
    Panel { id: "read", title: "Disk reads", unit: "bytes_per_sec", job: "stormvm",
        query: r#"sum by (drive) (rate(kubevirt_vmi_storage_read_traffic_bytes_total{job="stormvm",__THE__}[5m]))"#, legend: &["drive"] },
    Panel { id: "write", title: "Disk writes", unit: "bytes_per_sec", job: "stormvm",
        query: r#"sum by (drive) (rate(kubevirt_vmi_storage_write_traffic_bytes_total{job="stormvm",__THE__}[5m]))"#, legend: &["drive"] },
    Panel { id: "rx", title: "Network received", unit: "bytes_per_sec", job: "stormvm",
        query: r#"sum by (interface) (rate(kubevirt_vmi_network_receive_bytes_total{job="stormvm",__THE__}[5m]))"#, legend: &["interface"] },
    Panel { id: "tx", title: "Network sent", unit: "bytes_per_sec", job: "stormvm",
        query: r#"sum by (interface) (rate(kubevirt_vmi_network_transmit_bytes_total{job="stormvm",__THE__}[5m]))"#, legend: &["interface"] },
];

/// One object's series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entity {
    Drive { serial: String },
    Pod { ns: String, name: String },
    Vm { ns: String, name: String },
}

impl Entity {
    pub fn panels(&self) -> &'static [Panel] {
        match self {
            Entity::Drive { .. } => DRIVE,
            Entity::Pod { .. } => POD,
            Entity::Vm { .. } => VM,
        }
    }

    /// The project it is in, which decides whether this viewer may see it.
    pub fn namespace(&self) -> Option<&str> {
        match self {
            Entity::Drive { .. } => None,
            Entity::Pod { ns, .. } | Entity::Vm { ns, .. } => Some(ns),
        }
    }

    fn selector(&self) -> String {
        match self {
            Entity::Drive { serial } => format!("serial={}", quote(serial)),
            Entity::Pod { ns, name } => format!("namespace={},pod={}", quote(ns), quote(name)),
            Entity::Vm { ns, name } => format!("namespace={},name={}", quote(ns), quote(name)),
        }
    }

    pub fn title(&self) -> String {
        match self {
            Entity::Drive { serial } => format!("Drive {serial}"),
            Entity::Pod { ns, name } => format!("Pod {name} in {ns}"),
            Entity::Vm { ns, name } => format!("Virtual machine {name} in {ns}"),
        }
    }
}

/// A PromQL string. A serial comes from a drive, a name from a URL: neither may end the string.
fn quote(v: &str) -> String {
    let mut out = String::with_capacity(v.len() + 2);
    out.push('"');
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The query for this panel: for this object, within these projects (`None`: every project).
/// `Err` when the viewer has no project to read it in, which is an answer and not a failure.
pub fn query(panel: &Panel, entity: Option<&Entity>, visible: Option<&[String]>) -> Result<String, String> {
    let mut q = panel.query.to_string();
    if let Some(e) = entity {
        q = q.replace("__THE__", &e.selector());
    }
    if q.contains("__NS__") {
        let matcher = match visible {
            None => String::new(),
            Some(names) => {
                // A project's name is a DNS label. Anything else is left out, not escaped.
                let names: Vec<&str> = names
                    .iter()
                    .map(String::as_str)
                    .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
                    .collect();
                if names.is_empty() {
                    return Err("none of your projects is visible here".to_string());
                }
                format!(r#",namespace=~"{}""#, names.join("|"))
            }
        };
        q = q.replace("__NS__", &matcher);
    }
    Ok(q)
}

/// A line's name, from the labels the panel names: what the series has, joined.
fn line_name(metric: &Value, legend: &[&str]) -> String {
    let parts: Vec<&str> = legend.iter().filter_map(|l| metric.get(*l)?.as_str()).filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        "total".to_string()
    } else {
        parts.join(" / ")
    }
}

/// A sample. `NaN` is what a quantile answers with no traffic, and it is a gap and not a value.
fn sample(pair: &Value) -> Option<Value> {
    let t = pair.get(0)?.as_f64()?;
    let v = pair.get(1)?.as_str()?.parse::<f64>().ok().filter(|v| v.is_finite());
    Some(json!({"t": (t * 1000.0) as i64, "v": v}))
}

/// `data.result` of a range query as lines, each named and in a fixed order, so a line keeps its colour
/// from one refresh to the next. Returns the lines and how many were left out.
pub fn lines(result: &[Value], legend: &[&str]) -> (Vec<Value>, usize) {
    let mut named: Vec<(String, Vec<Value>)> = result
        .iter()
        .map(|s| {
            let name = line_name(s.get("metric").unwrap_or(&Value::Null), legend);
            let points = s.get("values").and_then(Value::as_array).map(|a| a.iter().filter_map(sample).collect()).unwrap_or_default();
            (name, points)
        })
        .collect();
    named.sort_by(|a, b| a.0.cmp(&b.0));
    let over = named.len().saturating_sub(MAX_SERIES);
    named.truncate(MAX_SERIES);
    let out = named.into_iter().enumerate().map(|(i, (name, points))| json!({"name": name, "slot": i, "points": points})).collect();
    (out, over)
}

/// How far apart the points of a window are: about 120 of them, and no finer than a scrape.
pub fn step_for(window: i64) -> i64 {
    (window / 120).max(30)
}

/// What the page may ask for: from 15 minutes to a week.
pub fn clamp_window(window: Option<i64>) -> i64 {
    window.unwrap_or(3600).clamp(900, 7 * 86400)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(level: &str, id: &str) -> &'static Panel {
        LEVELS.iter().find(|l| l.id == level).unwrap().panels.iter().find(|p| p.id == id).unwrap()
    }

    #[test]
    fn every_query_is_filled_and_scraped_by_its_job() {
        let e = Entity::Pod { ns: "a".into(), name: "b".into() };
        for p in LEVELS.iter().flat_map(|l| l.panels) {
            let q = query(p, None, None).unwrap();
            assert!(!q.contains("__"), "{q}");
            assert!(q.contains(&format!(r#"job="{}""#, p.job)), "{q}");
        }
        for p in [DRIVE, POD, VM].into_iter().flatten() {
            let q = query(p, Some(&e), None).unwrap();
            assert!(!q.contains("__THE__"), "{q}");
            assert!(q.contains(&format!(r#"job="{}""#, p.job)), "{q}");
        }
    }

    #[test]
    fn a_project_series_carries_the_viewers_projects() {
        let p = panel("containers", "cpu");
        let every = query(p, None, None).unwrap();
        assert!(!every.contains("namespace=~"), "{every}");
        let two = vec!["shop".to_string(), "lab-1".to_string()];
        let q = query(p, None, Some(&two)).unwrap();
        assert!(q.contains(r#"container!="",namespace=~"shop|lab-1"}"#), "{q}");
        assert!(query(p, None, Some(&[])).is_err(), "no project: nothing is asked");
        let odd = vec![r#"x"}|up{a="#.to_string()];
        assert!(query(p, None, Some(&odd)).is_err(), "a name that is not a project's is left out");
        // A level with no project in it is not narrowed, and not refused.
        assert!(query(panel("drives", "wear"), None, Some(&[])).is_ok());
    }

    #[test]
    fn every_panel_over_projects_is_narrowed() {
        // Containers and VMs carry a namespace; the rest of the levels' series do not.
        for l in LEVELS {
            for p in l.panels {
                assert_eq!(p.query.contains("__NS__"), p.query.contains("namespace,"), "{}", p.query);
            }
        }
    }

    #[test]
    fn an_object_cannot_end_its_string() {
        let e = Entity::Drive { serial: r#"Z"} or up{a="\"#.into() };
        let q = query(&DRIVE[0], Some(&e), None).unwrap();
        assert!(q.contains(r#"serial="Z\"} or up{a=\"\\"}"#), "{q}");
    }

    #[test]
    fn a_range_answer_becomes_named_lines() {
        let result = serde_json::from_value::<Vec<Value>>(json!([
            {"metric": {"verb": "POST"}, "values": [[100, "1.5"], [130.5, "NaN"]]},
            {"metric": {"verb": "GET"}, "values": [[100, "2"]]},
            {"metric": {}, "values": []}
        ]))
        .unwrap();
        let (l, over) = lines(&result, &["verb"]);
        assert_eq!(over, 0);
        assert_eq!(l.iter().map(|s| s["name"].as_str().unwrap()).collect::<Vec<_>>(), ["GET", "POST", "total"]);
        assert_eq!(l[1]["points"][0], json!({"t": 100000, "v": 1.5}));
        assert_eq!(l[1]["points"][1], json!({"t": 130500, "v": null}), "NaN is a gap");
    }

    #[test]
    fn too_many_lines_are_counted() {
        let result: Vec<Value> = (0..15).map(|i| json!({"metric": {"a": format!("{i:02}")}, "values": []})).collect();
        let (l, over) = lines(&result, &["a"]);
        assert_eq!((l.len(), over), (MAX_SERIES, 3));
    }

    #[test]
    fn windows_and_steps() {
        assert_eq!(clamp_window(None), 3600);
        assert_eq!(clamp_window(Some(1)), 900);
        assert_eq!(clamp_window(Some(i64::MAX)), 7 * 86400);
        assert_eq!(step_for(900), 30);
        assert_eq!(step_for(86400), 720);
    }
}
