#!/usr/bin/env python3
"""Stand-ins for what stormcluster calls on each node, for
deploy/verify-cluster.sh (#63).

  cluster.standins.py b1=127.0.0.11 b2=127.0.0.12 b3=127.0.0.13

Per node, on that node's address:

- :19500, the node lifecycle API (stormcos#38, as stormcluster's
  docs/api.md proposes it): `GET /api/v1/cluster` -> {role, caPin,
  clusterName, endpoint, blockers}; `POST /api/v1/cluster/seed|join|promote|
  demote|leave` change the role.
- :23790, fastetcd's v3 gateway, as much as stormcluster reads:
  `/v3/cluster/member/list`, `/v3/maintenance/status`, `GET /health`. A
  seed or promote makes the node a voter; demote or leave removes it.

Every request is logged to stdout, so the script can show what stormcluster
actually did to a node.
"""
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

NODE_PORT, ETCD_PORT = 19500, 23790
NODES = dict(a.split("=", 1) for a in sys.argv[1:])
lock = threading.Lock()
state = {n: {"role": "sno", "clusterName": None, "endpoint": None} for n in NODES}
voters = {}  # node -> member id


def member(n):
    a = NODES[n]
    return {"ID": str(voters[n]), "name": n, "peerURLs": [f"http://{a}:2380"],
            "clientURLs": [f"http://{a}:{ETCD_PORT}"], "isLearner": False}


def make(node, kind):
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

        def body(self):
            n = int(self.headers.get("content-length") or 0)
            try:
                return json.loads(self.rfile.read(n) or b"{}")
            except ValueError:
                return {}

        def do_GET(self):
            print(f"{kind} {node} GET {self.path}", flush=True)
            if kind == "node" and self.path == "/api/v1/cluster":
                with lock:
                    s = state[node]
                    return self.send(200, {"role": s["role"], "caPin": f"sha256:{node}",
                                           "clusterName": s["clusterName"], "endpoint": s["endpoint"],
                                           "blockers": []})
            if kind == "etcd" and self.path == "/health":
                return self.send(200, {"health": "true"})
            self.send(404, {"error": f"{self.path} is not served here"})

        def do_POST(self):
            b = self.body()
            print(f"{kind} {node} POST {self.path} {json.dumps(b)}", flush=True)
            with lock:
                if kind == "node":
                    s = state[node]
                    verb = self.path.rsplit("/", 1)[-1]
                    if verb == "seed":
                        s.update(role="master", clusterName=b.get("clusterName"), endpoint=b.get("endpoint"))
                        voters.setdefault(node, len(voters) + 1)
                    elif verb == "join":
                        s.update(role=b.get("role", "worker"), clusterName=b.get("clusterName"), endpoint=b.get("endpoint"))
                    elif verb == "promote":
                        s["role"] = "master"
                        voters.setdefault(node, len(voters) + 1)
                    elif verb == "demote":
                        s["role"] = "worker"
                        voters.pop(node, None)
                    elif verb == "leave":
                        s.update(role="sno", clusterName=None, endpoint=None)
                        voters.pop(node, None)
                    else:
                        return self.send(404, {"error": f"no verb {verb}"})
                    return self.send(200, {})
                if self.path == "/v3/cluster/member/list":
                    return self.send(200, {"members": [member(n) for n in voters]})
                if self.path == "/v3/maintenance/status":
                    lead = min(voters.values()) if voters else 0
                    return self.send(200, {"header": {"member_id": str(voters.get(node, 0))}, "leader": str(lead)})
            self.send(404, {"error": f"{self.path} is not served here"})

    return H


servers = []
for n, a in NODES.items():
    for kind, port in (("node", NODE_PORT), ("etcd", ETCD_PORT)):
        s = ThreadingHTTPServer((a, port), make(n, kind))
        threading.Thread(target=s.serve_forever, daemon=True).start()
        servers.append(s)
print("stand-ins up:", ", ".join(f"{n}@{a}" for n, a in NODES.items()), flush=True)
threading.Event().wait()
