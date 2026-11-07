#!/usr/bin/env python3
"""Stand-ins for deploy/verify-drive-replace.sh (#317): two stormdrives and this node's engine.

stormdrive, in v0.40's shapes (src/api/mod.rs, src/firmware.rs, src/drive.rs), this node laid out as forge1 is
(sda the system drive; sdb-sdd free) and with what forge1 does not have yet: a fleet drive (sde), an LVM disk
(sdf, another model), two members of RAID set ds0-a (sdk, sdm) and a blank drive at 4096 + PI 1 (sdl):
/api/v1/drives (firmware, firmware_run, firmware_update, drain, membership), the Drive objects, the feed,
the image store (GET, PUT), POST /api/v1/firmware (all-or-nothing, 409 {error}), GET /api/v1/firmware,
fleet join, and drain?leave=true. An update and a drain run on a clock, RUN seconds.

The engine, in stormblock's shapes (mgmt/api/arrays.rs, shelves.rs, drives.rs): RAID set ds0-a on shelf ds0,
sdk and sdm its members, sdl and sdc blank drives it has open; POST .../members/{slot}/replace {drive_uuid}
copies the slot on the clock (status.rebuild with the slot), then the new drive takes it.

Every write is recorded with its bearer at /_writes."""
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE, OTHER, ENGINE, RUN = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3]), float(sys.argv[4])
MODEL = "H7210A521SUN010T"
SUPPORTS = {"protect": True, "pi_types": [1, 2], "block_lengths": [], "physical_block_size": 4096}


def drive(id, name, serial, bay, **kw):
    d = {"id": id, "name": name, "serial": serial, "vendor": "HGST", "model": MODEL, "firmware": "A3Z0", "wwid": f"naa.{id}",
         "paths": [f"/dev/{name}"], "capacity_bytes": 10 * 10**12, "block_size": 512, "physical_block_size": 4096, "kind": "sas_hdd",
         "membership": "out", "activity": "idle", "designation": "none", "in_use_by": None, "health": {"status": "healthy"},
         "location": {"bay": bay}, "prot": 1, "supports": SUPPORTS, "drain": None, "firmware_update": None}
    d.update(kw)
    return d


DRIVES = {
    HERE: {d["id"]: d for d in [
        drive("d1", "sda", "SYS001", 0, owner="stormblock", in_use_by="stormblock (system slab partitions 2, 3)",
              slab_parts=[{"partition": 2, "name": "stormblock", "role": "system"}, {"partition": 3, "name": "stormblock-data", "role": "data"}],
              contents="vfat in partition 1 'EFI', stormblock slab in partition 2 'stormblock', stormblock slab in partition 3 'stormblock-data'"),
        drive("d2", "sdb", "NET002", 3),
        drive("d3", "sdc", "NET003", 4),
        drive("d4", "sdd", "NET004", 7, prot=0),
        drive("d5", "sde", "FLT005", 5, membership="fleet", block_size=4096, usage={"slabs": [{"id": "s1"}], "used": 2 * 10**12}),
        drive("d6", "sdf", "LVM006", 6, in_use_by="LVM2_member", model="ST4000NM0035", vendor="SEAGATE", firmware="TN05"),
        drive("d11", "sdk", "RD0011", 11, block_size=4096, in_use_by="stormraid (a RAID set member)"),
        drive("d13", "sdm", "RD0013", 13, block_size=4096, in_use_by="stormraid (a RAID set member)"),
        drive("d12", "sdl", "NEW012", 12, block_size=4096),
    ]},
    OTHER: {"b1": drive("b1", "sdx", "BEE001", 1)},
}
IMAGES = {HERE: {"loose-A3Z1.bin": b"x" * 4096}, OTHER: {}}
FW = {}      # drive id -> run
DRAINS = {}  # drive id -> start
WRITES = {HERE: [], OTHER: [], ENGINE: []}
LOCK = threading.Lock()
T0 = time.time()


def st(t):
    return {"secs_since_epoch": int(t), "nanos_since_epoch": 0}


