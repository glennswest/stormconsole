//! A drive's firmware, from every node (#317).
//!
//! stormdrive holds all of it (`firmware.rs`, `api/mod.rs`): an image store
//! per node (`GET|PUT /api/v1/firmware/images[/{name}]`), the runs, which it
//! keeps after they finish (`GET /api/v1/firmware`), and each drive's version
//! and last update on `/api/v1/drives` (`firmware`, `firmware_run`,
//! `firmware_update`). It updates data-serving drives one at a time per node
//! and holds one back while a volume on it is not fully redundant. So the
//! console keeps nothing: a reload reads the same answer.
//!
//! Two things stormdrive cannot check yet are checked here, before any node
//! is asked (stormdrive#126):
//!
//! - **The model.** An image in the store has a name, a size and a sha256,
//!   and nothing says what drive it is for. The console names what it
//!   uploads `<model>+<file>`, so an image named for a model is that model's;
//!   one named for another model is refused, and one that names none is
//!   started only when the person says they checked it.
//! - **A RAID set.** stormdrive's gate looks at volumes, not at the engine's
//!   sets, so two members of one set could reset one after the other while
//!   the set still resyncs from the first. The console refuses a selection
//!   with two members of one set, and a member of a set that is not clean.
//!
//! Starting an update is destructive storage (#82): the host's gate, as the
//! viewer, and a phrase typed on top.

use serde_json::{json, Value};

pub use crate::format::FRESH;

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// A model as it may stand in an image name: stormdrive takes letters,
/// digits and `. _ - +` there, and `+` is the separator.
pub fn model_key(model: &str) -> String {
    let k: String = model
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect();
    k.trim_matches('-').to_string()
}

/// The model an image is named for, if it was named the console's way.
pub fn image_model(name: &str) -> Option<&str> {
    name.split_once('+').map(|(m, _)| m).filter(|m| !m.is_empty())
}

/// What the console names an upload of `file` for `model`.
pub fn upload_name(model: &str, file: &str) -> Result<String, String> {
    let m = model_key(model);
    if m.is_empty() {
        return Err("the drive reports no model".into());
    }
    let f: String = file
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect();
    let f = f.trim_matches(['-', '.']).to_string();
    if f.is_empty() {
        return Err(format!("{file:?} is not a file name"));
    }
    let name = format!("{m}+{f}");
    if !valid_image_name(&name) {
        return Err(format!("{name:?}: letters, digits, . _ - + only, at most 128"));
    }
    Ok(name)
}

/// stormdrive's own rule (`firmware::valid_image_name`), said here so a bad
/// name reaches no node.
pub fn valid_image_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 128
        && !n.starts_with('.')
        && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
}

/// Why stormdrive would not start an update on this drive now
/// (`Drive::firmware_blocker`), without `force`, which the console does not
/// offer: a drive already failing is not worth the risk from a browser.
pub fn why_not(d: &Value) -> Option<String> {
    match text(d, "activity") {
        "updating_firmware" => return Some("a firmware update is already running".into()),
        "idle" | "" => {}
        other => return Some(format!("it is {}", other.replace('_', " "))),
    }
    let health = d.pointer("/health/status").and_then(Value::as_str).unwrap_or("");
    if matches!(health, "failing" | "failed") {
        return Some(format!("its health is {health}: replace it rather than update it"));
    }
    if text(d, "firmware").is_empty() {
        return Some("it reports no firmware version".into());
    }
    None
}

/// A run in a person's terms: stormdrive's `FwRun` (live) or `FirmwareRecord`
/// (kept), the same either way.
fn run_view(run: &Value) -> Value {
    let done = run.get("bytes_done").and_then(Value::as_u64);
    let total = run.get("bytes_total").and_then(Value::as_u64);
    json!({
        "image": run.get("image"),
        "from": run.get("from_version"),
        "to": run.get("to_version"),
        "state": run.get("state"),
        "phase": run.get("phase"),
        "percent": match (done, total) { (Some(d), Some(t)) if t > 0 => json!(((d * 100) / t).min(100)), _ => Value::Null },
        "started": run.get("started"),
        "finished": run.get("finished"),
        "error": run.get("error"),
        "resetRequired": run.get("reset_required"),
    })
}

