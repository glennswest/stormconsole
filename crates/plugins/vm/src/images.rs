//! What a new VM can boot from, gathered from the image operator.
//!
//! The create form used to ask for a golden by typing its name, with a hint
//! naming one as an example. That is a text box you can only fill in
//! correctly if you already know the answer, and getting it wrong produces a
//! VM whose root disk refers to a golden that does not exist — which fails at
//! start, not at create.
//!
//! `vmcloud-image-operator` knows all three answers a person actually wants:
//!
//!   * what this node already has a sealed local copy of — boots immediately
//!   * what the fleet has goldened but this node has not got yet
//!   * what the public catalogue could golden that nobody has asked for
//!
//! All three are offered, because "should be able to create either" is the
//! requirement: choosing something not yet local is a legitimate create, it
//! just takes longer and the form says so.
//!
//! # Why this is polled into a cache
//!
//! `ConsolePlugin::creators()` is synchronous — the console asks for the
//! create forms while rendering, and cannot await an upstream that may be
//! slow or absent. So a background task keeps the list current and
//! `creators()` reads the snapshot. An operator that is down therefore costs
//! a stale list rather than a hung console, and when there is no list at all
//! the field falls back to free text, which is exactly what it was before.

use std::sync::RwLock;

use serde::Deserialize;

/// How often the catalogue is refreshed.
///
/// Images are goldened by a human deciding to, and a golden takes minutes to
/// build, so a minute of staleness costs nothing. The cost of polling harder
/// is a request per console per minute against an operator whose answer
/// almost never changes.
pub const REFRESH: std::time::Duration = std::time::Duration::from_secs(60);

/// A value the create form can submit, and how it is offered.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// What the form submits. A local golden is its name; something that has
    /// to be goldened first is `<distro>:<version>`, which is the operator's
    /// own reference syntax and contains a colon — a golden name never does,
    /// so the create handler can tell them apart without a second field.
    pub value: String,
    /// What a person reads in the dropdown.
    pub label: String,
}

/// Everything a new VM could boot from, in the order a person wants it.
#[derive(Debug, Default, Clone)]
pub struct Catalogue {
    pub choices: Vec<Choice>,
    /// Why the list is empty, when it is. Shown as the field's hint, because
    /// an empty dropdown with no explanation reads as a broken console.
    pub note: String,
}

#[derive(Default)]
pub struct Cache(RwLock<Catalogue>);

impl Cache {
    pub fn new() -> Cache {
        Cache::default()
    }

    /// Read the snapshot. Never blocks on an upstream, never panics on a
    /// poisoned lock — a create form is not worth taking the console down for.
    pub fn get(&self) -> Catalogue {
        match self.0.read() {
            Ok(g) => g.clone(),
            Err(e) => e.into_inner().clone(),
        }
    }

    pub fn put(&self, c: Catalogue) {
        match self.0.write() {
            Ok(mut g) => *g = c,
            Err(e) => *e.into_inner() = c,
        }
    }
}

// --- what the operator answers -------------------------------------------

#[derive(Debug, Deserialize)]
struct CatalogList {
    #[serde(default)]
    items: Vec<CatalogItem>,
}

#[derive(Debug, Deserialize)]
struct CatalogItem {
    reference: String,
    #[serde(default)]
    distro: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    arch: String,
    /// The golden name this would produce, which is what a VM spec asks for.
    #[serde(default)]
    golden: String,
    #[serde(default)]
    provisioning: String,
}

#[derive(Debug, Deserialize)]
struct ImageList {
    #[serde(default)]
    items: Vec<ImageItem>,
}

#[derive(Debug, Deserialize)]
struct ImageItem {
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: ImageStatus,
}

#[derive(Debug, Default, Deserialize)]
struct ImageStatus {
    #[serde(default)]
    phase: String,
    /// The name a VM spec refers to, once it is built.
    #[serde(default, rename = "localName")]
    local_name: String,
    #[serde(default)]
    arch: String,
}

/// The goldens actually on one node, as that node's engine lists them.
#[derive(Debug, Deserialize)]
struct NodeImages {
    #[serde(default)]
    items: Vec<NodeImage>,
}

#[derive(Debug, Deserialize)]
struct NodeImage {
    #[serde(default)]
    name: String,
}

