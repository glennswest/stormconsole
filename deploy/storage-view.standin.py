#!/usr/bin/env python3
"""A stormdrive for deploy/verify-storage-view.sh (#96) with what a build VM has none of: a drive and an enclosure,
in stormdrive's src/api/kube.rs shapes, history and all, and its feed so the console's plugin has a node to read."""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
DRIVE = {"apiVersion": "storage.storm.io/v1", "kind": "Drive", "metadata": {"name": "d1", "uid": "d1", "labels": {"storm.io/component": "stormdrive"}},
         "spec": {"designation": "none", "fleet": "fleet", "drain": False},
         "status": {"node": "standin", "path": "/dev/sdz", "name": "sdz", "kind": "sas", "model": "H7210A521SUN010T", "serial": "ZC900", "capacityBytes": 10000000000000,
                    "blockSize": 4096, "enclosure": "5000a098aaaa0001", "bay": 7, "membership": "fleet", "activity": "idle",
                    "health": {"status": "warning", "temperature_c": 44, "wear_pct": 12}, "history": [{"secretly": "long history"}], "plannedFormat": {"blockSize": 4096}}}
ENCLOSURE = {"apiVersion": "storage.storm.io/v1", "kind": "Enclosure", "metadata": {"name": "5000a098aaaa0001", "uid": "e", "labels": {}}, "spec": {},
             "status": {"shelf": {"wwn": "5000a098aaaa0001"}, "hbas": ["mpt3sas0"], "driveCount": 24, "drives": [{"bay": 7}], "worstHealth": "warning", "needsReformat": 3,
                        "enclosure": {"status": "noncritical", "paths": 2, "maxTemperatureC": 51, "powerSupplies": {"ok": 1, "total": 2}, "fans": {"ok": 4, "total": 4}, "elements": []}}}
ROUTES = {"/apis/storage.storm.io/v1/drives": {"kind": "DriveList", "items": [DRIVE]},
          "/apis/storage.storm.io/v1/enclosures": {"kind": "EnclosureList", "items": [ENCLOSURE]},
          "/api/v1/components": [], "/api/v1/drives": []}


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
