#!/usr/bin/env python3
"""A stormipmi and its BMC for deploy/verify-users.sh (#391), in its Machines API's shape (src/machines.rs).

It answers what the Machines page reads, and takes the power verbs as the BMC would:
only with stormipmi's write token (`api.tokenFile`), which the console holds and adds.
Each write it takes is appended to LOG as `<method> <path> <power after>`; a write
without the token is refused 401 and logged as `refused <method> <path>`.

    ipmi-bmc.standin.py PORT TOKEN_FILE LOG
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT, TOKEN, LOG = int(sys.argv[1]), open(sys.argv[2]).read().strip(), sys.argv[3]
POWER = {"SIM1": "on"}
AFTER = {"on": "on", "off": "off", "soft": "off", "reboot": "on", "cycle": "on"}


def machines():
    return {"machines": [{"tag": t, "name": "sim-1", "host": "sim-1", "power": p, "state": "ready"} for t, p in POWER.items()],
            "default": None, "forge": None}


class H(BaseHTTPRequestHandler):
    def reply(self, code, body):
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(body).encode())

    def do_GET(self):
        p = self.path.split("?")[0]
        if p == "/api/v1/machines":
            return self.reply(200, machines())
        if p == "/api/v1/components":
            return self.reply(200, [{"id": "system", "kind": "system", "label": "stormipmi", "health": "ok",
                                     "detail": "1 machine", "metrics": [], "actions": [], "relations": []}])
        if p in ("/healthz", "/readyz"):
            return self.reply(200, {"ok": True})
        self.reply(404, {"error": "not here"})

    def write(self):
        p = self.path.split("?")[0]
        n = int(self.headers.get("content-length") or 0)
        if n:
            self.rfile.read(n)
        if self.headers.get("authorization") != f"Bearer {TOKEN}":
            with open(LOG, "a") as f:
                f.write(f"refused {self.command} {p}\n")
            return self.reply(401, {"error": "a write needs the token"})
        parts = p.strip("/").split("/")  # api v1 machines <tag> power <verb>
        if len(parts) == 6 and parts[2] == "machines" and parts[4] == "power" and parts[3] in POWER and parts[5] in AFTER:
            POWER[parts[3]] = AFTER[parts[5]]
            with open(LOG, "a") as f:
                f.write(f"{self.command} {p} {POWER[parts[3]]}\n")
            return self.reply(200, {"ok": True, "power": POWER[parts[3]]})
        self.reply(404, {"error": f"{p} is no verb here"})

    do_POST = do_PUT = do_DELETE = do_PATCH = write

    def log_message(self, *a):
        pass


ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
