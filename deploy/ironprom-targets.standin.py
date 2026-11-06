#!/usr/bin/env python3
"""ironprom's targets for verify-network-pages.sh (#285): stormcos's `ironprom-node.yml` lists the cilium
jobs on every build, so each network's job is here on both, the cilium ones down. Charts have no data."""
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

def target(job, health, error=""):
    return {"labels": {"job": job}, "scrapePool": job, "scrapeUrl": f"http://127.0.0.1:1/{job}", "lastError": error,
            "lastScrape": "2026-11-06T00:00:00.000Z", "lastScrapeDuration": 0.01, "health": health}

TARGETS = {"status": "success", "data": {"activeTargets": [
    target("stormdrive", "up"), target("cilium-agent", "down", "connection refused"),
    target("flowsdn-agent", "up")], "droppedTargets": []}}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        body = TARGETS if self.path.startswith("/api/v1/targets") else {"status": "success", "data": {"resultType": "matrix", "result": []}}
        b = json.dumps(body).encode()
        self.send_response(200); self.send_header("content-type", "application/json"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)

HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
