#!/usr/bin/env python3
"""ltp_family_boot.py — boot the LTP riscv image (init=/bin/sh) and run
the given LTP cases one at a time over serial, waiting for the shell
prompt between commands (long pasted lines get eaten by the polled
UART). Usage: python3 run/ltp_family_boot.py <kernel> <case>...
Prints CASE:<name>:<rc> lines plus TPASS/TFAIL tails.
"""
import os
import subprocess
import sys
import time
from ruxroot import repo_root

HERE = os.path.dirname(os.path.abspath(__file__))
WT = repo_root(__file__)
IMG = os.path.join(WT, "work", "ltp-riscv64-rootfs.img")
if not os.path.exists(IMG):
    subprocess.run(
        ["cp", "--reflink=auto",
         "/home/william/rux-images/testimages/ltp-riscv64-rootfs.img", IMG],
        check=True)

CASES = sys.argv[2:]
KERNEL = sys.argv[1]

cmd = [
    "qemu-system-riscv64", "-M", "virt", "-accel", "tcg,thread=multi",
    "-cpu", "rv64,zbb=true,zba=true,zbs=true", "-m", "1G", "-smp", "2",
    "-nographic", "-snapshot",
    "-drive", f"file={IMG},if=none,id=rootfs,format=raw,snapshot=on,file.locking=off",
    "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
    "-kernel", KERNEL,
    "-append", "root=/dev/vda rw init=/bin/sh console=ttyS0",
]
proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                        stderr=subprocess.STDOUT, bufsize=0)
log = open(os.environ.get("LTP_FAMILY_LOG", "/tmp/ltp-family-boot.log"), "wb")
buf = bytearray()
deadline = time.time() + float(os.environ.get("LTP_FAMILY_DEADLINE", "240"))
sent_setup = False
idx = 0
results = {}
case_start = 0.0  # current case's first-feed time (feed-pacing rig fix)
last_feed = 0.0

def feed(line):
    proc.stdin.write(line.encode() + b"\n")
    proc.stdin.flush()

def pump():
    import select
    r, _, _ = select.select([proc.stdout], [], [], 0.3)
    if r:
        chunk = os.read(proc.stdout.fileno(), 4096)
        if chunk:
            buf.extend(chunk)
            log.write(chunk)
            log.flush()

while time.time() < deadline:
    pump()
    text = bytes(buf[-4000:])
    if not sent_setup and (b"# " in text[-80:] or b"~ #" in text[-80:]):
        feed("export PATH=/bin:/sbin:/test/linux-ltp/testcases/bin:$PATH "
             "TMPDIR=/tmp LTPROOT=/test/linux-ltp HOME=/")
        time.sleep(0.4); pump()
        feed("cd /test/linux-ltp/testcases/bin")
        time.sleep(0.4); pump()
        buf.clear()
        sent_setup = True
        continue
    if sent_setup and idx < len(CASES):
        c = CASES[idx]
        marker = f"RC-{idx}".encode()
        if marker not in text:
            # Feed PACING (rig fix 2026-11-05): re-feeding a forking case
            # every loop iteration starts OVERLAPPING instances that kill
            # each other (the mmap11 KSIG9 storm) and wedge the family for
            # the whole deadline. Feed once, then only after an 8s quiet
            # gap; give up on the case after 45s and record "?".
            if case_start == 0.0:
                case_start = time.time()
            if last_feed == 0.0 or time.time() - last_feed >= 8.0:
                feed(f"/test/linux-ltp/testcases/bin/{c} > /tmp/o 2>&1; "
                     f"echo CASE-DONE:{c}:$?:RC-{idx}")
                last_feed = time.time()
            if time.time() - case_start > 45.0:
                results[c] = "?"
                idx += 1
                buf.clear()
                case_start = 0.0
                last_feed = 0.0
            time.sleep(0.5)
            pump()
            continue
        # marker seen: harvest the OUTPUT line (skip the echoed input —
        # it still contains the unexpanded "$?")
        line = next((l for l in text.decode(errors="replace").splitlines()
                     if l.startswith("CASE-DONE:") and "$" not in l), "")
        rc = line.split(":")[2] if line.count(":") >= 2 else "?"
        results[c] = rc
        idx += 1
        buf.clear()
        case_start = 0.0
        last_feed = 0.0
        continue
    if sent_setup and idx >= len(CASES):
        break

for c in CASES:
    print(f"CASE:{c}:{results.get(c, 'MISSING')}")
proc.kill()
log.close()
