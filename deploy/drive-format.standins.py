#!/usr/bin/env python3
"""Stand-in stormdrives for deploy/verify-drive-format.sh (#165, #308), in stormdrive
v0.39.0's shapes, this node laid out as forge1 is (sda the system drive: the ESP and
both stormblock slabs; sdb-sdd free, 512 today, PI 1 + 2, planned 4096 + PI type 1) (src/api/mod.rs, src/api/kube.rs, src/format.rs, src/history.rs):
/api/v1/drives, /apis/storage.storm.io/v1/drives, POST /api/v1/format
(all-or-nothing, 409 {error, code}) and a drive's history. A format runs on a
clock, RUN seconds, and records every write with its bearer at /_writes.
Two NVMe namespaces (#356) are formatted only as stormdrive does it, a worker job's
Format NVM step (src/worker.rs, erase.rs): GET/POST /api/v1/worker/jobs. nvme0n1 has a
4096-byte LBA format; nvme1n1 has none, so its job fails as stormdrive's does.
A SCSI run has v0.47.0's method (stormdrive#140, #335): `auto`, the fast format first; sdc
refuses it, so its run falls back to full, saying the sense. Each run keeps its log, written
on the clock (a progress line a second, so the whole log outgrows the Drive object's newest
20 in `status.format.logTail`), served whole at GET /api/v1/drives/{id}/format/log."""
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE, OTHER, RUN = int(sys.argv[1]), int(sys.argv[2]), float(sys.argv[3])

PLAN_TRY = {"blockSize": 4096, "protection": "type1", "basis": "try", "reason": "4K physical block", "fallback": False,
            "fallbacks": [{"blockSize": 4096, "protection": "none"}, {"blockSize": 512, "protection": "type1"}]}


SUPPORTS = {"protect": True, "pi_types": [1, 2], "block_lengths": [], "physical_block_size": 4096}


def drive(id, name, serial, bay, **kw):
    d = {"id": id, "name": name, "serial": serial, "model": "H7210A521SUN010T", "wwid": f"naa.{id}",
         "capacity_bytes": 10 * 10**12, "block_size": 520, "physical_block_size": 4096, "kind": "sas_hdd",
         "membership": "out", "activity": "idle", "designation": "none", "in_use_by": None,
         "location": {"bay": bay}, "plan": PLAN_TRY, "format": None, "prot": 0, "supports": SUPPORTS}
    d.update(kw)
    return d


DRIVES = {
    HERE: {d["id"]: d for d in [
        # forge1's sda as its stormdrive 0.39 reports it (stormdrive#123's report).
        drive("d1", "sda", "SYS001", 0, block_size=512, prot=1, owner="stormblock",
              in_use_by="stormblock (system slab partitions 2, 3)",
              slab_parts=[{"partition": 2, "name": "stormblock", "role": "system", "offset_bytes": 1048576},
                          {"partition": 3, "name": "stormblock-data", "role": "data", "offset_bytes": 2097152}],
              contents="vfat in partition 1 'EFI', stormblock slab in partition 2 'stormblock', stormblock slab in partition 3 'stormblock-data'"),
        drive("d2", "sdb", "NET002", 3, block_size=512, prot=1),
        drive("d3", "sdc", "NET003", 4, block_size=512, prot=1),
        drive("d4", "sdd", "NET004", 7, block_size=512),
        drive("d5", "sde", "FLT005", 5, membership="fleet", block_size=4096, usage={"slabs": [{"id": "s1"}]}),
        drive("d6", "sdf", "LVM006", 6, in_use_by="LVM2_member"),
        drive("d7", "sdg", "OLD007", 8, block_size=4096, plan={"error": "not probed yet (SCSI drives only)"}, supports=None),
        drive("d8", "sdh", "NET008", 9),
        drive("d9", "sdi", "NET009", 10),
        drive("n1", "nvme0n1", "NVM001", 11, kind="nvme_ssd", model="SAMSUNG MZQL23T8HCLS", block_size=512,
              physical_block_size=512, plan={"error": "not probed yet (SCSI drives only)"}, supports=None),
        drive("n2", "nvme1n1", "NVM002", 12, kind="nvme_ssd", model="INTEL SSDPE2KX010T8", block_size=512,
              physical_block_size=512, plan={"error": "not probed yet (SCSI drives only)"}, supports=None),
    ]},
    OTHER: {"b1": drive("b1", "sdx", "BEE001", 1)},
}
# How each drive's run ends, as forge1's could.
ENDS = {
    "d2": {"after": lambda r: (r["to"], 1 if r["protection"] == "type1" else 0)},
    # Refuses PI: the run takes the plan's first fallback, and says so.
    "d8": {"refuse": True, "after": lambda r: (r["to"], 0)},
    # Takes the format and reads back something else.
    "d9": {"after": lambda r: (512, 0), "fail": "reads back 512 after a format to 4096"},
    # Refuses the fast format (FFMT): stormdrive's auto falls back to a full FORMAT UNIT.
    "d3": {"fast_refused": True},
}
FAST_SENSE = "05/24/00 ILLEGAL REQUEST, invalid field in CDB"
LOG_TAIL = 20
RUNS = {}
HISTORY = {"d2": [{"at": "2026-10-01T00:00:00Z", "kind": "action", "severity": "info", "message": "format done", "action": {
    "op": "format", "params": {"block_size": 512, "protection": "none"}, "result": "done", "requester": "root",
    "before": {"block_size": 520}, "after": {"block_size": 512, "prot_type": 0}}},
    {"at": "2026-10-01T00:00:01Z", "kind": "moved", "severity": "info", "message": "moved"}]}
