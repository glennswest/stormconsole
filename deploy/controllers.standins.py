#!/usr/bin/env python3
"""Stand-ins for deploy/verify-controllers.sh (#325): two stormdrives.

HERE is forge1 as stormdrive v0.43.0 sees it (src/raidctl.rs, src/api/mod.rs, src/components.rs): a PERC H730
Mini at 0000:03:00.0 in RAID mode (inferred, no virtual disks), no perccli on the node, sda the system drive as
JBOD, sdb-sdd Failed by the controller (0 bytes offered, no block device), a disk in slot 7 the inventory has no
record of (unconfigured bad) and a hot spare in slot 8. /api/v1/controllers, /api/v1/drives, the feed with the
HBA card's Switch to HBA and each failed drive's Set good / Set JBOD, and the verbs:
  POST /api/v1/controllers/{key}/personality {"mode"}   501 tool_missing without perccli; 409 with virtual disks
  POST /api/v1/drives/{id}/controller-state {"state","force"}, .../controller-state/{state} (force implied)
  POST /api/v1/controllers/{key}/drives/{device}/state {"state","force"}
refused 409 as raidctl::plan_state refuses (the system drive, a fleet or in-use drive, Failed without force, a
member or spare). A disk set JBOD is seen by Linux at once (a block device, its size).

OTHER is a stormdrive older than v0.43.0: /api/v1/controllers is a 404.

/_perccli (POST) puts perccli on the node; every write is recorded with its bearer at /_writes."""
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE, OTHER = int(sys.argv[1]), int(sys.argv[2])
KEY = "0000:03:00.0"
TB = 10 * 10**12
LOCK = threading.Lock()
WRITES = {HERE: [], OTHER: []}
PERCCLI = {"Err": "perccli is not on this node: none at /opt/perccli, /usr/sbin or on PATH (stormcentral#1310)"}
CTL = {"key": KEY, "scsi_host": "host0", "pcie_addr": KEY, "driver": "megaraid_sas", "model": "PERC H730 Mini",
       "serial": "5CF1234", "firmware": "25.5.9.0001", "virtual_disks": 0, "jbod_enabled": True, "mode": "raid",
       "mode_basis": "inferred: disks in states only RAID mode has (failed ×3, unconfigured_bad ×1, hot_spare ×1)",
       "pending_mode": None, "error": None}
NAMES = {0: "unconfigured_good", 1: "unconfigured_bad", 2: "hot_spare", 17: "failed", 64: "jbod"}


def pd(dev, slot, code, drive=None, name=None, size=0, other=0):
    return {"device_id": dev, "seq": 3, "enclosure": 32, "slot": slot, "state": NAMES[code], "state_code": code,
            "vendor": "SEAGATE", "model": "ST10000NM0226", "sas_address": f"5000c500{dev:08x}", "media_errors": 0,
            "other_errors": other, "predictive_failures": 0, "size_bytes": size, "drive": drive, "name": name}


PDS = [pd(0, 0, 64, "d1", "sda", 480 * 10**9), pd(1, 1, 17, "d2", "sdb", other=2), pd(2, 2, 17, "d3", "sdc"),
       pd(3, 3, 17, "d4", "sdd"), pd(7, 7, 1), pd(8, 8, 2)]


def drive(id, name, serial, bay, **kw):
    d = {"id": id, "name": name, "serial": serial, "vendor": "SEAGATE", "model": "ST10000NM0226", "firmware": "TT55",
         "wwid": f"naa.{id}", "paths": [f"/dev/{name}"], "capacity_bytes": TB, "block_size": 512, "kind": "sas_hdd",
         "membership": "out", "activity": "idle", "designation": "none", "in_use_by": None, "health": {"status": "healthy"},
         "location": {"bay": bay, "controller": {"pcie_addr": KEY}}}
    d.update(kw)
    return d


DRIVES = {HERE: {d["id"]: d for d in [
    drive("d1", "sda", "SYS001", 0, owner="stormblock", in_use_by="stormblock (system slab partitions 2, 3)", capacity_bytes=480 * 10**9,
          slab_parts=[{"partition": 2, "name": "stormblock", "role": "system"}]),
    drive("d2", "sdb", "NET002", 1, paths=[], capacity_bytes=0, activity="missing"),
    drive("d3", "sdc", "NET003", 2, paths=[], capacity_bytes=0, activity="missing"),
    drive("d4", "sdd", "NET004", 3, paths=[], capacity_bytes=0, activity="missing"),
]}, OTHER: {"b1": drive("b1", "sdx", "BEE001", 1, location={"bay": 1})}}


