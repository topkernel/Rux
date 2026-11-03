#!/usr/bin/env python3
"""monq.py — send HMP commands to a QEMU unix monitor socket, print replies."""
import socket, sys, time
path = sys.argv[1]
cmds = sys.argv[2:]
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.settimeout(3)
s.connect(path)
time.sleep(0.3)
try:
    s.recv(4096)  # banner
except Exception:
    pass
for c in cmds:
    s.sendall((c + "\n").encode())
    time.sleep(0.7)
    out = b""
    try:
        while True:
            d = s.recv(4096)
            if not d:
                break
            out += d
    except socket.timeout:
        pass
    print(f"$ {c}\n{out.decode(errors='replace')}")
s.close()
