//! SAS cards and their flash (#376, stormdrive#166).
//!
//! stormdrive writes one `storage.storm.io/v1 HostBusAdapter` per SAS card
//! into the apiserver: storcli's inventory, read through the card's own
//! interface (identity, versions, IOC Facts, every phy and the device on it,
//! each firmware region read back with its sha256, the last flash). A
//! `CardOperation` is sas3flash's flash: the node that has the card runs it
//! once the apiserver has stamped who asked (rustkube#210) and that person
//! still holds `cardoperations` create.
//!
//! Both are read **as the viewer**, as every custom resource is (#348): the
//! apiserver's RBAC decides what they see, and a refusal is said in its
//! words. A flash is **created as the viewer** too — the bearer the host's
//! storage gate put on the request once the review allowed it and the card's
//! name was typed — so the requester stormdrive re-checks is the person, never
//! the console.
//!
//! The image is a golden by stormdrive's rule: a file in the node's firmware
//! store with the sha256 named here, and the URL it was recorded from. The
//! request is checked here as stormdrive checks it, so a malformed one never
//! becomes an object; the node's own checks (the image against the card,
//! the system half, I/O in flight) come back on the operation's status.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use console_core::storage::ActAs;
use console_core::Viewer;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::client::{As, RkError};
use crate::Inner;

pub const HBAS: &str = "/apis/storage.storm.io/v1/hostbusadapters";
pub const OPS: &str = "/apis/storage.storm.io/v1/cardoperations";

/// The regions a flash may write (stormdrive `card::check_image`).
pub const REGIONS: [&str; 2] = ["firmware", "bios"];

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({"error": msg.into()}))).into_response()
}

fn refused(e: &RkError, what: &str) -> (StatusCode, String) {
    match e {
        RkError::Status(s) if s.as_u16() == 403 => (StatusCode::FORBIDDEN, format!("the apiserver says you may not {what}")),
        RkError::Status(s) if s.as_u16() == 404 => (
            StatusCode::NOT_FOUND,
            format!("the apiserver does not serve {what}: stormdrive's CRDs (v0.56.0, stormcos#1242) are not installed"),
        ),
        other => (StatusCode::BAD_GATEWAY, other.to_string()),
    }
}