def cstate(d):
    """The drive's controller_state, as monitor.rs keeps it."""
    for p in PDS:
        if p["drive"] == d["id"]:
            return {"controller": KEY, "device_id": p["device_id"], "enclosure": p["enclosure"], "slot": p["slot"],
                    "state": p["state"], "state_code": p["state_code"], "size_bytes": p["size_bytes"]}
    return None


def findings():
    out = []
    if CTL["mode"] == "raid" and CTL["pending_mode"] != "hba":
        out.append({"code": "raid_mode", "severity": "warning",
                    "message": f"controller {KEY} (PERC H730 Mini) is in RAID mode ({CTL['mode_basis']}); HBA mode is mandatory"})
    if CTL["pending_mode"]:
        out.append({"code": "reboot_pending", "severity": "info",
                    "message": f"controller {KEY}: personality {CTL['pending_mode'].upper()} asked for; a reboot applies it"})
    for p in PDS:
        if p["state_code"] in (17, 1, 16):
            out.append({"code": "drive_failed", "severity": "warning",
                        "message": f"{KEY} slot {p['slot']}: the controller has it {p['state']} ({p['size_bytes']} bytes offered)" + (f", was {p['name']}" if p["name"] else "")})
    return out


def report():
    return dict(CTL, drives=PDS, findings=findings(), perccli=PERCCLI)


def card(d):
    base = f"/api/v1/drives/{d['id']}"
    act = lambda id, label, path, enabled=True, danger=False: {"id": id, "label": label, "method": "POST", "path": f"{base}/{path}", "enabled": enabled, "danger": danger}
    c = cstate(d)
    actions = [act("locate-on", "Locate", "locate/on")]
    metrics = [{"label": "serial", "value": d["serial"]}, {"label": "bay", "value": str(d["location"]["bay"])},
               {"label": "capacity", "value": f"{d['capacity_bytes'] / 10**12:.1f} TB"}, {"label": "hba", "value": KEY if c else ""}]
    detail = ["out of fleet"]
    health = "ok"
    if d["paths"]:
        metrics.append({"label": "dev", "value": d["paths"][0]})
    if c:
        bad = c["state_code"] in (17, 1, 16)
        metrics.append({"label": "controller", "value": c["state"].replace("_", " "), "tone": "error" if bad else "muted"})
        if c["state_code"] != 64:
            detail.append(f"{c['state'].replace('_', ' ')} (controller)")
            free = not d["in_use_by"] and d["membership"] == "out"
            from_bad = c["state_code"] in (17, 1, 0)
            actions.append(act("controller-good", "Set good", "controller-state/good", free and from_bad, True))
            actions.append(act("controller-jbod", "Set JBOD", "controller-state/jbod", free and from_bad, True))
        if bad:
            health = "error"
    return {"id": f"drive:{d['id']}", "kind": "drive", "label": f"{d['name']} · {d['model']}", "health": health,
            "detail": " · ".join(detail), "metrics": metrics, "actions": actions, "relations": []}


def hba_card():
    can = CTL["mode"] != "hba" and CTL["pending_mode"] != "hba" and "Ok" in PERCCLI and CTL["virtual_disks"] == 0
    metrics = [{"label": "drives", "value": "4"}, {"label": "mode", "value": CTL["mode"].upper(), "tone": "warn"},
               {"label": "firmware", "value": CTL["firmware"]}, {"label": "serial", "value": CTL["serial"]}]
    if CTL["pending_mode"]:
        metrics.insert(2, {"label": "after reboot", "value": CTL["pending_mode"].upper(), "tone": "accent"})
    return {"id": f"hba:{KEY}", "kind": "hba", "label": f"PERC H730 Mini · {KEY}", "health": "warn" if any(f["severity"] == "warning" for f in findings()) else "ok",
            "detail": f"megaraid_sas · 4 drives · {CTL['mode'].upper()} mode", "metrics": metrics,
            "actions": [{"id": "personality-hba", "label": "Switch to HBA", "method": "POST", "path": f"/api/v1/controllers/{KEY}/personality/hba", "enabled": can, "danger": True}],
            "relations": []}


