#!/usr/bin/env python3
"""Stand-ins for deploy/verify-metrics.sh (#46): the exporters with no drive, kubelet or guest on a
build VM, each serving /metrics in that exporter's series and labels (read from its code), with
counters that move. Also stormdrive's feed, so the Drives page has a drive to link from."""
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

START = time.time()


def up():
    return time.time() - START


def drive():
    out = []
    for i, (dev, serial) in enumerate([("sda", "ZC100"), ("sdb", "ZC101")]):
        l = f'device="{dev}",serial="{serial}",model="ST",enclosure="",bay="{i}"'
        out.append(f'smartctl_device_temperature{{{l},temperature_type="current"}} {35 + i}')
        out.append(f"smartctl_device_percentage_used{{{l}}} {7 + i}")
        out.append(f"stormdrive_drive_io_errors_total{{{l}}} {up() * i}")
    return out


def engine():
    out = []
    for op, per in (("read", 40.0), ("write", 10.0)):
        n = up() * per
        for le, f in (("0.001", 0.2), ("0.01", 0.9), ("0.1", 1.0), ("+Inf", 1.0)):
            out.append(f'stormblock_nvmeof_io_seconds_bucket{{op="{op}",le="{le}"}} {n * f}')
        out.append(f'stormblock_nvmeof_io_seconds_count{{op="{op}"}} {n}')
        out.append(f'stormblock_nvmeof_io_seconds_sum{{op="{op}"}} {n * 0.004}')
    return out


def kubelet():
    out = [f'container_cpu_usage_seconds_total{{container="",id="/",namespace="",pod=""}} {up() * 4}']
    for ns, pod, ctr, cores, mem in (("shop", "web", "app", 0.25, 48 << 20), ("shop", "web", "agent", 0.05, 8 << 20), ("secret", "vault", "app", 0.5, 96 << 20)):
        l = f'container="{ctr}",id="x-{pod}-{ctr}",namespace="{ns}",pod="{pod}"'
        out.append(f"container_cpu_usage_seconds_total{{{l}}} {up() * cores}")
        out.append(f"container_memory_working_set_bytes{{{l}}} {mem}")
    for ns, pod in (("shop", "web"), ("secret", "vault")):
        l = f'container="",id="s-{pod}",interface="eth0",namespace="{ns}",pod="{pod}"'
        out.append(f"container_network_receive_bytes_total{{{l}}} {up() * 50000}")
        out.append(f"container_network_transmit_bytes_total{{{l}}} {up() * 20000}")
    return out


def vm():
    out = []
    for ns, name in (("shop", "vm1"), ("secret", "vm2")):
        b = f'namespace="{ns}",name="{name}",uid="u-{name}"'
        out.append(f'kubevirt_vmi_vcpu_seconds_total{{{b},id="0",state="running"}} {up() * 0.5}')
        out.append(f"kubevirt_vmi_memory_resident_bytes{{{b}}} {512 << 20}")
        out.append(f"kubevirt_vmi_memory_domain_bytes{{{b}}} {4096 << 20}")
        out.append(f"kubevirt_vmi_memory_actual_balloon_bytes{{{b}}} {2048 << 20}")
        out.append(f'kubevirt_vmi_vcpu_delay_seconds_total{{{b},id="0"}} {up() * 0.1}')
        out.append(f'kubevirt_vmi_storage_iops_read_total{{{b},drive="root"}} {up() * 4}')
        out.append(f'kubevirt_vmi_storage_iops_write_total{{{b},drive="root"}} {up() * 8}')
        out.append(f'kubevirt_vmi_network_receive_packets_total{{{b},interface="default"}} {up() * 10}')
        out.append(f'kubevirt_vmi_network_transmit_packets_total{{{b},interface="default"}} {up() * 5}')
        out.append(f'kubevirt_vmi_storage_read_traffic_bytes_total{{{b},drive="root"}} {up() * 4096}')
        out.append(f'kubevirt_vmi_storage_write_traffic_bytes_total{{{b},drive="root"}} {up() * 8192}')
        out.append(f'kubevirt_vmi_network_receive_bytes_total{{{b},interface="default"}} {up() * 1000}')
        out.append(f'kubevirt_vmi_network_transmit_bytes_total{{{b},interface="default"}} {up() * 500}')
    return out


def cilium():
    return [f'cilium_drop_count_total{{reason="Policy denied",direction="INGRESS"}} {up() * 2}']


FEED = [{"id": "drive:ZC100", "kind": "drive", "label": "sda", "health": "ok", "detail": "ok",
         "metrics": [{"label": "serial", "value": "ZC100"}, {"label": "bay", "value": "0"}, {"label": "hba", "value": "PERC H730"}],
         "actions": [], "relations": []}]


def serve(port, text=None, routes=None):
    class H(BaseHTTPRequestHandler):
        def do_GET(self):
            path = self.path.split("?")[0]
            if text and path == "/metrics":
                body, ct = ("\n".join(text()) + "\n").encode(), "text/plain; version=0.0.4"
            elif routes and path in routes:
                body, ct = json.dumps(routes[path]).encode(), "application/json"
            elif path == "/healthz":
                body, ct = b"ok", "text/plain"
            else:
                self.send_response(404)
                self.end_headers()
                return
            self.send_response(200)
            self.send_header("content-type", ct)
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a):
            pass

    threading.Thread(target=ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever, daemon=True).start()


serve(19201, drive, {"/api/v1/components": FEED, "/api/v1/drives": []})
serve(19202, engine)
serve(19203, kubelet)
serve(19204, vm)
serve(19205, cilium)
print("stand-ins up", flush=True)
while True:
    time.sleep(60)
