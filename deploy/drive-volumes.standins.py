#!/usr/bin/env python3
"""Stand-ins for deploy/verify-drive-volumes.sh (#52). Three stormdrives in stormdrive's src/usage.rs shape: this node (:19301), n2 (:19302) with volumes, an empty drive
and an answer gone stale, and n3 (:19303) whose engine reports no placement, so `volumes` is absent. An engine (:19304) with placement for this node's drives."""
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CAP = 10 ** 13


def consumer(kind, ns, name):
    return {"kind": kind, "namespace": ns, "name": name}


def vol(id, name, c=None, bytes=4 * 2 ** 30, state="ok", rebuild="none"):
    v = {"id": id, "name": name, "kind": "volume", "bytes": bytes, "legs": 64, "shared_legs": 0, "state": state, "rebuild": rebuild, "policy": "mirror2", "health": "healthy"}
    if c:
        v["consumer"] = c
    return v


NOW = lambda: {"secs_since_epoch": int(time.time()), "nanos_since_epoch": 0}
OLD = lambda: {"secs_since_epoch": int(time.time()) - 3600, "nanos_since_epoch": 0}
DRIVES = {
    19301: [("HERE-1", [vol("v-local", "pvc-local", consumer("PersistentVolumeClaim", "shop", "db"))], NOW), ("HERE-2", None, None)],
    19302: [("FAR-1", [vol("v-far", "pvc-far", consumer("PersistentVolumeClaim", "shop", "db"), state="draining", rebuild="running"),
                       vol("v-sec", "vm-sec", consumer("VirtualMachineInstance", "secret", "vault"), bytes=2 ** 30),
                       vol("v-free", "scratch", bytes=2 ** 20)], NOW),
            ("FAR-EMPTY", [], NOW), ("FAR-STALE", [vol("v-old", "pvc-old")], OLD)],
    19303: [("OLD-1", None, None)],
}


def record(i, serial, vols, at):
    usage = {"capacity_bytes": CAP, "in_slabs_bytes": CAP, "used_bytes": 10 ** 12, "free_in_slabs_bytes": CAP - 10 ** 12, "outside_slabs_bytes": 0, "free_bytes": CAP - 10 ** 12,
             "promisable_bytes": CAP, "slabs": [{"id": f"slab-{serial}", "role": "data", "tier": "hot", "total_bytes": CAP, "allocated_bytes": 10 ** 12, "free_bytes": CAP - 10 ** 12}]}
    if vols is not None:
        usage["volumes"] = vols
        usage["volumes_collected_at"] = at()
    return {"id": str(i), "path": f"/dev/sd{i}", "serial": serial, "wwid": f"naa.{serial}", "capacity_bytes": CAP, "usage": usage}


def card(i, serial):
    return {"id": f"drive:{i}", "kind": "drive", "label": serial, "health": "ok", "detail": "ok", "actions": [], "relations": [],
            "metrics": [{"label": "serial", "value": serial}, {"label": "bay", "value": str(i)}, {"label": "hba", "value": "HBA0"}, {"label": "capacity", "value": "9.1 TB"}]}


# The engine's placement, in stormblock's shape: HERE-1's volume with one slab draining, and HERE-2's, which only the engine can say.
def placed(id, name, serial, slab_state):
    return {"id": id, "name": name, "kind": "volume", "in_use": True, "consumer": consumer("Pod", "shop", "from-the-engine"),
            "placement": {"rebuild": "none", "legs": {"policy": "mirror2", "health": "healthy"},
                          "drives": [{"drive": {"serial": serial}, "slabs": 1, "legs": 8, "bytes": 2 ** 30}],
                          "slabs": [{"id": f"slab-{serial}", "drive": {"serial": serial}, "state": slab_state, "legs": 8, "shared_legs": 0, "bytes": 2 ** 30}]}}


ENGINE = [placed("v-local", "pvc-local", "HERE-1", "draining"), placed("v-engine", "engine-says", "HERE-2", "ok")]


def serve(port):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_GET(self):
            path = self.path.split("?")[0]
            if port == 19304:
                body = {"items": ENGINE} if path == "/api/v1/volumes" else {"items": []}
            elif path == "/api/v1/components":
                body = [card(i, d[0]) for i, d in enumerate(DRIVES[port])]
            elif path == "/api/v1/drives":
                body = {"drives": [record(i, *d) for i, d in enumerate(DRIVES[port])]}
            else:
                body = {}
            data = json.dumps(body).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

    ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()


for p in [19301, 19302, 19303, 19304]:
    threading.Thread(target=serve, args=(p,), daemon=True).start()
print("stand-ins up", flush=True)
while True:
    time.sleep(60)
