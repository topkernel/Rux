#!/usr/bin/python3.10
# obs.py - poll /proc for process lifecycle (comm/state/ppid) without
# sleeping (time.sleep is broken on this kernel - EFAULT). Busy-polls
# with a light duty cycle, prints OBS lines on any state change.
import os
import sys
import time

duration = float(sys.argv[1]) if len(sys.argv) > 1 else 12.0
PREFIXES = ("dbus-daemon", "python3.10", "dconf-service", "dbus-send",
            "gsettings", "sh", "peer", "obs", "false", "dbus-run-ses")
t0 = time.monotonic()
seen = {}
spin = 0
while time.monotonic() - t0 < duration:
    spin += 1
    if spin % 3 != 1:
        continue  # crude duty-cycle: poll every 3rd spin
    try:
        entries = os.listdir("/proc")
    except OSError:
        continue
    for e in entries:
        if not e.isdigit():
            continue
        try:
            comm = open("/proc/%s/comm" % e).read().strip()
        except OSError:
            continue
        if not comm.startswith(PREFIXES):
            continue
        state = ppid = "?"
        try:
            st = open("/proc/%s/stat" % e).read().split()
            state = st[2]
            ppid = st[3]
        except (OSError, IndexError):
            pass
        key = (e, comm, ppid)
        prev = seen.get(key)
        if prev != state:
            seen[key] = state
            print("OBS: t=%.2f pid=%s comm=%s ppid=%s state=%s" % (
                time.monotonic() - t0, e, comm, ppid, state), flush=True)
print("OBS: end spins=%d" % spin, flush=True)
