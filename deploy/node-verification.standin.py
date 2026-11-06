#!/usr/bin/env python3
"""Another node's sectionsystems: GET /api/v1/status from a file, in the shape of its docs/api.md (#74)."""
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

addr, path = sys.argv[1], sys.argv[2]

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def do_GET(self):
        body, code = b'{"error":"no such route"}', 404
        if self.path == '/api/v1/status':
            body, code = open(path, 'rb').read(), 200
        elif self.path == '/api/v1/components':
            body, code = json.dumps([{"id": "verification:x", "kind": "verification", "label": "verification", "health": "ok",
                                      "detail": "stand-in", "metrics": [], "actions": [], "relations": []}]).encode(), 200
        elif self.path == '/healthz':
            body, code = b'{"ok":true}', 200
        self.send_response(code)
        self.send_header('content-type', 'application/json')
        self.send_header('content-length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

HTTPServer((addr, 9103), H).serve_forever()
