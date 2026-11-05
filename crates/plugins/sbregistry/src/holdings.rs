//! Who holds what (#163). The owner on a forge: "goldens, and who has clones"; and
//! in OCI Distribution's words: repository, tag, digest.
//!
//! **The engine is the source.** Its listing has every sealed volume, each clone's
//! `parent`, `owner` and what is serving it; `GET /api/v1/boothost` says which
//! machine holds which boot clone. The registry's catalog adds what the engine
//! cannot say: the repository and tag, the digest, the releases carrying it. So
//! with no registry answering the page still answers, with names worked out from
//! the volume's own (`golden-<component>-<tag>`, `<component>-<tag>`).
//!
//! **A machine's own image is not an image here.** A boot clone's parent is the
//! machine's sealed host image, itself a clone of the release's; a person asking
//! who holds a tag wants the machine, so a clone rolls up through it.
//!
//! No I/O in this file: `build` takes the three answers.

use std::collections::{BTreeMap, HashMap, HashSet};

use console_core::value::{field, u64_field};
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Holder {
    /// `boot-host`, `pod`, `claim`, `vm`, `service`, `release-disk`, `build`,
    /// `attached`, `other` or `nobody`.
    pub kind: String,
    /// The machine, the pod, the mount: what to call it.
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub namespace: String,
    /// More about it: the MAC, the owner's kind, why nobody.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// A boot host's last claim, unix seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<u64>,
    /// Its project is one this viewer may not see, so the name is not given.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub withheld: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Clone_ {
    pub id: String,
    pub name: String,
    pub holder: Holder,
    /// What it costs beyond the image it reads through.
    pub allocated_bytes: u64,
    pub virtual_size_bytes: u64,
    pub in_use: bool,
    /// Clones between it and the image, when it is not a direct one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub via: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Image {
    pub id: String,
    /// The volume's name, which is what a person finds it by on the engine.
    pub name: String,
    pub repository: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tag: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub digest: String,
    /// The engine's word (`golden`, `blank`, `media`, `template`, …) or the
    /// catalog's, for the page to say in its own.
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub namespace: String,
    pub virtual_size_bytes: u64,
    pub allocated_bytes: u64,
    pub releases: Vec<String>,
    /// The image this one is built on, by name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub base: String,
    /// Images built on this one.
    pub built_on_it: usize,
    pub clones: Vec<Clone_>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Repository {
    pub name: String,
    pub images: Vec<Image>,
    pub clones: usize,
    pub orphans: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Holdings {
    pub repositories: Vec<Repository>,
    /// Clones of a parent the engine no longer has.
    pub unparented: Vec<Clone_>,
    pub images: usize,
    pub clones: usize,
    pub orphans: usize,
    pub by_holder: BTreeMap<String, usize>,
}

/// The machines, by the clones they hold.
struct Machines {
    /// Boot clone id → (machine, MAC, claimed at).
    clones: HashMap<String, (String, String, Option<u64>)>,
    /// Each machine's own sealed image, rolled up through.
    host_images: HashSet<String>,
}

fn machines(hosts: &[Value]) -> Machines {
    let mut m = Machines { clones: HashMap::new(), host_images: HashSet::new() };
    for h in hosts {
        let Some(name) = field(h, &["name"]) else { continue };
        let mut claims: Vec<&Value> = h.get("last_claim").into_iter().filter(|c| c.is_object()).collect();
        if let Some(by) = h.get("claims_by_agent").and_then(Value::as_object) {
            claims.extend(by.values());
        }
        let alias = h
            .get("aliases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .find_map(|a| a.strip_prefix("mac-").map(String::from))
            .unwrap_or_default();
        for c in claims {
            if let Some(g) = field(c, &["host_golden"]) {
                m.host_images.insert(g);
            }
            if let Some(id) = field(c, &["clone"]) {
                let mac = field(c, &["mac"]).unwrap_or_else(|| alias.clone());
                m.clones.entry(id).or_insert((name.clone(), mac, c.get("claimed_at").and_then(Value::as_u64)));
            }
        }
        if let Some(id) = h.get("host_golden").and_then(|g| g.get("target")).and_then(|t| field(t, &["id"])) {
            m.host_images.insert(id);
        }
    }
    m
}

/// Who holds a clone. What the engine was told first (a machine's claim, the owner
/// set on it), then what it is serving; names last, and only the two the platform
/// gives such volumes. Nothing found is the answer: an orphan.
fn holder(v: &Value, m: &Machines) -> Holder {
    let id = field(v, &["id"]).unwrap_or_default();
    let name = field(v, &["name"]).unwrap_or_default();
    if let Some((machine, mac, at)) = m.clones.get(&id) {
        return Holder { kind: "boot-host".into(), name: machine.clone(), detail: mac.clone(), claimed_at: *at, ..Default::default() };
    }
    let who = v.get("owner").filter(|o| o.is_object()).or_else(|| v.get("consumer").filter(|o| o.is_object()));
    if let Some(o) = who {
        let kind = field(o, &["kind"]).unwrap_or_default();
        let h = |k: &str| Holder {
            kind: k.into(),
            name: field(o, &["name"]).unwrap_or_default(),
            namespace: field(o, &["namespace"]).unwrap_or_default(),
            ..Default::default()
        };
        return match kind.as_str() {
            "Pod" => h("pod"),
            "PersistentVolumeClaim" => h("claim"),
            "VirtualMachine" | "VirtualMachineInstance" => h("vm"),
            "Mount" => h("service"),
            _ => Holder { detail: kind, ..h("other") },
        };
    }
    if name.starts_with("disk-") {
        return Holder { kind: "release-disk".into(), name: name.trim_start_matches("disk-").to_string(), ..Default::default() };
    }
    if name.starts_with("build-") {
        return Holder { kind: "build".into(), name, ..Default::default() };
    }
    if v.get("in_use").and_then(Value::as_bool) == Some(true) {
        let via = v
            .get("attachments")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .map(|a| [field(a, &["transport"]), field(a, &["mounted_at"]).or_else(|| field(a, &["target"]))].into_iter().flatten().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        return Holder { kind: "attached".into(), name: via, ..Default::default() };
    }
    let detail = if name.starts_with("boothost-") {
        "an old boot clone: no machine's current one"
    } else {
        "no owner, no machine, and nothing is serving it"
    };
    Holder { kind: "nobody".into(), detail: detail.into(), ..Default::default() }
}

/// A template's volume by the template's name, which is what anyone calls it:
/// the engine names it `fstemplate-<name>-raw` or `<name>-sealed`.
fn plain_name(name: &str) -> &str {
    let n = name.strip_prefix("fstemplate-").unwrap_or(name);
    n.strip_suffix("-raw").or_else(|| n.strip_suffix("-sealed")).unwrap_or(n)
}

/// The repository and tag a volume's name says, for a component's image:
/// `golden-<component>-<tag>` or `<component>-<tag>`, the tag a build id (12 hex)
/// or a content sha (16 hex). Anything else is its own repository, with no tag.
pub fn from_name(name: &str) -> (String, String) {
    let plain = plain_name(name);
    let bare = plain.strip_prefix("golden-").unwrap_or(plain);
    if let Some((repo, tag)) = bare.rsplit_once('-') {
        if !repo.is_empty() && matches!(tag.len(), 12 | 16) && tag.bytes().all(|b| b.is_ascii_hexdigit()) {
            return (repo.to_string(), tag.to_string());
        }
    }
    (bare.to_string(), String::new())
}

/// The repository and tag the catalog's item says. A `source` is `repo:tag`.
fn from_catalog(item: &Value, name: &str) -> (String, String) {
    let (by_name, name_tag) = from_name(name);
    // A component's image: the catalog says whose, its name says which build.
    if let Some(c) = field(item, &["component"]) {
        return (c, name_tag);
    }
    if let Some(src) = field(item, &["source"]) {
        // The last `:` past the last `/`, so a registry's port is not a tag.
        let tail = src.rsplit('/').next().unwrap_or(&src);
        if let Some((_, tag)) = tail.rsplit_once(':') {
            let repo = &src[..src.len() - tag.len() - 1];
            return (repo.to_string(), tag.to_string());
        }
        return (src, name_tag);
    }
    (by_name, name_tag)
}

pub fn build(volumes: &[Value], catalog: &[Value], hosts: &[Value]) -> Holdings {
    let m = machines(hosts);
    let by_id: HashMap<String, &Value> = volumes.iter().filter_map(|v| Some((field(v, &["id"])?, v))).collect();
    let sealed = |v: &Value| v.get("sealed").and_then(Value::as_bool) == Some(true);
    let host_image = |id: &str| m.host_images.contains(id);
    // The catalog's item per volume, this node's only: a peer's is said to be on
    // the peer, and its clones are not this engine's to count.
    let cat_by_id: HashMap<String, &Value> = catalog.iter().filter_map(|i| Some((field(i, &["volume_id"])?, i))).collect();
    let cat_by_name: HashMap<String, &Value> = catalog.iter().filter_map(|i| Some((field(i, &["name"])?, i))).collect();

    let mut images: BTreeMap<String, Image> = BTreeMap::new();
    for v in volumes.iter().filter(|v| sealed(v)) {
        let Some(id) = field(v, &["id"]) else { continue };
        if host_image(&id) {
            continue;
        }
        let name = field(v, &["name"]).unwrap_or_else(|| id.clone());
        let item = cat_by_id.get(&id).or_else(|| cat_by_name.get(plain_name(&name))).copied();
        let (repository, tag) = item.map_or_else(|| from_name(&name), |i| from_catalog(i, &name));
        let base = field(v, &["parent"])
            .and_then(|p| by_id.get(&p).copied())
            .filter(|p| sealed(p))
            .and_then(|p| field(p, &["name"]))
            .unwrap_or_default();
        images.insert(
            id.clone(),
            Image {
                id,
                name,
                repository,
                tag,
                digest: item.and_then(|i| field(i, &["digest"])).unwrap_or_default(),
                kind: item.and_then(|i| field(i, &["kind"])).or_else(|| field(v, &["kind"])).unwrap_or_default(),
                namespace: field(v, &["namespace"]).unwrap_or_default(),
                virtual_size_bytes: u64_field(v, "virtual_size_bytes").unwrap_or(0),
                allocated_bytes: u64_field(v, "allocated_bytes").unwrap_or(0),
                releases: item
                    .and_then(|i| i.get("releases"))
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
                    .unwrap_or_default(),
                base,
                built_on_it: 0,
                clones: Vec::new(),
            },
        );
    }

    let mut out = Holdings::default();
    // Images built on images, through a machine's own too.
    let bases: Vec<String> = volumes
        .iter()
        .filter(|v| sealed(v) && !field(v, &["id"]).is_some_and(|i| host_image(&i)))
        .filter_map(|v| field(v, &["parent"]))
        .collect();
    for b in bases {
        if let Some(i) = images.get_mut(&b) {
            i.built_on_it += 1;
        }
    }
    for v in volumes.iter().filter(|v| !sealed(v)) {
        let Some(id) = field(v, &["id"]) else { continue };
        let mut via = Vec::new();
        let mut at = field(v, &["parent"]);
        // Up to the image. A loop in the records must not hang the page.
        let mut steps = 0;
        let image = loop {
            let Some(p) = at.clone() else { break None };
            steps += 1;
            if steps > 64 {
                break None;
            }
            if images.contains_key(&p) {
                break Some(p);
            }
            let Some(pv) = by_id.get(&p) else { break Some(String::new()) };
            if !host_image(&p) {
                via.push(field(pv, &["name"]).unwrap_or_else(|| p.clone()));
            }
            at = field(pv, &["parent"]);
        };
        // Made empty: not a clone of anything, so not this page's.
        let Some(image) = image else { continue };
        let c = Clone_ {
            holder: holder(v, &m),
            name: field(v, &["name"]).unwrap_or_else(|| id.clone()),
            allocated_bytes: u64_field(v, "allocated_bytes").unwrap_or(0),
            virtual_size_bytes: u64_field(v, "virtual_size_bytes").unwrap_or(0),
            in_use: v.get("in_use").and_then(Value::as_bool).unwrap_or(false),
            via,
            id,
        };
        out.clones += 1;
        *out.by_holder.entry(c.holder.kind.clone()).or_default() += 1;
        if c.holder.kind == "nobody" {
            out.orphans += 1;
        }
        match images.get_mut(&image) {
            Some(i) => i.clones.push(c),
            None => out.unparented.push(c),
        }
    }

    let mut repos: BTreeMap<String, Repository> = BTreeMap::new();
    out.images = images.len();
    for (_, mut i) in images {
        i.clones.sort_by(|a, b| a.holder.kind.cmp(&b.holder.kind).then_with(|| a.name.cmp(&b.name)));
        let r = repos.entry(i.repository.clone()).or_insert_with(|| Repository { name: i.repository.clone(), ..Default::default() });
        r.clones += i.clones.len();
        r.orphans += i.clones.iter().filter(|c| c.holder.kind == "nobody").count();
        r.images.push(i);
    }
    out.repositories = repos.into_values().collect();
    for r in &mut out.repositories {
        // Newest build id is not knowable from the id: the ones in use first, so
        // what a machine runs is at the top of its repository.
        r.images.sort_by(|a, b| b.clones.len().cmp(&a.clones.len()).then_with(|| a.name.cmp(&b.name)));
    }
    out
}

/// A viewer's copy: a holder in a project they may not see is counted and not named.
/// The clone's own name goes too, because a claim's volume is named after the claim.
pub fn for_viewer(h: &Holdings, hidden: Option<&HashSet<String>>) -> Holdings {
    let Some(hidden) = hidden else { return h.clone() };
    let scrub = |c: &mut Clone_| {
        if !c.holder.namespace.is_empty() && hidden.contains(&c.holder.namespace) {
            c.holder.name = String::new();
            c.holder.namespace = String::new();
            c.holder.withheld = true;
            c.name = "in a project you cannot see".into();
            c.via.clear();
        }
    };
    let mut out = h.clone();
    for r in &mut out.repositories {
        for i in &mut r.images {
            i.clones.iter_mut().for_each(scrub);
        }
    }
    out.unparented.iter_mut().for_each(scrub);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn vol(id: &str, name: &str, sealed: bool, parent: Option<&str>) -> Value {
        json!({"id": id, "name": name, "sealed": sealed, "parent": parent, "kind": if sealed {"golden"} else {"volume"},
               "virtual_size_bytes": 1000, "allocated_bytes": 7, "in_use": false, "namespace": "default"})
    }

    fn with(mut v: Value, extra: Value) -> Value {
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        v
    }

    #[test]
    fn names_say_repository_and_tag_only_when_they_do() {
        assert_eq!(from_name("golden-stormlb-aaaaaaaaaaaa"), ("stormlb".into(), "aaaaaaaaaaaa".into()));
        assert_eq!(from_name("stormlb-aaaaaaaaaaaa"), ("stormlb".into(), "aaaaaaaaaaaa".into()));
        assert_eq!(from_name("fedora-base-0123456789abcdef"), ("fedora-base".into(), "0123456789abcdef".into()));
        assert_eq!(from_name("pvc-256M"), ("pvc-256M".into(), String::new()));
        assert_eq!(from_name("pvc-256M-sealed"), ("pvc-256M".into(), String::new()));
        assert_eq!(from_name("fstemplate-pvc-64M-raw"), ("pvc-64M".into(), String::new()));
        assert_eq!(from_name("fstemplate-golden-stormlb-aaaaaaaaaaaa-raw"), ("stormlb".into(), "aaaaaaaaaaaa".into()));
        assert_eq!(from_name("img-3f9a0c1d2e4b-root"), ("img-3f9a0c1d2e4b-root".into(), String::new()));
        assert_eq!(from_catalog(&json!({"source": "forge:5000/library/nats:2.10"}), "img-x"), ("forge:5000/library/nats".into(), "2.10".into()));
        assert_eq!(from_catalog(&json!({"source": "forge:5000/library/nats"}), "img-x"), ("forge:5000/library/nats".into(), String::new()));
        assert_eq!(from_catalog(&json!({"component": "stormlb"}), "golden-stormlb-aaaaaaaaaaaa"), ("stormlb".into(), "aaaaaaaaaaaa".into()));
        assert_eq!(from_catalog(&json!({"component": "stormlb", "source": "template:x"}), "golden-stormlb-aaaaaaaaaaaa-sealed"), ("stormlb".into(), "aaaaaaaaaaaa".into()), "whatever its source");
    }

    #[test]
    fn every_clone_says_who_holds_it() {
        let g = vol("g", "golden-stormlb-aaaaaaaaaaaa", true, None);
        let hi = vol("hi", "hostgolden-r230", true, Some("g"));
        let hosts = [json!({"name": "r230", "aliases": ["mac-aabb"], "last_claim": {"clone": "boot", "host_golden": "hi", "claimed_at": 1234, "mac": "aa:bb"}})];
        let v = [
            g,
            hi,
            vol("boot", "boothost-r230-2", false, Some("hi")),
            vol("old", "boothost-r230-1", false, Some("hi")),
            with(vol("p", "x", false, Some("g")), json!({"owner": {"kind": "Pod", "namespace": "shop", "name": "web"}})),
            with(vol("c", "pvc-1", false, Some("g")), json!({"owner": {"kind": "PersistentVolumeClaim", "namespace": "shop", "name": "db"}})),
            with(vol("m", "vm-root", false, Some("g")), json!({"consumer": {"kind": "VirtualMachineInstance", "namespace": "lab", "name": "vm1"}})),
            with(vol("s", "stormlb", false, Some("g")), json!({"consumer": {"kind": "Mount", "name": "/pallets/stormlb"}, "in_use": true})),
            with(vol("o", "y", false, Some("g")), json!({"owner": {"kind": "CloudImage", "name": "fedora"}})),
            vol("d", "disk-12.61", false, Some("g")),
            vol("b", "build-77", false, Some("g")),
            with(vol("a", "z", false, Some("g")), json!({"in_use": true, "attachments": [{"transport": "nvme-tcp", "target": "nqn.x"}]})),
            vol("n", "clone-lost", false, Some("g")),
            vol("cc", "deep", false, Some("n")),
            vol("e", "made-empty", false, None),
            vol("u", "lost-parent", false, Some("gone")),
        ];
        let h = build(&v, &[], &hosts);
        assert_eq!(h.images, 1, "the machine's own image is not one");
        assert_eq!(h.repositories.len(), 1);
        let r = &h.repositories[0];
        assert_eq!(r.name, "stormlb");
        assert_eq!(r.images[0].tag, "aaaaaaaaaaaa");
        let kind = |n: &str| r.images[0].clones.iter().find(|c| c.name == n).unwrap().holder.clone();
        let boot = kind("boothost-r230-2");
        assert_eq!((boot.kind.as_str(), boot.name.as_str(), boot.detail.as_str(), boot.claimed_at), ("boot-host", "r230", "aa:bb", Some(1234)));
        let old = kind("boothost-r230-1");
        assert_eq!(old.kind, "nobody");
        assert!(old.detail.contains("old boot clone"));
        assert_eq!((kind("x").kind.as_str(), kind("x").name.as_str(), kind("x").namespace.as_str()), ("pod", "web", "shop"));
        assert_eq!(kind("pvc-1").kind, "claim");
        assert_eq!(kind("vm-root").kind, "vm");
        assert_eq!((kind("stormlb").kind.as_str(), kind("stormlb").name.as_str()), ("service", "/pallets/stormlb"));
        assert_eq!((kind("y").kind.as_str(), kind("y").detail.as_str()), ("other", "CloudImage"));
        assert_eq!((kind("disk-12.61").kind.as_str(), kind("disk-12.61").name.as_str()), ("release-disk", "12.61"));
        assert_eq!(kind("build-77").kind, "build");
        assert_eq!((kind("z").kind.as_str(), kind("z").name.as_str()), ("attached", "nvme-tcp nqn.x"));
        assert_eq!(kind("clone-lost").kind, "nobody");
        // A clone of a clone is the image's, and says what it came through.
        assert_eq!(r.images[0].clones.iter().find(|c| c.name == "deep").unwrap().via, ["clone-lost"]);
        // Made empty is nothing's clone; a parent gone is said apart.
        assert!(!r.images[0].clones.iter().any(|c| c.name == "made-empty"));
        assert_eq!(h.unparented[0].name, "lost-parent");
        assert_eq!(h.by_holder["boot-host"], 1);
        assert_eq!(h.orphans, r.orphans + 1, "nobody holds the one with no parent either");
        assert_eq!(r.images[0].clones[0].allocated_bytes, 7, "what it costs beyond the image");
    }

    #[test]
    fn the_catalog_names_what_the_engine_cannot() {
        let v = [vol("g", "img-3f9a", true, None), vol("base", "base-1", true, None), with(vol("g2", "img-4", true, Some("base")), json!({}))];
        let cat = [json!({"name": "img-3f9a", "volume_id": "g", "kind": "golden", "source": "library/nats:2.10", "digest": "sha256:ab", "releases": ["12.61"]})];
        let h = build(&v, &cat, &[]);
        let nats = h.repositories.iter().find(|r| r.name == "library/nats").unwrap();
        let i = &nats.images[0];
        assert_eq!((i.tag.as_str(), i.digest.as_str()), ("2.10", "sha256:ab"));
        assert_eq!(i.releases, ["12.61"]);
        let b = h.repositories.iter().flat_map(|r| &r.images).find(|i| i.name == "base-1").unwrap();
        assert_eq!(b.built_on_it, 1);
        let on = h.repositories.iter().flat_map(|r| &r.images).find(|i| i.name == "img-4").unwrap();
        assert_eq!(on.base, "base-1");
    }

    #[test]
    fn another_projects_holder_is_counted_and_not_named() {
        let v = [
            vol("g", "golden-a-aaaaaaaaaaaa", true, None),
            with(vol("c", "pvc-secret-db", false, Some("g")), json!({"owner": {"kind": "PersistentVolumeClaim", "namespace": "theirs", "name": "secret-db"}})),
            with(vol("d", "pvc-mine", false, Some("g")), json!({"owner": {"kind": "PersistentVolumeClaim", "namespace": "mine", "name": "db"}})),
        ];
        let h = build(&v, &[], &[]);
        let hidden: HashSet<String> = ["theirs".to_string()].into();
        let seen = for_viewer(&h, Some(&hidden));
        let text = serde_json::to_string(&seen).unwrap();
        assert!(!text.contains("secret-db") && !text.contains("theirs"), "{text}");
        assert!(text.contains("pvc-mine"));
        assert_eq!(seen.clones, 2, "still counted");
        assert!(seen.repositories[0].images[0].clones.iter().any(|c| c.holder.withheld && c.holder.kind == "claim"));
        assert!(serde_json::to_string(&for_viewer(&h, None)).unwrap().contains("secret-db"));
    }
}
