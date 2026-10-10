#!/usr/bin/env python3
"""A stand-in flowsdn agent for deploy/verify-flowsdn.sh (#83).

Serves the agent's read-only TCP listener as flowsdn documents and codes it
(docs/agent-api.md; crates/flowsdn-agent/src/{api,health_api}.rs at 8fa0cc8):
HTTP/1.1 answers, one request per connection then close, 403 for anything
that would change state, 404 for an unknown route, the Kubernetes routes only
in Kubernetes mode. The endpoint list is read from a JSON file on every
request so the rig can change it under a running console. Every request is
logged to stdout as `METHOD PATH`, so the rig can say the console never wrote.

  flowsdn.standins.py <port> <k8s|standalone> <endpoints.json>
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT, MODE, EPFILE = int(sys.argv[1]), sys.argv[2], sys.argv[3]
K8S = MODE == "k8s"
NEVER = "0001-01-01T00:00:00Z"


def modules():
    rows = [
        ("api", "OK", "initial endpoint API listening", ""),
        ("restore", "OK", "endpoint restore and deletion replay completed", ""),
        ("controllers", "Degraded",
         "Kubernetes node discovery enabled; identity and policy controllers are not" if K8S
         else "Kubernetes, identity and policy controllers are not enabled", "not implemented"),
    ]
    return [{"ID": {"Module": ["agent"], "Component": [c]}, "Level": lvl, "Message": msg, "Error": err,
             "LastOK": "2026-10-10T10:00:00Z" if lvl == "OK" else NEVER,
             "Updated": "2026-10-10T10:00:00Z", "Stopped": NEVER, "Final": "", "Count": 1}
            for c, lvl, msg, err in rows]


def healthz():
    h = {"agent": {"state": "Ok", "msg": "initial endpoint API ready"}}
    if K8S:
        h["kubernetes"] = {"state": "Ok", "msg": "node-a: 2 nodes, 3 pods, 2 frontends", "node-name": "node-a",
                           "auto-direct-node-routes": True, "service-lb": True}
    return h


IPAM = {"pools": [
    {"pool": "default", "family": "ipv4", "cidr": "10.5.0.0/24", "capacity": "254", "allocated": "4",
     "excluded": "1", "allocated-excluded": "0", "available": "249"},
    {"pool": "default", "family": "ipv6", "cidr": "f00d::a05:0:0:0/64", "capacity": "18446744073709551614",
     "allocated": "3", "excluded": "0", "allocated-excluded": "0", "available": "18446744073709551611"},
]}
CONFIG = {"status": {"datapath-mode": "veth", "ipam-mode": "kubernetes", "device-mtu": 1500, "route-mtu": 1450,
                     "host-addressing": {"ipv4": {"enabled": True, "ip": "10.5.0.1"}}}}
SERVICES = [
    {"spec": {"id": 1, "frontend-address": {"ip": "10.96.0.1", "port": 443, "protocol": "TCP", "scope": "external"},
              "backend-addresses": [{"ip": "192.168.8.10", "port": 6443, "protocol": "TCP", "state": "active"}],
              "flags": {"type": "ClusterIP", "name": "kubernetes", "namespace": "default", "port-name": "https"}},
     "status": {"realized": {"id": 1}}},
    {"spec": {"id": 0, "frontend-address": {"ip": "10.96.44.7", "port": 80, "protocol": "TCP", "scope": "external"},
              "backend-addresses": [{"ip": "10.5.0.7", "port": 8080, "protocol": "TCP", "state": "active"}],
              "flags": {"type": "NodePort", "name": "web", "namespace": "shop"}}},
]
ROUTES = [{"destination": "10.6.0.0/16", "gateway": "192.168.8.11", "node": "node-b", "state": "installed"},
          {"destination": "10.7.0.0/16", "gateway": "192.168.8.12", "node": "node-c",
           "state": "skipped: gateway not on a directly connected network"}]
IDENTITIES = [{"id": 31337, "labels": ["k8s:app=web", "k8s:io.kubernetes.pod.namespace=shop"]}]


def endpoints():
    with open(EPFILE) as f:
        return json.load(f)


class Agent(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def answer(self, status, body, raw=False):
        data = body.encode() if raw else json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(data)
        self.close_connection = True

    def route(self, method):
        print(f"{method} {self.path}", flush=True)
        path = self.path.split("?")[0]
        if path in ("/v1/statedb/query", "/statedb/query") and method in ("GET", "POST"):
            n = int(self.headers.get("Content-Length") or 0)
            q = json.loads(self.rfile.read(n) or b"{}")
            if q.get("table") != "health":
                return self.answer(404, {"code": 404, "message": "only the health table is exposed"})
            lines = "".join(json.dumps({"rev": i + 1, "obj": m}) + "\n" for i, m in enumerate(modules()))
            return self.answer(200, lines, raw=True)
        if method != "GET":
            return self.answer(403, {"code": 403, "message": "the TCP listener is read-only; use the Unix socket"})
        eps = endpoints()
        if path == "/v1/healthz":
            return self.answer(200, healthz())
        if path in ("/v1/health/modules", "/health/modules"):
            return self.answer(200, modules())
        if path == "/v1/endpoint":
            return self.answer(200, eps)
        if path.startswith("/v1/endpoint/"):
            rest = path[len("/v1/endpoint/"):]
            ident, _, tail = rest.partition("/")
            from urllib.parse import unquote
            ident = unquote(ident)
            ep = next((e for e in eps if str(e["id"]) == ident
                       or e["status"].get("external-identifiers", {}).get("cni-attachment-id") == ident), None)
            if not ep:
                return self.answer(404, {"code": 404, "message": "endpoint not found"})
            if tail == "healthz":
                ok = ep["status"]["state"] == "ready"
                return self.answer(200, {"overallHealth": "OK" if ok else "Failure", "bpf": "OK" if ok else "Failure",
                                         "policy": "Disabled", "connected": ok})
            return self.answer(200, ep)
        if path == "/v1/ipam":
            return self.answer(200, IPAM)
        if path == "/v1/config":
            return self.answer(200, CONFIG)
        if path in ("/v1/ip", "/v1/identity", "/v1/service", "/v1/node/routes"):
            if not K8S:
                return self.answer(404, {"code": 404, "message": "kubernetes node discovery is not enabled"})
            return self.answer(200, {"/v1/service": SERVICES, "/v1/node/routes": ROUTES,
                                     "/v1/identity": IDENTITIES, "/v1/ip": []}[path])
        return self.answer(404, {"code": 404, "message": "not found"})

    def do_GET(self):
        self.route("GET")

    def do_POST(self):
        self.route("POST")

    def do_PUT(self):
        self.route("PUT")

    def do_DELETE(self):
        self.route("DELETE")


# One request at a time, as the agent's accept loop does.
HTTPServer(("127.0.0.1", PORT), Agent).serve_forever()
