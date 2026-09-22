//! Adding and removing a machine's disks.
//!
//! **This is not hotplug, and it does not pretend to be.** Neither
//! hypervisor is asked to attach anything to a running guest: stormvm's
//! console router serves `pause`, `unpause`, `softreboot`, `reset`,
//! `status`, `freeze`, `thaw` and the migration verbs, and no device verb
//! at all (stormvm `docs/console.md`). So a disk added here is written to
//! the `VirtualMachine`, and the guest sees it when it next boots — which
//! is said in the answer, on the row, and in the form, because a disk that
//! appears in the console and not in the guest is the worst of the three
//! possible outcomes.
//!
//! A disk is two things that have to agree: an entry in
//! `domain.devices.disks` naming the bus, and an entry in `volumes` with
//! the same name saying what backs it. Written together here for the same
//! reason `stormvm_node::plan::Sockets` exists over there — two `format!`s
//! in two places is a machine that boots with a disk pointing at nothing
//! on the day one of them changes.
//!
//! The arrays are sent whole. A JSON merge patch replaces an array rather
//! than merging into it, so appending means reading what is there and
//! writing it back with one more element — which also means this refuses
//! to work from a spec it could not read, rather than replacing a
//! machine's disks with a list of one.

use serde::Deserialize;
use serde_json::{json, Value};

/// What a new disk is backed by. Deliberately small: these are the three
/// a console can offer honestly, and anything else is the YAML tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// An existing golden or DataVolume, cloned — what a root disk is.
    Golden,
    /// A PersistentVolumeClaim somebody already made.
    Claim,
    /// Scratch space that does not survive the machine.
    Empty,
}

