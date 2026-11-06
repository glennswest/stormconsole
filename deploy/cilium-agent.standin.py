#!/usr/bin/env python3
"""A Cilium agent's two loopback ports for verify-cilium-metrics.sh (#4): :9879/healthz, and :9962/metrics from a file."""
import sys, threading
from http.server import BaseHTTPRequestHandler, HTTPServer

scrape = sys.argv[1]

class Health(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        self.send_response(200); self.send_header('content-length', '2'); self.end_headers(); self.wfile.write(b'ok')

class Metrics(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        try:
            body = open(scrape, 'rb').read()
            self.send_response(200)
        except OSError:
            body = b''
            self.send_response(404)
        self.send_header('content-type', 'text/plain; version=0.0.4')
        self.send_header('content-length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

threading.Thread(target=HTTPServer(('127.0.0.1', 9879), Health).serve_forever, daemon=True).start()
if len(sys.argv) < 3:
    HTTPServer(('127.0.0.1', 9962), Metrics).serve_forever()
else:
    threading.Event().wait()