fn items(v: Value) -> Vec<Value> {
    match v {
        Value::Object(mut o) => match o.remove("items") {
            Some(Value::Array(a)) => a,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn s<'a>(v: &'a Value, ptr: &str) -> &'a str {
    v.pointer(ptr).and_then(Value::as_str).unwrap_or("")
}

/// One CardOperation as the page shows it.
pub fn operation(o: &Value) -> Value {
    let st = &o["status"];
    json!({
        "name": o.pointer("/metadata/name"),
        "created": o.pointer("/metadata/creationTimestamp"),
        "requester": st.get("requester").filter(|v| !v.is_null()).or_else(|| o.pointer("/metadata/annotations/storage.storm.io~1requester")),
        "node": o.pointer("/spec/node"),
        "card": o.pointer("/spec/card"),
        "region": o.pointer("/spec/region"),
        "image": o.pointer("/spec/image"),
        "dryRun": o.pointer("/spec/dryRun").and_then(Value::as_bool).unwrap_or(false),
        "phase": st.get("phase").and_then(Value::as_str).unwrap_or(""),
        "message": st.get("message"),
        "step": st.get("step"),
        "bytesDone": st.get("bytesDone"),
        "bytesTotal": st.get("bytesTotal"),
        "imageVersion": st.get("imageVersion"),
        "before": st.get("before"),
        "after": st.get("after"),
        "started": st.get("started"),
        "finished": st.get("finished"),
    })
}

/// Is this operation about this card? stormdrive takes the card's object name
/// or its PCI address, on the card's node.
pub fn is_for(op: &Value, card: &Value) -> bool {
    let want = s(op, "/spec/card");
    let node = s(op, "/spec/node");
    let name = s(card, "/metadata/name");
    let pci = s(card, "/status/pci");
    let cnode = s(card, "/status/node");
    (want == name || (!pci.is_empty() && want == pci && node.eq_ignore_ascii_case(cnode))) && !want.is_empty()
}

/// One card: its object's name and status, and its operations, newest first.
pub fn card(o: &Value, ops: &[Value]) -> Value {
    let mut mine: Vec<&Value> = ops.iter().filter(|op| is_for(op, o)).collect();
    mine.sort_by(|a, b| s(b, "/metadata/creationTimestamp").cmp(s(a, "/metadata/creationTimestamp")).then(s(b, "/metadata/name").cmp(s(a, "/metadata/name"))));
    json!({
        "name": o.pointer("/metadata/name"),
        "labels": o.pointer("/metadata/labels"),
        "status": o.get("status").cloned().unwrap_or(Value::Null),
        "operations": mine.into_iter().map(operation).collect::<Vec<_>>(),
    })
}

/// `GET /hbas`: every card this viewer may see, each with its operations.
/// The operations may be refused while the cards are not (a storage-viewer
/// role that reads only some kinds): then they are said to be unread.
pub async fn list(State(inner): State<Arc<Inner>>, viewer: Viewer) -> Response {
    let Some(client) = &inner.client else { return err(StatusCode::SERVICE_UNAVAILABLE, "no apiserver") };
    let token = viewer.token.as_deref();
    let (cards, ops) = tokio::join!(client.get_as(HBAS, token), client.get_as(OPS, token));
    let cards = match cards {
        Ok(v) => items(v),
        Err(e) => {
            let (code, msg) = refused(&e, "list hostbusadapters.storage.storm.io");
            return err(code, msg);
        }
    };
    let (ops, ops_error) = match ops {
        Ok(v) => (items(v), Value::Null),
        Err(e) => (Vec::new(), json!(refused(&e, "list cardoperations.storage.storm.io").1)),
    };
    let mut out: Vec<Value> = cards.iter().map(|c| card(c, &ops)).collect();
    out.sort_by(|a, b| {
        let k = |c: &Value| (s(c, "/status/node").to_string(), s(c, "/name").to_string());
        k(a).cmp(&k(b))
    });
    // An operation naming no card this viewer sees still happened: it is listed apart.
    let orphans: Vec<Value> = ops.iter().filter(|op| !cards.iter().any(|c| is_for(op, c))).map(operation).collect();
    Json(json!({"items": out, "orphans": orphans, "operationsError": ops_error})).into_response()
}

/// What the page sends to flash a card.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlashRequest {
    pub region: String,
    pub image: Golden,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Golden {
    pub name: String,
    pub sha256: String,
    pub source: String,
}

/// A firmware store name, as stormdrive takes one (`firmware::valid_image_name`).
pub fn store_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 128
        && !n.starts_with('.')
        && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
}

/// The request, checked as stormdrive checks it; `Err` says what is wrong.
pub fn check(r: &FlashRequest) -> Result<(), String> {
    if !REGIONS.contains(&r.region.as_str()) {
        return Err(format!("region {:?}: a flash writes firmware or bios", r.region));
    }
    if !store_name(&r.image.name) {
        return Err(format!("image {:?}: not a firmware store name (letters, digits, . _ - +, at most 128)", r.image.name));
    }
    let sha = r.image.sha256.trim();
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("image sha256: 64 hex digits".into());
    }
    let src = r.image.source.trim();
    if !(src.starts_with("https://") || src.starts_with("http://")) || src.len() < 10 || src.chars().any(char::is_whitespace) {
        return Err("image source: the http(s) URL the golden was recorded from (compliance-ops#23)".into());
    }
    Ok(())
}

/// The CardOperation for this card. Its name is the card's, the region and
/// the time, so two flashes never collide and the list sorts by when.
pub fn object(card: &str, node: &str, r: &FlashRequest, now: u64) -> Value {
    let mut name = format!("{card}-{}-{now}", r.region);
    name.truncate(253);
    let mut spec = json!({
        "node": node,
        "card": card,
        "op": "flash",
        "region": r.region,
        "image": {"name": r.image.name, "sha256": r.image.sha256.trim().to_lowercase(), "source": r.image.source.trim()},
    });
    if r.dry_run {
        spec["dryRun"] = json!(true);
    }
    json!({
        "apiVersion": "storage.storm.io/v1",
        "kind": "CardOperation",
        "metadata": {"name": name.trim_end_matches('-')},
        "spec": spec,
    })
}

