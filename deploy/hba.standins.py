#!/usr/bin/env python3
"""Stand-in for deploy/verify-hba.sh (#376): forge1's stormdrive, in v0.56.0's shapes.

What the console reads from a stormdrive for the SAS cards page and the Flash dialog (the HostBusAdapter and
CardOperation objects themselves are in the real apiserver, as stormdrive writes them there):
  /api/v1/components           the drive cards for sdb and sdc (src/components.rs), so the Drives page has rows
  /api/v1/drives               the same drives (src/api/mod.rs)
  /apis/storage.storm.io/v1/drives  their Drive objects with status.link (#149), for each drive's link level
  /api/v1/firmware/images      the node's firmware store (src/firmware.rs `Image`): name, size, sha256
Nothing is written here: a flash is the apiserver's object and stormdrive's controller, not this API.

`hba.standins.py PORT empty` is a node with no drives and an empty store (the console's own node in the rig, so
forge1 is read as the named node it is in a cluster)."""
import hashlib
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
EMPTY = sys.argv[2:] == ["empty"]
IMAGE = b"MPI-FW-16.17.01.00" * 4096
IMAGES = [
    {"name": "hba330-16.17.01.00.bin", "size": len(IMAGE), "sha256": hashlib.sha256(IMAGE).hexdigest(), "modified": None},
    {"name": "mptsas3-uefi-18.00.00.00.rom", "size": 4096, "sha256": hashlib.sha256(b"rom").hexdigest(), "modified": None},
]
TB = 4 * 10**12


def drive(id, name, serial, bay, level):
    return {"id": id, "name": name, "serial": serial, "vendor": "HGST", "model": "HUS726T4TAL5204", "firmware": "C925",
            "wwid": f"naa.{id}", "paths": [f"/dev/{name}"], "capacity_bytes": TB, "block_size": 4096, "kind": "sas_hdd",
            "membership": "out", "activity": "idle", "designation": "none", "in_use_by": None, "health": {"status": "healthy"},
            "location": {"bay": bay, "controller": {"pcie_addr": "0000:01:00.0", "scsi_host": "host0"}}, "_level": level}


DRIVES = [] if EMPTY else [drive("wwn-5000cca0", "sdb", "K4G0001", 0, "ok"), drive("wwn-5000cca1", "sdc", "K4G0002", 1, "degraded")]
if EMPTY:
    IMAGES = []


def card(d):
    return {"id": f"drive:{d['id']}", "kind": "drive", "label": f"{d['name']} · {d['model']}", "health": "ok",
            "detail": "out of fleet", "relations": [], "actions": [],
            "metrics": [{"label": "serial", "value": d["serial"]}, {"label": "bay", "value": str(d["location"]["bay"])},
                        {"label": "capacity", "value": "4.0 TB"}, {"label": "hba", "value": "host0"},
                        {"label": "dev", "value": d["paths"][0]}]}


def obj(d):
    return {"apiVersion": "storage.storm.io/v1", "kind": "Drive", "metadata": {"name": d["id"], "labels": {"storm.io/node": "forge1"}},
            "status": {"node": "forge1", "name": d["name"], "serial": d["serial"], "bay": d["location"]["bay"],
                       "link": {"level": d["_level"], "message": f"{d['name']}: {d['_level']}", "hostPhy": "phy-0:0"}}}


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

    def do_GET(self):
        path = self.path.split("?")[0]
        if path in ("/healthz", "/api/v1/health"):
            return self.send(200, {"status": "ok", "node": "forge1"})
        if path == "/api/v1/components":
            return self.send(200, [card(d) for d in DRIVES])
        if path == "/api/v1/drives":
            return self.send(200, {"drives": [{k: v for k, v in d.items() if not k.startswith("_")} for d in DRIVES]})
        if path == "/apis/storage.storm.io/v1/drives":
            return self.send(200, {"apiVersion": "storage.storm.io/v1", "kind": "DriveList", "items": [obj(d) for d in DRIVES]})
        if path == "/api/v1/firmware/images":
            return self.send(200, {"images": IMAGES})
        return self.send(404, {"error": "no such path"})


ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
