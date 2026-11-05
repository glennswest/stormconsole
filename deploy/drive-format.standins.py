#!/usr/bin/env python3
"""Stand-in stormdrives for deploy/verify-drive-format.sh (#165), in stormdrive
v0.39.0's shapes (src/api/mod.rs, src/api/kube.rs, src/format.rs, src/history.rs):
/api/v1/drives, /apis/storage.storm.io/v1/drives, POST /api/v1/format
(all-or-nothing, 409 {error, code}) and a drive's history. A format runs on a
clock, RUN seconds, and records every write with its bearer at /_writes."""
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE, OTHER, RUN = int(sys.argv[1]), int(sys.argv[2]), float(sys.argv[3])

PLAN_TRY = {"blockSize": 4096, "protection": "type1", "basis": "try", "reason": "4K physical block", "fallback": False,
            "fallbacks": [{"blockSize": 4096, "protection": "none"}, {"blockSize": 512, "protection": "type1"}]}


def drive(id, name, serial, bay, **kw):
    d = {"id": id, "name": name, "serial": serial, "model": "H7210A521SUN010T", "wwid": f"naa.{id}",
         "capacity_bytes": 10 * 10**12, "block_size": 520, "physical_block_size": 4096, "kind": "sas_hdd",
         "membership": "out", "activity": "idle", "designation": "none", "in_use_by": None,
         "location": {"bay": bay}, "plan": PLAN_TRY, "format": None}
    d.update(kw)
    return d


DRIVES = {
    HERE: {d["id"]: d for d in [
        drive("d1", "sda", "SYS001", 1, designation="system", block_size=512),
        drive("d2", "sdb", "NET002", 2),
        drive("d3", "sdc", "NET003", 3),
        drive("d4", "sdd", "NET004", 4),
        drive("d5", "sde", "FLT005", 5, membership="fleet", block_size=4096, usage={"slabs": [{"id": "s1"}]}),
        drive("d6", "sdf", "LVM006", 6, in_use_by="LVM2_member"),
        drive("d7", "sdg", "OLD007", 7, block_size=4096, plan={"error": "not probed yet (SCSI drives only)"}),
    ]},
    OTHER: {"b1": drive("b1", "sdx", "BEE001", 1)},
}
# How each drive's run ends, as forge1's could.
ENDS = {
    "d2": {"after": lambda r: (r["to"], 1 if r["protection"] == "type1" else 0)},
    # Refuses PI: the run takes the plan's first fallback, and says so.
    "d3": {"refuse": True, "after": lambda r: (r["to"], 0)},
    # Takes the format and reads back something else.
    "d4": {"after": lambda r: (512, 0), "fail": "reads back 512 after a format to 4096"},
}
RUNS = {}
HISTORY = {"d2": [{"at": "2026-10-01T00:00:00Z", "kind": "action", "severity": "info", "message": "format done", "action": {
    "op": "format", "params": {"block_size": 512, "protection": "none"}, "result": "done", "requester": "root",
    "before": {"block_size": 520}, "after": {"block_size": 512, "prot_type": 0}}},
    {"at": "2026-10-01T00:00:01Z", "kind": "moved", "severity": "info", "message": "moved"}]}
WRITES = {HERE: [], OTHER: []}
LOCK = threading.Lock()


def status(id):
    """status.format, on a clock."""
    r = RUNS.get(id)
    if not r:
        return None
    now = time.time()
    ends = ENDS.get(id, {})
    pct = min(99, int((now - r["start"]) / RUN * 100))
    f = {"fromBlockSize": 520, "toBlockSize": r["to"], "protection": r["protection"], "startedUnix": int(r["start"]),
         "refused": r["refused"], "state": "running", "phase": "formatting", "progressPct": pct,
         "progressAtUnix": int(now), "etaSeconds": max(1, int(r["start"] + RUN - now)), "finishedUnix": None,
         "blockSizeAfter": None, "protTypeAfter": None, "error": None}
    if now - r["start"] >= RUN:
        size, typ = ends.get("after", lambda x: (x["to"], 1 if x["protection"] == "type1" else 0))(r)
        f.update(state="failed" if ends.get("fail") else "done", phase="failed" if ends.get("fail") else "done",
                 progressPct=100, etaSeconds=None, finishedUnix=int(r["start"] + RUN),
                 blockSizeAfter=size, protTypeAfter=typ, error=ends.get("fail"))
    return f


