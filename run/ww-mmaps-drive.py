#!/usr/bin/env python3
"""ww-mmaps-drive.py — drive the OH console shell of a ww-mmaps boot to
dump /proc/<pid>/maps around the prelinked 0x3fbf region.

Connects to the serial socket, wakes the console, runs a bounded scan
over /proc/[0-9]*/maps for 3fbf-containing processes, plus a full maps
dump of one victim and of the shell itself. Everything lands in the
serial logfile (tty echo) and stdout.

Usage: ww-mmaps-drive.py <sock> [wait_for_boot_secs]
"""

import socket
import sys
import time

sock_path = sys.argv[1]
boot_wait = int(sys.argv[2]) if len(sys.argv) > 2 else 100

s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sock_path)
s.settimeout(2.0)


def send(line, wait=3.0):
    s.sendall(line.encode() + b"\r")
    time.sleep(wait)
    try:
        data = s.recv(65536)
        sys.stdout.write(data.decode(errors="replace"))
        sys.stdout.flush()
    except socket.timeout:
        pass


# Wake the console; OH's console service spawns a shell on ttyS0.
time.sleep(boot_wait)
send("", 2)
send("", 2)
send("echo WWMAPS-READY", 3)
# Bounded scan: which pids carry 0x3fbf mappings at all.
send("for p in /proc/[0-9]*; do grep -q 3fbf $p/maps 2>/dev/null && echo HIT $p; done", 12)
# First victim: full 3fbf-region lines (offset + path presence).
send("for p in /proc/[0-9]*; do if grep -q 3fbf $p/maps 2>/dev/null; then echo ==== $p; grep 3fbf $p/maps; break; fi; done", 12)
# The shell's own full maps (reference for fd-closed path loss).
send("cat /proc/self/maps", 12)
send("echo WWMAPS-DONE", 3)
