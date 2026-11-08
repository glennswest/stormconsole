//! RAID controllers, every node's (#325, stormdrive#132).
//!
//! stormdrive v0.43.0 reads each `megaraid_sas` controller (a PERC) over MFI
//! and, where perccli is on the node, its personality: `GET
//! /api/v1/controllers` → `{controllers:[ControllerReport], findings}`
//! (`raidctl.rs`). A controller in RAID mode hides its disks from Linux
//! behind its own configuration, so HBA mode is mandatory and stormdrive
//! raises `raid_mode` until HBA is asked for, then `reboot_pending`. A disk
//! the controller has failed is invisible to Linux (0 bytes offered) and
//! reads `failed` here, not "missing".
//!
//! This only reshapes what each node said: the node, the proxy its verbs go
//! through, `perccli` as `{ok, path|why}` rather than serde's `Ok`/`Err`, and
//! each disk joined to the drive row the console serves it as.

use serde_json::{json, Value};

/// How long an answer is reused: a page polls every few seconds, and
/// stormdrive itself rescans controllers on its shelf scan.
pub const FRESH: std::time::Duration = std::time::Duration::from_secs(4);

/// A disk state the controller holds against the drive: Linux sees nothing
/// of it until it is set good (and JBOD, or the controller is HBA).
pub fn failed_state(state: &str) -> bool {
    matches!(state, "failed" | "unconfigured_bad" | "offline")
}

/// One node's answer, reduced. `prefix` is the node's component prefix
/// (`drive`, `drive:@<host>`) and `proxy` its proxy base.
pub fn reduce(body: &Value, node: &str, prefix: &str, proxy: &str) -> Vec<Value> {
    let list = body.get("controllers").and_then(Value::as_array).cloned().unwrap_or_default();
    list.iter()
        .map(|c| {
            let perccli = match c.get("perccli") {
                Some(p) if p.get("Ok").is_some() => json!({"ok": true, "path": p["Ok"]}),
                Some(p) if p.get("Err").is_some() => json!({"ok": false, "why": p["Err"]}),
                _ => json!({"ok": false, "why": "stormdrive did not say whether perccli is on the node"}),
            };
            let drives: Vec<Value> = c
                .get("drives")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .map(|d| {
                    let mut d = d.clone();
                    let id = d.get("drive").and_then(Value::as_str).map(str::to_string);
                    d["component"] = id.as_ref().map_or(Value::Null, |i| json!(format!("{prefix}:drive:{i}")));
                    let failed = failed_state(d.get("state").and_then(Value::as_str).unwrap_or(""));
                    d["failed"] = json!(failed);
                    d
                })
                .collect();
            json!({
                "node": node,
                "proxy": proxy,
                "key": c.get("key"),
                "scsi_host": c.get("scsi_host"),
                "pcie_addr": c.get("pcie_addr"),
                "driver": c.get("driver"),
                "model": c.get("model"),
                "serial": c.get("serial"),
                "firmware": c.get("firmware"),
                "virtual_disks": c.get("virtual_disks"),
                "jbod_enabled": c.get("jbod_enabled"),
                "mode": c.get("mode").cloned().unwrap_or(json!("unknown")),
                "mode_basis": c.get("mode_basis"),
                "pending_mode": c.get("pending_mode"),
                "error": c.get("error"),
                "perccli": perccli,
                "findings": c.get("findings").cloned().unwrap_or(json!([])),
                "drives": drives,
            })
        })
        .collect()
}

