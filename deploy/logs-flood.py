#!/usr/bin/env python3
"""Sender for deploy/verify-logs-full.sh (#128): RFC 5424 lines, every one
distinct (so dedup cannot absorb them), about the size of the fleet's own,
to a multicast group.

  logs-flood.py <group:port> <count> [rate/s] [tag]

Apps rotate through stormdrive, stormblock, kubelet and stormd, so a
filtered read (`?app=stormdrive`) always has something to find.
"""
import socket
import sys
import time

group, port = sys.argv[1].rsplit(":", 1)
count = int(sys.argv[2])
rate = float(sys.argv[3]) if len(sys.argv) > 3 else 4000.0
tag = sys.argv[4] if len(sys.argv) > 4 else "flood"
apps = ["stormdrive", "stormblock", "kubelet", "stormd"]
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
s.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_TTL, 1)
s.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_LOOP, 1)
start = time.time()
for i in range(count):
    app = apps[i % len(apps)]
    msg = (f"<14>1 2026-10-10T12:00:00.000Z node-{i % 7} {app} - - - {tag} {i}: "
           f"reconciled volume pvc-{i:08d} on slab {i % 97} after {i % 1000} ms; "
           f"placement drives=[SN{i % 160:04d},SN{(i + 1) % 160:04d}] legs=2 state=ok "
           f"generation={i} checksum={hash((tag, i)) & 0xffffffffffff:012x}")
    s.sendto(msg.encode(), (group, int(port)))
    # Paced, so the console's socket buffer is not the thing measured.
    ahead = (i + 1) / rate - (time.time() - start)
    if ahead > 0:
        time.sleep(ahead)
print(f"sent {count} in {time.time() - start:.1f}s")
