//! The registry's catalog (#19): goldens, blanks and media are images, and
//! they are the registry's to show.
//!
//! sbregistry (v0.22.0, the `component` kind from v0.23.0) derives `/v1/catalog/images` from the engine's
//! listings, its own records and what the OS releases say — nothing is
//! copied, a golden stays the engine's volume. Each entry becomes a
//! `reg:cat:<name>` component (`reg:cat:<location>/<name>` for a peer's), with
//! its base as an upward edge so lineage reads image → base, and its clones
//! counted. `/v1/media/jobs` is merged in, so an image still arriving says
//! where from and how far along, and a failed fetch says whose fault it was.

use console_core::value::{field, human_bytes, u64_field};
use console_core::{ComponentSummary, Health, Metric, Relation};
use serde_json::Value;

/// The catalog's kinds, in the order the page groups them.
pub const KINDS: &[&str] =
    &["component", "blank", "media", "golden", "base", "slab_golden", "release_part", "sealed"];

pub fn id(v: &Value) -> String {
    let name = field(v, &["name"]).unwrap_or_default();
    match field(v, &["location"]).filter(|l| l != "local") {
        Some(loc) => format!("reg:cat:{loc}/{name}"),
        None => format!("reg:cat:{name}"),
    }
}

/// A job's progress in words: "downloading from mirror.example, 42%".
pub fn progress(job: &Value) -> (Health, String) {
    let phase = field(job, &["phase"]).unwrap_or_default();
    let source = field(job, &["source"]).unwrap_or_default();
    let host = source
        .split("://")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .map(str::to_string)
        .unwrap_or_else(|| source.clone());
    let pct = job.get("percent").and_then(Value::as_f64);
    let bytes = u64_field(job, "bytes").unwrap_or(0);
    match phase.as_str() {
        "fetching" => (
            Health::Warn,
            match pct {
                Some(p) => format!("downloading from {host}, {p:.0}%"),
                None => format!("downloading from {host}, {} so far", human_bytes(bytes)),
            },
        ),
        "importing" => (Health::Warn, format!("importing into the engine (from {host})")),
        "failed" => {
            let fault = field(job, &["fault"]).unwrap_or_else(|| "unknown".into());
            let err = field(job, &["error"]).unwrap_or_default();
            (Health::Error, format!("failed ({fault}): {err}"))
        }
        _ => (Health::Ok, format!("fetched from {host}")),
    }
}

fn active(job: &Value) -> bool {
    matches!(field(job, &["phase"]).as_deref(), Some("fetching" | "importing" | "failed"))
}

