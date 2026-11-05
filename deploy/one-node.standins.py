#!/usr/bin/env python3
"""Stand-ins for deploy/verify-one-node.sh (#161).

  one-node.standins.py <host> <log port> <release file>

A stormdrive on every address at :9092 (5 drives), a stormstorage at :19193
and a stormcluster at :19102 (plain http), in those daemons' feed shapes; and
the segment: this node and other clusters' machines sending lines and
beacons to the console's collector, each from an address of its own on lo.
"""
import json
import socket
import sys
import threading
import time
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HOST, LOG_PORT, RELEASE = sys.argv[1], int(sys.argv[2]), sys.argv[3]


def card(id, kind, label, detail="", health="ok", metrics=None):
    return {
        "id": id, "kind": kind, "label": label, "health": health, "detail": detail,
        "metrics": [{"label": k, "value": v} for k, v in (metrics or {}).items()],
        "actions": [], "relations": [],
    }


DRIVES = [card(f"drive:SN{i}", "drive", f"SN{i}", "ok", metrics={"bay": str(i), "hba": "PERC H730"}) for i in range(5)]
STORAGE = [
    card("system", "system", "stormstorage", "1 node"),
    card(f"node:{HOST}", "node", HOST, "http://127.0.0.1:9090 · reachable",
         metrics={"free": "8.9 TB", "total": "8.9 TB", "vols": "206"}),
]
free = {"address": "127.0.0.51", "release": "12.61-flowsdn", "network": "cilium", "cores": "8"}
CLUSTER = [
    card("system", "cluster", f"{HOST} (SNO)", f"{HOST} is a single-node cluster; 5 peer(s) discovered", "idle", {"peers": "5"}),
    card(f"peer:{HOST}", "peer", HOST, "single-node cluster, available", "idle",
         {"address": "127.0.0.2", "release": "12.61", "network": "cilium", "cores": "48"}),
    card("peer:free-1", "peer", "free-1", "single-node cluster, available", "idle", free),
    card("peer:gone-1", "peer", "gone-1", "single-node cluster, available", "error", {"address": "127.0.0.52"}),
    card("peer:taken-1", "peer", "taken-1", "worker in other", "unknown", {"address": "127.0.0.53"}),
    card("peer:laptop-1", "peer", "laptop-1", "laptop: a single-node cluster only, never joined", "unknown", {"address": "127.0.0.54"}),
    card("peer:quiet-1", "peer", "quiet-1", "beacon only: stormcluster is not running there", "unknown", {"address": "127.0.0.55"}),
]


def server(port, routes, bind="127.0.0.1"):
    class H(BaseHTTPRequestHandler):
        def do_GET(self):
            body = routes.get(self.path.split("?")[0])
            self.send_response(200 if body is not None else 404)
            self.send_header("content-type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(body if body is not None else {"error": "no"}).encode())

        def log_message(self, *a):
            pass

    ThreadingHTTPServer((bind, port), H).serve_forever()


for args in [
    (9092, {"/api/v1/components": DRIVES, "/api/v1/drives": []}, "0.0.0.0"),
    (19193, {"/api/v1/components": STORAGE}),
    (19102, {"/api/v1/components": CLUSTER, "/api/v1/self": {"node": HOST}}),
]:
    threading.Thread(target=server, args=args, daemon=True).start()


def line(addr, host, app, msg, sev=6, beacon=None):
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")
    sd = "-"
    if beacon:
        sd = "[storm-beacon@0 " + " ".join(f'{k}="{v}"' for k, v in beacon.items()) + "]"
    text = f"<{8 + sev}>1 {now} {host} {app} - - {sd} {msg}"
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind((addr, 0))
    s.sendto(text.encode(), ("127.0.0.1", LOG_PORT))
    s.close()


while True:
    release = open(RELEASE).read().strip()
    # This node, also under a name from before its hostname was set.
    line("127.0.0.2", HOST, "beacon", "node beacon", beacon={"cores": "48", "drives": "5", "release": release})
    line("127.0.0.2", HOST, "kubelet", "mine: this node is fine")
    line("127.0.0.2", "forge1-old", "kubelet", "mine: under my old name")
    # Other clusters' machines, one of them failing and heard under two names.
    line("127.0.0.31", "storm-003101", "beacon", "node beacon", beacon={"cores": "8", "drives": "100", "release": "12.61"})
    line("127.0.0.31", "pvetest1", "kubelet", "theirs: pvetest1", sev=3)
    line("127.0.0.32", "storm-003102", "beacon", "node beacon", beacon={"drives": "18", "release": "12.61-flowsdn"})
    line("127.0.0.32", "storm-003102", "theirapp", "theirs: storm-003102", sev=2)
    # A machine that joins this cluster partway through.
    line("127.0.0.41", "member2", "kubelet", "member2 here")
    line("127.0.0.41", "member2-old", "kubelet", "member2 under an old name")
    time.sleep(2)