def set_state(p, want, force):
    """raidctl::plan_state, then apply: (code, body)."""
    d = DRIVES[HERE].get(p["drive"]) if p["drive"] else None
    if d:
        if d.get("owner") == "stormblock" or d["in_use_by"] and "system" in d["in_use_by"]:
            return 409, {"error": f"{d['name']}: {d['name']} holds this node's system slab; its controller state is not changed from here", "code": "conflict"}
        if d["membership"] == "fleet":
            return 409, {"error": f"{d['name']}: in the fleet; leave it first", "code": "conflict"}
    code = p["state_code"]
    if code == 17 and not force:
        return 409, {"error": f"the controller has device {p['device_id']} failed: set good needs force (perccli's `set good force`)", "code": "conflict"}
    if code not in (0, 1, 17, 64):
        return 409, {"error": f"device {p['device_id']} is {p['state']} (a member or spare of the controller's configuration): not changed from here", "code": "conflict"}
    sent = [] if code == 0 or (code == 64 and want == "jbod") else ["unconfigured_good"]
    if want == "jbod" and code != 64:
        sent.append("jbod")
    final = 64 if want == "jbod" else 0
    p["state_code"], p["state"] = final, NAMES[final]
    if final == 64:
        p["size_bytes"] = TB
        if d:
            d.update(paths=[f"/dev/{d['name']}"], capacity_bytes=TB, activity="idle")
    return 200, {"controller": KEY, "device_id": p["device_id"], "drive": p["drive"], "sent": sent, "state": p["state"], "size_bytes": p["size_bytes"]}


def handler(port):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def send(self, code, body):
            data = json.dumps(body).encode()
            self.send_response(code)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def rest(self, d):
            out = {k: v for k, v in d.items() if v is not None}
            c = cstate(d) if port == HERE else None
            if c:
                out["controller_state"] = c
            return out

        def do_GET(self):
            path = self.path.split("?")[0]
            with LOCK:
                if path == "/_writes":
                    return self.send(200, WRITES[port])
                if path == "/api/v1/components":
                    cards = [card(d) for d in DRIVES[port].values()]
                    return self.send(200, cards + ([hba_card()] if port == HERE else []))
                if path == "/api/v1/drives":
                    return self.send(200, {"drives": [self.rest(d) for d in DRIVES[port].values()]})
                if path == "/api/v1/controllers" and port == HERE:
                    r = report()
                    return self.send(200, {"controllers": [r], "findings": r["findings"]})
            return self.send(404, {"error": "no such path"})

        def do_POST(self):
            global PERCCLI
            n = int(self.headers.get("content-length") or 0)
            raw = self.rfile.read(n)
            try:
                body = json.loads(raw) if raw else {}
            except ValueError:
                body = {}
            path = self.path.split("?")[0]
            with LOCK:
                if path == "/_perccli":
                    PERCCLI = {"Ok": "/opt/perccli/perccli64"}
                    return self.send(200, {})
                a = self.headers.get("authorization") or ""
                WRITES[port].append({"method": "POST", "path": self.path, "bearer": a[7:] if a.startswith("Bearer ") else None, "body": body})
                if port != HERE:
                    return self.send(404, {"error": "no such path"})
                s = path.strip("/").split("/")
                # api v1 controllers KEY personality [mode]
                if s[:3] == ["api", "v1", "controllers"] and len(s) >= 5 and s[4] == "personality":
                    if s[3] != KEY:
                        return self.send(404, {"error": f"controller {s[3]!r}", "code": "not_found"})
                    mode = (s[5] if len(s) > 5 else body.get("mode", "")).upper()
                    if "HBA" not in mode and "JBOD" not in mode:
                        return self.send(400, {"error": f"mode {mode!r}: HBA or JBOD", "code": "bad_request"})
                    if "Err" in PERCCLI:
                        return self.send(501, {"error": PERCCLI["Err"], "code": "tool_missing"})
                    if CTL["virtual_disks"]:
                        return self.send(409, {"error": f"controller {KEY} has {CTL['virtual_disks']} virtual disk(s): they are not deleted from here", "code": "conflict"})
                    CTL["pending_mode"] = "hba"
                    return self.send(200, {"controller": KEY, "mode": "hba", "reboot_required": True, "perccli": "Personality change to HBA successful; reboot required"})
                if s[:3] == ["api", "v1", "drives"] and len(s) >= 5 and s[4] == "controller-state":
                    want = s[5] if len(s) > 5 else body.get("state")
                    force = True if len(s) > 5 else bool(body.get("force"))
                    p = next((p for p in PDS if p["drive"] == s[3]), None)
                    if not p:
                        return self.send(409, {"error": f"drive {s[3]}: no RAID controller lists it", "code": "conflict"})
                    code, out = set_state(p, want, force)
                    return self.send(code, out)
                if s[:3] == ["api", "v1", "controllers"] and len(s) == 7 and s[4] == "drives" and s[6] == "state":
                    p = next((p for p in PDS if str(p["device_id"]) == s[5]), None)
                    if s[3] != KEY or not p:
                        return self.send(404, {"error": "no such device", "code": "not_found"})
                    code, out = set_state(p, body.get("state"), bool(body.get("force")))
                    return self.send(code, out)
            return self.send(404, {"error": "no such path"})

    return H


for port in (HERE, OTHER):
    srv = ThreadingHTTPServer(("127.0.0.1", port), handler(port))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
threading.Event().wait()
