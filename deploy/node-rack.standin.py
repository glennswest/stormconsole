#!/usr/bin/env python3
"""A stormipmi for deploy/verify-node-rack.sh (#77), in its Machines API's shape (src/machines.rs): a machine for this node with its rack recorded, and its feed so the console has it as answering."""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT, HOST = int(sys.argv[1]), sys.argv[2]
ROUTES = {"/api/v1/machines": {"machines": [{"tag": "T1", "name": "h1", "node": HOST, "location": {"rack": "rack-ipmi"}}], "default": None, "forge": None},
          "/api/v1/components": [{"id": "system", "kind": "system", "label": "stormipmi", "health": "ok", "detail": "1 machine", "metrics": [], "actions": [], "relations": []}],
          "/healthz": {"ok": True}}


class H(BaseHTTPRequestHandler):
    def do_GET(self):
        body = ROUTES.get(self.path.split("?")[0])
        self.send_response(200 if body is not None else 404)
        self.send_header("content-type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(body if body is not None else {}).encode())

    def log_message(self, *a):
        pass


ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
