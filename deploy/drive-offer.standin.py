#!/usr/bin/env python3
"""A stand-in stormdrive for deploy/verify-drive-offer.sh: its feed lists the
engine's file-backed drives by path, so the Drives page's join and filter have a
stormdrive record to work on. argv: port, path…"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port = int(sys.argv[1])
paths = sys.argv[2:]


def card(i, p):
    return {
        "id": f"drive:d{i}", "kind": "drive", "label": p.rsplit("/", 1)[-1], "health": "ok",
        "detail": f"4 GiB · bay {i}",
        "metrics": [{"label": "capacity", "value": "4 GiB"}, {"label": "dev", "value": p}, {"label": "bay", "value": str(i)}],
        "actions": [], "relations": [], "link": None,
    }


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        if self.path.split("?")[0] == "/api/v1/components":
            body, code = json.dumps([card(i, p) for i, p in enumerate(paths)]).encode(), 200
        elif self.path == "/healthz":
            body, code = b"ok", 200
        else:
            body, code = b"{}", 404
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


HTTPServer(("127.0.0.1", port), H).serve_forever()