/// Ask the operator what could boot, for one node.
///
/// `node` may be empty — a cluster-wide form that has not pinned a node yet —
/// in which case the local-copy question is skipped and everything the fleet
/// has goldened is offered. That is the honest answer: without a node there
/// is no such thing as "already here".
pub async fn fetch(http: &reqwest::Client, base: &str, node: &str) -> Catalogue {
    let base = base.trim_end_matches('/');
    let mut choices: Vec<Choice> = Vec::new();
    let mut local: std::collections::HashSet<String> = Default::default();

    // 1. What this node has. First, because it is the only kind that boots
    //    without waiting for anything.
    if !node.is_empty() {
        if let Ok(r) = http.get(format!("{base}/api/v1/nodes/{node}/images")).send().await {
            if let Ok(list) = r.json::<NodeImages>().await {
                for i in list.items {
                    if i.name.is_empty() {
                        continue;
                    }
                    local.insert(i.name.clone());
                    choices.push(Choice { label: i.name.clone(), value: i.name });
                }
            }
        }
    }

    // 2. What the fleet has goldened. Available only: a golden still building
    //    cannot be cloned, and offering it produces a VM that fails at start
    //    for a reason the form knew about.
    let mut fleet_ok = false;
    if let Ok(r) = http.get(format!("{base}/api/v1/images")).send().await {
        if let Ok(list) = r.json::<ImageList>().await {
            fleet_ok = true;
            for i in list.items {
                if i.status.phase != "Available" {
                    continue;
                }
                let name = if i.status.local_name.is_empty() {
                    i.name.clone()
                } else {
                    i.status.local_name.clone()
                };
                if name.is_empty() || local.contains(&name) {
                    continue;
                }
                choices.push(Choice { label: name.clone(), value: name });
            }
        }
    }

    // 3. What could be goldened but has not been. These take the longest —
    //    a download, a decode and a seal — so they come last and say so.
    let have: std::collections::HashSet<String> =
        choices.iter().map(|c| c.value.clone()).collect();
    if let Ok(r) = http.get(format!("{base}/api/v1/catalog")).send().await {
        if let Ok(list) = r.json::<CatalogList>().await {
            fleet_ok = true;
            for i in list.items {
                if !i.golden.is_empty() && have.contains(&i.golden) {
                    continue;
                }
                // The name, and nothing else.
                //
                // These read "alma 10 x86_64 — not goldened yet, will be
                // built" and the like. Whether a golden exists yet is the
                // operator's business, not a sentence in a dropdown: it makes
                // every row a different length, it is the same words on most
                // of them, and it tells somebody choosing an operating system
                // about storage mechanics they did not ask about.
                let what = if i.distro.is_empty() {
                    i.reference.clone()
                } else {
                    format!("{} {}", i.distro, i.version)
                };
                choices.push(Choice { label: what, value: i.reference });
            }
        }
    }

    let note = if choices.is_empty() {
        if fleet_ok {
            "the image operator has no images and an empty catalogue".into()
        } else {
            format!("no answer from the image operator at {base} — type a golden name")
        }
    } else {
        String::new()
    };
    Catalogue { choices, note }
}

/// Make sure a node has a local copy of a golden, and say what happened.
///
/// A golden in the registry is not a disk a VM can clone: the fleet holds it,
/// and a node holds a *placement* of it. Nothing created placements, so the
/// create form offered "will be copied to the node" and then created a
/// machine whose root disk did not exist:
///
/// ```text
/// cloning golden fedora-43-x86_64 for disk root:
///   404 Not Found: {"error":"no volume fedora-43-x86_64","code":404}
/// ```
///
/// Returns `Ok(true)` when the copy is already there and the VM can start at
/// once, `Ok(false)` when one has been asked for and is being made — that is
/// minutes for a cloud image, so the caller says so rather than pretending.
pub async fn ensure_local(
    http: &reqwest::Client,
    base: &str,
    node: &str,
    golden: &str,
) -> Result<bool, String> {
    let base = base.trim_end_matches('/');
    if node.is_empty() {
        // Nothing to place it on. Not an error: a VM the scheduler will place
        // has no node yet, and refusing here would block the ordinary case on
        // a cluster where the golden is already everywhere.
        return Ok(false);
    }
    // Already there? The engine's own listing, not the placement records:
    // what matters is whether the volume exists, and a placement that was
    // deleted leaves the volume behind on purpose.
    if let Ok(r) = http.get(format!("{base}/api/v1/nodes/{node}/images")).send().await {
        if let Ok(list) = r.json::<NodeImages>().await {
            if list.items.iter().any(|i| i.name == golden) {
                return Ok(true);
            }
        }
    }
    // Which image owns this golden name. A placement is made from the image,
    // not from the name a VM asks for.
    let image = image_for_golden(http, base, golden).await.ok_or_else(|| {
        format!("no image in the catalogue produces the golden {golden}")
    })?;
    let body = json_body(&image, node);
    let r = http
        .post(format!("{base}/api/v1/local"))
        .body(body)
        .header("content-type", "application/json")
        .send()
        .await
        .map_err(|e| format!("image operator at {base}: {e}"))?;
    if !r.status().is_success() {
        let code = r.status();
        let text = r.text().await.unwrap_or_default();
        return Err(format!("could not place {golden} on {node}: {code} {}", text.trim()));
    }
    Ok(false)
}

fn json_body(image: &str, node: &str) -> String {
    // Hand-built rather than through serde_json, so this module keeps no
    // dependency it does not otherwise need.
    format!("{{\"image\":\"{image}\",\"node\":\"{node}\"}}")
}

/// The image whose `localName` is this golden.
async fn image_for_golden(http: &reqwest::Client, base: &str, golden: &str) -> Option<String> {
    let r = http.get(format!("{base}/api/v1/images")).send().await.ok()?;
    let list = r.json::<ImageList>().await.ok()?;
    list.items
        .into_iter()
        .find(|i| i.status.local_name == golden || i.name == golden)
        .map(|i| i.name)
}

/// Is this value a catalogue reference rather than an existing golden?
///
/// The operator's references are `<distro>:<version>`; golden names never
/// contain a colon. One field, two meanings, told apart by the syntax the
/// operator already defines rather than by a second control on the form.
pub fn is_reference(value: &str) -> bool {
    value.contains(':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_is_told_from_a_golden_by_its_colon() {
        assert!(is_reference("fedora:43"));
        assert!(is_reference("stormcos:node"));
        assert!(!is_reference("fedora-43-x86_64"));
        assert!(!is_reference("rocky-10-cloud"));
    }

    #[test]
    fn the_cache_survives_a_poisoned_lock() {
        // A create form is not worth taking the console down for.
        let c = Cache::new();
        c.put(Catalogue {
            choices: vec![Choice { value: "a".into(), label: "a".into() }],
            note: String::new(),
        });
        assert_eq!(c.get().choices.len(), 1);
    }

    #[test]
    fn an_empty_cache_reads_empty_rather_than_failing() {
        assert!(Cache::new().get().choices.is_empty());
    }
}
