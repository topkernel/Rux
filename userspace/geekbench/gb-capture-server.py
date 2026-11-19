#!/usr/bin/env python3
"""gb-capture-server.py — TLS server that captures the Geekbench result
upload POST bodies. Geekbench (tryout build) only exposes scores inside
the HTTPS upload to browser.geekbench.com; the browser result page is
bot-guarded, so the rig redirects that upload here (guest: /etc/hosts
pins browser.geekbench.com to 10.0.2.2, the QEMU user-network host
alias; host: /etc/hosts pins it to 127.0.0.1) and parses the JSON
payload with parse-capture.py.

Usage: sudo python3 gb-capture-server.py <cert.pem> <key.pem> [port]
       (default port 443; captures appended to ./captures.log)
"""
import http.server
import os
import ssl
import sys
import datetime

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "captures.log")


class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *a):
        pass

    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n) if n else b""
        with open(OUT, "ab") as f:
            f.write(b"==== %s %s %s ====\n" % (
                datetime.datetime.now().isoformat().encode(),
                self.command.encode(), self.path.encode()))
            f.write(b"Host: %s\n" % (self.headers.get("Host") or "?").encode())
            f.write(body)
            f.write(b"\n")
        resp = b'{"id": "captured", "ok": true}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(resp)))
        self.end_headers()
        self.wfile.write(resp)

    do_POST = _handle
    do_GET = _handle
    do_PUT = _handle


if __name__ == "__main__":
    cert = sys.argv[1] if len(sys.argv) > 1 else "srv.pem"
    key = sys.argv[2] if len(sys.argv) > 2 else "srv.key"
    port = int(sys.argv[3]) if len(sys.argv) > 3 else 443
    httpd = http.server.HTTPServer(("0.0.0.0", port), H)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(cert, key)
    httpd.socket = ctx.wrap_socket(httpd.socket, server_side=True)
    print("capture server on :%d (log: %s)" % (port, OUT), flush=True)
    httpd.serve_forever()
