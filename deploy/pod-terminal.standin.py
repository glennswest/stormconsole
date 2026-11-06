#!/usr/bin/env python3
"""A kubelet for deploy/verify-pod-terminal.sh (#12): TLS on 127.0.0.1:10250, bearer required, serving
/exec/{ns}/{pod}/{container} over a WebSocket as rustkube-node's remotecommand.rs does: v4.channel.k8s.io,
the channel as first byte (0 stdin, 1 stdout, 2 stderr, 3 error, 4 resize), a real process on a real pty,
the result as a metav1.Status with ExitCode in its causes. A command that cannot run is refused before the
upgrade with a 500 in the runtime's words. Each session's start and end go to the log."""
import base64
import fcntl
import hashlib
import json
import os
import pty
import select
import shutil
import ssl
import struct
import sys
import termios
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

CERT, KEY = sys.argv[1:3]
GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


def say(*a):
    print(*a, flush=True)


def send(conn, payload):
    n = len(payload)
    if n < 126:
        head = bytes([0x82, n])
    elif n < 65536:
        head = bytes([0x82, 126]) + struct.pack(">H", n)
    else:
        head = bytes([0x82, 127]) + struct.pack(">Q", n)
    conn.sendall(head + payload)


def frames(buf):
    """Whole client frames in buf: yields (opcode, payload), returns what is left."""
    out = []
    while len(buf) >= 2:
        op, n = buf[0] & 0x0F, buf[1] & 0x7F
        at = 2
        if n == 126:
            if len(buf) < 4:
                break
            n = struct.unpack(">H", buf[2:4])[0]
            at = 4
        elif n == 127:
            if len(buf) < 10:
                break
            n = struct.unpack(">Q", buf[2:10])[0]
            at = 10
        if len(buf) < at + 4 + n:
            break
        mask = buf[at:at + 4]
        data = bytes(b ^ mask[i % 4] for i, b in enumerate(buf[at + 4:at + 4 + n]))
        out.append((op, data))
        buf = buf[at + 4 + n:]
    return out, buf


class Kubelet(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def answer(self, code, text):
        body = text.encode()
        self.send_response(code)
        self.send_header("content-type", "text/plain")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        u = urlparse(self.path)
        if not self.headers.get("authorization", "").startswith("Bearer "):
            return self.answer(401, "no bearer")
        parts = u.path.strip("/").split("/")
        if parts[0] != "exec" or len(parts) != 4:
            return self.answer(404, "not served here")
        q = parse_qs(u.query)
        argv = q.get("command", [])
        container = parts[3]
        if q.get("input", [""])[0] not in ("1", "true") or q.get("tty", [""])[0] not in ("1", "true"):
            return self.answer(400, f"expected input and tty, got {u.query}")
        if not argv:
            return self.answer(400, "no command")
        if shutil.which(argv[0]) is None:
            return self.answer(500, f'exec: "{argv[0]}": executable file not found in $PATH')
        key = self.headers.get("sec-websocket-key", "")
        accept = base64.b64encode(hashlib.sha1((key + GUID).encode()).digest()).decode()
        self.send_response(101)
        self.send_header("Upgrade", "websocket")
        self.send_header("Connection", "Upgrade")
        self.send_header("Sec-WebSocket-Accept", accept)
        self.send_header("Sec-WebSocket-Protocol", "v4.channel.k8s.io")
        self.end_headers()
        self.wfile.flush()
        self.session(argv, container)
        self.close_connection = True

    def session(self, argv, container):
        pid, master = pty.fork()
        if pid == 0:
            os.environ.update({"CONTAINER": container, "PS1": "$ "})
            os.execvp(argv[0], argv)
        say("started", container, json.dumps(argv))
        conn = self.connection
        buf = b""
        gone = False
        while True:
            pending = conn.pending() if hasattr(conn, "pending") else 0
            ready = [conn] if pending else select.select([conn, master], [], [], 1)[0]
            if master in ready:
                try:
                    data = os.read(master, 4096)
                except OSError:
                    data = b""
                if not data:
                    break
                send(conn, b"\x01" + data)
            if conn in ready:
                try:
                    data = conn.recv(4096)
                except OSError:
                    data = b""
                if not data:
                    gone = True
                    break
                got, buf = frames(buf + data)
                for op, payload in got:
                    if op == 8:
                        gone = True
                    elif op == 2 and payload[:1] == b"\x00":
                        os.write(master, payload[1:])
                    elif op == 2 and payload[:1] == b"\x04":
                        s = json.loads(payload[1:])
                        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", s["Height"], s["Width"], 0, 0))
                        say("resized", s["Width"], s["Height"])
                if gone:
                    break
        if gone:
            # The client going away stops an exec.
            os.kill(pid, 9)
            os.waitpid(pid, 0)
            say("stopped: the client went", container)
            return
        _, st = os.waitpid(pid, 0)
        code = os.waitstatus_to_exitcode(st)
        say("ended", container, code)
        if code == 0:
            status = {"kind": "Status", "status": "Success"}
        else:
            status = {"kind": "Status", "status": "Failure", "reason": "NonZeroExitCode", "message": f"command terminated with exit code {code}",
                      "details": {"causes": [{"reason": "ExitCode", "message": str(code)}]}}
        send(conn, b"\x03" + json.dumps(status).encode())
        conn.sendall(bytes([0x88, 0]))


server = ThreadingHTTPServer(("127.0.0.1", 10250), Kubelet)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(CERT, KEY)
server.socket = ctx.wrap_socket(server.socket, server_side=True)
say("kubelet up")
server.serve_forever()
