#!/usr/bin/env python3
"""One node's exporters for deploy/verify-remote-charts.sh (#369), in the series and labels the console's
panels read (crates/plugins/metrics/src/panels.rs): a drive (stormdrive's SMART), a host port (stormport),
machines (stormvm, KubeVirt's names) and a pod (the kubelet's cAdvisor). Counters move.

    remote-charts.standin.py <addr> <port> <node> <serial> <ifname> [<ns>/<vm> ...]
"""
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ADDR, PORT, NODE, SERIAL, IFNAME = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4], sys.argv[5]
VMS = [a.split("/", 1) for a in sys.argv[6:]]
START = time.time()


def body():
    up = time.time() - START
    out = [
        f'smartctl_device_temperature{{device="sdb",serial="{SERIAL}",model="ST",enclosure="",bay="3",temperature_type="current"}} 41',
        f'storm_port_link_up{{node="{NODE}",ifname="{IFNAME}"}} 1',
    ]
    for ns, name in VMS:
        out.append(f'kubevirt_vmi_vcpu_seconds_total{{namespace="{ns}",name="{name}",id="0"}} {up * 0.5}')
        out.append(f'container_cpu_usage_seconds_total{{namespace="{ns}",pod="web-{name}",container="app"}} {up * 0.25}')
    return "\n".join(out) + "\n"


class H(BaseHTTPRequestHandler):
    def do_GET(self):
        b = body().encode()
        self.send_response(200 if self.path == "/metrics" else 404)
        self.send_header("content-type", "text/plain; version=0.0.4")
        self.send_header("content-length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def log_message(self, *a):
        pass


ThreadingHTTPServer((ADDR, PORT), H).serve_forever()
