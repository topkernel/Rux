#!/usr/bin/env python3
"""Arm a QEMU write-watchpoint via HMP and log every hit PC.

Usage: hmp_watch.py <hmp.sock> <addr-hex> <out-log> [delay-seconds]

Reads monitor lines forever; timestamps every line. Exit on EOF."""
import socket, sys, time, os

sock_path, addr, out = sys.argv[1], sys.argv[2], sys.argv[3]
delay = float(sys.argv[4]) if len(sys.argv) > 4 else 40.0
end = time.time() + delay
while time.time() < end:
    if os.path.exists(sock_path):
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.connect(sock_path)
            break
        except OSError:
            pass
    time.sleep(2)
else:
    sys.exit("sock never appeared")
s.settimeout(None)
f = open(out, "w", buffering=1)
f.write(f"# armed watch {addr} at {time.strftime('%H:%M:%S')}\n")
s.sendall(f"watch {addr}\n".encode())
t0 = time.time()
try:
    while True:
        data = s.recv(4096)
        if not data:
            break
        for ln in data.decode(errors="replace").splitlines():
            f.write(f"{time.time()-t0:9.2f} {ln}\n")
except OSError:
    pass
finally:
    f.close()