/// `POST /hbas/{name}/flash`: create the CardOperation, as the viewer. The
/// host's storage gate has already asked the apiserver whether this person
/// may create cardoperations and had them type the card's name; without its
/// mark the request did not pass it and goes nowhere.
pub async fn flash(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    act: Option<Extension<ActAs>>,
    Path(name): Path<String>,
    Json(req): Json<FlashRequest>,
) -> Response {
    let Some(Extension(act)) = act else {
        return err(StatusCode::FORBIDDEN, "flash a SAS card: not authorised for this request");
    };
    if let Err(why) = check(&req) {
        return err(StatusCode::BAD_REQUEST, why);
    }
    let Some(client) = &inner.client else { return err(StatusCode::SERVICE_UNAVAILABLE, "no apiserver") };
    // The bearer the gate chose: the person's own, or the console's for root.
    let token = act.0.as_deref();
    let card = match client.get_as(&format!("{HBAS}/{name}"), token).await {
        Ok(c) => c,
        Err(RkError::Status(s)) if s.as_u16() == 404 => return err(StatusCode::NOT_FOUND, format!("no SAS card {name}")),
        Err(e) => {
            let (code, msg) = refused(&e, &format!("read the SAS card {name}"));
            return err(code, msg);
        }
    };
    let node = s(&card, "/status/node");
    if node.is_empty() {
        return err(StatusCode::CONFLICT, format!("{name} names no node: stormdrive has not written its status"));
    }
    if card.pointer("/status/present").and_then(Value::as_bool) == Some(false) {
        return err(StatusCode::CONFLICT, format!("{name} is not seen on {node} now: nothing to flash"));
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let obj = object(&name, node, &req, now);
    match client.post_json_as(OPS, &obj, As::chosen(token)).await {
        Ok((st, body)) if st.is_success() => {
            tracing::info!(user = viewer.user.as_deref().unwrap_or("root"), card = %name, op = %s(&body, "/metadata/name"), dry_run = req.dry_run, "hba: CardOperation created");
            (StatusCode::CREATED, Json(json!({"operation": operation(&body)}))).into_response()
        }
        Ok((st, body)) => {
            let why = body.get("message").and_then(Value::as_str).unwrap_or("").to_string();
            let msg = match st.as_u16() {
                403 => format!("the apiserver says you may not create cardoperations.storage.storm.io{}", if why.is_empty() { String::new() } else { format!(": {why}") }),
                404 => "the apiserver does not serve cardoperations.storage.storm.io: stormdrive's CRDs (stormcos#1242) are not installed".into(),
                _ => format!("the apiserver answered {st}{}", if why.is_empty() { String::new() } else { format!(": {why}") }),
            };
            err(StatusCode::from_u16(st.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY), msg)
        }
        Err(e) => err(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hba() -> Value {
        json!({"metadata": {"name": "forge1-0000-01-00-0"}, "status": {"node": "forge1", "pci": "0000:01:00.0", "present": true, "board": "HBA330 Mini"}})
    }
    fn op(name: &str, card: &str, node: &str, at: &str, phase: &str) -> Value {
        json!({"metadata": {"name": name, "creationTimestamp": at, "annotations": {"storage.storm.io/requester": "alice"}},
            "spec": {"node": node, "card": card, "op": "flash", "region": "firmware", "image": {"name": "a.bin", "sha256": "ab", "source": "https://x/a.bin"}},
            "status": {"phase": phase, "message": "m", "before": "16.17.00.03", "after": null}})
    }
    fn req(region: &str, name: &str, sha: &str, src: &str) -> FlashRequest {
        FlashRequest { region: region.into(), image: Golden { name: name.into(), sha256: sha.into(), source: src.into() }, dry_run: false }
    }
    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789ABCDEF0123456789abcdef";

    #[test]
    fn an_operation_belongs_to_its_card_by_name_or_by_pci_on_its_node() {
        let c = hba();
        assert!(is_for(&op("a", "forge1-0000-01-00-0", "forge1", "t", ""), &c));
        assert!(is_for(&op("a", "0000:01:00.0", "forge1", "t", ""), &c));
        assert!(!is_for(&op("a", "0000:01:00.0", "forge2", "t", ""), &c), "the same PCI address on another node is another card");
        assert!(!is_for(&op("a", "", "forge1", "t", ""), &c));
    }

    #[test]
    fn a_card_carries_its_operations_newest_first_with_their_requester() {
        let ops = vec![
            op("old", "forge1-0000-01-00-0", "forge1", "2026-11-19T10:00:00Z", "Succeeded"),
            op("new", "forge1-0000-01-00-0", "forge1", "2026-11-19T11:00:00Z", "Refused"),
            op("other", "forge2-0000-01-00-0", "forge2", "2026-11-19T12:00:00Z", "Running"),
        ];
        let c = card(&hba(), &ops);
        let names: Vec<&str> = c["operations"].as_array().unwrap().iter().map(|o| o["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["new", "old"]);
        assert_eq!(c["operations"][0]["requester"], "alice", "from the apiserver's stamp when the status has none");
        assert_eq!(c["operations"][0]["phase"], "Refused");
        assert_eq!(c["status"]["board"], "HBA330 Mini");
    }

    #[test]
    fn a_request_is_checked_as_stormdrive_checks_it() {
        assert!(check(&req("firmware", "hba330-16.17.01.00.bin", SHA, "https://dl.example/hba330.bin")).is_ok());
        assert!(check(&req("bios", "x+rom.bin", SHA, "http://mirror/rom.bin")).is_ok());
        assert!(check(&req("manufacturing", "a.bin", SHA, "https://x/a.bin")).unwrap_err().contains("firmware or bios"));
        assert!(check(&req("firmware", "../a.bin", SHA, "https://x/a.bin")).unwrap_err().contains("store name"));
        assert!(check(&req("firmware", ".a", SHA, "https://x/a.bin")).is_err());
        assert!(check(&req("firmware", "a.bin", "abc", "https://x/a.bin")).unwrap_err().contains("64 hex"));
        assert!(check(&req("firmware", "a.bin", &SHA.replace('0', "g"), "https://x/a.bin")).is_err());
        assert!(check(&req("firmware", "a.bin", SHA, "ftp://x/a.bin")).unwrap_err().contains("recorded from"));
        assert!(check(&req("firmware", "a.bin", SHA, "https://x /a.bin")).is_err());
    }

    #[test]
    fn the_object_is_stormdrives_spec_with_the_sha_lowered() {
        let mut r = req("firmware", "a.bin", SHA, " https://x/a.bin ");
        let o = object("forge1-0000-01-00-0", "forge1", &r, 1795000000);
        assert_eq!(o["metadata"]["name"], "forge1-0000-01-00-0-firmware-1795000000");
        assert_eq!(o["spec"]["op"], "flash");
        assert_eq!(o["spec"]["node"], "forge1");
        assert_eq!(o["spec"]["image"]["sha256"], SHA.to_lowercase());
        assert_eq!(o["spec"]["image"]["source"], "https://x/a.bin");
        assert!(o["spec"].get("dryRun").is_none(), "stormdrive denies unknown fields; a real flash carries no dryRun");
        r.dry_run = true;
        assert_eq!(object("c", "n", &r, 1)["spec"]["dryRun"], true);
    }

    #[test]
    fn a_request_with_a_field_stormdrive_does_not_take_is_refused() {
        let bad: Result<FlashRequest, _> = serde_json::from_value(json!({"region": "firmware", "image": {"name": "a", "sha256": "b", "source": "c"}, "force": true}));
        assert!(bad.is_err());
        let ok: FlashRequest = serde_json::from_value(json!({"region": "bios", "image": {"name": "a", "sha256": "b", "source": "c"}, "dryRun": true})).unwrap();
        assert!(ok.dry_run);
    }
}
