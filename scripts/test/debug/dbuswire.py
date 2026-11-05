#!/usr/bin/python3
# dbuswire.py - minimal D-Bus wire-protocol peer/caller for Rux kernel
# testing. Pure stdlib: no python-dbus, no glib. Modes:
#   serve : connect, RequestName, reply to every method call
#   call  : N sequential method calls, latency summary line "RSLT: ..."
#   probe : single method call, one "RSLT: ..." line
import argparse
import os
import socket
import struct
import sys
import time

MSG_CALL, MSG_RETURN, MSG_ERROR, MSG_SIGNAL = 1, 2, 3, 4


def pad(b, n):
    while len(b) % n:
        b += b"\x00"
    return b


def marsh_str(s):
    b = s.encode()
    return struct.pack("<I", len(b)) + b + b"\x00"


def marsh_sig(s):
    b = s.encode()
    return bytes([len(b)]) + b + b"\x00"


def build_msg(mtype, flags, serial, fields, body=b""):
    """fields: list of (code, sig, value); body must already be marshalled
    and padded per its signature."""
    hdr = bytearray()
    hdr += bytes([ord("l"), mtype, flags, 1])
    hdr += struct.pack("<I", len(body))
    hdr += struct.pack("<I", serial)
    arr = bytearray()
    for code, sig, val in fields:
        arr = bytearray(pad(bytes(arr), 8))
        arr += struct.pack("<B", code)
        arr += marsh_sig(sig)
        arr += marsh_str(val) if sig in ("s", "o") else \
            bytes([val]) + b"\x00" * 3 if sig == "y" else \
            marsh_sig(val) if sig == "g" else \
            struct.pack("<I", val)
    hdr += struct.pack("<I", len(arr))
    return pad(bytes(hdr) + bytes(arr), 8) + body


