#!/usr/bin/env python3
"""waitloss_boot.py — boot the x86_64 Rux kernel with the waitloss probe
as init and judge the wait4/SIGCHLD wakeup-loss family (99th, §87.8).

Verdicts:
  WAITLOSS-PASS  (WAITLOSS-DONE seen)               -> rc 0
  WAITLOSS-STALL (>=3 identical WD lines: the parent
                  froze while the guest stayed alive
                  — the wake was lost)              -> rc 1
  WAITLOSS-PANIC (kernel panic / trap / deadlock)   -> rc 2
  WAITLOSS-TIMEOUT (no DONE, no stall signature)    -> rc 3

Usage:
  ./waitloss_boot.py [--smp 2] [--secs 90] [--mem 2048M]
      [--kernel bzImage] [--accel kvm|tcg] [--show]
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(os.path.dirname(HERE))
QEMU = "qemu-system-x86_64"
MKFS = "/usr/sbin/mkfs.ext4"
MUSL_CC = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl", "bin",
                       "musl-gcc")

DONE_LINE = re.compile(rb"WAITLOSS-DONE")
WD_LINE = re.compile(
    rb"WD phase=(\d+) iter=(\d+) stage=(\d+) child=(\d+)")
FAIL_LINE = re.compile(
    rb"Kernel panic|panic: |DEADLOCK: spinlock stuck|BUG: soft lockup|"
    rb"trap: #GP|general protection fault|unexpected|fork-failed")


def build_image(img, probe_bin, name):
    staging = tempfile.mkdtemp(prefix="waitloss_root")
    os.makedirs(os.path.join(staging, "test"))
    shutil.copy(probe_bin, os.path.join(staging, "test", name))
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^64bit",
                        "-d", staging, img, "64M"],
                       capture_output=True)
    shutil.rmtree(staging)
    if r.returncode != 0:
        print("mkfs failed:", r.stderr.decode(), file=sys.stderr)
        sys.exit(4)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--smp", default="2")
    ap.add_argument("--secs", default="90")
    ap.add_argument("--mem", default="2048M",
                    help="Kernel.toml ships physical_memory=2048; a "
                         "smaller guest leaves the kernel heap carve "
                         "empty (boot PANIC: Allocation error 4MiB)")
    ap.add_argument("--kernel", default=None)
    ap.add_argument("--accel", default="kvm")
    ap.add_argument("--load", default="0",
                    help="number of guest-side loader children (I/O + "
                         "fork/exit churn); appended to init argv")
    ap.add_argument("--show", action="store_true")
    args = ap.parse_args()

    src = os.path.join(HERE, "waitloss_probe.c")
    binp = os.path.join(WT, "work", "waitloss_probe-x86_64")
    os.makedirs(os.path.join(WT, "work"), exist_ok=True)
    env = dict(os.environ)
    env.setdefault("REALGCC", "gcc")
    subprocess.run([MUSL_CC, "-static", "-O1", "-Wall",
                    "-o", binp, src], check=True, env=env)

    img = os.path.join(WT, "work", "waitloss_probe.img")
    build_image(img, binp, "waitloss_probe")

    kernel = args.kernel or os.path.join(
        WT, "target/x86_64-unknown-none/debug/bzImage")
    accel = (["-accel", "kvm", "-accel", "tcg"]
             if args.accel == "kvm" else ["-accel", "tcg,thread=multi"])
    cmd = [QEMU, "-M", "q35", "-cpu", "max", *accel,
           "-m", args.mem, "-smp", args.smp,
           "-display", "none", "-serial", "stdio", "-monitor", "none",
           "-no-reboot", "-snapshot",
           "-drive", f"file={img},if=none,id=rootfs,format=raw,file.locking=off",
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", kernel,
           "-append",
           "root=/dev/vda rw init=/test/waitloss_probe console=ttyS0"
           + (f" {args.load}" if args.load != "0" else "")]

    if args.show:
        print(" ".join(cmd))
    import time
    t0 = time.time()
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT)
    serial_path = os.path.join(WT, "work", "waitloss-serial.log")
    serial = open(serial_path, "wb")

    wd_counts = {}
    verdict = None
    detail = ""
    saw_done = False
    while True:
        if time.time() - t0 > float(args.secs):
            verdict = "TIMEOUT"
            detail = "no DONE within --secs"
            break
        line = proc.stdout.readline()
        if not line:
            if proc.poll() is not None:
                break
            continue
        serial.write(line)
        serial.flush()
        if DONE_LINE.search(line):
            saw_done = True
            break
        if FAIL_LINE.search(line):
            # "unexpected"/"fork-failed" are probe-side failures; kernel
            # text dominates this class, verdict PANIC either way but
            # note which line triggered.
            verdict = "PANIC"
            detail = line.decode("utf-8", "replace").strip()
            break
        m = WD_LINE.search(line)
        if m:
            key = m.groups()
            wd_counts[key] = wd_counts.get(key, 0) + 1
            if wd_counts[key] >= 3:
                verdict = "STALL"
                detail = ("parent frozen at phase=%s iter=%s stage=%s "
                          "child=%s while guest alive (WD repeated %d)"
                          % (key + (wd_counts[key],)))
                break

    if saw_done:
        verdict = "PASS"
    if verdict is None:
        verdict = "TIMEOUT"
        detail = "serial ended without DONE/stall"
    # Let the probe's own tail print; then kill the VM.
    try:
        proc.wait(timeout=3)
    except subprocess.TimeoutExpired:
        proc.kill()
    serial.close()

    print(f"WAITLOSS-{verdict} {detail}")
    print(f"serial: {serial_path} elapsed={time.time()-t0:.1f}s")
    rc = {"PASS": 0, "STALL": 1, "PANIC": 2, "TIMEOUT": 3}.get(verdict, 3)
    sys.exit(rc)


if __name__ == "__main__":
    main()
