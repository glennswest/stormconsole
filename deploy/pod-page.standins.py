#!/usr/bin/env python3
"""Stand-ins for deploy/verify-pod-page.sh (#69).

A kubelet on 127.0.0.1:10250 (TLS, bearer required) serving what
rustkube-node serves and nothing more:

  /containerLogs/{ns}/{pod}/{c}  ?previous ?tailLines ?follow ?timestamps
  /metrics/cadvisor              rx/tx bytes per pod interface, growing
  /stats/summary                 rustkube-node 7bfe4d2's shape (#124): per
                                 container cpu.usageCoreNanoSeconds (app at
                                 0.25 core) and memory.workingSetBytes; the
                                 pod's network with packets, errors, drops

and a stormcentral serving /api/v1/goldens?component= behind a bearer.

The current run's restart count is read from the file named by RCFILE, so
`previous` names the run that just ended, as the node's would.
"""
import json
import os
import ssl
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

CERT, KEY, RCFILE, SC_PORT, SC_TOKEN = sys.argv[1:6]
START = time.time()
seen_bearer = {"kubelet": 0, "kubelet_without": 0}


def restarts():
    try:
        return int(open(RCFILE).read().strip())
    except Exception:
        return 0


class Kubelet(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def send_text(self, code, text, ctype="text/plain"):
        body = text.encode()
        self.send_response(code)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        auth = self.headers.get("authorization", "")
        if not auth.startswith("Bearer "):
            seen_bearer["kubelet_without"] += 1
            return self.send_text(401, "Unauthorized")
        seen_bearer["kubelet"] += 1
        u = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query).items()}
        parts = u.path.strip("/").split("/")
        if u.path == "/metrics/cadvisor":
            t = time.time() - START
            rx = int(1_000_000 + t * 50_000)
            tx = int(200_000 + t * 8_000)
            lines = [
                "# HELP container_network_receive_bytes_total Cumulative count of bytes received.",
                "# TYPE container_network_receive_bytes_total counter",
                f'container_network_receive_bytes_total{{container="",id="sb1",interface="eth0",namespace="shop",pod="web-1"}} {rx}',
                f'container_network_receive_bytes_total{{container="",id="sb2",interface="eth0",namespace="shop",pod="other"}} 5',
                "# HELP container_network_transmit_bytes_total Cumulative count of bytes transmitted.",
                "# TYPE container_network_transmit_bytes_total counter",
                f'container_network_transmit_bytes_total{{container="",id="sb1",interface="eth0",namespace="shop",pod="web-1"}} {tx}',
            ]
            return self.send_text(200, "\n".join(lines) + "\n")
        if u.path == "/stats/summary":
            t = time.time() - START
            rx = int(1_000_000 + t * 50_000)
            iface = {"name": "eth0", "rxBytes": rx, "txBytes": int(200_000 + t * 8_000),
                     "rxPackets": int(t * 40), "txPackets": int(t * 10),
                     "rxErrors": 2, "txErrors": 0, "rxDropped": int(t // 20), "txDropped": 0}
            body = {"node": {"cpu": {"time": "2026-10-02T12:00:00Z", "usageCoreNanoSeconds": int(t * 2e9)},
                             "memory": {"workingSetBytes": 4 << 30}},
                    "pods": [
                        {"podRef": {"name": "web-1", "namespace": "shop"},
                         "containers": [
                             {"name": "app", "cpu": {"usageCoreNanoSeconds": int(5e9 + t * 0.25e9)},
                              "memory": {"workingSetBytes": 48 << 20}},
                             {"name": "agent", "cpu": {"usageCoreNanoSeconds": int(1e9 + t * 0.05e9)},
                              "memory": {"workingSetBytes": 96 << 20}}],
                         "network": {**iface, "interfaces": [iface]}},
                        {"podRef": {"name": "other", "namespace": "shop"},
                         "containers": [{"name": "x", "cpu": {"usageCoreNanoSeconds": 1}, "memory": {"workingSetBytes": 1}}]}]}
            return self.send_text(200, json.dumps(body), "application/json")
        if len(parts) == 4 and parts[0] == "containerLogs":
            _, ns, pod, c = parts
            rc = restarts()
            ts = q.get("timestamps") == "true"
            stamp = lambda i: f"2026-10-02T12:00:{i % 60:02d}.000000000Z " if ts else ""
            if q.get("previous") == "true":
                if rc == 0:
                    return self.send_text(400, json.dumps({"kind": "Status", "message": f'previous terminated container "{c}" in pod "{pod}" not found'}), "application/json")
                text = "".join(f"{stamp(i)}{c} run {rc - 1} line {i}\n" for i in range(20))
                text += f"{c} run {rc - 1}: panic: could not reach the database\n"
                return self.send_text(200, text)
            n = int(q.get("tailLines", "100"))
            lines = [f"{stamp(i)}{c} run {rc} line {i} GET /healthz 200" for i in range(n)]
            if q.get("follow") != "true":
                return self.send_text(200, "\n".join(lines) + "\n")
            self.send_response(200)
            self.send_header("content-type", "text/plain")
            self.send_header("transfer-encoding", "chunked")
            self.end_headers()

            def chunk(s):
                b = s.encode()
                self.wfile.write(f"{len(b):x}\r\n".encode() + b + b"\r\n")
                self.wfile.flush()

            try:
                chunk("\n".join(lines) + "\n")
                i = n
                while True:
                    time.sleep(0.5)
                    chunk(f"{stamp(i)}{c} run {rc} line {i} live\n")
                    i += 1
            except Exception:
                return
        return self.send_text(404, "not found")


class Central(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        u = urlparse(self.path)
        if self.headers.get("authorization", "") != f"Bearer {SC_TOKEN}":
            self.send_response(401)
            self.end_headers()
            return
        comp = parse_qs(u.query).get("component", [""])[0]
        goldens = []
        if u.path == "/api/v1/goldens" and comp == "cilium":
            goldens = [
                {"name": "golden-cilium-abc123def456", "component": "cilium", "version": "1.16.3",
                 "commit": "abc123def4567890", "build_id": "b-42", "built_at": 1790900000,
                 "built_by": "stormcentral", "tar_sha256": "e" * 64, "device_sha256": "f" * 64,
                 "sources": {"cilium": "abc123"}, "releases": ["11.6"]},
                {"name": "golden-cilium-older", "component": "cilium", "built_at": 1790000000},
            ]
        body = json.dumps({"goldens": goldens, "held": []}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


kubelet = ThreadingHTTPServer(("127.0.0.1", 10250), Kubelet)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(CERT, KEY)
kubelet.socket = ctx.wrap_socket(kubelet.socket, server_side=True)
central = ThreadingHTTPServer(("127.0.0.1", int(SC_PORT)), Central)
threading.Thread(target=central.serve_forever, daemon=True).start()
print("stand-ins up", flush=True)
kubelet.serve_forever()