/// What a node that did not give controllers says: a 404 is a stormdrive
/// older than v0.43.0, which has none to give, not a failure.
pub fn unread(node: &str, error: &str) -> Value {
    if error.contains("404") {
        json!({"node": node, "ok": true, "controllers": 0, "predates": true,
               "note": "its stormdrive predates controller management (v0.43.0, stormdrive#132)"})
    } else {
        json!({"node": node, "ok": false, "error": error})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// forge1's PERC H730 as stormdrive v0.43.0 serialises it: RAID mode
    /// (inferred), no perccli, sdb–sdd failed by the controller.
    fn forge1() -> Value {
        json!({"controllers": [{
            "key": "0000:03:00.0", "scsi_host": "host0", "pcie_addr": "0000:03:00.0", "driver": "megaraid_sas",
            "model": "PERC H730 Mini", "serial": "5CF1234", "firmware": "25.5.9.0001", "virtual_disks": 0,
            "jbod_enabled": true, "mode": "raid", "mode_basis": "inferred: disks in states only RAID mode has (failed ×3)",
            "pending_mode": null,
            "drives": [
                {"device_id": 0, "seq": 1, "enclosure": 32, "slot": 0, "state": "jbod", "state_code": 64, "media_errors": 0,
                 "other_errors": 0, "predictive_failures": 0, "size_bytes": 480_103_981_056_u64, "drive": "a1", "name": "sda"},
                {"device_id": 1, "seq": 4, "enclosure": 32, "slot": 1, "state": "failed", "state_code": 17, "media_errors": 0,
                 "other_errors": 2, "predictive_failures": 0, "size_bytes": 0, "drive": "b2", "name": "sdb"},
                {"device_id": 7, "seq": 2, "enclosure": 32, "slot": 7, "state": "unconfigured_bad", "state_code": 1,
                 "media_errors": 0, "other_errors": 0, "predictive_failures": 0, "size_bytes": 0}
            ],
            "findings": [{"code": "raid_mode", "severity": "warning", "message": "controller 0000:03:00.0 (PERC H730 Mini) is in RAID mode (…); HBA mode is mandatory"}],
            "perccli": {"Err": "perccli is not on this node (stormcentral#1310)"}
        }], "findings": []})
    }

    #[test]
    fn a_controller_is_reshaped_with_its_node_and_proxy() {
        let r = reduce(&forge1(), "forge1", "drive:@forge1", "/api/plugins/drive/node/forge1/proxy");
        assert_eq!(r.len(), 1);
        let c = &r[0];
        assert_eq!(c["node"], "forge1");
        assert_eq!(c["proxy"], "/api/plugins/drive/node/forge1/proxy");
        assert_eq!(c["mode"], "raid");
        assert_eq!(c["perccli"], json!({"ok": false, "why": "perccli is not on this node (stormcentral#1310)"}));
        assert_eq!(c["findings"][0]["code"], "raid_mode");
    }

    #[test]
    fn each_disk_is_joined_to_its_drive_row_and_a_failed_one_said() {
        let r = reduce(&forge1(), "here", "drive", "/api/plugins/drive/proxy");
        let d = r[0]["drives"].as_array().unwrap();
        assert_eq!(d[0]["component"], "drive:drive:a1");
        assert_eq!(d[0]["failed"], false, "jbod is what Linux sees");
        assert_eq!(d[1]["component"], "drive:drive:b2");
        assert_eq!(d[1]["failed"], true);
        assert!(d[2]["component"].is_null(), "a disk with no inventory record has no row");
        assert_eq!(d[2]["failed"], true, "unconfigured bad");
        assert_eq!(d[2]["device_id"], 7, "kept, for the controller's own route");
    }

    #[test]
    fn perccli_found_is_its_path() {
        let mut b = forge1();
        b["controllers"][0]["perccli"] = json!({"Ok": "/usr/sbin/perccli64"});
        let r = reduce(&b, "here", "drive", "/p");
        assert_eq!(r[0]["perccli"], json!({"ok": true, "path": "/usr/sbin/perccli64"}));
    }

    #[test]
    fn an_older_stormdrive_has_none_and_is_not_an_error() {
        assert_eq!(unread("old", "stormdrive answered 404 Not Found")["predates"], true);
        let e = unread("gone", "connection refused");
        assert_eq!(e["ok"], false);
        assert_eq!(e["error"], "connection refused");
        assert!(reduce(&json!({}), "x", "drive", "/p").is_empty());
    }
}