/// One catalog image. `jobs` are the registry's media jobs; one naming this
/// image and not done is what the row says first.
pub fn image(v: &Value, jobs: &[Value]) -> ComponentSummary {
    let name = field(v, &["name"]).unwrap_or_default();
    let kind = field(v, &["kind"]).unwrap_or_else(|| "sealed".into());
    let sealed = v.get("sealed").and_then(Value::as_bool).unwrap_or(true);
    let size = u64_field(v, "virtual_size_bytes").map(human_bytes);
    let alloc = u64_field(v, "allocated_bytes").map(human_bytes);
    let clones = v.get("clones").and_then(Value::as_u64).unwrap_or(0);
    let clone_names: Vec<String> = v
        .get("clone_names")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let releases: Vec<String> = v
        .get("releases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();

    let (mut health, mut first) = if sealed {
        (Health::Ok, String::new())
    } else {
        // A template still being written.
        (Health::Warn, format!("being written{}", field(v, &["state"]).map(|s| format!(" ({s})")).unwrap_or_default()))
    };
    if let Some(j) = jobs.iter().find(|j| field(j, &["golden"]).as_deref() == Some(name.as_str()) && active(j)) {
        let (h, words) = progress(j);
        health = h;
        first = words;
    }

    let what = field(v, &["component"])
        .or_else(|| field(v, &["source"]))
        .unwrap_or_default();
    let mut detail: Vec<String> = vec![kind.clone()];
    if !first.is_empty() {
        detail.insert(0, first);
    }
    if !what.is_empty() {
        detail.push(what);
    }
    if let Some(s) = &size {
        detail.push(s.clone());
    }

    let mut metrics = vec![Metric::new("kind", kind.clone()).tone("accent")];
    metrics.push(
        Metric::new("clones", clones.to_string()).tone(if clones > 0 { "ok" } else { "muted" }),
    );
    if let Some(p) = field(v, &["parent"]) {
        metrics.push(Metric::new("base", p).tone("muted"));
    }
    if let Some(r) = releases.last() {
        metrics.push(Metric::new(
            "releases",
            if releases.len() > 1 { format!("{} (+{} older)", r, releases.len() - 1) } else { r.clone() },
        ));
    }
    if let Some(a) = alloc {
        metrics.push(Metric::new("allocated", a).tone("muted"));
    }
    if let Some(d) = field(v, &["digest"]) {
        let short = d.split(':').nth(1).unwrap_or(&d).chars().take(12).collect::<String>();
        metrics.push(Metric::new("digest", short).tone("muted"));
    }
    if let Some(l) = field(v, &["location"]).filter(|l| l != "local") {
        metrics.push(Metric::new("location", l).tone("warn"));
    }
    if !clone_names.is_empty() {
        let shown: Vec<&str> = clone_names.iter().take(4).map(String::as_str).collect();
        let more = clone_names.len().saturating_sub(shown.len());
        metrics.push(
            Metric::new(
                "cloned as",
                if more > 0 { format!("{} (+{more})", shown.join(", ")) } else { shown.join(", ") },
            )
            .tone("muted"),
        );
    }

    let mut relations = vec![Relation::belongs_to("registry", "reg:registry")];
    if let Some(p) = field(v, &["parent"]) {
        // Upward: an image is built on its base, and does not contain it.
        let base = match field(v, &["location"]).filter(|l| l != "local") {
            Some(loc) => format!("reg:cat:{loc}/{p}"),
            None => format!("reg:cat:{p}"),
        };
        relations.push(Relation::belongs_to("base", base));
    }
    if let Some(vid) = field(v, &["volume_id"]) {
        // The engine volume it is, underneath — nothing moved.
        relations.push(Relation::belongs_to("volume", format!("sb:volume:{vid}")));
    }

    ComponentSummary {
        id: id(v),
        kind: "catalog-image".into(),
        label: name,
        health,
        detail: detail.join(" · "),
        metrics,
        actions: vec![],
        relations,
        link: None,
    }
}

/// A media job with no image to belong to yet — a fetch still running, or
/// one that failed before anything was imported.
pub fn orphan_job(j: &Value, known: &[String]) -> Option<ComponentSummary> {
    if !active(j) {
        return None;
    }
    if let Some(g) = field(j, &["golden"]) {
        if known.iter().any(|k| k == &g) {
            return None;
        }
    }
    let repo = field(j, &["repository"]).unwrap_or_default();
    let reference = field(j, &["reference"]).unwrap_or_else(|| "latest".into());
    let (health, words) = progress(j);
    let mut metrics = vec![Metric::new("kind", "media").tone("accent")];
    if let Some(a) = j.get("attempts").and_then(Value::as_u64).filter(|a| *a > 1) {
        metrics.push(Metric::new("attempts", a.to_string()).tone("warn"));
    }
    if let Some(r) = u64_field(j, "resumed_from").filter(|r| *r > 0) {
        metrics.push(Metric::new("resumed from", human_bytes(r)).tone("muted"));
    }
    Some(ComponentSummary {
        id: format!("reg:job:{repo}:{reference}"),
        kind: "media-job".into(),
        label: format!("{repo}:{reference}"),
        health,
        detail: words,
        metrics,
        actions: vec![],
        relations: vec![Relation::belongs_to("registry", "reg:registry")],
        link: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_image_reads_as_its_kind_lineage_and_clones() {
        let v = json!({
            "name": "img-3f9a0c1d2e4b-root", "kind": "golden", "sealed": true,
            "source": "nginx:1.27", "digest": "sha256:3f9a0c1d2e4b5566",
            "parent": "base-aa11bb22cc33", "virtual_size_bytes": 1073741824u64,
            "allocated_bytes": 52428800u64, "clones": 3,
            "clone_names": ["web-1", "web-2", "web-3"], "releases": ["11.40", "11.43"],
            "volume_id": "9c2e", "location": "local"
        });
        let c = image(&v, &[]);
        assert_eq!(c.id, "reg:cat:img-3f9a0c1d2e4b-root");
        assert_eq!(c.health, Health::Ok);
        assert_eq!(c.detail, "golden · nginx:1.27 · 1.0 GB");
        let m = |l: &str| c.metrics.iter().find(|x| x.label == l).map(|x| x.value.clone());
        assert_eq!(m("clones"), Some("3".into()));
        assert_eq!(m("releases"), Some("11.43 (+1 older)".into()));
        assert_eq!(m("digest"), Some("3f9a0c1d2e4b".into()));
        assert!(c.relations.iter().any(|r| r.name == "base" && r.targets == vec!["reg:cat:base-aa11bb22cc33"]));
        assert!(c.relations.iter().any(|r| r.name == "volume" && r.targets == vec!["sb:volume:9c2e"]));
    }

    #[test]
    fn a_peers_image_is_its_own_and_says_where() {
        let c = image(&json!({"name": "golden-stormlb", "kind": "component", "component": "stormlb", "location": "forge"}), &[]);
        assert_eq!(c.id, "reg:cat:forge/golden-stormlb");
        assert!(c.metrics.iter().any(|m| m.label == "location" && m.value == "forge"));
        assert!(c.detail.contains("stormlb"));
    }

    #[test]
    fn an_image_arriving_says_from_where_and_how_far() {
        let jobs = vec![json!({"repository": "media/rocky", "reference": "9.4", "phase": "fetching",
            "source": "https://dl.rockylinux.org/pub/rocky/9/x.qcow2", "bytes": 100, "percent": 42.4,
            "attempts": 1, "resumed_from": 0, "golden": "media-abc"})];
        let c = image(&json!({"name": "media-abc", "kind": "media", "sealed": false, "state": "writing"}), &jobs);
        assert_eq!(c.health, Health::Warn);
        assert!(c.detail.starts_with("downloading from dl.rockylinux.org, 42%"), "{}", c.detail);
    }

    #[test]
    fn a_job_with_no_image_yet_is_a_row_and_a_done_one_is_not() {
        let fetching = json!({"repository": "media/deb", "reference": "12", "phase": "fetching",
            "source": "http://mirror.lan/deb.iso", "bytes": 5242880u64, "attempts": 3, "resumed_from": 1048576u64});
        let c = orphan_job(&fetching, &[]).unwrap();
        assert_eq!(c.detail, "downloading from mirror.lan, 5.0 MB so far");
        assert!(c.metrics.iter().any(|m| m.label == "attempts" && m.value == "3"));
        let failed = json!({"repository": "media/x", "phase": "failed", "source": "http://h/x",
            "fault": "upstream", "error": "404 Not Found"});
        let f = orphan_job(&failed, &[]).unwrap();
        assert_eq!(f.health, Health::Error);
        assert_eq!(f.detail, "failed (upstream): 404 Not Found");
        let done = json!({"repository": "media/y", "phase": "done", "source": "local", "golden": "media-1"});
        assert!(orphan_job(&done, &[]).is_none());
        // A job whose image is listed is that image's row, not a second one.
        let mut j = fetching.clone();
        j["golden"] = json!("media-9");
        assert!(orphan_job(&j, &["media-9".into()]).is_none());
    }
}