WRITES = {HERE: [], OTHER: []}
# The namespaces' LBA formats by data size (Identify Namespace, no metadata).
LBAF = {"n1": [512, 4096], "n2": [512]}
JOBS = {HERE: [], OTHER: []}


def systime(t):
    return None if t is None else {"secs_since_epoch": int(t), "nanos_since_epoch": 0}


def job_view(port, j):
    """A worker job (worker.rs Job::view), on the clock: queued a moment, Format NVM for RUN seconds."""
    now = time.time()
    drives = []
    for dj in j["drives"]:
        dj = dict(dj)
        if dj["state"] != "refused":
            start = j["created"] + 1
            if now < start:
                dj.update(state="queued", phase="queued")
            elif now < start + RUN:
                d = DRIVES[port][dj["drive"]]
                dj.update(state="running", phase=f"format nvm lbaf {LBAF[dj['drive']].index(j['bs']) if j['bs'] in LBAF[dj['drive']] else '?'} (was {d['block_size']})", started=systime(start))
            elif j["bs"] in LBAF[dj["drive"]]:
                DRIVES[port][dj["drive"]]["block_size"] = j["bs"]
                dj.update(state="done", phase="done", started=systime(start), finished=systime(start + RUN),
                          done=[f"Format NVM to {j['bs']}-byte blocks"])
            else:
                dj.update(state="failed", phase="failed", started=systime(start), finished=systime(start + 1),
                          error=f"no LBA format with {j['bs']}-byte data and no metadata")
        drives.append(dj)
    return {"id": j["id"], "created": systime(j["created"]), "select": j["select"], "steps": j["steps"], "drives": drives,
            "finished": all(d["state"] not in ("queued", "running") for d in drives)}
LOCK = threading.Lock()


def run_log(id, r, now):
    """The run's log as stormdrive's formatcli.rs and format.rs write it, up to `now`."""
    n, to, ends = r["name"], r["to"], ENDS.get(id, {})
    prot = "PI type 1" if r["protection"] == "type1" else "no PI"
    t0 = r["start"]
    out = [(t0, f"{n}: auto format to {to} with {prot} through /dev/{n}")]
    for why in r["refused"]:
        out.append((t0, f"{n}: {why}; {to} with no PI instead"))
    out.append((t0, f"{n}: fast format (FFMT 01b) to {to} with {prot}"))
    if ends.get("fast_refused"):
        out.append((t0 + 1, f"{n}: fast format refused ({FAST_SENSE}); a full format instead"))
        out.append((t0 + 1, f"{n}: full format (FORMAT UNIT) to {to} with {prot}"))
    for s in range(2, int(min(now - t0, RUN))):
        out.append((t0 + s, f"{n}: formatting {min(99, int(s / RUN * 100))}% after {s} s"))
    if now - t0 >= RUN:
        out.append((t0 + RUN, f"{n}: failed: {ends['fail']}" if ends.get("fail") else f"{n}: done"))
    return out


def status(id):
    """status.format, on a clock."""
    r = RUNS.get(id)
    if not r:
        return None
    now = time.time()
    ends = ENDS.get(id, {})
    pct = min(99, int((now - r["start"]) / RUN * 100))
    f = {"fromBlockSize": r["from"], "toBlockSize": r["to"], "protection": r["protection"], "startedUnix": int(r["start"]),
         "refused": r["refused"], "state": "running", "progressPct": pct,
         # stormdrive's phases (format.rs): prepare, formatting, then rescan + verify (the read-back).
         "phase": "prepare" if pct < 8 else "verify" if pct >= 80 else "formatting",
         "progressAtUnix": int(now), "etaSeconds": max(1, int(r["start"] + RUN - now)), "finishedUnix": None,
         "blockSizeAfter": None, "protTypeAfter": None, "error": None,
         "method": "auto", "methodUsed": "full" if ends.get("fast_refused") else "fast",
         "fastSkipped": FAST_SENSE if ends.get("fast_refused") else None, "already": False,
         "logTail": [{"atUnix": int(t), "line": l} for t, l in run_log(id, r, now)[-LOG_TAIL:]]}
    if now - r["start"] >= RUN:
        size, typ = ends.get("after", lambda x: (x["to"], 1 if x["protection"] == "type1" else 0))(r)
        f.update(state="failed" if ends.get("fail") else "done", phase="failed" if ends.get("fail") else "done",
                 progressPct=100, etaSeconds=None, finishedUnix=int(r["start"] + RUN),
                 blockSizeAfter=size, protTypeAfter=typ, error=ends.get("fail"))
    return f


