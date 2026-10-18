#!/usr/bin/env python3
"""iostorm.py — focused virtio-blk I/O storm probe (the PCI_BLK wedge hunt).

Boots ONE QEMU on a private copy of the musl rootfs at SMP2 and feeds a
storage-heavy churn script (sequential + random writes/reads + fsync)
through the serial console, from two parallel shell loops. Watches the
serial log for the wedge markers:

  DEADLOCK: spinlock stuck      (the b125 wedge family)
  PCI-BLK-CONTEND / PCI-BLK-WEDGE (holder forensics from this branch)
  KERNPANIC / panic!

Discipline: only the QEMU PID this script started is killed; the log is
written under runs/iostorm-<tag>/.

Usage:
  iostorm.py --minutes 30 [--smp 2] [--arch riscv64] [--tag foo]
"""
import argparse
import os
import re
import shutil
import signal
import subprocess
import sys
import time

MARKERS = [
    b"DEADLOCK: spinlock stuck",
    b"PCI-BLK-CONTEND",
    b"PCI-BLK-WEDGE",
    b"KERNPANIC",
    b"panic!",
]

# Sequential + random churn, run via `toybox sh /stormN.sh` (mrsh's
# pasted-multiline parser is unreliable; scripts live in the image).
# Discipline: every file is removed the same iteration (no accumulation
# — an earlier version piled up dirty pages until OOM, which derails the
# probe into the KNOWN S001/S002 EIO cascade instead of exercising the
# BLK lock path). sync every 8 iterations keeps flush pressure without
# the every-iteration fsync storm.
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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--minutes", type=int, default=30)
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--arch", default="riscv64", choices=["riscv64", "x86_64"])
    ap.add_argument("--tag", default=None)
    ap.add_argument("--kernel", default=None,
                    help="override kernel path (baseline comparisons)")
    args = ap.parse_args()

    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    tag = args.tag or time.strftime("iostorm-%H%M%S")
    outdir = os.path.join(root, "runs", tag)
    os.makedirs(outdir, exist_ok=True)
    serial_path = os.path.join(outdir, "serial.log")

    if args.arch == "riscv64":
        qemu = "qemu-system-riscv64"
        kernel = os.path.join(root, "target/riscv64gc-unknown-none-elf/debug/rux")
        cmd = [qemu, "-M", "virt", "-accel", "tcg,thread=multi", "-cpu", "rv64",
               "-m", args.mem, "-smp", str(args.smp), "-nographic",
               "-drive", "file=test/rootfs.img,if=none,id=rootfs,format=raw",
               "-device", "virtio-blk-pci,drive=rootfs",
               "-kernel", kernel,
               "-append", "root=/dev/vda rw init=/bin/sh console=ttyS0"]
    else:
        qemu = "qemu-system-x86_64"
        kernel = os.path.join(root, "target/x86_64-unknown-none/debug/bzImage")
        cmd = [qemu, "-M", "q35", "-accel", "tcg,thread=multi", "-cpu", "max",
               "-m", args.mem, "-smp", str(args.smp), "-nographic",
               "-drive", "file=test/rootfs.img,if=none,id=rootfs,format=raw",
               "-device", "virtio-blk-pci,drive=rootfs",
               "-kernel", kernel,
               "-append", "root=/dev/vda rw init=/bin/sh console=ttyS0"]

    if args.kernel:
        kernel = args.kernel
    if not os.path.exists(kernel):
        print(f"kernel missing: {kernel}", file=sys.stderr)
        return 2

    # PRIVATE rootfs copy (monkey.py discipline): the shared test/rootfs.img
    # may be locked by another QEMU; real writes also exercise the disk.
    rootfs_private = os.path.join(outdir, "rootfs.img")
    if not os.path.exists(rootfs_private):
        shutil.copyfile(os.path.join(root, "test/rootfs.img"), rootfs_private)
        # Inject the churn scripts (mrsh's pasted-multiline parser is
        # unreliable; /bin/sh scripts run under toybox sh).
        for name, body in (("storm1.sh", STORM1), ("storm2.sh", STORM2)):
            local = os.path.join(outdir, name)
            with open(local, "w") as f:
                f.write(body)
            subprocess.run(["debugfs", "-w", "-R", f"write {local} /{name}",
                            rootfs_private], capture_output=True, check=False)
    cmd = [c.replace("file=test/rootfs.img", f"file={rootfs_private}") for c in cmd]

    print(f"[iostorm] booting {args.arch} smp={args.smp} for {args.minutes}m")
    print(f"[iostorm] serial -> {serial_path}")
    log = open(serial_path, "wb")
    proc = subprocess.Popen(cmd, cwd=root, stdin=subprocess.PIPE,
                            stdout=log, stderr=subprocess.STDOUT,
                            start_new_session=False)

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
                print(f"[iostorm] qemu exited rc={proc.returncode}")
                qemu_died = True
                break
            size = os.path.getsize(serial_path)
            if not fed and size > 4000:
                # Boot reached the shell (heuristic: enough output).
                script = b"toybox sh /storm1.sh &\ntoybox sh /storm2.sh &\n"
                try:
                    proc.stdin.write(script)
                    proc.stdin.flush()
                    fed = True
                    churn_sent_at = time.time()
                    print("[iostorm] churn scripts launched")
                except BrokenPipeError:
                    pass
            # Watchdog: no serial progress for 120s after churn started.
            if fed and size == last_len and churn_sent_at and \
                    time.time() - churn_sent_at > 60:
                if time.time() - last_progress > 120:
                    print(f"[iostorm] WEDGE: no serial progress for 120s "
                          f"(size={size})")
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

    # Classify.
    data = open(serial_path, "rb").read()
    hits = {m.decode(): len(re.findall(re.escape(m), data)) for m in MARKERS}
    seq_done = b"SEQ-DONE" in data
    rand_progress = re.findall(rb"RAND-(\d+)", data)
    print(f"[iostorm] markers: {hits}")
    print(f"[iostorm] seq-done={seq_done} rand-last="
          f"{max((int(x) for x in rand_progress), default=0)}")
    verdict = ("QEMU-DIED" if qemu_died else
               "WEDGE" if wedged else
               "MARKERS" if any(hits.values()) else "CLEAN")
    print(f"[iostorm] verdict: {verdict}")
    with open(os.path.join(outdir, "verdict.txt"), "w") as f:
        f.write(f"{verdict}\n{hits}\n")
    return 0 if verdict == "CLEAN" else 1


if __name__ == "__main__":
    sys.exit(main())
