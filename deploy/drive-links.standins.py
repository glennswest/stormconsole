#!/usr/bin/env python3
"""Stand-ins for deploy/verify-drive-links.sh (#339): two stormdrives in v0.49.0's shapes
(stormdrive src/link.rs, src/api/kube.rs, src/components.rs) and this node's ironprom.

forge1 is this node: four drives on one HBA, bay 3 (NET003) failing with the drive side's
loss of sync at 500/min and the host side 0, the other three ok. storm-b is another node:
bay 1 behind an expander, degraded. Each Drive object's link sample advances one minute
whenever it is read, so a console reading once a minute keeps a trend.

  GET  /api/v1/components                   the feed (a drive's `link errors` when not ok)
  GET  /api/v1/drives                       the REST list
  GET  /apis/storage.storm.io/v1/drives     the Drive objects, status.link + conditions
  POST /_set {"bay": 3, "level": "ok", "okFor": 120}   bay 3 ok again, okFor seconds ago
  POST /_set {"bay": 3, "smart": {...}}                 bay 3's own counters now (#384)

Each Drive object's status.health is stormdrive's HealthReport (src/drive.rs), with `smart` its
SmartCounters (src/smart/mod.rs): NET001 and NET003 from LOG SENSE (NET003 with 2 uncorrected
reads and 5 grown defects), storm-b's BEE001 from ATA SMART predicting its failure; NET002 and
NET004 give none.

ironprom (third port): /api/v1/targets, and /api/v1/query_range answering the drive
link chart for NET003 with 24 h of loss of sync, both sides."""
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE, OTHER, PROM = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
LOCK = threading.Lock()
T0 = int(time.time()) - 3600
RECO_DRIVE = "the drive's receiver or its slot: reseat; if it continues, swap bays or replace"
RECO_BOTH = "the cable or the expander port"


def rfc(t):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t))


def sas_smart(read_unc, grown, read_corr):
    """A SAS drive's SmartCounters from LOG SENSE: the error counter pages and the grown defect list."""
    return {"source": "log_sense",
            "read_errors": {"corrected": read_corr, "uncorrected": read_unc, "bytes": 9000000000},
            "write_errors": {"corrected": 4, "uncorrected": 0, "bytes": 7000000000},
            "verify_errors": {"corrected": 0, "uncorrected": 0},
            "grown_defects": grown}


def health(d):
    """status.health as stormdrive serializes HealthReport (a SystemTime is secs/nanos)."""
    h = {"status": "good", "temperature_c": 36, "power_on_hours": 21000, "media_errors": 0, "critical_warning": 0,
         "messages": [], "collected_at": {"secs_since_epoch": int(time.time()) - 30, "nanos_since_epoch": 0}}
    if d["smart"] is not None:
        h["smart"] = d["smart"]
    return h


def new(id, name, serial, bay, level, host_phy, drive_rate=0.0, host_rate=0.0, smart=None):
    return {"id": id, "name": name, "serial": serial, "bay": bay, "level": level, "since": T0, "at": T0, "smart": smart,
            "hostPhy": host_phy, "driveRate": drive_rate, "hostRate": host_rate,
            # (on, when it last changed); 0 for a condition that never has (the epoch).
            "degraded": (level in ("degraded", "failing"), T0 if level != "ok" else 0),
            "failing": (level == "failing", T0 + 60 if level == "failing" else 0)}


DRIVES = {
    HERE: [new("5000cca26a819e61", "sda", "NET001", 0, "ok", "phy-1:0", smart=sas_smart(0, 0, 0)),
           new("5000cca26a819e62", "sdb", "NET002", 1, "ok", "phy-1:1"),
           new("5000cca26a819e69", "sdc", "NET003", 3, "failing", "phy-1:3", drive_rate=500.0, smart=sas_smart(2, 5, 120)),
           new("5000cca26a819e64", "sdd", "NET004", 4, "ok", "phy-1:4")],
    OTHER: [new("5000c500aaaa0001", "sdx", "BEE001", 1, "degraded", "phy-2:0:1", drive_rate=3.0, host_rate=2.0,
                smart={"source": "ata_smart", "reallocated_sectors": 8, "pending_sectors": 1, "offline_uncorrectable": 0,
                       "reported_uncorrectable": 0, "crc_errors": 3, "predicted_failure": "reallocated sector count past its threshold"})],
}


def link(d):
    """status.link, as stormdrive's link::status() serializes it; a read is the next minute's sample."""
    d["at"] += 60
    phy = lambda p, rate, los: {"phy": p, "rateMbps": 12000, "invalidDwords": 4294967295 if p == "0" and d["driveRate"] else 0,
                                "disparityErrors": 0, "lossOfSync": los, "phyResets": 0}
    rate = lambda p, r: {"phy": p, "invalidDwords": 0.0, "disparityErrors": 0.0, "lossOfSync": r, "phyResets": 0.0}
    moving = d["level"] in ("degraded", "failing")
    msg = "" if not moving else f"drive phy 0 loss of sync {int(d['driveRate'])}/min" + (f", host {d['hostPhy']} loss of sync {int(d['hostRate'])}/min" if d["hostRate"] else "")
    out = {"at": d["at"], "level": d["level"], "since": d["since"], "streak": 2 if moving else 0, "syncStreak": 2 if moving else 0,
           "hostPhy": d["hostPhy"], "negotiatedMbps": 12000, "hostMaximumMbps": 12000, "hostMaximumHwMbps": 12000,
           "drive": {"phys": [phy("0", 0, 73506628 if d["driveRate"] else 13)], "rates": [rate("0", d["driveRate"] if moving else 0.0)]},
           "host": {"phys": [phy(d["hostPhy"], 0, 15 if d["hostRate"] else 0)], "rates": [rate(d["hostPhy"], d["hostRate"] if moving else 0.0)]},
           "message": msg or f"link {d['level']}", "policy": None, "maximum": "maximum 12 Gbit (hardware)"}
    if moving:
        out["recommendation"] = RECO_BOTH if d["hostRate"] else RECO_DRIVE
    return out


