#!/usr/bin/env python3
"""bigfair.py — EXT4_BIG_LOCK fairness probe driver (the b139 hunt).

Boots ONE QEMU on a private copy of the musl rootfs at SMP2 (riscv64
TCG by default) and runs /test/biglock_fair: many tenants driving
page-cache MISS reads through ext4 while a 10 Hz canary reader asserts
bounded latency. The run fails on either signal:

  - BIGFAIR-FAIL from the probe (canary max latency >= bound), or
  - any EXT4-BIGLOCK-STALL line in the serial log (kernel tripwire).

Usage:
  bigfair.py [--minutes 2] [--smp 2] [--tag foo] [--kernel PATH] \\
             [--bound-ms 2000] [--mem 1G]
"""
import argparse
import os
import re
import shutil
import signal
import subprocess
import sys
import time

STALL_RE = re.compile(rb"EXT4-BIGLOCK-STALL")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--minutes", type=int, default=2)
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="1G")
    ap.add_argument("--tag", default=None)
    ap.add_argument("--kernel", default=None)
    ap.add_argument("--bound-ms", type=int, default=2000)
    ap.add_argument("--rootfs", default=None,
                    help="use this prepared rootfs copy (probe injected) "
                         "instead of copying test/rootfs.img")
    args = ap.parse_args()

    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    tag = args.tag or time.strftime("bigfair-%H%M%S")
    outdir = os.path.join(root, "runs", tag)
    os.makedirs(outdir, exist_ok=True)
    serial_path = os.path.join(outdir, "serial.log")

    qemu = "qemu-system-riscv64"
    kernel = os.path.join(root, "target/riscv64gc-unknown-none-elf/debug/rux")
    if args.kernel:
        kernel = args.kernel
    if not os.path.exists(kernel):
        print(f"kernel missing: {kernel}", file=sys.stderr)
        return 2

    # PRIVATE rootfs copy (iostorm discipline): the shared test/rootfs.img
    # may be locked by another QEMU; the probe also writes real files.
    rootfs_private = os.path.join(outdir, "rootfs.img")
    if not os.path.exists(rootfs_private):
        if args.rootfs:
            shutil.copyfile(args.rootfs, rootfs_private)
        else:
            rootfs_shared = os.path.join(root, "test/rootfs.img")
            shutil.copyfile(rootfs_shared, rootfs_private)

    cmd = [qemu, "-M", "virt", "-accel", "tcg,thread=multi", "-cpu", "rv64",
           "-m", args.mem, "-smp", str(args.smp), "-nographic",
           "-drive", f"file={rootfs_private},if=none,id=rootfs,format=raw,file.locking=off,snapshot=on",
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", kernel,
           "-append", "root=/dev/vda rw init=/bin/sh console=ttyS0"]

    probe_secs = args.minutes * 60
    # Probe runtime + corpus setup (40MB of writes under TCG) + boot.
    hard_deadline = probe_secs + 15 * 60

    print(f"[bigfair] booting smp={args.smp} mem={args.mem} "
          f"probe={probe_secs}s bound={args.bound_ms}ms")
    print(f"[bigfair] serial -> {serial_path}")
    log = open(serial_path, "wb")
    proc = subprocess.Popen(cmd, cwd=root, stdin=subprocess.PIPE,
                            stdout=log, stderr=subprocess.STDOUT)

    fed = False
    verdict = None
    last_len = 0
    last_progress = time.time()
    deadline = time.time() + hard_deadline
    try:
        while time.time() < deadline:
            time.sleep(5)
            if proc.poll() is not None:
                print(f"[bigfair] qemu exited rc={proc.returncode}")
                verdict = verdict or "QEMU-DIED"
                break
            data_len = os.path.getsize(serial_path)
            data = open(serial_path, "rb").read()
            if not fed and b"mrsh" in data[-4000:] or (not fed and data_len > 8000):
                line = f"/test/biglock_fair {probe_secs} {args.bound_ms}\n"
                try:
                    proc.stdin.write(line.encode())
                    proc.stdin.flush()
                    fed = True
                    print("[bigfair] probe launched")
                except BrokenPipeError:
                    pass
            if b"BIGFAIR-PASS" in data:
                verdict = "PROBE-PASS"
                break
            if b"BIGFAIR-FAIL" in data:
                verdict = "PROBE-FAIL"
                break
            if data_len == last_len and time.time() - last_progress > 180:
                verdict = "WEDGE"
                print(f"[bigfair] WEDGE: no serial progress for 180s")
                break
            if data_len != last_len:
                last_progress = time.time()
            last_len = data_len
    finally:
        try:
            proc.send_signal(signal.SIGTERM)
            proc.wait(timeout=15)
        except Exception:
            proc.kill()
        log.close()

    data = open(serial_path, "rb").read()
    stalls = len(STALL_RE.findall(data))
    m = re.search(
        rb"BIGFAIR-RESULT pass=(\d) canary_n=(\d+) canary_max_ms=(\d+) "
        rb"over500ms=(\d+) over1s=(\d+) hammer_ops=(\d+) churn_ops=(\d+) "
        rb"fault_ops=(\d+)", data)
    prog = re.findall(rb"BIGFAIR-T (\d+) .*canary_max=(\d+)ms", data)
    if m:
        print(f"[bigfair] probe: pass={m.group(1).decode()} "
              f"canary_n={m.group(2).decode()} max={m.group(3).decode()}ms "
              f">500ms={m.group(4).decode()} >1s={m.group(5).decode()} "
              f"hammer={m.group(6).decode()} churn={m.group(7).decode()} "
              f"fault={m.group(8).decode()}")
    if prog:
        print(f"[bigfair] canary_max timeline: "
              + " ".join(f"{a.decode()}s:{b.decode()}" for a, b in prog[-6:]))
    print(f"[bigfair] EXT4-BIGLOCK-STALL lines: {stalls}")

    ok = (verdict == "PROBE-PASS") and stalls == 0
    final = ("PASS" if ok else
             f"FAIL({verdict},stalls={stalls})")
    print(f"[bigfair] verdict: {final}")
    with open(os.path.join(outdir, "verdict.txt"), "w") as f:
        f.write(f"{final}\nstalls={stalls}\n")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
