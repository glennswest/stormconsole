#!/usr/bin/env python3
"""Stand-ins for deploy/verify-api-health.sh (#123).

  api <port> <mode file>
      A service with one cheap read, GET /api/v1/things, that a real stormd
      probes. What it does is read from <mode file> on every request:
      ok (answers at once), slow (0.4 s), stall (30 s, past any timeout),
      down (500).

  merge <health.d> <pid1.json> <out> [interval]
      PID 1's merge (stormpump#127), which needs to be PID 1 to run for
      real: every <interval> s, read each <health.d>/<container>.json a
      stormd writes (stormd#52), add what PID 1 adds to each item (source,
      container, running, file_age_secs, stale; stalled with
      reported_state when the file is older than 3 x its largest
      interval), put PID 1's own probes from <pid1.json> first, and write
      {updated, worst, apis} to <out> by tmp + rename — the shape of
      stormpump 272470b's apihealth::summary and healthd::Merged::json.
"""
import json
import os
import sys
import time
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def api(port, mode_file):
    class H(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path.split("?")[0] != "/api/v1/things":
                self.send_response(404)
                self.end_headers()
                return
            try:
                mode = open(mode_file).read().strip()
            except OSError:
                mode = "ok"
            if mode == "slow":
                time.sleep(0.4)
            elif mode == "stall":
                time.sleep(30)
            if mode == "down":
                self.send_response(500)
                self.end_headers()
                self.wfile.write(b"broken")
                return
            body = b'{"things":[]}'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a):
            pass

    ThreadingHTTPServer(("127.0.0.1", int(port)), H).serve_forever()


RANK = {"healthy": 0, "unknown": 1, "slow": 2, "down": 3, "stalled": 4}


def now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def merge(healthd, pid1, out, interval=1.0):
    while True:
        apis = []
        try:
            apis.extend(json.load(open(pid1)))
        except (OSError, ValueError):
            pass
        for name in sorted(os.listdir(healthd)):
            if not name.endswith(".json") or name.startswith("."):
                continue
            path = os.path.join(healthd, name)
            container = name[: -len(".json")]
            age = int(time.time() - os.stat(path).st_mtime)
            try:
                doc = json.load(open(path))
                items = doc["items"]
            except (OSError, ValueError, KeyError) as e:
                apis.append({"source": "stormd", "container": container, "process": None, "api": None,
                             "url": None, "state": "down", "since": None, "running": None,
                             "last_error": f"not stormd's: {e}", "file_age_secs": age, "stale": False})
                continue
            limit = 3 * max([i.get("interval_secs") or 15 for i in items] or [15])
            stale = age > limit
            for i in items:
                e = {"source": "stormd", "container": container, "running": None, **i,
                     "file_age_secs": age, "stale": stale}
                if stale:
                    e["reported_state"] = i.get("state")
                    e["state"] = "stalled"
                    e["last_error"] = f"its stormd has not rewritten {name} for {age} s"
                apis.append(e)
        worst = max((a.get("state", "unknown") for a in apis), key=lambda s: RANK.get(s, 1), default="healthy")
        tmp = out + ".tmp"
        with open(tmp, "w") as f:
            json.dump({"updated": now(), "worst": worst, "apis": apis}, f)
        os.rename(tmp, out)
        time.sleep(float(interval))


if __name__ == "__main__":
    if sys.argv[1] == "api":
        api(sys.argv[2], sys.argv[3])
    elif sys.argv[1] == "merge":
        merge(*sys.argv[2:])
    else:
        sys.exit(f"unknown role {sys.argv[1]}")