/// One node's answers → its drives and its images.
pub fn reduce(drives: &Value, images: Option<&Value>, node: &str, prefix: &str) -> (Vec<Value>, Vec<Value>) {
    let list = drives.get("drives").and_then(Value::as_array).cloned().unwrap_or_default();
    let ds = list
        .iter()
        .filter_map(|d| {
            let id = crate::usage::drive_id(d)?;
            let live = d.get("firmware_run").filter(|r| r.is_object());
            let kept = d.get("firmware_update").filter(|r| r.is_object());
            Some(json!({
                "component": format!("{prefix}:drive:{id}"),
                "node": node,
                "id": id,
                "name": text(d, "name"),
                "vendor": d.get("vendor"),
                "model": text(d, "model").trim(),
                "modelKey": model_key(text(d, "model")),
                "serial": text(d, "serial"),
                "kind": text(d, "kind"),
                "version": text(d, "firmware"),
                "use": crate::inuse::of(d),
                "run": live.or(kept).map(run_view),
                "whyNot": why_not(d),
            }))
        })
        .collect();
    let imgs = images
        .and_then(|v| v.get("images"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|i| {
            let name = text(i, "name");
            json!({
                "node": node,
                "name": name,
                "size": i.get("size"),
                "sha256": i.get("sha256"),
                "modified": i.get("modified"),
                "model": image_model(name),
            })
        })
        .collect();
    (ds, imgs)
}

/// One drive to update, as the page asks.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Wanted {
    pub node: String,
    pub id: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct Start {
    pub drives: Vec<Wanted>,
    pub image: String,
    #[serde(default)]
    pub confirm: String,
    /// The person checked an image that names no model is for this one.
    #[serde(default)]
    pub image_checked: bool,
}

/// The phrase that confirms an update: one drive's serial, or for several how
/// many are about to reset.
pub fn phrase(serials: &[&str]) -> String {
    match serials {
        [one] if !one.is_empty() => (*one).to_string(),
        many => format!("update {} drives", many.len()),
    }
}

/// One model per update, and the image named for it (or checked by hand).
pub fn check_model(drives: &[&Value], image: &str, checked: bool) -> Result<(), String> {
    let mut models: Vec<&str> = drives.iter().map(|d| d["modelKey"].as_str().unwrap_or("")).collect();
    models.sort_unstable();
    models.dedup();
    let [model] = models.as_slice() else {
        let names: Vec<String> = drives.iter().map(|d| format!("{} ({})", d["name"].as_str().unwrap_or(""), d["model"].as_str().unwrap_or("no model"))).collect();
        return Err(format!("one model at a time: {}", names.join(", ")));
    };
    if model.is_empty() {
        return Err("the drive reports no model, so no image can be matched to it".into());
    }
    match image_model(image) {
        Some(m) if m.eq_ignore_ascii_case(model) => Ok(()),
        Some(m) => Err(format!("{image} is for {m}, and these drives are {model}")),
        None if checked => Ok(()),
        None => Err(format!(
            "{image} does not name a model, and stormdrive does not check one (stormdrive#126): say you checked it is for {model}"
        )),
    }
}

/// A RAID member on this node: its set, and the set's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub set: String,
    pub state: String,
}

/// Each member of this engine's sets, by drive serial and by device path.
pub fn members(arrays: &[Value]) -> Vec<(String, String, Member)> {
    let mut out = Vec::new();
    for a in arrays {
        let set = a.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).or_else(|| a.get("id").and_then(Value::as_str)).unwrap_or("").to_string();
        let state = a.pointer("/status/state").and_then(Value::as_str).unwrap_or("unknown").to_string();
        for m in a.get("members").and_then(Value::as_array).into_iter().flatten() {
            let serial = m.pointer("/drive/serial").and_then(Value::as_str).unwrap_or("").to_string();
            let path = m.get("device_path").and_then(Value::as_str).unwrap_or("").to_string();
            out.push((serial, path, Member { set: set.clone(), state: state.clone() }));
        }
    }
    out
}