class Conn:
    def __init__(self, addr):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
        except OSError:
            pass
        self.sock.connect(addr)
        self.sock.sendall(b"\x00")
        uid = str(os.getuid()).encode()
        self.sock.sendall(b"AUTH EXTERNAL " + uid.hex().encode() + b"\r\n")
        self.buf = b""
        resp = self._readline()
        if not resp.startswith(b"OK"):
            self.sock.sendall(b"AUTH ANONYMOUS\r\n")
            resp = self._readline()
            if not resp.startswith(b"OK"):
                raise RuntimeError("auth failed: %r" % resp)
        self.sock.sendall(b"BEGIN\r\n")
        self.serial = 1

    def _readline(self):
        while b"\r\n" not in self.buf:
            d = self.sock.recv(4096)
            if not d:
                raise RuntimeError("eof in auth")
            self.buf += d
        line, self.buf = self.buf.split(b"\r\n", 1)
        return line

    def send(self, mtype, fields, body=b"", flags=0):
        s = self.serial
        self.serial += 1
        self.sock.sendall(build_msg(mtype, flags, s, fields, body))
        return s

    def _need(self, n):
        while len(self.buf) < n:
            d = self.sock.recv(65536)
            if not d:
                raise RuntimeError("eof")
            self.buf += d

    def read_msg(self, timeout=None):
        """Returns (mtype, fields, body, serial) or None on timeout."""
        self.sock.settimeout(timeout)
        try:
            self._need(16)
            _e, mtype, _f, _v, blen, ser, alen = struct.unpack(
                "<BBBBIII", self.buf[:16])
            hdr_end = ((16 + alen + 7) // 8) * 8
            self._need(hdr_end)
            fields = {}
            arr = self.buf[16:16 + alen]
            i = 0
            while i < len(arr):
                while i % 8:
                    i += 1
                code = arr[i]; i += 1
                slen = arr[i]; i += 1
                sig = arr[i:i + slen].decode(); i += slen + 1
                if sig in ("s", "o"):
                    while i % 4:
                        i += 1
                    n = struct.unpack("<I", arr[i:i + 4])[0]
                    val = arr[i + 4:i + 4 + n].decode(errors="replace")
                    i += 4 + n + 1
                elif sig in ("u", "b"):
                    while i % 4:
                        i += 1
                    val = struct.unpack("<I", arr[i:i + 4])[0]
                    i += 4
                elif sig == "g":
                    n = arr[i]
                    val = arr[i + 1:i + 1 + n].decode()
                    i += 1 + n + 1
                else:
                    raise ValueError("hdr field sig " + sig)
                fields[code] = val
            self.buf = self.buf[hdr_end:]
            self._need(blen)
            body = self.buf[:blen]
            self.buf = self.buf[blen:]
            return mtype, fields, body, ser
        except socket.timeout:
            return None
        except (ConnectionResetError, BrokenPipeError):
            raise

    def call(self, dest, path, iface, member, timeout, sig=None, body=b""):
        """Returns (kind, errname, elapsed) where kind is 'return' |
        'error' | 'timeout'."""
        fields = [(1, "o", path), (2, "s", iface), (3, "s", member),
                  (6, "s", dest)]
        if sig:
            fields.append((8, "g", sig))
        ser = self.send(MSG_CALL, fields, body)
        t0 = time.monotonic()
        while True:
            left = timeout - (time.monotonic() - t0)
            if left <= 0:
                return "timeout", "", time.monotonic() - t0
            m = self.read_msg(timeout=left)
            if m is None:
                return "timeout", "", time.monotonic() - t0
            mtype, fh, bdy, _ser = m
            if mtype in (MSG_RETURN, MSG_ERROR) and fh.get(5) == ser:
                el = time.monotonic() - t0
                if mtype == MSG_RETURN:
                    return "return", "", el
                name = fh.get(4, "")
                if not name and bdy[:4]:
                    n = struct.unpack("<I", bdy[0:4])[0]
                    name = bdy[4:4 + n].decode(errors="replace")
                return "error", name, el

    def hello(self, timeout=10):
        k, e, el = self.call("org.freedesktop.DBus", "/org/freedesktop/DBus",
                             "org.freedesktop.DBus", "Hello", timeout)
        if k != "return":
            raise RuntimeError("Hello %s %s" % (k, e))
        return el

    def request_name(self, name, timeout=10):
        body = pad(marsh_str(name), 4) + struct.pack("<I", 4)
        fields = [(1, "o", "/org/freedesktop/DBus"),
                  (2, "s", "org.freedesktop.DBus"),
                  (3, "s", "RequestName"),
                  (6, "s", "org.freedesktop.DBus"),
                  (8, "g", "su")]
        ser = self.send(MSG_CALL, fields, body)
        m = self.read_msg(timeout=timeout)
        while not (m and m[0] == MSG_RETURN and m[1].get(5) == ser):
            m = self.read_msg(timeout=timeout)
            if m is None:
                raise RuntimeError("RequestName timeout")
        return struct.unpack("<I", m[2][0:4])[0]


def bus_addr(explicit=None):
    if explicit:
        return explicit
    a = os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")
    a = a.split(",")[0]  # drop ",guid=..." style suffixes
    if a.startswith("unix:path="):
        return a[len("unix:path="):]
    if a.startswith("unix:abstract="):
        return "\0" + a[len("unix:abstract="):]
    return "/run/dbus/session_bus_socket"


def do_serve(args):
    c = Conn(bus_addr(args.address))
    c.hello()
    rc = c.request_name(args.own)
    sys.stderr.write("RSLT: serve own=%s rc=%d\n" % (args.own, rc))
    sys.stderr.flush()
    try:
        with open("/root/peer-owned", "w") as f:
            f.write("pid=%d name=%s rc=%d\n" % (os.getpid(), args.own, rc))
    except OSError:
        pass
    while True:
        m = c.read_msg(timeout=None)
        if m is None:
            continue
        mtype, fh, bdy, ser = m
        if mtype != MSG_CALL:
            continue
        member = fh.get(3, "?")
        caller = fh.get(7, "")
        body = b""
        sig = ""
        if member == "Echo":
            sig = "s"
            body = marsh_str("pong")
        elif member == "Introspect":
            sig = "s"
            body = marsh_str('<node name="/"></node>')
        fields = [(6, "s", caller), (5, "u", ser)]
        if sig:
            fields.append((8, "g", sig))
        c.send(MSG_RETURN, fields, body, flags=1)


def do_call(args):
    c = Conn(bus_addr(args.address))
    c.hello()
    lat = []
    kinds = {}
    for _ in range(args.count):
        k, e, el = c.call(args.dest, args.path, args.iface, args.member,
                          args.timeout)
        lat.append(el)
        key = k + (":" + e if e else "")
        kinds[key] = kinds.get(key, 0) + 1
        if k != "return":
            break  # stop at first failure so activation hangs are visible
    ms = sorted(int(x * 1000) for x in lat)
    n = len(ms)
    timeouts = sum(v for k, v in kinds.items() if k == "timeout")
    print("RSLT: call dest=%s member=%s n=%d/%d timeouts=%d "
          "min=%d p50=%d p90=%d p99=%d max=%d kinds=%s" % (
              args.dest, args.member, n, args.count, timeouts,
              ms[0], ms[n // 2], ms[int(n * 0.9)],
              ms[min(n - 1, int(n * 0.99))], ms[-1], kinds), flush=True)
    sys.exit(0 if timeouts == 0 and n == args.count else 1)


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="mode", required=True)
    s = sub.add_parser("serve")
    s.add_argument("--own", default="org.rux.Test")
    s.add_argument("--address", default=None)
    for name in ("call", "probe"):
        c = sub.add_parser(name)
        c.add_argument("--dest", required=True)
        c.add_argument("--path", default="/")
        c.add_argument("--iface", required=True)
        c.add_argument("--member", required=True)
        c.add_argument("--count", type=int, default=1)
        c.add_argument("--timeout", type=float, default=25.0)
        c.add_argument("--address", default=None)
    args = ap.parse_args()
    if args.mode == "serve":
        do_serve(args)
    else:
        do_call(args)


if __name__ == "__main__":
    main()
