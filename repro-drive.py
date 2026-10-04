#!/usr/bin/env python3
"""repro-drive.py — batched-read + exit-storm repro driver.

Copies the shared rootfs image (sparse), injects /root/repro.sh via
debugfs, boots the given kernel with -snapshot, waits for the shell
prompt, sends ONE short launch line over serial (avoids the serial-input
overrun that mangles long piped scripts), streams serial to a log, and
reports a verdict (panic markers / round progression).

Usage: repro-drive.py --kernel PATH --tag NAME [--dur 600] [--rounds 6]
Discipline: -snapshot + file.locking=off; kills only its own QEMU child.
"""
import argparse
import os
import re
import select
import shutil
import signal
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
BASE_IMG = "/home/william/Rux/test/rootfs.img"
DEBUGFS = "/usr/sbin/debugfs"
QEMU = "/usr/bin/qemu-system-riscv64"

REPRO_SH = """#!/bin/sh
echo REPRO-SCRIPT-START
r=0
while [ $r -lt {rounds} ]; do
  echo ROUND-$r-BEGIN
  head -c 67108864 /dev/zero > /tmp/big
  dd if=/tmp/big of=/dev/null bs=1048576 skip=0 count=4 & dd if=/tmp/big of=/dev/null bs=1048576 skip=31 count=4 & dd if=/tmp/big of=/dev/null bs=1048576 skip=62 count=4 & dd if=/tmp/big of=/dev/null bs=1048576 skip=93 count=4 & dd if=/tmp/big of=/dev/null bs=1048576 skip=124 count=4 & dd if=/tmp/big of=/dev/null bs=1048576 skip=155 count=4 & dd if=/tmp/big of=/dev/null bs=1048576 skip=186 count=4 & dd if=/tmp/big of=/dev/null bs=1048576 skip=217 count=4 & sleep 0
  j=0; while [ $j -lt 8 ]; do head -c 5242880 /tmp/big > /dev/null & j=$((j+1)); done
  j=0; while [ $j -lt 4 ]; do cat /tmp/big > /dev/null & j=$((j+1)); done
  wait
  rm /tmp/big
  echo ROUND-$r-END
  r=$((r+1))
done
echo REPRO-SCRIPT-ALL-DONE
"""