/// The set a drive is in, by serial first (a file-backed drive's serial is
/// `file`, which names nothing), then its device.
pub fn member_of<'a>(members: &'a [(String, String, Member)], serial: &str, dev: &str) -> Option<&'a Member> {
    let by_serial = (!serial.is_empty() && serial != "file").then(|| members.iter().find(|m| m.0 == serial)).flatten();
    by_serial.or_else(|| (!dev.is_empty()).then(|| members.iter().find(|m| m.1 == dev)).flatten()).map(|m| &m.2)
}

/// Never two members of one set at once, and never a member of a set that is
/// not clean: until stormdrive waits for the set (stormdrive#126), the next
/// member could reset while the set still resyncs from the last.
pub fn check_sets(chosen: &[(String, Option<Member>)]) -> Result<(), String> {
    for (name, m) in chosen {
        if let Some(m) = m {
            if m.state != "clean" {
                return Err(format!("{name} is in RAID set {}, which is {}: update it once the set is clean", m.set, m.state));
            }
        }
    }
    for (i, (a, ma)) in chosen.iter().enumerate() {
        let Some(ma) = ma else { continue };
        if let Some((b, _)) = chosen[i + 1..].iter().find(|(_, mb)| mb.as_ref().is_some_and(|mb| mb.set == ma.set)) {
            return Err(format!(
                "{a} and {b} are both in RAID set {}: one at a time, so the set never loses redundancy. Update {a}, then {b} once the set is clean again (stormdrive#126)",
                ma.set
            ));
        }
    }
    Ok(())
}