def fw_run(port, id):
    r = FW.get(id)
    if not r:
        return None
    now = time.time()
    frac = min(1.0, (now - r["start"]) / RUN)
    run = {"drive": id, "name": DRIVES[port][id]["name"], "image": r["image"], "from_version": r["from"], "to_version": None,
           "state": "running", "phase": "download" if frac < 0.6 else "activate" if frac < 0.8 else "verify",
           "bytes_done": int(r["size"] * min(frac / 0.6, 1)), "bytes_total": r["size"], "started": st(r["start"]),
           "finished": None, "error": None, "reset_required": False, "mode": "deferred"}
    if frac >= 1:
        if r["fail"]:
            run.update(state="failed", phase="failed", finished=st(r["start"] + RUN), error=r["fail"])
        else:
            run.update(state="done", phase="done", to_version=r["to"], finished=st(r["start"] + RUN), bytes_done=r["size"])
    return run


def settle(port):
    """What a finished run or drain leaves on the drive."""
    for id, d in DRIVES[port].items():
        run = fw_run(port, id)
        if run:
            d["activity"] = "updating_firmware" if run["state"] == "running" else "idle"
            if run["state"] == "done":
                d["firmware"] = run["to_version"]
            d["firmware_update"] = {"image": run["image"], "from_version": run["from_version"], "to_version": run["to_version"],
                                    "state": "running" if run["state"] == "running" else run["state"], "started": run["started"],
                                    "finished": run["finished"], "error": run["error"], "reset_required": False}
        if id in DRAINS:
            frac = min(1.0, (time.time() - DRAINS[id]) / RUN)
            total = 100
            moved = int(total * frac)
            d["drain"] = {"state": "empty" if frac >= 1 else "running", "moved": moved, "failed": 0, "remaining": total - moved,
                          "errors": [], "reason": "operator", "then_leave": True}
            d["activity"] = "idle" if frac >= 1 else "draining"
            if frac >= 1:
                d["membership"] = "out"
                d["usage"] = None


def card(d):
    base = f"/api/v1/drives/{d['id']}"
    idle = d["activity"] == "idle"
    act = lambda id, label, path, enabled=True, danger=False: {"id": id, "label": label, "method": "POST", "path": f"{base}/{path}", "enabled": enabled, "danger": danger}
    fleet = d["membership"] == "fleet"
    actions = [act("locate-on", "Locate", "locate/on"),
               act("fleet-leave", "Leave fleet", "fleet/leave", idle, True) if fleet else act("fleet-join", "Join fleet", "fleet/join", idle and not d["in_use_by"]),
               act("test-smoke", "Smoke test", "test/smoke", idle)]
    metrics = [{"label": "serial", "value": d["serial"]}, {"label": "bay", "value": str(d["location"]["bay"])},
               {"label": "capacity", "value": "10.0 TB"}, {"label": "dev", "value": f"/dev/{d['name']}"},
               {"label": "sector", "value": str(d["block_size"])}, {"label": "kind", "value": "sas_hdd"}, {"label": "smart", "value": "ok"}]
    return {"id": f"drive:{d['id']}", "kind": "drive", "label": f"{d['name']} · {d['model']}", "health": "ok",
            "detail": f"{'in fleet' if fleet else 'out of fleet'} · {d['activity']}", "metrics": metrics, "actions": actions, "relations": []}


def fw_blocker(d):
    if d["activity"] == "updating_firmware":
        return "a firmware update is already running"
    if d["activity"] != "idle":
        return f"activity is {d['activity']}"
    return None


# --- the engine --------------------------------------------------------------
SET = {"id": "a1", "name": "ds0-a", "pool": "ds0", "level": "raid1", "capacity_human": "10 TB", "stripe_human": "64 KiB", "events": 3,
       "members": [
           {"index": 0, "uuid": "m0", "state": "active", "device_path": "/dev/sdk", "labels": "shelf=ds0/bay=11",
            "drive": {"uuid": "u-sdk", "serial": "RD0011", "model": MODEL, "path": "/dev/sdk"}},
           {"index": 1, "uuid": "m1", "state": "active", "device_path": "/dev/sdm", "labels": "shelf=ds0/bay=13",
            "drive": {"uuid": "u-sdm", "serial": "RD0013", "model": MODEL, "path": "/dev/sdm"}}],
       "volumes": [{"id": "v1", "name": "someones-volume"}]}
