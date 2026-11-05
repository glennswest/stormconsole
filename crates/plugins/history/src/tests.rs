//! Fixtures here are what the writers write: the shapes are stormblock's
//! `/init` block, stormpump's `history.rs`/`crash.rs` and stormdrive's
//! `history.rs`/`assets.rs`. `deploy/verify-history.sh` runs the writers.

use std::path::Path;

use serde_json::json;
use tempfile::TempDir;

use crate::store::{plain_name, stamp, Miss, Store};

const BOOT_A: &str = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
const BOOT_B: &str = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";
const CMD: &str = "console=ttyS0 stormblock.volume=stormpump";

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn unix(s: &str) -> i64 {
    stamp(s).unwrap().timestamp()
}

fn store(d: &TempDir) -> Store {
    write(d.path(), "history/boots/.keep", "");
    Store::new(d.path()).with_boot_id(None)
}

/// stormblock's record of a boot.
fn flat(root: &Path, at: &str, release: &str, cmdline: &str, installed: bool) {
    let handover = if installed {
        json!({"slabs": ["/dev/sda"], "installed": {"version": release, "previous": "11.95", "carried": ["img-1"]}})
    } else {
        json!({"slabs": ["/dev/sda"]})
    };
    let rec = json!({"at": at, "release": release, "tag": "C2NR0Q2", "cmdline": cmdline,
        "local_disk": {"state": "taken", "drive": "/dev/sda", "controllers": [], "drives": []},
        "install_config": "none", "handover": handover});
    write(root, &format!("history/boots/{at}.json"), &rec.to_string());
    if installed {
        write(root, &format!("history/installs/{at}.json"), &rec.to_string());
    }
}

/// PID 1's timeline, the way it appends one.
fn timeline(root: &Path, id: &str, at: &str, cmdline: &str, rest: &[&str]) {
    let mut t = format!(
        "{}\n",
        json!({"t_ms": 900, "event": "boot", "boot_id": id, "release": "pump-release", "kernel": "6.17.1",
               "cmdline": cmdline, "hostname": "node1", "wall_unix": unix(at)})
    );
    t.push_str("{\"t_ms\":900,\"event\":\"pid1\",\"detail\":\"--init\"}\n{\"t_ms\":900,\"event\":\"phase\",\"name\":\"early\",\"ms\":12}\n");
    for l in rest {
        t.push_str(l);
        t.push('\n');
    }
    write(root, &format!("history/boots/{id}/stormpump.jsonl"), &t);
}

#[test]
fn a_node_with_no_volume_says_so() {
    let d = TempDir::new().unwrap();
    let s = Store::new(d.path().join("absent")).with_boot_id(None);
    assert!(!s.available());
    let st = s.status();
    assert!(!st.available && st.note.contains("no system-data on this node") && st.note.contains("diskless"), "{}", st.note);
    let l = s.boots(10);
    assert!(!l.available && l.items.is_empty() && l.note.contains("no system-data"));
    assert!(matches!(s.boot("x"), Err(Miss::NoSystemData(_))));
    assert!(matches!(s.drive("x", 1), Err(Miss::NoSystemData(_))));
}

#[test]
fn an_empty_mount_point_is_not_a_record() {
    // stormblock's mkdir, then a mount that failed.
    let d = TempDir::new().unwrap();
    let s = Store::new(d.path()).with_boot_id(None);
    assert!(!s.available());
    assert!(s.status().note.contains("not mounted"));
}

#[test]
fn a_boot_is_one_row_however_it_was_written() {
    let d = TempDir::new().unwrap();
    let r = d.path();
    let s = store(&d);
    // Joined by time and command line, whatever the quoting did to it.
    flat(r, "20261001T100000Z", "12.60", CMD, false);
    timeline(r, BOOT_A, "20261001T100030Z", &format!("{CMD}\n"), &[
        "{\"t_ms\":5000,\"event\":\"started\",\"name\":\"stormdrive\"}",
        "{\"t_ms\":6000,\"event\":\"refused\",\"name\":\"x\",\"detail\":\"no\"}",
        "{\"t_ms\":9000,\"event\":\"all-running\",\"detail\":\"2 service(s)\"}",
        "{\"t_ms\":99000,\"event\":\"shutdown\",\"detail\":\"reboot\"}",
    ]);
    // The newest boot installed; stormblock wrote it, and PID 1 never ran.
    flat(r, "20261002T100000Z", "12.61", CMD, true);

    let l = s.boots(10);
    assert_eq!(l.total, 2);
    let (new, old) = (&l.items[0], &l.items[1]);
    assert_eq!(new.id, "20261002T100000Z");
    assert_eq!((new.end, new.installed.as_deref(), new.boot_id.as_deref()), ("no timeline", Some("12.61"), None));
    assert_eq!(old.id, BOOT_A);
    assert_eq!(old.joined, Some("time"));
    assert_eq!(old.release.as_deref(), Some("12.60"), "stormblock's release first");
    assert_eq!(old.kernel.as_deref(), Some("6.17.1"));
    assert_eq!(old.disk.as_deref(), Some("taken"));
    assert_eq!((old.end, old.end_detail.as_deref()), ("shut down", Some("reboot")));
    assert_eq!((old.all_running_ms, old.started, old.refused, old.pid1_starts), (Some(9000), 1, 1, 1));

    // Either name reaches the whole boot, so an install links to its boot.
    let by_flat = s.boot("20261001T100000Z").unwrap();
    assert_eq!(by_flat.row.id, BOOT_A);
    assert!(by_flat.stormblock.is_some());
    assert_eq!(by_flat.timeline.len(), 7);
    assert_eq!(s.boot(BOOT_A).unwrap().cmdline.as_deref().map(str::trim), Some(CMD));
}

