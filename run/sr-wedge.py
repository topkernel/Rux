#!/usr/bin/env python3
"""sr-wedge.py — contended-I/O wedge stress for the sync-realign lane.

Boots THIS worktree's x86 release kernel on a private copy of the musl
rootfs at SMP2 KVM and feeds three parallel churn loops through the
serial console — the LTP r2 WEDGE family shape (inode02/ftest02/04/06,
creat09, sendmsg02: many tasks creating/unlinking entries while the
EXT4_BIG_LOCK / VFS_MUTATION_LOCK holders block in virtio I/O):

  storm1: dd+cp+rm file churn (write + unlink paths, EXT4_BIG_LOCK)
  storm2: variable-size dd+rm churn (second CPU's I/O stream)
  storm3: creat-by-name + unlink + mkdir/rmdir churn (VFS_MUTATION_LOCK
          lookup+mutate windows: open O_CREAT, unlink, mkdir, rmdir)

Wedge detection: serial silence >150s after churn start, or the marker
set (DEADLOCK: spinlock stuck / KERNPANIC / panic! / EXT4-BIGLOCK-STALL
is informational, not a failure — it only prints at 3s+ waiter ages).

Usage: sr-wedge.py --minutes 6
"""
import argparse
import os
import re
import shutil
import signal
import subprocess
import sys
import time

MARKERS = [b"DEADLOCK: spinlock stuck", b"KERNPANIC", b"panic!"]

STORM1 = """#!/bin/sh
cd /tmp || exit 1
i=0
while [ $i -lt 1000000 ]; do
  dd if=/dev/zero of=/tmp/s$i bs=4k count=16 2>/dev/null
  head -c 4096 /tmp/s$i > /dev/null 2>&1
  cp /tmp/s$i /tmp/c$i 2>/dev/null
  rm -f /tmp/s$i /tmp/c$i
  i=$((i+1))
  if [ $((i % 8)) -eq 0 ]; then sync; fi
  [ $((i % 100)) -eq 0 ] && echo SEQ-$i
done
echo SEQ-DONE
"""

STORM2 = """#!/bin/sh
cd /tmp || exit 1
j=0
while [ $j -lt 1000000 ]; do
  n=$((j % 97 + 1))
  dd if=/dev/zero of=/tmp/z$j bs=512 count=$n 2>/dev/null
  head -c 1024 /tmp/z$j > /dev/null 2>&1
  rm -f /tmp/z$j
  j=$((j+1))
  if [ $((j % 8)) -eq 0 ]; then sync; fi
  [ $((j % 100)) -eq 0 ] && echo RAND-$j
done
echo RAND-DONE
"""