FREE = [{"uuid": "u-sdl", "path": "/dev/sdl", "serial": "NEW012", "capacity_human": "10 TB", "offer": {"state": "blank"}},
        {"uuid": "u-sdc", "path": "/dev/sdc", "serial": "NET003", "capacity_human": "10 TB", "offer": {"state": "blank"}}]
REPLACE = {}  # slot -> (start, drive)


def engine_set():
    s = json.loads(json.dumps(SET))
    s["status"] = {"state": "clean", "failed": 0, "rebuilding": 0, "tolerated": 1}
    for slot, (start, d) in list(REPLACE.items()):
        frac = (time.time() - start) / RUN
        if frac < 1:
            s["status"]["rebuild"] = {"slots": [slot], "done_bytes": int(frac * 100), "total_bytes": 100, "percent": round(frac * 100, 1),
                                      "running": True, "rate_bytes_per_sec": 200 * 2**20}
        else:
            m = s["members"][slot]
            m.update(device_path=d["path"], drive={"uuid": d["uuid"], "serial": d["serial"], "model": MODEL, "path": d["path"]}, labels="shelf=ds0/bay=12")
    return s


def engine_drives():
    taken = {d["uuid"] for _, d in REPLACE.values()}
    return [d for d in FREE if d["uuid"] not in taken]


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
            out = {k: v for k, v in d.items() if k not in ("prot",) and v is not None}
            run = fw_run(port, d["id"])
            if run:
                out["firmware_run"] = run
            return out

        def obj(self, d):
            return {"metadata": {"name": d["id"]}, "status": {
                "currentFormat": {"blockSize": d["block_size"], "protection": "type1" if d["prot"] else "none", "protType": d["prot"]},
                "plannedFormat": {"blockSize": 4096, "protection": "type1", "basis": "try", "fallbacks": []}, "supports": d["supports"], "format": None}}

        def do_GET(self):
            path = self.path.split("?")[0]
            with LOCK:
                if path == "/_writes":
                    return self.send(200, WRITES[port])
                if port == ENGINE:
                    if path == "/api/v1/arrays":
                        return self.send(200, {"items": [engine_set()]})
                    if path == "/api/v1/shelves":
                        s = engine_set()
                        return self.send(200, {"items": [{"name": "ds0", "state": s["status"]["state"], "usable_human": "10 TB", "sets": [s], "spares": []}]})
                    if path == "/api/v1/drives":
                        return self.send(200, {"items": engine_drives()})
                    if path in ("/api/v1/volumes", "/api/v1/slabs", "/api/v1/exports", "/api/v1/spares"):
                        return self.send(200, {"items": []})
                    return self.send(404, {"error": "no such path"})
                settle(port)
                if path == "/api/v1/components":
                    return self.send(200, [card(d) for d in DRIVES[port].values()])
                if path == "/api/v1/drives":
                    return self.send(200, {"drives": [self.rest(d) for d in DRIVES[port].values()]})
                if path == "/apis/storage.storm.io/v1/drives":
                    return self.send(200, {"items": [self.obj(d) for d in DRIVES[port].values()]})
                if path == "/api/v1/firmware/images":
                    return self.send(200, {"images": [{"name": n, "size": len(b), "sha256": "5e" * 32, "modified": st(T0)} for n, b in sorted(IMAGES[port].items())]})
                if path == "/api/v1/firmware":
                    runs = [fw_run(port, i) for i in FW if i in DRIVES[port]]
                    return self.send(200, {"active": sum(1 for r in runs if r["state"] == "running"), "updates": runs})
            return self.send(404, {"error": "no such path"})

        def record(self, body):
            a = self.headers.get("authorization") or ""
            WRITES[port].append({"method": self.command, "path": self.path, "bearer": a[7:] if a.startswith("Bearer ") else None, "body": body})

        def do_PUT(self):
            n = int(self.headers.get("content-length") or 0)
            data = self.rfile.read(n)
            with LOCK:
                self.record({"bytes": len(data)})
                if self.path.startswith("/api/v1/firmware/images/") and port != ENGINE:
                    name = self.path.rsplit("/", 1)[1]
                    IMAGES[port][name] = data
                    return self.send(200, {"name": name, "size": len(data), "sha256": "ab" * 32, "modified": st(time.time())})
            return self.send(404, {"error": "no such path"})

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)) or b"{}")
            path = self.path.split("?")[0]
            with LOCK:
                self.record(body)
                if port == ENGINE:
                    parts = path.split("/")
                    if len(parts) == 8 and parts[3] == "arrays" and parts[7] == "replace":
                        slot = int(parts[6])
                        d = next((x for x in engine_drives() if x["uuid"] == body.get("drive_uuid")), None)
                        if not d:
                            return self.send(400, {"error": f"cannot replace slot {slot}: drive {body.get('drive_uuid')} is not free"})
                        if REPLACE:
                            return self.send(400, {"error": f"cannot replace slot {slot}: slot {next(iter(REPLACE))} is being replaced already"})
                        REPLACE[slot] = (time.time(), d)
                        return self.send(202, engine_set())
                    return self.send(404, {"error": "no such path"})
                settle(port)
                held = DRIVES[port]
                if path == "/api/v1/firmware":
                    name = body.get("image", "")
                    if name not in IMAGES[port]:
                        return self.send(404, {"error": f"image {name!r} is not in the store"})
                    blocked = [f"{held[i]['name']} ({fw_blocker(held[i])})" for i in body.get("drives", []) if i in held and fw_blocker(held[i])]
                    blocked += [f"{i} (unknown drive)" for i in body.get("drives", []) if i not in held]
                    if blocked:
                        return self.send(409, {"error": "not started: " + "; ".join(blocked)})
                    started = []
                    for i in body["drives"]:
                        to = name.split("+", 1)[-1].rsplit(".", 1)[0]
                        # sdd's drive refuses the download, as a real one can.
                        FW[i] = {"start": time.time(), "image": name, "from": held[i]["firmware"], "to": to, "size": len(IMAGES[port][name]),
                                 "fail": "WRITE BUFFER mode 0x0e: ILLEGAL REQUEST, invalid field in CDB" if i == "d4" else None}
                        held[i]["activity"] = "updating_firmware"
                        started.append(fw_run(port, i))
                    return self.send(200, {"image": name, "started": started})
                parts = path.split("/")
                if len(parts) == 7 and parts[3] == "drives" and parts[4] in held:
                    d = held[parts[4]]
                    if parts[5:] == ["fleet", "join"]:
                        if d["membership"] == "fleet":
                            return self.send(409, {"error": f"{d['name']} is in the fleet already"})
                        d["membership"] = "fleet"
                        return self.send(200, {"message": f"{d['name']} joined the fleet"})
                    if parts[5:] == ["locate", "on"]:
                        return self.send(200, {"message": "locate on"})
                if len(parts) == 6 and parts[3] == "drives" and parts[5] == "drain" and parts[4] in held:
                    d = held[parts[4]]
                    if d["membership"] != "fleet":
                        return self.send(409, {"error": f"{d['name']} is not in the fleet"})
                    if "leave=true" not in self.path:
                        return self.send(400, {"error": "the stand-in wants leave=true"})
                    DRAINS[d["id"]] = time.time()
                    settle(port)
                    return self.send(200, {"id": d["id"], "drain": d["drain"], "then_leave": True})
            return self.send(404, {"error": "no such path"})
    return H


for p in (HERE, OTHER, ENGINE):
    threading.Thread(target=ThreadingHTTPServer(("127.0.0.1", p), handler(p)).serve_forever, daemon=True).start()
threading.Event().wait()