#[test]
fn another_boots_record_is_not_taken() {
    let d = TempDir::new().unwrap();
    let r = d.path();
    let s = store(&d);
    // Same time, another command line.
    flat(r, "20261001T100000Z", "12.60", "console=tty0", false);
    timeline(r, BOOT_A, "20261001T100010Z", CMD, &[]);
    // A boot that never reached PID 1, an hour before the next one.
    flat(r, "20261002T090000Z", "12.61", CMD, false);
    flat(r, "20261002T100000Z", "12.61", CMD, false);
    timeline(r, BOOT_B, "20261002T100010Z", CMD, &[]);
    let l = s.boots(10);
    assert_eq!(l.total, 4, "{:?}", l.items.iter().map(|b| &b.id).collect::<Vec<_>>());
    assert_eq!(l.items[0].id, BOOT_B);
    assert_eq!(l.items[0].stormblock_file.as_deref(), Some("20261002T100000Z"));
    assert_eq!(l.items[1].id, "20261002T090000Z");
    assert!(l.items.iter().find(|b| b.id == BOOT_A).unwrap().stormblock_file.is_none());
}

#[test]
fn a_writer_naming_the_boot_is_taken_at_its_word() {
    let d = TempDir::new().unwrap();
    let r = d.path();
    let s = store(&d);
    timeline(r, BOOT_A, "20261001T100000Z", CMD, &[]);
    // Three days off and another command line, but it says which boot.
    write(r, "history/boots/20261004T100000Z.json", &json!({"at": "20261004T100000Z", "boot_id": BOOT_A, "cmdline": "other", "release": "r"}).to_string());
    // Where the issue says stormblock's record goes.
    timeline(r, BOOT_B, "20261002T100000Z", CMD, &[]);
    write(r, &format!("history/boots/{BOOT_B}/stormblock.json"), &json!({"release": "named"}).to_string());
    let l = s.boots(10);
    assert_eq!(l.total, 2);
    assert!(l.items.iter().all(|b| b.joined == Some("named")));
    assert_eq!(l.items.iter().find(|b| b.id == BOOT_B).unwrap().release.as_deref(), Some("named"));
}

#[test]
fn how_a_boot_ended() {
    let d = TempDir::new().unwrap();
    let r = d.path();
    timeline(r, BOOT_A, "20261001T100000Z", CMD, &["{\"t_ms\":7000,\"event\":\"crashed\",\"name\":\"d\",\"detail\":\"Kernel panic - not syncing\"}"]);
    timeline(r, BOOT_B, "20261002T100000Z", CMD, &[]);
    timeline(r, "cccc", "20261003T100000Z", CMD, &[]);
    write(r, "history/boots/.keep", "");
    let s = Store::new(r).with_boot_id(Some("cccc"));
    let l = s.boots(10);
    let end = |id: &str| l.items.iter().find(|b| b.id == id).unwrap();
    assert_eq!((end(BOOT_A).end, end(BOOT_A).end_detail.as_deref()), ("crashed", Some("Kernel panic - not syncing")));
    assert_eq!(end(BOOT_B).end, "no clean end");
    assert!(end("cccc").current);
    assert_eq!(end("cccc").end, "running");
    // Newest first, and a limit keeps the total.
    let two = s.boots(2);
    assert_eq!((two.total, two.items.len(), two.items[0].id.as_str()), (3, 2, "cccc"));
}

#[test]
fn a_torn_last_line_costs_nothing() {
    let d = TempDir::new().unwrap();
    let r = d.path();
    let s = store(&d);
    timeline(r, BOOT_A, "20261001T100000Z", CMD, &["{\"t_ms\":5000,\"event\":\"sta"]);
    write(r, "history/drives/wwn-1/2026-10.jsonl", "{\"at\":\"a\",\"status\":\"ok\"}\n{\"at\":");
    assert_eq!(s.boot(BOOT_A).unwrap().timeline.len(), 3);
    assert_eq!(s.drive("wwn-1", 10).unwrap().records.len(), 1);
    assert_eq!(s.drives(10).items[0].status.as_deref(), Some("ok"));
}