STORM3 = """#!/bin/sh
# creat09/ftest shape: named create/unlink + mkdir/rmdir under
# VFS_MUTATION_LOCK + EXT4_BIG_LOCK, with reads interleaved.
cd /tmp || exit 1
k=0
while [ $k -lt 1000000 ]; do
  echo payload-$k > /tmp/w$k
  cat /tmp/w$k > /dev/null 2>&1
  rm -f /tmp/w$k
  mkdir /tmp/d$k
  echo inner > /tmp/d$k/f
  rm -f /tmp/d$k/f
  rmdir /tmp/d$k
  k=$((k+1))
  [ $((k % 100)) -eq 0 ] && echo NAME-$k
done
echo NAME-DONE
"""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--minutes", type=int, default=6)
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--tag", default=None)
    args = ap.parse_args()

    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    outdir = os.path.join(root, "runs", args.tag or time.strftime("srwedge-%H%M%S"))
    os.makedirs(outdir, exist_ok=True)
    serial_path = os.path.join(outdir, "serial.log")

    kernel = os.path.join(root, "target/x86_64-unknown-none/release/bzImage")
    if not os.path.exists(kernel):
        print(f"kernel missing: {kernel}", file=sys.stderr)
        return 2

    # PRIVATE copy of the x86 lmbench image (full Ubuntu-class toolset:
    # sh/dd/rm/mkdir/cp/cat/sync/head — the shared test/rootfs.img is
    # riscv64 binaries, unexecutable on the x86 kernel); inject the
    # churn scripts via debugfs.
    rootfs_private = os.path.join(outdir, "rootfs.img")
    if not os.path.exists(rootfs_private):
        shutil.copyfile("/home/william/rux-images/testimages/lmbench-x86.img",
                        rootfs_private)
        for name, body in (("storm1.sh", STORM1), ("storm2.sh", STORM2),
                           ("storm3.sh", STORM3)):
            local = os.path.join(outdir, name)
            with open(local, "w") as f:
                f.write(body)
            subprocess.run(
                ["debugfs", "-w", "-R", f"write {local} /{name}", rootfs_private],
                capture_output=True, check=False)

    cmd = ["qemu-system-x86_64", "-M", "q35", "-accel", "kvm", "-cpu", "host",
           "-m", "2G", "-smp", str(args.smp), "-nographic",
           "-drive", f"file={rootfs_private},if=none,id=rootfs,format=raw",
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", kernel,
           "-append", "root=/dev/vda rw init=/bin/sh console=ttyS0"]

    print(f"[sr-wedge] booting smp={args.smp} kvm for {args.minutes}m")
    print(f"[sr-wedge] serial -> {serial_path}")
    log = open(serial_path, "wb")
    proc = subprocess.Popen(cmd, cwd=root, stdin=subprocess.PIPE,
                            stdout=log, stderr=subprocess.STDOUT)

    deadline = time.time() + args.minutes * 60
    fed = False
    churn_sent_at = None
    last_len = 0
    last_progress = time.time()
    wedged = False
    qemu_died = False
    try:
        while time.time() < deadline:
            time.sleep(5)
            if proc.poll() is not None:
                print(f"[sr-wedge] qemu exited rc={proc.returncode}")
                qemu_died = True
                break
            size = os.path.getsize(serial_path)
            if not fed and size > 4000:
                try:
                    proc.stdin.write(b"exec /bin/sh -c 'sh /storm1.sh & sh /storm2.sh & sh /storm3.sh &'\n")
                    proc.stdin.flush()
                    fed = True
                    churn_sent_at = time.time()
                    print("[sr-wedge] churn scripts launched")
                except BrokenPipeError:
                    pass
            if fed and size == last_len and churn_sent_at and \
                    time.time() - churn_sent_at > 60:
                if time.time() - last_progress > 150:
                    print(f"[sr-wedge] WEDGE: no serial progress for 150s (size={size})")
                    wedged = True
                    break
            else:
                last_progress = time.time()
            last_len = size
    finally:
        try:
            proc.send_signal(signal.SIGTERM)
            proc.wait(timeout=15)
        except Exception:
            proc.kill()
        log.close()

    data = open(serial_path, "rb").read()
    hits = {m.decode(): len(re.findall(re.escape(m), data)) for m in MARKERS}
    stalls = len(re.findall(rb"STALL", data))
    prog = {
        "SEQ": max((int(x) for x in re.findall(rb"SEQ-(\d+)", data)), default=0),
        "RAND": max((int(x) for x in re.findall(rb"RAND-(\d+)", data)), default=0),
        "NAME": max((int(x) for x in re.findall(rb"NAME-(\d+)", data)), default=0),
    }
    print(f"[sr-wedge] markers: {hits} stall-prints: {stalls}")
    print(f"[sr-wedge] progress: {prog}")
    verdict = ("QEMU-DIED" if qemu_died else
               "WEDGE" if wedged else
               "MARKERS" if any(hits.values()) else "CLEAN")
    print(f"[sr-wedge] verdict: {verdict}")
    with open(os.path.join(outdir, "verdict.txt"), "w") as f:
        f.write(f"{verdict}\n{hits}\nstalls={stalls}\n{prog}\n")
    return 0 if verdict == "CLEAN" else 1


if __name__ == "__main__":
    sys.exit(main())