PANIC_MARKERS = [
    b"KERNPANIC", b"WAKE-WILD-PTR", b"panicked", b"Kernel panic",
    b"soft lockup", b"SOFT-LOCKUP", b"ALLOCTHROW", b"trap: stack:",
    b"lost completion", b"buffer io wait timeout", b"dead bh in chain",
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kernel", required=True)
    ap.add_argument("--tag", required=True)
    ap.add_argument("--dur", type=int, default=600)
    ap.add_argument("--rounds", type=int, default=6)
    ap.add_argument("--smp", type=int, default=4)
    args = ap.parse_args()

    out = os.path.join(HERE, "runs", args.tag)
    os.makedirs(out, exist_ok=True)
    work_img = os.path.join(HERE, "work", "repro.img")
    os.makedirs(os.path.dirname(work_img), exist_ok=True)

    if not os.path.exists(work_img) or os.path.getmtime(work_img) < os.path.getmtime(BASE_IMG):
        print("[prep] sparse copy of rootfs image")
        shutil.copyfile(BASE_IMG, work_img)

    sh_path = os.path.join(out, "repro.sh")
    with open(sh_path, "w") as f:
        f.write(REPRO_SH.format(rounds=args.rounds))
    # NOTE: the rootfs has no /root dir; /tmp exists. debugfs exits rc=0
    # even on failure — check output and verify with stat. The initial rm
    # of a not-yet-existing file is fine to fail.
    subprocess.run([DEBUGFS, "-w", "-R", "rm /tmp/repro.sh", work_img],
                   capture_output=True, text=True)
    r = subprocess.run([DEBUGFS, "-w", "-R", "write %s /tmp/repro.sh" % sh_path, work_img],
                       capture_output=True, text=True)
    if "ext2_lookup" in (r.stdout + r.stderr).lower() or r.returncode != 0:
        print("[prep] debugfs write failed: %s%s" % (r.stdout.strip(), r.stderr.strip()))
        return 2
    r = subprocess.run([DEBUGFS, "-R", "stat /tmp/repro.sh", work_img],
                       capture_output=True, text=True)
    # The stat dump also contains "Fragment: ... Size: 0" — match the real
    # file size on the inode header line only.
    m = re.search(r"Project:\s+\d+\s+Size:\s+(\d+)", r.stdout)
    if not m or int(m.group(1)) == 0:
        print("[prep] verification failed: %s%s" % (r.stdout.strip(), r.stderr.strip()))
        return 2
    print("[prep] repro.sh injected (%d bytes)" % os.path.getsize(sh_path))

    cmd = [QEMU,
           "-M", "virt", "-accel", "tcg,thread=multi", "-cpu", "rv64",
           "-m", "2G", "-smp", str(args.smp),
           "-nographic", "-serial", "stdio", "-monitor", "none",
           "-snapshot",
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % work_img,
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", args.kernel,
           "-append", "root=/dev/vda rw init=/bin/sh console=ttyS0"]
    print("[boot] tag=%s dur=%ds rounds=%d" % (args.tag, args.dur, args.rounds))
    log = open(os.path.join(out, "serial.log"), "wb")
    proc = subprocess.Popen(cmd, stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

    t0 = time.time()
    launched = False
    verdict = {"panic": None, "rounds_done": 0, "last_round": None, "all_done": False}
    buf = bytearray()

    def kill_own():
        try:
            proc.terminate()
            proc.wait(timeout=10)
        except Exception:
            try:
                proc.kill()
            except Exception:
                pass

    try:
        while time.time() - t0 < args.dur:
            r, _, _ = select.select([proc.stdout], [], [], 1.0)
            if not r:
                continue
            chunk = os.read(proc.stdout.fileno(), 65536)
            if not chunk:
                break
            buf.extend(chunk)
            log.write(chunk)
            log.flush()
            text = bytes(buf[-32768:])
            if not launched and b"root" in text and text.rstrip().endswith(b"#"):
                time.sleep(3.0)  # let boot noise drain
                proc.stdin.write(b"\n/bin/sh /tmp/repro.sh\n")
                proc.stdin.flush()
                launched = True
                print("[boot] repro launched at t=%.0fs" % (time.time() - t0))
                buf.clear()
                continue
            for line in text.split(b"\n"):
                if b"ROUND-" in line and b"-END" in line:
                    verdict["rounds_done"] += 1
                    verdict["last_round"] = line.strip().decode(errors="replace")
                    print("[load] %s (t=%.0fs)" % (verdict["last_round"], time.time() - t0))
                if b"REPRO-SCRIPT-ALL-DONE" in line:
                    verdict["all_done"] = True
            for m in PANIC_MARKERS:
                if m in text:
                    hard = m in (b"KERNPANIC", b"WAKE-WILD-PTR", b"panicked",
                                 b"Kernel panic", b"dead bh in chain")
                    print("[marker] %s at t=%.0fs (hard=%s)" %
                          (m.decode(), time.time() - t0, hard))
                    if hard:
                        verdict["panic"] = m.decode()
                        break
            if verdict["panic"]:
                # capture a bit more context, then stop
                time.sleep(15.0)
                break
            if verdict["all_done"]:
                time.sleep(2.0)
                break
    finally:
        kill_own()
        log.close()

    with open(os.path.join(out, "verdict.txt"), "w") as f:
        f.write("tag=%s kernel=%s dur=%d\n" % (args.tag, args.kernel, args.dur))
        f.write("panic=%s\n" % verdict["panic"])
        f.write("rounds_done=%d last=%s all_done=%s\n" %
                (verdict["rounds_done"], verdict["last_round"], verdict["all_done"]))
        with open(os.path.join(out, "serial.log"), "rb") as lf:
            data = lf.read()
        for m in PANIC_MARKERS:
            f.write("marker %-24s count=%d\n" % (m.decode(), data.count(m)))
        f.write("serial_bytes=%d\n" % len(data))
    print("[verdict]", verdict)
    return 0 if not verdict["panic"] else 1


if __name__ == "__main__":
    sys.exit(main())
