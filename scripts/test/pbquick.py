#!/usr/bin/env python3
"""pbquick.py — fast pbench landmine classifier for the mine-hunt lane.

Boots one bzImage against a SHARED prebuilt pbench image (snapshot mode,
so every run sees byte-identical userspace) and classifies:

  CRASH    the user-heap landmine fired: pid=1 pbench SIGSEGV in the
           malloc get_meta window (the seventh agent's deterministic
           signature: USEGV-FOR pid=1 comm='pbench', cr2 around 0xe7)
  CLEAN    full probe run reached PBENCH DONE rc=0
  PANIC    kernel panic before DONE (kernel-side bug, not the landmine)
  TIMEOUT  no verdict within --timeout

Exit codes: 1 CRASH, 0 CLEAN, 2 PANIC/build error, 3 TIMEOUT.
Kills only the QEMU it started.
"""
import argparse
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(WT, "scripts", "test"))
QEMU = "qemu-system-x86_64"

CRASH_MARKS = (
    b"USEGV-FOR: pid=1 comm='pbench'",
    b"pagefault: Segfault at 0xe7, rip=0x4063eb",
)
# The landmine fires between p1_clock and p2 on every mainline-lineage
# binary seen so far; a binary that reports p4 rows has cleared the
# window plus the fork/exec heavy paths.  DONE still required for CLEAN.
EARLY_CLEAN = b"PBENCH p4_"
DONE_MARK = b"PBENCH DONE rc=0"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bzimage", required=True)
    ap.add_argument("--tag", default="pbq")
    ap.add_argument("--img", default=os.path.join(WT, "work", "pbmine.img"))
    ap.add_argument("--timeout", type=int, default=420)
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--extra", default=None, help="extra -append tokens")
    ap.add_argument("--full-only", action="store_true",
                    help="ignore the p4 early-clean shortcut (diagnosis runs)")
    args = ap.parse_args()

    if not os.path.exists(args.bzimage):
        print("BZIMAGE-MISSING %s" % args.bzimage)
        return 2
    serial = os.path.join(WT, "work", "pbquick-%s-serial.log" % args.tag)
    if os.path.exists(serial):
        os.remove(serial)

    append = "root=/dev/vda rw init=/test/pbench console=ttyS0"
    if args.extra:
        append += " " + args.extra
    cmd = [QEMU, "-M", "q35", "-accel", "kvm", "-cpu", "max",
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none", "-snapshot",
           "-serial", "file:" + serial,
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % args.img,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", args.bzimage, "-append", append]
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    verdict, t_fire = None, None
    try:
        t0 = time.time()
        seen = 0
        while time.time() - t0 < args.timeout:
            time.sleep(2)
            if proc.poll() is not None:
                # exited on its own: read what it printed
                pass
            try:
                with open(serial, "rb") as f:
                    data = f.read()
            except FileNotFoundError:
                continue
            for mark in CRASH_MARKS:
                if mark in data:
                    verdict, t_fire = "CRASH", time.time() - t0
                    break
            if verdict:
                break
            if b"Kernel panic" in data or b"panicked at" in data:
                verdict, t_fire = "PANIC", time.time() - t0
                break
            if b"PBENCH DONE rc=0" in data:
                verdict, t_fire = "CLEAN", time.time() - t0
                break
            if not args.full_only and EARLY_CLEAN in data[seen if seen < len(data) else 0:]:
                pass  # keep waiting for DONE; early marker only logged
            # log progress rows once
            lines = data.split(b"\n")
            while seen < len(lines):
                ln = lines[seen]
                if ln.startswith(b"PBENCH ") and (b" n=" in ln or b"SKIP" in ln
                                                     or b"PREP" in ln or b"START" in ln
                                                     or b"CAL" in ln):
                    print("  [%5.1fs] %s" % (time.time() - t0,
                                             ln.decode(errors="replace")), flush=True)
                seen += 1
        if not verdict:
            verdict, t_fire = "TIMEOUT", time.time() - t0
    finally:
        try:
            proc.terminate()
            proc.wait(5)
        except Exception:
            try:
                proc.kill()
            except Exception:
                pass
    print("VERDICT %s %s t=%.1fs serial=%s" % (verdict, args.tag, t_fire, serial),
          flush=True)
    return {"CRASH": 1, "CLEAN": 0, "PANIC": 2, "TIMEOUT": 3}[verdict]


if __name__ == "__main__":
    sys.exit(main())
