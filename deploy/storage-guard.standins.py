#!/usr/bin/env python3
"""Stand-ins for deploy/verify-storage-guard.sh (#82).

Two stormdrives (this node's and storm-b's) and an engine. Each serves just
enough for the console's plugins to put a drive and a volume in the feed,
in the shapes stormdrive's `components.rs` and stormblock's
`/api/v1/volumes` give, and **records every write with the bearer it
carried**, so the check can say whose identity reached the component: the
person's, the console's, or the node's engine token.

    storage-guard.standins.py <drive-port> <drive-b-port> <engine-port> <engine-token>

GET /_writes on any of them returns what it recorded.
"""
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

DRIVE_PORT, DRIVE_B_PORT, ENGINE_PORT = (int(p) for p in sys.argv[1:4])
ENGINE_TOKEN = sys.argv[4]


def act(id, label, method, path, danger=False):
    return {"id": id, "label": label, "method": method, "path": path,
            "enabled": True, "danger": danger}


def drive(uuid, name, serial):
    base = f"/api/v1/drives/{uuid}"
    return {
        "id": f"drive:{uuid}", "kind": "drive", "label": f"{name} · ST4000NM",
        "health": "ok", "detail": f"{name} · 4.0 TB · bay 3",
        "metrics": [{"label": "serial", "value": serial, "tone": "muted"},
                    {"label": "bay", "value": "3"}],
        "actions": [
            act("locate-on", "Locate", "POST", f"{base}/locate/on"),
            act("test-destructive", "Destructive test", "POST", f"{base}/test/destructive_sample", True),
            act("format-4k", "Format 4K", "POST", f"{base}/format/4096", True),
        ],
        "relations": [], "link": None,
    }


FEEDS = {
    DRIVE_PORT: [drive("7f3a0000-0000-4000-8000-000000000001", "sdb", "ZC1234")],
    DRIVE_B_PORT: [drive("7f3a0000-0000-4000-8000-0000000000b2", "sdc", "ZB5678")],
}

VOLUMES = [{"id": "vol-9a1", "name": "scratch", "kind": "volume", "in_use": False,
            "size": 10737418240, "allocated": 1073741824, "health": "healthy"}]


def handler_for(port):
    writes = []
    lock = threading.Lock()

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

        def bearer(self):
            a = self.headers.get("authorization") or ""
            return a[7:] if a.startswith("Bearer ") else None

        def do_GET(self):
            path = self.path.split("?")[0]
            if path == "/_writes":
                with lock:
                    return self.send(200, list(writes))
            if port == ENGINE_PORT:
                # The engine's reads take its node token, as since #107.
                if self.bearer() != ENGINE_TOKEN:
                    return self.send(401, {"error": "missing or invalid bearer token"})
                if path == "/api/v1/volumes":
                    return self.send(200, {"items": VOLUMES})
                if path.startswith("/api/v1/"):
                    return self.send(200, {"items": []})
                return self.send(404, {"error": "no such path"})
            if path == "/api/v1/components":
                return self.send(200, FEEDS[port])
            if path == "/api/v1/drives":
                return self.send(200, {"drives": []})
            return self.send(404, {"error": "no such path"})

        def write(self):
            n = int(self.headers.get("content-length") or 0)
            if n:
                self.rfile.read(n)
            with lock:
                writes.append({"method": self.command, "path": self.path, "bearer": self.bearer()})
            return self.send(200, {"message": f"{self.command} {self.path.split('?')[0]} accepted"})

        do_POST = do_PUT = do_DELETE = write

    return H


servers = [ThreadingHTTPServer(("127.0.0.1", p), handler_for(p)) for p in (DRIVE_PORT, DRIVE_B_PORT, ENGINE_PORT)]
for s in servers[1:]:
    threading.Thread(target=s.serve_forever, daemon=True).start()
servers[0].serve_forever()
