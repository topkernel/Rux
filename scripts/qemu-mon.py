#!/usr/bin/env python3
"""mon.py — one-shot QEMU monitor command client.

Usage: mon.py <socket> <command...>
Sends each command, prints output until the monitor prompt returns, exits.
"""
import socket
import sys
import time


def main() -> int:
    if len(sys.argv) < 3:
        print("usage: mon.py <unix-socket> <command...>", file=sys.stderr)
        return 2
    path = sys.argv[1]
    cmds = sys.argv[2:]
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(3.0)
    s.connect(path)
    # drain banner + prompt
    try:
        time.sleep(0.2)
        s.setblocking(False)
        try:
            s.recv(65536)
        except BlockingIOError:
            pass
        s.setblocking(True)
    except OSError:
        pass
    for cmd in cmds:
        s.sendall((cmd + "\n").encode())
        deadline = time.time() + 10.0
        buf = b""
        while time.time() < deadline:
            s.settimeout(1.0)
            try:
                chunk = s.recv(65536)
            except socket.timeout:
                continue
            if not chunk:
                break
            buf += chunk
            if b"(qemu)" in buf[-64:]:
                break
        sys.stdout.write(buf.decode("utf-8", "replace"))
    s.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