def card(d):
    """A drive in stormdrive's feed (components.rs): its actions as it lists them, Format 4K included,
    which the console takes off (#308)."""
    base = f"/api/v1/drives/{d['id']}"
    idle = d["activity"] == "idle" and not (d["id"] in RUNS and status(d["id"])["state"] == "running")
    act = lambda id, label, path, enabled=True, danger=False: {"id": id, "label": label, "method": "POST", "path": f"{base}/{path}", "enabled": enabled, "danger": danger}
    fleet = d["membership"] == "fleet"
    actions = [act("locate-on", "Locate", "locate/on"), act("locate-off", "LED off", "locate/off"),
               act("fleet-leave", "Leave fleet", "fleet/leave", idle, True) if fleet else act("fleet-join", "Join fleet", "fleet/join", idle and not d["in_use_by"]),
               act("test-smoke", "Smoke test", "test/smoke", idle), act("test-scan", "Read scan", "test/read_scan", idle),
               act("test-destructive", "Destructive test", "test/destructive_sample", idle and not d["in_use_by"] and not fleet, True),
               act("enroll", "Enrol (data slab)", "enroll", False, True),
               act("format-4k", "Format 4K", "format/4096", blocker(d) is None, True),
               act("overcommit-2x", "Overcommit 2×", "overcommit/2", True, True),
               act("mark-spare", "Mark spare", "designation/spare", not d["in_use_by"]),
               act("mark-reserved", "Mark reserved", "designation/reserved"),
               act("mark-failed", "Mark failed", "designation/failed", True, True)]
    metrics = [{"label": "serial", "value": d["serial"]}, {"label": "bay", "value": str(d["location"]["bay"])},
               {"label": "hba", "value": "PERC H730 Mini"}, {"label": "capacity", "value": "10.0 TB"}, {"label": "dev", "value": f"/dev/{d['name']}"},
               {"label": "sector", "value": str(d["block_size"])}, {"label": "temp", "value": "31", "unit": "°C"}, {"label": "hours", "value": "41022"},
               {"label": "kind", "value": "sas_hdd"}, {"label": "smart", "value": "ok"}, {"label": "wear", "value": "0"}, {"label": "link", "value": "12 Gb/s"}]
    if d["in_use_by"]:
        metrics.append({"label": "in use", "value": d["in_use_by"], "tone": "accent"})
    return {"id": f"drive:{d['id']}", "kind": "drive", "label": f"{d['name']} · {d['model']}", "health": "ok",
            "detail": f"{'in fleet' if fleet else 'out of fleet'} · {d['activity']}", "metrics": metrics, "actions": actions, "relations": []}


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
            out = {k: v for k, v in d.items() if k not in ("plan", "format", "prot") and v is not None}
            f = status(d["id"])
            if f and f["state"] == "running":
                out["activity"] = "formatting"
            if nvme_running(port, d["id"]):
                out["activity"] = "formatting"
            # A controller reset rides on the live run only (format.rs).
            if d["id"] in RUNS and d["id"] == "d2":
                out["format_run"] = {"resets": ["host3: IOC reset count 0 → 1"]}
            return out

        def obj(self, d):
            return {"metadata": {"name": d["id"]}, "status": {
                "enclosure": "shelf0", "currentFormat": {"blockSize": d["block_size"], "protection": "type1" if d["prot"] else "none", "protType": d["prot"]},
                "plannedFormat": d["plan"], "supports": d["supports"] or {"pi_types": []}, "format": status(d["id"])}}

        def do_GET(self):
            path = self.path.split("?")[0]
            with LOCK:
                if path == "/_writes":
                    return self.send(200, WRITES[port])
                if path == "/api/v1/components":
                    return self.send(200, [card(d) for d in DRIVES[port].values()])
                if path == "/api/v1/worker/jobs":
                    return self.send(200, {"jobs": [job_view(port, j) for j in JOBS[port]]})
                if path == "/api/v1/drives":
                    return self.send(200, {"drives": [self.rest(d) for d in DRIVES[port].values()]})
                if path == "/apis/storage.storm.io/v1/drives":
                    return self.send(200, {"items": [self.obj(d) for d in DRIVES[port].values()]})
                parts = path.split("/")
                if len(parts) == 7 and parts[5:] == ["format", "log"] and parts[4] in DRIVES[port]:
                    id, r = parts[4], RUNS.get(parts[4])
                    log = [{"at": systime(t), "line": l} for t, l in run_log(id, r, time.time())] if r else []
                    return self.send(200, {"drive": id, "name": DRIVES[port][id]["name"],
                                           "running": (status(id)["state"] == "running") if r else None, "log": log})
                if len(parts) == 6 and parts[5] == "history" and parts[4] in DRIVES[port]:
                    return self.send(200, {"active": True, "log": HISTORY.get(parts[4], [])})
            return self.send(404, {"error": "no such path", "code": "not_found"})

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)) or b"{}")
            a = self.headers.get("authorization") or ""
            with LOCK:
                WRITES[port].append({"path": self.path, "bearer": a[7:] if a.startswith("Bearer ") else None, "body": body})
                if self.path.startswith("/api/v1/drives/") and self.path.endswith("/locate/on"):
                    return self.send(200, {"message": "locate on"})
                held = DRIVES[port]
                if self.path == "/api/v1/worker/jobs":
                    return self.worker(body, held)
                if self.path != "/api/v1/format":
                    return self.send(404, {"error": "no such path", "code": "not_found"})
                blocked = []
                for id in body["drives"]:
                    why = blocker(held[id]) if id in held else "unknown drive"
                    if not why and held[id]["kind"] == "nvme_ssd":
                        why = 'NVMe: this is SCSI FORMAT UNIT; reformat an NVMe namespace with a worker job\'s "format" step (POST /api/v1/worker/jobs, Format NVM)'
                    if why:
                        blocked.append(f"{held[id]['name'] if id in held else id} ({why})")
                if blocked:
                    return self.send(409, {"error": "not started: " + "; ".join(blocked), "code": "conflict"})
                for id in body["drives"]:
                    run = {"start": time.time(), "name": held[id]["name"], "from": held[id]["block_size"], "to": body["block_size"], "protection": body["protection"], "refused": []}
                    if ENDS.get(id, {}).get("refuse") and body["protection"] == "type1":
                        run["protection"] = "none"
                        run["refused"] = ["4096 + type1: ILLEGAL REQUEST, invalid field in CDB"]
                    RUNS[id] = run
                    # What a finished run leaves in the drive's history.
                    HISTORY.setdefault(id, []).append({"at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "kind": "action", "severity": "info",
                        "message": "format", "action": {"op": "format", "params": {"block_size": body["block_size"], "protection": body["protection"]},
                        "result": "done", "requester": "alice", "before": {"block_size": 520}, "after": {"block_size": body["block_size"], "prot_type": 0}}})
                return self.send(200, {"block_size": body["block_size"], "protection": body["protection"], "started": body["drives"]})
        def worker(self, body, held):
            """POST /api/v1/worker/jobs (worker.rs submit): the guards per drive; refused ones ride in the job."""
            steps = body.get("steps") or []
            if [s.get("op") for s in steps] != ["format"]:
                return self.send(400, {"error": "this stand-in runs one format step", "code": "bad_request"})
            bs = steps[0]["block_size"]
            djs = []
            for id in body["select"]["drives"]:
                if id not in held:
                    return self.send(400, {"error": f"unknown drive {id!r}", "code": "bad_request"})
                d = held[id]
                why = blocker(d) or ("busy: formatting" if nvme_running(port, id) else None)
                if not why and d["kind"] != "nvme_ssd":
                    why = None  # a SCSI drive in a worker job: not what the console sends for one
                djs.append({"drive": id, "name": d["name"], "serial": d["serial"], "wwn": d["wwid"], "kind": d["kind"],
                            "state": "refused" if why else "queued", "step": 0, "phase": "refused" if why else "queued",
                            "progress_pct": None, "error": why, "done": [], "destroy_named": False, "started": None, "finished": None})
            if not any(dj["state"] == "queued" for dj in djs):
                return self.send(409, {"error": "nothing to run: " + json.dumps([{"name": x["name"], "reason": x["error"]} for x in djs]), "code": "conflict"})
            j = {"id": f"j{len(JOBS[port]) + 1:02x}", "created": time.time(), "select": body["select"], "steps": steps, "drives": djs, "bs": bs}
            JOBS[port].append(j)
            return self.send(200, job_view(port, j))
    return H


def nvme_running(port, id):
    for j in JOBS[port]:
        for dj in job_view(port, j)["drives"]:
            if dj["drive"] == id and dj["state"] in ("queued", "running"):
                return True
    return False


for p in (HERE, OTHER):
    threading.Thread(target=ThreadingHTTPServer(("127.0.0.1", p), handler(p)).serve_forever, daemon=True).start()
threading.Event().wait()
