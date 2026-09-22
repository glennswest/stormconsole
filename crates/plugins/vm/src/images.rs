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

/// How often to retry while there is no list at all.
///
/// Different from `REFRESH` because the two are different questions. Once
/// there is a list, a minute of staleness costs nothing. While there is no
/// list, the form cannot be used, and on a node that is the first minute
/// after every boot — the operator and the console come up together.
pub const RETRY: std::time::Duration = std::time::Duration::from_secs(3);

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

    /// Store a refresh, **keeping the last good list when this one is
    /// empty**.
    ///
    /// A cache that forgets the moment its upstream hiccups is not a cache.
    /// The operator and the console start together on a node, so for the
    /// first seconds of every boot the operator is not answering yet — and
    /// the create form, finding no choices, turned its root-disk dropdown
    /// into a free-text box asking a person to type a golden name from
    /// memory. That is the failure this whole module was written to remove,
    /// reintroduced by a poll that overwrote 22 good choices with nothing.
    ///
    /// An empty answer now keeps the choices and changes only the note, so
    /// the form still offers what was last seen and says it may be stale.
    pub fn put(&self, c: Catalogue) {
        let mut g = match self.0.write() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        if c.choices.is_empty() && !g.choices.is_empty() {
            g.note = if c.note.is_empty() {
                "the image operator has not answered — this is the list it last gave".into()
            } else {
                format!("{} — this is the list it last gave", c.note)
            };
            return;
        }
        *g = c;
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
    #[serde(default, deserialize_with = "nullable")]
    reference: String,
    #[serde(default, deserialize_with = "nullable")]
    distro: String,
    #[serde(default, deserialize_with = "nullable")]
    version: String,
    #[serde(default, deserialize_with = "nullable")]
    arch: String,
    /// The golden name this would produce, which is what a VM spec asks for.
    #[serde(default, deserialize_with = "nullable")]
    golden: String,
    #[serde(default, deserialize_with = "nullable")]
    provisioning: String,
}

#[derive(Debug, Deserialize)]
struct ImageList {
    #[serde(default)]
    items: Vec<ImageItem>,
}

#[derive(Debug, Deserialize)]
struct ImageItem {
    #[serde(default, deserialize_with = "nullable")]
    name: String,
    #[serde(default)]
    spec: ImageSpec,
    #[serde(default)]
    status: ImageStatus,
}

/// A string field that may arrive as `null`.
///
/// `#[serde(default)]` covers a field that is *absent*; it does nothing for
/// one that is present and `null`, which fails with "invalid type: null,
/// expected a string" and takes the whole response down with it.
///
/// That is not a hypothetical. `status.golden` is null on an image that is
/// still building — it has no volume yet, which is the honest answer — and
/// one such image in the list made `ImageList` fail to deserialise, so *every*
/// fleet golden vanished from the create form at once. The symptom was
/// "rawhide is not in the list", where rawhide was Available, had a golden,
/// and was innocent: the image poisoning the list was a different one, still
/// downloading.
///
/// One row that cannot be read must cost that row, never the list.
fn nullable<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Default, Deserialize)]
struct ImageSpec {
    /// The catalogue reference this image was built from, `fedora:43`. The
    /// identity that says "this catalogue entry is already goldened", which
    /// a name cannot: two images of one reference differ by arch, not by
    /// being different things to golden.
    #[serde(default, deserialize_with = "nullable")]
    reference: String,
}

#[derive(Debug, Default, Deserialize)]
struct ImageStatus {
    #[serde(default, deserialize_with = "nullable")]
    phase: String,
    /// What a person calls it: `fedora-43-x86_64`. **Not** a volume — see
    /// `golden`.
    #[serde(default, rename = "localName", deserialize_with = "nullable")]
    local_name: String,
    /// The volume the engine actually holds, `media-846574c8a97c`.
    ///
    /// This is the name a VM's root disk must ask for. It is a digest, not a
    /// word, and that is the whole reason this field exists separately from
    /// `local_name`: a golden is content, and two Fedora 43 images that
    /// differ by a byte are two goldens with one pretty name.
    #[serde(default, deserialize_with = "nullable")]
    golden: String,
    #[serde(default, deserialize_with = "nullable")]
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
    #[serde(default, deserialize_with = "nullable")]
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
    // Which catalogue references already have an image, so step 3 does not
    // offer "golden this" for something already goldened.
    let mut goldened: std::collections::HashSet<String> = Default::default();
    if let Ok(r) = http.get(format!("{base}/api/v1/images")).send().await {
        if let Ok(list) = r.json::<ImageList>().await {
            fleet_ok = true;
            for i in list.items {
                if !i.spec.reference.is_empty() {
                    goldened.insert(i.spec.reference.clone());
                }
                if i.status.phase != "Available" {
                    continue;
                }
                // The value is the volume, the label is the name.
                //
                // These were the same string until a VM created from this
                // form failed at start with
                //
                //     cloning golden fedora-43 for disk root:
                //       404 Not Found: {"error":"no volume fedora-43"}
                //
                // The engine holds `media-846574c8a97c`. `fedora-43` is the
                // image resource and `fedora-43-x86_64` is what a person
                // calls it; neither is a volume, and the form was submitting
                // one of them as though it were. Offering a digest as the
                // label would be honest and unreadable, so the two are now
                // separate: read the name, submit the volume.
                let label = if i.status.local_name.is_empty() {
                    i.name.clone()
                } else {
                    i.status.local_name.clone()
                };
                let volume = i.status.golden.clone();
                if volume.is_empty() || label.is_empty() || local.contains(&volume) {
                    continue;
                }
                choices.push(Choice { label, value: volume });
            }
        }
    }

