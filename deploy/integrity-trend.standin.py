#!/usr/bin/env python3
"""One node's stormraid /metrics for deploy/verify-integrity-trend.sh (#385), in the series and labels
stormraid serves (stormraid `src/http.rs` `integrity_metrics`, v0.27.0): per member
`stormraid_member_{checksum_mismatches_total{level},blocks_repaired_total,read_errors_total,
write_errors_total,pi_errors_total{kind},last_rot_seconds}` labelled `node, set, drive, bay, slot, member`;
per set `stormraid_set_*` and `stormraid_scrub_*` labelled `node, set`.

    integrity-trend.standin.py <addr> <port> <node> <set> <slot>:<drive>[:rot] ...

A member marked `rot` finds a mismatch in the scrub and rewrites the block every 6 s from the start (7 of
each before it), and has 2 PI and 3 read errors: the counters step, so a chart over the run rises.
"""
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ADDR, PORT, NODE, SET = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]
MEMBERS = [m.split(":") for m in sys.argv[5:]]
START = time.time()


def body():
    rose = int((time.time() - START) // 6)
    out, total_mm, total_rep = [], 0, 0
    for i, m in enumerate(MEMBERS):
        slot, drive, rot = m[0], m[1], len(m) > 2
        l = f'node="{NODE}",set="{SET}",drive="{drive}",bay="{slot}",slot="{slot}",member="uuid-{SET}-{slot}"'
        l3 = 7 + rose if rot else 0
        rep = l3
        mm = {"l1": 1 if rot else 0, "l2": 0, "l3": l3}
        for lvl, n in mm.items():
            out.append(f"stormraid_member_checksum_mismatches_total{{{l},level=\"{lvl}\"}} {n}")
        out.append(f"stormraid_member_blocks_repaired_total{{{l}}} {rep}")
        out.append(f"stormraid_member_read_errors_total{{{l}}} {3 if rot else 0}")
        out.append(f"stormraid_member_write_errors_total{{{l}}} 0")
        out.append(f"stormraid_member_pi_errors_total{{{l},kind=\"any\"}} {2 if rot else 0}")
        out.append(f"stormraid_member_last_rot_seconds{{{l}}} {int(time.time()) if rot else 0}")
        total_mm += sum(mm.values())
        total_rep += rep
    sl = f'node="{NODE}",set="{SET}"'
    out += [
        f"stormraid_set_unrecoverable_rows_total{{{sl}}} 0",
        f"stormraid_set_checksum_mismatches_total{{{sl}}} {total_mm}",
        f"stormraid_set_blocks_repaired_total{{{sl}}} {total_rep}",
        f"stormraid_scrub_passes_total{{{sl}}} 4",
        f"stormraid_scrub_running{{{sl}}} 0",
    ]
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
