#!/usr/bin/env python3
"""Stand-ins for the words check in deploy/verify-pod-page.sh (#70).

sbregistry, the stormblock engine and the VM image operator, each serving
fixed answers in the shapes the plugins read. Every *name* here avoids
the word "golden", so any "golden" a page shows is the console's wording,
not data — while every *kind* and API field keeps it, as the real
components send it.
"""
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

REG, SB, IMG = (int(p) for p in sys.argv[1:4])
D = "sha256:" + "a" * 64

REGISTRY = {
    "/readyz": {"ready": True, "warmup": {"complete": True, "done": 3, "total": 3, "failed": 0}},
    "/v1/goldens": {"items": [{"name": "nats-2-10-sealed", "image": "library/nats:2.10", "digest": D,
                               "template_name": "tpl-nats", "verified": True}]},
    "/v1/clones": {"items": [{"id": "c1", "volume_name": "pvc-nats-data", "volume_id": "v9",
                              "golden": "nats-2-10-sealed", "template": "tpl-nats"}]},
    "/v1/pallets": {"items": []},
    "/v1/images": {"items": []},
    "/v1/media/jobs": {"items": []},
    "/v1/catalog/images": {"items": [
        {"name": "img-nats-root", "kind": "golden", "sealed": True, "source": "library/nats:2.10",
         "digest": D, "size": 1073741824, "location": "local", "clones": 1, "clone_names": ["pvc-nats-data"]},
        {"name": "stormlb-component", "kind": "component", "component": "stormlb", "location": "forge", "sealed": True},
        {"name": "slab-part-a", "kind": "slab_golden", "location": "local", "sealed": True},
        {"name": "blank-1g", "kind": "blank", "location": "local", "sealed": True},
    ]},
}
ENGINE = {
    "/api/v1/volumes": {"items": [
        {"id": "v1", "name": "nats-root-sealed", "kind": "golden", "sealed": True,
         "virtual_size_human": "1.0 GiB", "allocated_human": "300 MiB", "in_use": False},
        {"id": "v9", "name": "pvc-nats-data", "kind": "volume", "parent": "v1",
         "virtual_size_human": "1.0 GiB", "allocated_human": "10 MiB", "in_use": True,
         "consumer": {"kind": "PersistentVolumeClaim", "namespace": "shop", "name": "nats-data"}},
    ]},
    "/api/v1/slabs": {"items": []},
    "/api/v1/arrays": {"items": []},
    "/api/v1/exports": {"items": []},
    "/api/v1/drives": {"items": []},
    "/api/v1/slabs/pool": {"items": []},
}
OPERATOR = {
    "/api/v1/version": {"version": "0.9.0", "building": []},
    "/api/v1/catalog": {"items": [
        {"reference": "rocky:10", "distro": "rocky", "version": "10", "arch": "x86_64", "format": "qcow2",
         "provisioning": "cloud-init", "source": "builtin"},
        {"reference": "debian:13", "distro": "debian", "version": "13", "arch": "x86_64", "format": "qcow2",
         "provisioning": "cloud-init", "source": "builtin"},
    ]},
    "/api/v1/images": {"items": [
        {"name": "rocky-10-x86-64", "metadata": {"name": "rocky-10-x86-64"}, "spec": {"reference": "rocky:10"},
         "status": {"phase": "Available", "golden": "media-55797d0c4e57", "localName": "rocky-10-x86_64",
                    "repository": "media/rocky"}},
    ]},
    "/api/v1/local": {"items": []},
    "/api/v1/nodes": {"items": []},
}


def handler(table):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_GET(self):
            body = table.get(urlparse(self.path).path)
            code = 200 if body is not None else 404
            out = json.dumps(body if body is not None else {"error": "not found"}).encode()
            self.send_response(code)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(out)))
            self.end_headers()
            self.wfile.write(out)

    return H


for port, table in ((REG, REGISTRY), (SB, ENGINE), (IMG, OPERATOR)):
    s = ThreadingHTTPServer(("127.0.0.1", port), handler(table))
    threading.Thread(target=s.serve_forever, daemon=True).start()
print("word stand-ins up", flush=True)
threading.Event().wait()
