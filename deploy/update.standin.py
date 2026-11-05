#!/usr/bin/env python3
"""The release catalog for deploy/verify-update.sh, in stormcentral's shape: the
GET /api/v1/releases stormupdate's catalog client reads. argv: port."""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

RELEASES = [
    {"version": "12.61", "number": "12.61", "flavor": "", "state": "published", "notes": "better things", "published_at": 1790000000,
     "components": {"stormlb": "bbb", "stormconsole": "ccc"}},
    {"version": "12.60", "number": "12.60", "flavor": "", "state": "composed", "notes": "", "components": {"stormlb": "aaa"}},
    {"version": "12.58", "number": "12.58", "flavor": "", "state": "published", "notes": "", "published_at": 1780000000,
     "components": {"stormlb": "aaa", "stormconsole": "ccc"}},
]


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        p = self.path.split("?")[0]
        if p == "/api/v1/releases":
            body, code = RELEASES, 200
        elif p.startswith("/api/v1/releases/"):
            v = p.rsplit("/", 1)[-1]
            r = next((x for x in RELEASES if x["version"] == v), None)
            body, code = (dict(r, manifest=[], pallets={}) if r else {"error": "no such"}), (200 if r else 404)
        else:
            body, code = {}, 404
        b = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)


HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