#[test]
fn kernel_logs_whole_and_tailed() {
    let d = TempDir::new().unwrap();
    let r = d.path();
    let s = store(&d);
    timeline(r, BOOT_A, "20261001T100000Z", CMD, &[]);
    flat(r, "20261003T100000Z", "12.61", CMD, false);
    let log: String = (0..50).map(|i| format!("{i} [    0.{i:06}] <6> line {i}\n")).collect();
    write(r, &format!("logs/kernel/{BOOT_A}.log"), &log);
    assert_eq!(s.kernel(BOOT_A, None).unwrap(), log);
    assert_eq!(s.kernel(BOOT_A, Some(2)).unwrap(), "48 [    0.000048] <6> line 48\n49 [    0.000049] <6> line 49\n");
    assert_eq!(s.boots(10).items[1].kernel_log_bytes, Some(log.len() as u64));
    // stormblock's boot alone has none, said as such.
    assert!(matches!(s.kernel("20261003T100000Z", None), Err(Miss::NotFound(m)) if m.contains("no kernel log")));
    assert!(matches!(s.kernel("nope", None), Err(Miss::NotFound(_))));
}

#[test]
fn nothing_reaches_outside_the_record_it_names() {
    let d = TempDir::new().unwrap();
    let r = d.path().join("sd");
    write(&r, "history/installs/20261001T100000Z.json", "{\"release\":\"r\"}");
    write(&r, "logs/kernel/crash/d/manifest.json", "{}");
    write(d.path(), "secret.json", "{\"secret\":true}");
    let s = Store::new(&r).with_boot_id(None);
    for bad in ["..", "../..", "a/b", "..\\x", ".hidden", "", "a\0b"] {
        assert!(!plain_name(bad), "{bad:?}");
        assert!(matches!(s.install(bad), Err(Miss::BadName(_))), "{bad:?}");
        assert!(matches!(s.crash_file("d", bad), Err(Miss::BadName(_))), "{bad:?}");
        assert!(matches!(s.drive(bad, 1), Err(Miss::BadName(_))), "{bad:?}");
    }
    assert!(matches!(s.install("../../secret"), Err(Miss::BadName(_))));
    // A link in the volume is not followed either.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(d.path().join("secret.json"), r.join("history/installs/link.json")).unwrap();
        assert!(matches!(s.install("link"), Err(Miss::NotFound(_))));
    }
    assert_eq!(s.install("20261001T100000Z").unwrap()["release"], "r");
}

#[test]
fn installs_say_what_they_put_down() {
    let d = TempDir::new().unwrap();
    let r = d.path();
    let s = store(&d);
    flat(r, "20261001T100000Z", "12.60", CMD, true);
    flat(r, "20261002T100000Z", "12.61", CMD, true);
    let l = s.installs(10);
    assert_eq!(l.total, 2);
    let i = &l.items[0];
    assert_eq!(i.id, "20261002T100000Z");
    assert_eq!((i.version.as_deref(), i.previous.as_deref()), (Some("12.61"), Some("11.95")));
    assert_eq!((i.disk.as_deref(), i.drive.as_deref()), (Some("taken"), Some("/dev/sda")));
    assert_eq!(i.carried, Some(json!(["img-1"])));
    assert_eq!(s.install(&i.id).unwrap()["tag"], "C2NR0Q2");
    assert!(matches!(s.install("nope"), Err(Miss::NotFound(_))));
    assert_eq!(s.installs(1).items.len(), 1);
}