    // 3. What could be goldened but has not been. These take the longest —
    //    a download, a decode and a seal — so they come last and say so.
    if let Ok(r) = http.get(format!("{base}/api/v1/catalog")).send().await {
        if let Ok(list) = r.json::<CatalogList>().await {
            fleet_ok = true;
            for i in list.items {
                // Already goldened? Asked of the reference, because the
                // catalogue's `golden` field is the name an image *would*
                // be given and the fleet list now answers in volumes. The
                // reference is the one identity both sides share.
                if goldened.contains(&i.reference) {
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
        .find(|i| i.status.golden == golden || i.status.local_name == golden || i.name == golden)
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

    /// One image still building must not empty the list.
    ///
    /// `status.golden` is null while an image downloads. With
    /// `#[serde(default)]` alone that is "invalid type: null, expected a
    /// string" for the whole `ImageList`, so every *Available* golden
    /// disappeared from the create form too — reported as "rawhide is not in
    /// the list", by someone looking at a rawhide that was Available, had a
    /// golden, and had nothing wrong with it.
    #[test]
    fn a_building_image_with_no_golden_yet_does_not_take_the_list_with_it() {
        let body = serde_json::json!({"items": [
            {"name": "fedora-44", "spec": {"reference": "fedora:44"},
             "status": {"phase": "Building", "golden": null,
                        "localName": "fedora-44-x86_64", "message": null}},
            {"name": "fedora-rawhide", "spec": {"reference": "fedora:rawhide"},
             "status": {"phase": "Available", "golden": "media-55797d0c4e57",
                        "localName": "fedora-rawhide-x86_64"}},
        ]});
        let list: ImageList = serde_json::from_value(body).expect("a null golden is not fatal");
        assert_eq!(list.items.len(), 2, "both rows survive");
        assert_eq!(list.items[0].status.golden, "", "the building one reads as no volume");
        assert_eq!(list.items[1].status.golden, "media-55797d0c4e57",
                   "and the one that is ready is still offerable");
    }

    /// The same for the catalogue half.
    #[test]
    fn a_null_field_in_the_catalogue_costs_one_field_not_the_list() {
        let body = serde_json::json!({"items": [
            {"reference": "alma:10", "distro": null, "version": "10", "golden": null},
        ]});
        let list: CatalogList = serde_json::from_value(body).expect("nulls are tolerated");
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.items[0].distro, "");
        assert_eq!(list.items[0].reference, "alma:10");
    }

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

    #[test]
    fn a_silent_operator_does_not_empty_the_list() {
        // The bug this is here for: the console and the operator start
        // together, the first poll finds nothing, and the create form turned
        // its dropdown into a text box asking for a golden name from memory.
        let c = Cache::new();
        c.put(Catalogue {
            choices: vec![Choice { value: "media-846574c8a97c".into(), label: "fedora-43".into() }],
            note: String::new(),
        });
        c.put(Catalogue { choices: vec![], note: "no answer from the image operator".into() });
        let after = c.get();
        assert_eq!(after.choices.len(), 1, "the last good list is kept");
        assert!(after.note.contains("last gave"), "and it says it may be stale: {}", after.note);
    }

    #[test]
    fn a_real_answer_replaces_the_remembered_one() {
        let c = Cache::new();
        c.put(Catalogue {
            choices: vec![Choice { value: "old".into(), label: "old".into() }],
            note: String::new(),
        });
        c.put(Catalogue {
            choices: vec![Choice { value: "new".into(), label: "new".into() }],
            note: String::new(),
        });
        let after = c.get();
        assert_eq!(after.choices.len(), 1);
        assert_eq!(after.choices[0].value, "new");
        assert!(after.note.is_empty());
    }

    #[test]
    fn the_form_submits_the_volume_and_shows_the_name() {
        // What the operator answers for a built image, trimmed to the
        // fields that decide this. `golden` is the volume the engine holds;
        // the other two are what a person calls it, and submitting either
        // produced `404 no volume fedora-43` at start.
        let body = r#"{"items":[{"name":"fedora-43",
            "spec":{"reference":"fedora:43","arch":"x86_64"},
            "status":{"phase":"Available","localName":"fedora-43-x86_64",
                      "golden":"media-846574c8a97c","arch":"x86_64"}}]}"#;
        let list: ImageList = serde_json::from_str(body).expect("parses");
        let i = &list.items[0];
        assert_eq!(i.status.golden, "media-846574c8a97c");
        assert_eq!(i.status.local_name, "fedora-43-x86_64");
        assert_eq!(i.spec.reference, "fedora:43");

        let label = if i.status.local_name.is_empty() { i.name.clone() } else { i.status.local_name.clone() };
        let choice = Choice { label, value: i.status.golden.clone() };
        assert_eq!(choice.value, "media-846574c8a97c", "the value is a volume");
        assert_eq!(choice.label, "fedora-43-x86_64", "the label is readable");
        assert!(!is_reference(&choice.value), "a volume is not a catalogue reference");
    }

    #[test]
    fn an_image_that_has_not_resolved_yet_is_not_offered() {
        // No golden means no volume to clone. Offering it would create a VM
        // that fails at start, which is the failure being fixed.
        let body = r#"{"items":[{"name":"alma-10","spec":{"reference":"alma:10"},
            "status":{"phase":"Available","localName":"alma-10-x86_64","golden":""}}]}"#;
        let list: ImageList = serde_json::from_str(body).expect("parses");
        assert!(list.items[0].status.golden.is_empty());
    }
}
