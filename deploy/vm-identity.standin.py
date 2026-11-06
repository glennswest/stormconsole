#!/usr/bin/env python3
"""A kubelet's /vmInstance/{address} for deploy/verify-vm-net.sh (#11), as rustkube-node's
vm_manager.rs and server.rs answer it: 200 with the JSON below, 404 for nobody, 503 not synced, 401 for a bad
bearer. A real kubelet needs PID 1's runtime. POST /_normal, /_another, /_cold, /_refuse, /_keys to change it.
"""
import json, ssl, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CERT, KEY, UID = sys.argv[1:4]
mode = "normal"


def machine(uid):
    return {"instance_id": uid, "hostname": "bridged-host", "local_ipv4": "192.168.8.61", "region": "storm", "zone": "storm-06f96d",
            "tags": {"namespace": "default", "name": "bridged"}, "labels": {"app": "web"},
            "network": {"interfaces": [{"device_index": 0, "mac": "52:54:00:aa:bb:cc", "ipv4_addresses": ["192.168.8.61"], "ipv6_addresses": ["fd00:8::61"]}]},
            "launched_at": 1793970000}


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def send_json(self, code, body):
        b = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def do_GET(self):
        if self.path == "/healthz":
            return self.send_json(200, {})
        if not self.path.startswith("/vmInstance/"):
            return self.send_json(404, {"error": "no such route"})
        if mode == "refuse":
            return self.send_json(401, {})
        if mode == "cold":
            return self.send_json(503, {"error": "this node has not synced yet"})
        addr = self.path.rsplit("/", 1)[1]
        if addr != "192.168.8.61":
            return self.send_json(404, {"error": f"no machine at {addr} on this node"})
        m = machine("someone-else" if mode == "another" else UID)
        if mode == "keys":
            m["public_keys"] = ["ssh-ed25519 AAAAsecret alice@laptop"]
            m["user_data"] = "#cloud-config\nhunter2\n"
        self.send_json(200, m)

    def do_POST(self):
        global mode
        mode = self.path.lstrip("/_")
        self.send_json(200, {"mode": mode})


s = ThreadingHTTPServer(("127.0.0.1", 10250), H)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(CERT, KEY)
s.socket = ctx.wrap_socket(s.socket, server_side=True)
s.serve_forever()