def conditions(d, l):
    def c(kind, on_since, reason):
        on, since = on_since
        return {"type": kind, "status": "True" if on else "False", "reason": reason if on else "LinkOk",
                "message": (l["message"] + (f" — {l['recommendation']}" if l.get("recommendation") else "")) if on else f"link {d['level']}",
                "lastTransitionTime": rfc(since) if since else "1970-01-01T00:00:00Z"}
    return [{"type": "Ready", "status": "True"}, c("LinkDegraded", d["degraded"], "ErrorsMoving"), c("LinkFailing", d["failing"], "SyncLost")]


def obj(d):
    l = link(d)
    return {"apiVersion": "storage.storm.io/v1", "kind": "Drive", "metadata": {"name": d["id"]},
            "status": {"name": d["name"], "serial": d["serial"], "bay": d["bay"], "enclosure": "shelf0", "wwid": f"naa.{d['id']}", "link": l,
                       "health": health(d), "conditions": conditions(d, l)}}


def card(d):
    metrics = [{"label": "serial", "value": d["serial"]}, {"label": "bay", "value": str(d["bay"])},
               {"label": "capacity", "value": "10.0 TB"}, {"label": "dev", "value": f"/dev/{d['name']}"},
               {"label": "kind", "value": "sas_hdd"}, {"label": "link", "value": "12 Gb/s", "tone": "muted"}]
    if d["level"] == "degraded":
        metrics.append({"label": "link errors", "value": "degraded", "tone": "warn"})
    if d["level"] == "failing":
        metrics.append({"label": "link errors", "value": "failing", "tone": "error"})
    return {"id": f"drive:{d['id']}", "kind": "drive", "label": f"{d['name']} · X378_HLBRE10TA07", "health": "ok",
            "detail": "out of fleet · idle", "metrics": metrics, "actions": [], "relations": []}


def stormdrive(port):
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

        def do_GET(self):
            path = self.path.split("?")[0]
            with LOCK:
                if path == "/api/v1/components":
                    return self.send(200, [card(d) for d in DRIVES[port]])
                if path == "/api/v1/drives":
                    return self.send(200, {"drives": [{"id": d["id"], "name": d["name"], "serial": d["serial"], "location": {"bay": d["bay"]}} for d in DRIVES[port]]})
                if path == "/apis/storage.storm.io/v1/drives":
                    return self.send(200, {"items": [obj(d) for d in DRIVES[port]]})
                if path == "/api/v1/summary":
                    return self.send(200, {"health": "ok", "detail": "stand-in"})
            return self.send(404, {"error": "no such path"})

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)) or b"{}")
            if self.path != "/_set":
                return self.send(404, {"error": "no such path"})
            with LOCK:
                d = next(x for x in DRIVES[port] if x["bay"] == body["bay"])
                now = int(time.time())
                if "smart" in body:
                    d["smart"] = body["smart"]
                    return self.send(200, {"ok": True})
                d["level"] = body["level"]
                if body["level"] == "ok":
                    turned = now - body.get("okFor", 0)
                    d["since"] = turned
                    d["degraded"] = (False, turned)
                    d["failing"] = (False, turned)
                    d["driveRate"] = 0.0
            return self.send(200, {"ok": True})
    return H


def ironprom():
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

        def do_GET(self):
            from urllib.parse import urlparse, parse_qs
            u = urlparse(self.path)
            q = parse_qs(u.query)
            if u.path == "/api/v1/targets":
                return self.send(200, {"status": "success", "data": {"activeTargets": [
                    {"scrapePool": "stormdrive", "scrapeUrl": f"http://127.0.0.1:{HERE}/metrics", "health": "up", "lastError": "", "lastScrape": rfc(int(time.time())), "lastScrapeDuration": 0.01}]}})
            if u.path == "/api/v1/query_range":
                query = q.get("query", [""])[0]
                start, end, step = int(float(q["start"][0])), int(float(q["end"][0])), int(float(q["step"][0]))
                result = []
                if 'serial="NET003"' in query and "loss_of_sync" in query:
                    ts = range(start, end + 1, step)
                    # Quiet, then climbing ~500/min for the last six hours: bay 3.
                    drive = [[t, "0" if t < end - 6 * 3600 else str(480 + (t // step) % 40)] for t in ts]
                    host = [[t, "0"] for t in ts]
                    result = [{"metric": {"side": "drive"}, "values": drive}, {"metric": {"side": "host"}, "values": host}]
                return self.send(200, {"status": "success", "data": {"resultType": "matrix", "result": result}})
            return self.send(404, {"error": "no such path"})
    return H


for port, h in [(HERE, stormdrive(HERE)), (OTHER, stormdrive(OTHER)), (PROM, ironprom())]:
    s = ThreadingHTTPServer(("127.0.0.1", port), h)
    threading.Thread(target=s.serve_forever, daemon=True).start()
print("stand-ins up", flush=True)
threading.Event().wait()