impl Source {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "golden" | "dataVolume" => Ok(Source::Golden),
            "pvc" | "claim" => Ok(Source::Claim),
            "empty" | "blank" => Ok(Source::Empty),
            other => Err(format!(
                "{other:?} is not a disk source this console offers — golden, pvc or empty"
            )),
        }
    }

    fn volume(self, name: &str, source: &str) -> Value {
        match self {
            Source::Golden => json!({"name": name, "dataVolume": {"name": source}}),
            Source::Claim => json!({"name": name, "persistentVolumeClaim": {"claimName": source}}),
            Source::Empty => json!({"name": name, "emptyDisk": {"capacity": source}}),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Add {
    /// What the guest will call it, and the key that ties the disk to its
    /// volume.
    pub name: String,
    /// `golden`, `pvc` or `empty`.
    #[serde(default)]
    pub source: String,
    /// The golden's name, the claim's name, or the capacity for an empty
    /// disk.
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub bus: String,
}

/// The `spec.template.spec` patch that adds one disk to a machine.
///
/// `spec` is the machine's current `spec.template.spec`. Both arrays come
/// back whole, because a merge patch replaces them.
pub fn add(spec: &Value, a: &Add) -> Result<Value, String> {
    let name = a.name.trim();
    if name.is_empty() {
        return Err("a disk needs a name — it is what ties it to its volume".into());
    }
    // The guest sees this as a device name and KubeVirt as an object key.
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(format!(
            "{name:?} is not a usable disk name — lower-case letters, digits and hyphens"
        ));
    }
    let from = a.from.trim();
    if from.is_empty() {
        return Err("a disk needs something behind it: a golden, a claim, or a size".into());
    }
    let source = Source::parse(a.source.trim())?;
    let bus = match a.bus.trim() {
        "" => "virtio",
        b if ["virtio", "sata", "scsi", "usb"].contains(&b) => b,
        b => return Err(format!("{b:?} is not a disk bus this console offers")),
    };

    let mut disks = array(spec, "/domain/devices/disks");
    let mut volumes = array(spec, "/volumes");
    if disks.iter().chain(volumes.iter()).any(|d| named(d) == Some(name)) {
        return Err(format!(
            "this machine already has a disk called {name:?} — a second one would shadow it"
        ));
    }
    disks.push(json!({"name": name, "disk": {"bus": bus}}));
    volumes.push(source.volume(name, from));
    Ok(json!({"spec": {"template": {"spec": {
        "domain": {"devices": {"disks": disks}},
        "volumes": volumes
    }}}}))
}

/// The patch that removes one disk, with both halves taken out together.
///
/// The root disk and the cloud-init seed are refused. Removing the root
/// disk leaves a machine with nothing to boot, and removing the seed
/// leaves one whose next boot has no key and no user — both of which look
/// like a machine that broke rather than one somebody edited.
pub fn remove(spec: &Value, name: &str) -> Result<Value, String> {
    let name = name.trim();
    let disks = array(spec, "/domain/devices/disks");
    let volumes = array(spec, "/volumes");
    if !disks.iter().any(|d| named(d) == Some(name)) {
        return Err(format!("this machine has no disk called {name:?}"));
    }
    if name == "root" {
        return Err("the root disk is what the machine boots from: removing it leaves a \
                    machine that cannot start, which reads as broken rather than edited. \
                    Delete the machine, or replace the disk in the YAML"
            .into());
    }
    if name == "seed" {
        return Err("the seed is the cloud-init the guest reads at first boot — without it a \
                    rebuilt machine has no user and no key, and nothing can log into it"
            .into());
    }
    Ok(json!({"spec": {"template": {"spec": {
        "domain": {"devices": {"disks":
            disks.into_iter().filter(|d| named(d) != Some(name)).collect::<Vec<_>>()}},
        "volumes": volumes.into_iter().filter(|v| named(v) != Some(name)).collect::<Vec<_>>()
    }}}}))
}

fn array(spec: &Value, ptr: &str) -> Vec<Value> {
    spec.pointer(ptr).and_then(Value::as_array).cloned().unwrap_or_default()
}

fn named(v: &Value) -> Option<&str> {
    v.get("name").and_then(Value::as_str)
}

/// Every disk the machine will have or does have, merged by name.
///
/// A disk the definition carries and the instance does not is written and
/// not yet attached; one the instance carries and the definition does not
/// was removed and is still there until the machine restarts. Both are
/// facts the page has to be able to state, and reading either object
/// alone can state only one of them.
pub fn merged(definition: Option<&Value>, instance: Option<&Value>) -> Vec<Value> {
    let listed = |spec: Option<&Value>| {
        spec.map(|s| crate::components::disks(s)).unwrap_or_default()
    };
    let want = listed(definition);
    let have = listed(instance);
    // A machine that is not running has nothing to be pending against.
    //
    // Flagging every disk "added — at next boot" on a stopped machine is
    // the same mistake as telling somebody editing a stopped machine that
    // a field needs a restart: true in a useless sense, and the way real
    // warnings stop being read. With no instance, the definition simply
    // *is* what the machine has.
    let running = instance.is_some();
    // The definition's order where there is one, because that is the
    // order somebody wrote; anything only the instance has goes after.
    let mut out: Vec<Value> = Vec::new();
    let mut push = |name: &str, backing: &str, attached: bool, written: bool| {
        out.push(json!({
            "name": name,
            "backing": backing,
            "attached": attached,
            "written": written,
            // The root disk and the seed are refused by `remove`, and a
            // button that exists in order to be refused is worse than no
            // button. Decided here rather than in the view, so the rule
            // lives beside the refusal it mirrors.
            "removable": !matches!(name, "root" | "seed"),
        }));
    };
    for (name, backing) in &want {
        let attached = !running || have.iter().any(|(n, _)| n == name);
        push(name, backing, attached, true);
    }
    for (name, backing) in &have {
        if !want.iter().any(|(n, _)| n == name) {
            push(name, backing, true, false);
        }
    }
    // A machine with no definition has only what is running, and every
    // one of those is both written and attached as far as anyone can act
    // on it.
    if definition.is_none() {
        for d in &mut out {
            d["written"] = json!(true);
        }
    }
    out
}

/// What to say after the write. A machine that is running has not got the
/// disk yet, and the difference matters enough to be the whole message.
pub fn effect(running: bool, verb: &str, name: &str) -> String {
    if running {
        format!(
            "{name} {verb} — the guest sees it at its next boot. Nothing attaches a disk to a \
             running machine on this platform (stormvm has no device verb), so restart it when \
             you want it"
        )
    } else {
        format!("{name} {verb}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Value {
        json!({
            "domain": {"devices": {"disks": [
                {"name": "root", "disk": {"bus": "virtio"}},
                {"name": "seed", "disk": {"bus": "virtio"}}
            ]}},
            "volumes": [
                {"name": "root", "dataVolume": {"name": "rocky-10"}},
                {"name": "seed", "cloudInitNoCloud": {}}
            ]
        })
    }

    #[test]
    fn a_disk_is_added_as_both_halves_at_once() {
        let p = add(&spec(), &Add {
            name: "data".into(),
            source: "pvc".into(),
            from: "pg".into(),
            bus: "scsi".into(),
        })
        .unwrap();
        let disks = p.pointer("/spec/template/spec/domain/devices/disks").unwrap();
        let volumes = p.pointer("/spec/template/spec/volumes").unwrap();
        // Whole arrays, because a merge patch replaces rather than merges —
        // sending only the new element would leave a machine with one disk.
        assert_eq!(disks.as_array().unwrap().len(), 3);
        assert_eq!(volumes.as_array().unwrap().len(), 3);
        assert_eq!(disks.pointer("/2/disk/bus").unwrap(), "scsi");
        assert_eq!(volumes.pointer("/2/persistentVolumeClaim/claimName").unwrap(), "pg");
    }

    #[test]
    fn each_source_writes_the_volume_kubevirt_expects() {
        let of = |source: &str, from: &str| {
            add(&spec(), &Add {
                name: "d".into(),
                source: source.into(),
                from: from.into(),
                ..Default::default()
            })
            .unwrap()
            .pointer("/spec/template/spec/volumes/2")
            .unwrap()
            .clone()
        };
        assert!(of("golden", "rocky-10").pointer("/dataVolume/name").is_some());
        assert!(of("pvc", "pg").pointer("/persistentVolumeClaim/claimName").is_some());
        assert_eq!(of("empty", "10Gi").pointer("/emptyDisk/capacity").unwrap(), "10Gi");
    }

    #[test]
    fn a_name_that_already_exists_is_refused_rather_than_shadowing() {
        let e = add(&spec(), &Add {
            name: "root".into(),
            source: "pvc".into(),
            from: "pg".into(),
            ..Default::default()
        })
        .unwrap_err();
        assert!(e.contains("already has a disk"), "{e}");
    }

    #[test]
    fn a_disk_needs_a_name_a_source_and_something_behind_it() {
        let bad = |a: Add| add(&spec(), &a).unwrap_err();
        assert!(bad(Add::default()).contains("needs a name"));
        assert!(bad(Add { name: "Data!".into(), ..Default::default() }).contains("not a usable"));
        assert!(bad(Add { name: "d".into(), ..Default::default() }).contains("something behind"));
        assert!(bad(Add { name: "d".into(), from: "x".into(), ..Default::default() })
            .contains("not a disk source"));
        assert!(bad(Add {
            name: "d".into(),
            from: "x".into(),
            source: "pvc".into(),
            bus: "ide".into()
        })
        .contains("not a disk bus"));
    }

    #[test]
    fn removing_a_disk_takes_both_halves() {
        let with = add(&spec(), &Add {
            name: "data".into(),
            source: "pvc".into(),
            from: "pg".into(),
            ..Default::default()
        })
        .unwrap();
        let inner = with.pointer("/spec/template/spec").unwrap();
        let p = remove(inner, "data").unwrap();
        assert_eq!(p.pointer("/spec/template/spec/domain/devices/disks").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(p.pointer("/spec/template/spec/volumes").unwrap().as_array().unwrap().len(), 2);
    }

    /// Both refusals exist because the result looks like a broken machine
    /// rather than an edited one.
    #[test]
    fn the_root_disk_and_the_seed_are_refused() {
        assert!(remove(&spec(), "root").unwrap_err().contains("cannot start"));
        assert!(remove(&spec(), "seed").unwrap_err().contains("nothing can log into it"));
        assert!(remove(&spec(), "nope").unwrap_err().contains("no disk called"));
    }

    #[test]
    fn a_disk_written_but_not_attached_is_visible_and_says_so() {
        let definition = add(&spec(), &Add {
            name: "data".into(),
            source: "pvc".into(),
            from: "pg".into(),
            ..Default::default()
        })
        .unwrap();
        let definition = definition.pointer("/spec/template/spec").unwrap().clone();
        let out = merged(Some(&definition), Some(&spec()));
        let by = |n: &str| out.iter().find(|d| d["name"] == n).unwrap().clone();
        // Added to the definition, not yet in the guest — and there,
        // rather than vanishing from the page the moment it was added.
        assert_eq!(by("data")["attached"], json!(false));
        assert_eq!(by("data")["written"], json!(true));
        assert_eq!(by("data")["removable"], json!(true));
        assert_eq!(by("root")["attached"], json!(true));
        assert_eq!(by("root")["removable"], json!(false));
    }

    /// The same mistake as warning a stopped machine about a restart it
    /// does not need.
    #[test]
    fn a_stopped_machine_has_no_pending_disks() {
        let definition = add(&spec(), &Add {
            name: "data".into(),
            source: "pvc".into(),
            from: "pg".into(),
            ..Default::default()
        })
        .unwrap();
        let definition = definition.pointer("/spec/template/spec").unwrap().clone();
        let out = merged(Some(&definition), None);
        assert!(out.iter().all(|d| d["attached"] == json!(true)), "{out:?}");
    }

    #[test]
    fn a_disk_removed_but_still_attached_is_visible_too() {
        // Definition without `seed`, instance with it: removed, and the
        // guest still has it until it restarts.
        let mut definition = spec();
        definition["domain"]["devices"]["disks"] = json!([{"name": "root", "disk": {"bus": "virtio"}}]);
        definition["volumes"] = json!([{"name": "root", "dataVolume": {"name": "rocky-10"}}]);
        let out = merged(Some(&definition), Some(&spec()));
        let seed = out.iter().find(|d| d["name"] == "seed").unwrap();
        assert_eq!(seed["written"], json!(false));
        assert_eq!(seed["attached"], json!(true));
    }

    /// The whole point: a running machine has not got the disk yet, and
    /// the message is where that is said.
    #[test]
    fn a_running_machine_is_told_the_disk_is_not_there_yet() {
        let m = effect(true, "added", "data");
        assert!(m.contains("next boot"), "{m}");
        assert!(m.contains("no device verb"), "{m}");
        assert_eq!(effect(false, "added", "data"), "data added");
    }
}