#[test]
fn a_drives_history_across_months() {
    let d = TempDir::new().unwrap();
    let r = d.path();
    let s = store(&d);
    let k = "wwn-naa.5000c500a1b2c3d4";
    let rec = |at: &str, media: u64, findings: serde_json::Value| {
        json!({"at": at, "unix": 1, "boot_id": BOOT_A, "node": "node1", "why": "changed",
               "drive": {"id": "d1", "serial": "ZC1", "model": "ST", "firmware": "1", "kind": "sas", "path": "/dev/sda", "capacity_bytes": 4096},
               "status": "healthy", "temperature_c": 31, "counters": {"media_errors": media}, "findings": findings})
        .to_string()
    };
    let finding = json!([{"counter": "media_errors", "change": "grew", "from": 32, "to": 33, "since": "x", "other_boot": true}]);
    write(r, &format!("history/drives/{k}/2026-09.jsonl"), &format!("{}\n{}\n", rec("2026-09-01T00:00:00Z", 30, json!([])), rec("2026-09-02T00:00:00Z", 32, json!([]))));
    write(r, &format!("history/drives/{k}/2026-10.jsonl"), &format!("{}\n", rec("2026-10-01T00:00:00Z", 33, finding.clone())));
    write(r, &format!("history/drives/{k}/log/2026-10.jsonl"), &format!("{}\n", json!({"at": "2026-10-01T00:00:00Z", "kind": "format", "severity": "info", "message": "format done"})));
    write(r, &format!("history/drives/{k}/summary.json"), &json!({"key": k, "entries": 1, "last_format": {"op": "format", "result": "done", "at": "x"}}).to_string());

    let l = s.drives(10);
    assert_eq!(l.total, 1);
    let row = &l.items[0];
    assert_eq!((row.key.as_str(), row.months), (k, 2));
    assert_eq!(row.drive["serial"], "ZC1");
    assert_eq!(row.last_at.as_deref(), Some("2026-10-01T00:00:00Z"));
    assert_eq!(row.findings, finding);
    assert_eq!(row.summary["last_format"]["result"], "done");

    let one = s.drive(k, 10).unwrap();
    let media: Vec<u64> = one.records.iter().map(|r| r["counters"]["media_errors"].as_u64().unwrap()).collect();
    assert_eq!(media, [33, 32, 30], "newest first, across months");
    assert_eq!(s.drive(k, 2).unwrap().records.len(), 2);
    assert_eq!(one.log[0]["message"], "format done");
    assert!(matches!(s.drive("wwn-none", 1), Err(Miss::NotFound(_))));
}

#[test]
fn hardware_by_boot_and_crashes() {
    let d = TempDir::new().unwrap();
    let r = d.path();
    let s = store(&d);
    timeline(r, BOOT_B, "20261002T100000Z", CMD, &[]);
    let at = |id: &str, ts: &str, changes: serde_json::Value| {
        write(r, &format!("assets/{ts}-{id}.json"), &json!({"at": ts, "updated_at": ts, "boot_id": id, "node": "node1", "kernel": "6", "stormdrive": "0.39",
            "memory_total_bytes": 1024, "items": {"system": {"vendor": "Dell"}, "cpu/0": {"cores": 8}}, "changes": changes}).to_string());
    };
    at(BOOT_A, "20261001T100000Z", json!([]));
    at(BOOT_B, "20261002T100000Z", json!([{"item": "dimm/A1", "change": "removed"}]));
    let l = s.assets(10);
    assert_eq!(l.total, 2);
    assert_eq!(l.items[0].id, format!("20261002T100000Z-{BOOT_B}"));
    assert_eq!((l.items[0].items, l.items[0].boot_id.as_deref()), (2, Some(BOOT_B)));
    assert_eq!(l.items[0].changes[0]["item"], "dimm/A1");
    assert_eq!(s.asset(&l.items[1].id).unwrap()["items"]["cpu/0"]["cores"], 8);
    // A boot links to its hardware record.
    assert_eq!(s.boot(BOOT_B).unwrap().assets.as_deref(), Some(l.items[0].id.as_str()));

    write(r, &format!("logs/kernel/crash/{BOOT_B}/manifest.json"), &json!({"boot_id": BOOT_B, "panic": "Kernel panic", "record_unix": 5}).to_string());
    write(r, &format!("logs/kernel/crash/{BOOT_B}/dmesg.txt"), "panic text\n");
    write(r, "logs/kernel/crash/at-99/manifest.json", &json!({"panic": "other", "record_unix": 99}).to_string());
    let c = s.crashes();
    assert_eq!(c.total, 2);
    assert_eq!(c.items[0].dir, "at-99", "newest first");
    assert_eq!(c.items[1].files, ["dmesg.txt", "manifest.json"]);
    assert_eq!(s.crash_file(BOOT_B, "dmesg.txt").unwrap(), "panic text\n");
    assert_eq!(s.boot(BOOT_B).unwrap().crash.unwrap().manifest["panic"], "Kernel panic");
    assert!(matches!(s.crash_file(BOOT_B, "nope"), Err(Miss::NotFound(_))));
}

#[test]
fn what_is_not_here_yet_is_said() {
    // The volume is there, stormdrive has not been bound in yet.
    let d = TempDir::new().unwrap();
    let s = store(&d);
    assert!(s.drives(10).note.contains("stormdrive"));
    assert!(s.assets(10).note.contains("stormdrive"));
    assert!(s.installs(10).available);
    let st = s.status();
    assert!(st.available && st.note.is_empty());
    // A record that will not parse is named, not dropped silently.
    write(d.path(), "assets/20261001T100000Z-x.json", "{not json");
    let a = s.assets(10);
    assert!(a.unreadable[0].contains("20261001T100000Z-x.json"), "{:?}", a.unreadable);
    assert!(matches!(s.asset("20261001T100000Z-x"), Err(Miss::Unreadable(_))));
}
