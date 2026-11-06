#!/usr/bin/env python3
"""A stand-in engine's open /api/v1/health for deploy/browser/boot.cjs (#271), in stormblock's shape
(`mgmt/api/mod.rs`, `flowprogress.rs`, `slab_report.rs`). A real flow-over needs an appliance to move
from, which a build VM has not got. POST /_mode/<moving|checking|corrupt|stalled|settling|done|none> to
change state. While moving, bytes grow at a rate that changes, so a sparkline from the counter has a shape.
"""
import json, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

mode = "moving"
t0 = time.time()
bytes_ = 0.0
last = time.time()
SLABS = {"diskless": False, "system": "mixed", "data": "mixed",
         "items": [{"role": "system", "source": "local", "device": "/dev/sda@1048576", "volumes": 61},
                   {"role": "data", "source": "remote", "transport": "nvme-tcp", "volumes": 2}]}
LOCAL = {"diskless": False, "system": "local", "data": "local",
         "items": [{"role": "system", "source": "local", "device": "/dev/sda@1048576", "volumes": 61}]}


def flow(extra=None):
    secs = time.time() - t0
    moved = int(bytes_ / 4194304)
    f = {"moved": moved, "zeroed": moved // 3, "bytes": int(bytes_), "seconds": secs,
         "mb_per_s": bytes_ / 1e6 / max(secs, 0.1), "extents_per_s": (moved + moved // 3) / max(secs, 0.1),
         "persists": moved // 8, "hot_moved": 2}
    f.update(extra or {})
    return f


def health():
    global bytes_, last
    now = time.time()
    if mode == "moving":
        # 150 to 450 MB/s, swinging over 30 s.
        bytes_ += (300 + 150 * ((now % 30) / 15 - 1)) * 1e6 * (now - last)
    last = now
    h = {"status": "ok", "service": "stormblock"}
    if mode == "moving":
        h.update(flow_over_remaining=9000, flow_over=dict(flow(), eta_seconds=754.0), slabs=SLABS)
    elif mode == "checking":
        h.update(flow_over_remaining=0, flow_over=flow({"validation": {"state": "running", "total": 900, "validated": 300, "skipped": 0, "mismatched": 0, "repaired": 0, "unrepaired": 0, "bytes": 1, "seconds": 3.0, "mb_per_s": 90.0}}), slabs=LOCAL)
    elif mode == "corrupt":
        h.update(flow_over_remaining=0, flow_over=flow({"validation": {"state": "corrupt", "total": 900, "validated": 898, "skipped": 0, "mismatched": 2, "repaired": 1, "unrepaired": 1, "bytes": 1, "seconds": 3.0, "mb_per_s": 90.0}}), slabs=SLABS)
    elif mode == "stalled":
        h.update(flow_over_remaining=512, flow_over=flow(), flow_over_stalled="destination full: 512 extents left on the network", slabs=SLABS)
    elif mode == "settling":
        h.update(flow_over_remaining=0, flow_over=flow(), slabs=SLABS)
    elif mode == "done":
        h.update(flow_over_remaining=0, flow_over=flow({"validation": {"state": "passed", "total": 900, "validated": 900, "skipped": 0, "mismatched": 0, "repaired": 0, "unrepaired": 0, "bytes": 1, "seconds": 9.0, "mb_per_s": 90.0}}), slabs=LOCAL)
    elif mode == "none":
        h.update(slabs=LOCAL)
    return h


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def send_json(self, code, body):
        b = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def do_GET(self):
        if self.path == "/api/v1/health":
            return self.send_json(200, health())
        # The plugin's poll asks for the rest; it has none of it.
        return self.send_json(200, {"items": []} if "volumes" not in self.path else [])

    def do_POST(self):
        global mode
        if self.path.startswith("/_mode/"):
            mode = self.path.rsplit("/", 1)[1]
            return self.send_json(200, {"mode": mode})
        self.send_json(404, {})


HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
