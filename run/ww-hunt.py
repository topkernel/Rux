#!/usr/bin/env python3
"""ww-hunt.py — wildwrite attribution hunt orchestrator.

Starts ww-boot.sh (QEMU, own PID, killed at the deadline — never touches
other processes), waits for the guest to get through early boot, then
attaches gdb with hardware watchpoints on the hpet victim statics.
When the run ends, prints a hit summary.

Usage: ww-hunt.py <tag> [seconds] [gdbport]
Defaults: seconds=2700, gdbport=12371 (per-tag auto: +10*index not done —
pass distinct ports when running two hunts concurrently).
"""

import os
import signal
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RUN = os.path.join(ROOT, "run")

tag = sys.argv[1] if len(sys.argv) > 1 else "ww1"
secs = int(sys.argv[2]) if len(sys.argv) > 2 else 2700
port = int(sys.argv[3]) if len(sys.argv) > 3 else 12371

serial = os.path.join(RUN, f"{tag}-serial.log")
gdblog = os.path.join(RUN, f"{tag}-gdb.log")

env = dict(os.environ, TAG=tag, GDBPORT=str(port))
# s30: ride the kernel-side tripwire on every hunt boot. dfx=wildwatch
# RO-arms the whole 2-page hpet guard cluster in every alias (any CPU
# store there page-faults at the writer's RIP with a full KERNPANIC
# attribution dump) and 1s-verifies the canary page (the only detector
# for PTE-invisible/DMA-class writers). The frozen kernel carries the
# code (42eb2006, pre-refreeze); heal paths are guard_armed()-gated, so
# there are zero legit post-arm writers. Capture markers this adds in
# SERIAL (the gdb watchpoints only cover 8 of the cluster's 1024
# qwords): WILDWATCH-CANARY, "hpet: clock {base,div,offset} healed",
# and the KERNPANIC of the RO trap itself.
env["EXTRA_APPEND"] = (env.get("EXTRA_APPEND", "") + " dfx=wildwatch").strip()
qemu = subprocess.Popen(
    [os.path.join(RUN, "ww-boot.sh")],
    env=env,
    stdout=subprocess.DEVNULL,
    stderr=subprocess.STDOUT,
)
print(f"[ww] qemu pid={qemu.pid} tag={tag} port={port} budget={secs}s", flush=True)

# Wait for the serial log to show the kernel is up (paging live) before
# attaching; hpet::init runs in the first second, before we arm.
attach_after = 20
deadline = time.time() + secs
gdb_proc = None
try:
    time.sleep(attach_after)
    if qemu.poll() is not None:
        print(f"[ww] qemu exited early rc={qemu.returncode}", flush=True)
        sys.exit(1)
    gdb_cmds = subprocess.run(
        [sys.executable, os.path.join(RUN, "ww-gdbscript.py"), str(port)],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    cmdfile = os.path.join(RUN, f"{tag}-gdb.cmd")
    with open(cmdfile, "w") as f:
        f.write(gdb_cmds)
    with open(gdblog, "w") as lf:
        gdb_proc = subprocess.Popen(
            ["gdb-multiarch", "-batch", "-x", cmdfile],
            stdout=lf,
            stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL,
        )
    print(f"[ww] gdb pid={gdb_proc.pid} armed from {cmdfile}", flush=True)

    # Supervise until the budget is spent or QEMU dies on its own.
    while time.time() < deadline:
        if qemu.poll() is not None:
            print(f"[ww] qemu exited rc={qemu.returncode}", flush=True)
            break
        time.sleep(10)
    else:
        print("[ww] budget spent, terminating qemu", flush=True)
finally:
    # Kill ONLY the processes this script started (PID discipline).
    if gdb_proc is not None and gdb_proc.poll() is None:
        gdb_proc.terminate()
        try:
            gdb_proc.wait(10)
        except subprocess.TimeoutExpired:
            gdb_proc.kill()
    if qemu.poll() is None:
        qemu.terminate()
        try:
            qemu.wait(15)
        except subprocess.TimeoutExpired:
            qemu.kill()
    qemu.wait()

# Summary.
print(f"[ww] serial lines: {sum(1 for _ in open(serial, 'rb'))}", flush=True)
if os.path.exists(gdblog):
    hits = [l for l in open(gdblog, errors="replace") if "WW-HIT" in l]
    print(f"[ww] WW-HIT count: {len(hits)}", flush=True)
    for h in hits[:20]:
        print("    " + h.rstrip(), flush=True)
    if not hits:
        print("[ww] no watchpoint hits this run", flush=True)
heals = 0
if os.path.exists(serial):
    with open(serial, errors="replace") as f:
        heals = sum(1 for l in f if "healed" in l or "PERCPU-SCRIBBLE" in l)
print(f"[ww] heal-line count in serial: {heals}", flush=True)