def blocker(d):
    if d["membership"] == "fleet":
        return "in the fleet — leave (drain) first"
    if d["in_use_by"]:
        return f"in use: holds data for {d['in_use_by']}"
    if d["id"] in RUNS and status(d["id"])["state"] == "running":
        return "a format is already running"
    if d["designation"] == "system":
        return 'designated system — a worker job naming it in "destroy" may format it'
    return None


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
            out = {k: v for k, v in d.items() if k not in ("plan", "format")}
            f = status(d["id"])
            if f and f["state"] == "running":
                out["activity"] = "formatting"
            # A controller reset rides on the live run only (format.rs).
            if d["id"] in RUNS and d["id"] == "d2":
                out["format_run"] = {"resets": ["host3: IOC reset count 0 → 1"]}
            return out

        def obj(self, d):
            return {"metadata": {"name": d["id"]}, "status": {
                "enclosure": "shelf0", "currentFormat": {"blockSize": d["block_size"], "protection": "none", "protType": 0},
                "plannedFormat": d["plan"], "supports": {"pi_types": []}, "format": status(d["id"])}}

        def do_GET(self):
            path = self.path.split("?")[0]
            with LOCK:
                if path == "/_writes":
                    return self.send(200, WRITES[port])
                if path == "/api/v1/components":
                    return self.send(200, [])
                if path == "/api/v1/drives":
                    return self.send(200, {"drives": [self.rest(d) for d in DRIVES[port].values()]})
                if path == "/apis/storage.storm.io/v1/drives":
                    return self.send(200, {"items": [self.obj(d) for d in DRIVES[port].values()]})
                parts = path.split("/")
                if len(parts) == 6 and parts[5] == "history" and parts[4] in DRIVES[port]:
                    return self.send(200, {"active": True, "log": HISTORY.get(parts[4], [])})
            return self.send(404, {"error": "no such path", "code": "not_found"})

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)) or b"{}")
            a = self.headers.get("authorization") or ""
            with LOCK:
                WRITES[port].append({"path": self.path, "bearer": a[7:] if a.startswith("Bearer ") else None, "body": body})
                if self.path != "/api/v1/format":
                    return self.send(404, {"error": "no such path", "code": "not_found"})
                held = DRIVES[port]
                blocked = []
                for id in body["drives"]:
                    why = blocker(held[id]) if id in held else "unknown drive"
                    if why:
                        blocked.append(f"{held[id]['name'] if id in held else id} ({why})")
                if blocked:
                    return self.send(409, {"error": "not started: " + "; ".join(blocked), "code": "conflict"})
                for id in body["drives"]:
                    run = {"start": time.time(), "to": body["block_size"], "protection": body["protection"], "refused": []}
                    if ENDS.get(id, {}).get("refuse") and body["protection"] == "type1":
                        run["protection"] = "none"
                        run["refused"] = ["4096 + type1: ILLEGAL REQUEST, invalid field in CDB"]
                    RUNS[id] = run
                    # What a finished run leaves in the drive's history.
                    HISTORY.setdefault(id, []).append({"at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "kind": "action", "severity": "info",
                        "message": "format", "action": {"op": "format", "params": {"block_size": body["block_size"], "protection": body["protection"]},
                        "result": "done", "requester": "alice", "before": {"block_size": 520}, "after": {"block_size": body["block_size"], "prot_type": 0}}})
                return self.send(200, {"block_size": body["block_size"], "protection": body["protection"], "started": body["drives"]})
    return H


for p in (HERE, OTHER):
    threading.Thread(target=ThreadingHTTPServer(("127.0.0.1", p), handler(p)).serve_forever, daemon=True).start()
threading.Event().wait()