/// The requests to make: one per node, because stormdrive checks a request
/// all-or-nothing.
pub fn batches(wanted: &[Wanted]) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for w in wanted {
        match out.iter_mut().find(|b| b.0 == w.node) {
            Some(b) => {
                if !b.1.contains(&w.id) {
                    b.1.push(w.id.clone());
                }
            }
            None => out.push((w.node.clone(), vec![w.id.clone()])),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_stands_in_a_name_and_an_upload_is_named_for_it() {
        assert_eq!(model_key(" H7210A521SUN010T "), "H7210A521SUN010T");
        assert_eq!(model_key("Samsung SSD 870 EVO"), "Samsung-SSD-870-EVO");
        assert_eq!(upload_name("H7210A521SUN010T", "C:\\fw\\A3Z0 build.bin").unwrap(), "H7210A521SUN010T+A3Z0-build.bin");
        assert_eq!(image_model("H7210A521SUN010T+A3Z0.bin"), Some("H7210A521SUN010T"));
        assert_eq!(image_model("A3Z0.bin"), None);
        assert_eq!(image_model("+x"), None);
        assert!(upload_name("", "x.bin").is_err());
        assert!(upload_name("M", "/").is_err());
        assert!(!valid_image_name(".hidden") && !valid_image_name("a b") && valid_image_name("M+a.bin"));
    }

    #[test]
    fn stormdrives_refusals_are_said_before_anyone_asks() {
        let w = |v: Value| why_not(&v);
        assert!(w(json!({"activity": "idle", "firmware": "A3Z0"})).is_none());
        assert!(w(json!({"activity": "updating_firmware", "firmware": "A"})).unwrap().contains("already running"));
        assert_eq!(w(json!({"activity": "formatting", "firmware": "A"})).unwrap(), "it is formatting");
        assert!(w(json!({"activity": "idle", "firmware": "A", "health": {"status": "failing"}})).unwrap().contains("replace it"));
        assert!(w(json!({"activity": "idle"})).unwrap().contains("no firmware version"));
    }

    #[test]
    fn a_drive_says_its_version_and_its_run_and_images_their_model() {
        let drives = json!({"drives": [
            {"id": "a1", "name": "sdb", "vendor": "HGST", "model": "H7210A521SUN010T ", "serial": "SN1", "firmware": "A3Z0", "activity": "updating_firmware",
             "firmware_run": {"image": "H7210A521SUN010T+A3Z2.bin", "from_version": "A3Z0", "to_version": null, "state": "running", "phase": "download", "bytes_done": 512, "bytes_total": 2048},
             "firmware_update": {"image": "old", "state": "done"}},
            {"id": "a2", "name": "sdc", "model": "M", "serial": "SN2", "firmware": "1", "activity": "idle",
             "firmware_update": {"image": "M+2.bin", "from_version": "1", "to_version": "2", "state": "done"}},
        ]});
        let images = json!({"images": [{"name": "H7210A521SUN010T+A3Z2.bin", "size": 2048, "sha256": "ab"}, {"name": "loose.bin", "size": 1}]});
        let (d, i) = reduce(&drives, Some(&images), "here", "drive");
        assert_eq!(d[0]["component"], "drive:drive:a1");
        assert_eq!(d[0]["model"], "H7210A521SUN010T");
        assert_eq!(d[0]["run"]["percent"], 25, "the live run, not the kept one");
        assert_eq!(d[0]["run"]["phase"], "download");
        assert!(d[0]["whyNot"].as_str().unwrap().contains("already running"));
        assert_eq!(d[1]["run"]["to"], "2", "the kept record, read back");
        assert_eq!(i[0]["model"], "H7210A521SUN010T");
        assert!(i[1]["model"].is_null());
        assert_eq!(reduce(&drives, None, "here", "drive").1.len(), 0, "no store answered, no images");
    }

    #[test]
    fn one_model_and_its_image() {
        let d = |name: &str, m: &str| json!({"name": name, "model": m, "modelKey": model_key(m)});
        let (b, c, x) = (d("sdb", "H7210A521SUN010T"), d("sdc", "H7210A521SUN010T"), d("sdx", "OTHER"));
        assert!(check_model(&[&b, &c], "H7210A521SUN010T+A3Z2.bin", false).is_ok());
        assert!(check_model(&[&b, &c], "h7210a521sun010t+A3Z2.bin", false).is_ok(), "case is not a model");
        assert!(check_model(&[&b, &x], "H7210A521SUN010T+A3Z2.bin", false).unwrap_err().starts_with("one model at a time"));
        assert_eq!(check_model(&[&b], "OTHER+1.bin", false).unwrap_err(), "OTHER+1.bin is for OTHER, and these drives are H7210A521SUN010T");
        assert!(check_model(&[&b], "OTHER+1.bin", true).is_err(), "checking by hand never overrides a name");
        assert!(check_model(&[&b], "loose.bin", false).unwrap_err().contains("stormdrive#126"));
        assert!(check_model(&[&b], "loose.bin", true).is_ok());
        assert!(check_model(&[&d("sdz", "")], "loose.bin", true).is_err());
    }

    #[test]
    fn one_member_of_a_set_at_a_time_and_only_when_it_is_clean() {
        let arrays = [json!({"name": "ds0-a", "status": {"state": "clean"}, "members": [
            {"device_path": "/dev/sdb", "drive": {"serial": "SN1"}}, {"device_path": "/dev/sdc", "drive": {"serial": "file"}}]}),
            json!({"id": "a9", "status": {"state": "rebuilding"}, "members": [{"device_path": "/dev/sdk", "drive": {"serial": "SN9"}}]})];
        let ms = members(&arrays);
        let b = member_of(&ms, "SN1", "").cloned();
        let c = member_of(&ms, "file", "/dev/sdc").cloned();
        assert_eq!(c.as_ref().unwrap().set, "ds0-a", "a file drive by its path");
        let k = member_of(&ms, "SN9", "").cloned();
        assert_eq!(k.as_ref().unwrap().set, "a9", "a set with no name by its id");
        assert!(member_of(&ms, "ZZ", "/dev/sdz").is_none());
        assert!(check_sets(&[("sdb".into(), b.clone()), ("sdd".into(), None)]).is_ok());
        let two = check_sets(&[("sdb".into(), b), ("sdc".into(), c)]).unwrap_err();
        assert!(two.starts_with("sdb and sdc are both in RAID set ds0-a"), "{two}");
        assert!(check_sets(&[("sdk".into(), k)]).unwrap_err().contains("which is rebuilding"));
    }

    #[test]
    fn one_request_per_node() {
        let w = |n: &str, id: &str| Wanted { node: n.into(), id: id.into() };
        let b = batches(&[w("a", "1"), w("b", "2"), w("a", "3"), w("a", "1")]);
        assert_eq!(b, vec![("a".to_string(), vec!["1".to_string(), "3".to_string()]), ("b".to_string(), vec!["2".to_string()])]);
        assert_eq!(phrase(&["SN1"]), "SN1");
        assert_eq!(phrase(&["SN1", "SN2"]), "update 2 drives");
    }
}
