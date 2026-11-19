#!/usr/bin/env python3
"""lmbench_boot.py — boot the Rux x86_64 kernel with the lmbench
benchmark binaries as the payload and collect results.

x86_64 + KVM per the 2026-10-31 directive: all performance work runs on
this rig. Mirrors pbench_boot.py (static musl binaries into a small
ext4 rootfs, init runs the suite, PBENCH-style lines off the serial).

  rc 0  full run (DONE seen)
  rc 1  kernel panic before DONE
  rc 3  no DONE within --timeout
  rc 2  build/image failure

Usage: ./lmbench_boot.py [--timeout 1800] [--bzimage PATH] [--tag NAME]
       [--accel kvm] [--smp 2] [--mem 1G] [--bench-set quick|full]
"""
import argparse
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(HERE)
QEMU = "qemu-system-x86_64"
MKFS = "/usr/sbin/mkfs.ext4"
LMB = os.path.dirname(os.path.abspath(__file__))
DEFAULT_BZIMAGE = os.path.join(WT, "target", "x86_64-unknown-none", "release", "bzImage")
WORK = "/home/william/rux-agents/code-review-perf/work"
FS_BLOCKS = 262144  # 256 MiB in 1KiB blocks

QUICK_SET = [
    ("lat_syscall", "null"), ("lat_syscall", "read"), ("lat_syscall", "write"),
    ("lat_syscall", "stat"), ("lat_syscall", "fstat"), ("lat_syscall", "open"),
    ("lat_pagefault", "1M"), ("lat_mmap", "1M"),
    ("lat_pipe", ""), ("lat_unix", ""), ("lat_fifo", ""),
    ("lat_ctx", "2 4 8"), ("lat_proc", "fork"), ("lat_proc", "exec"),
    ("lat_sem", ""), ("lat_fs", "1k"),
    ("lat_mem_rd", "512"), ("lat_mem_rd", "1m"),
    ("bw_mem", "1m"), ("bw_mem", "4m"),
    ("bw_pipe", ""), ("bw_unix", ""),
    ("bw_file_rd", "1m"), ("bw_mmap_rd", "1m"),
    ("lat_tcp", ""), ("lat_udp", ""), ("lat_connect", ""),
    ("bw_tcp", ""), ("bw_udp", ""),
    ("lat_usleep", ""),
]


def foreign_qemus():
    try:
        out = subprocess.run(["pgrep", "-c", "qemu-system"], capture_output=True,
                             text=True).stdout.strip()
        return int(out) if out else 0
    except Exception:
        return -1


def build_image(img, bindir, script):
    """Small ext4 with the lmbench binaries + runner script as init."""
    if os.path.exists(img):
        os.remove(img)
    stage = os.path.join(WORK, "lmbench-stage")
    shutil.rmtree(stage, ignore_errors=True)
    os.makedirs(os.path.join(stage, "test"), exist_ok=True)
    for b in os.listdir(bindir):
        shutil.copy(os.path.join(bindir, b), os.path.join(stage, "test", b))
    shutil.copy(script, os.path.join(stage, "init.sh"))
    os.makedirs(stage, exist_ok=True)

    r = subprocess.run([MKFS, "-q", "-O",
                        "^inline_data,^meta_bg,^encrypt,^verity,^huge_file,^bigalloc,^orphan_file,^ext_attr,^metadata_csum,^metadata_csum_seed",
                        "-d", stage, "-i", "8192", img, str(FS_BLOCKS)],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[image] mkfs failed: %s" % r.stderr)
        return False
    print("[image] %s (%d KiB blocks)" % (img, FS_BLOCKS))
    return True


def build_runner(out):
    """Compile the C init-runner (Rux cannot exec shell scripts)."""
    r = subprocess.run(["/home/william/rux-agents/code-review-perf/toolchain/"
                        "x86_64-rux-linux-musl/bin/musl-gcc", "-static", "-O2",
                        "-o", out, os.path.join(LMB, "lmbench_run.c")],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[runner] build failed: %s" % r.stderr)
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=int, default=1800)
    ap.add_argument("--bzimage", default=DEFAULT_BZIMAGE)
    ap.add_argument("--tag", default="lm1")
    ap.add_argument("--accel", default="kvm")
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--bench-set", default="quick", choices=["quick"])
    args = ap.parse_args()

    if not os.path.exists(args.bzimage):
        print("[boot] bzImage missing: %s" % args.bzimage)
        return 2

    bindir = os.path.join(LMB, "bin")
    if not os.path.isdir(bindir) or not os.listdir(bindir):
        r = subprocess.run([os.path.join(LMB, "build-lmbench.sh")],
                           capture_output=True, text=True)
        if r.returncode != 0:
            print("[build] lmbench binaries missing")
            return 2

    os.makedirs(WORK, exist_ok=True)
    img = os.path.join(WORK, "lmbench-%s.img" % args.tag)
    runner = os.path.join(WORK, "lmbench_init")
    if not build_runner(runner):
        return 2
    if not build_image(img, bindir, runner):
        return 2

    serial = os.path.join(WORK, "lmbench-%s-serial.log" % args.tag)
    if os.path.exists(serial):
        os.remove(serial)
    fq = foreign_qemus()
    print("[boot] tag=%s accel=%s smp=%d mem=%s foreign_qemus=%d"
          % (args.tag, args.accel, args.smp, args.mem, fq))

    qemu = [
        QEMU, "-M", "q35", "-accel", args.accel, "-cpu", "host",
        "-m", args.mem, "-smp", str(args.smp),
        "-display", "none", "-vga", "none",
        "-serial", "file:" + serial,
        "-monitor", "none",
        "-kernel", args.bzimage,
        "-append", "root=/dev/vda rw init=/init.sh console=ttyS0",
        "-drive", "file=%s,if=none,id=rootfs,format=raw" % img,
        "-device", "virtio-blk-pci,drive=rootfs",
        "-device", "virtio-net-pci,netdev=n0",
        "-netdev", "user,id=n0",
    ]
    proc = subprocess.Popen(qemu, stdout=sys.stdout, stderr=sys.stderr)
    deadline = time.time() + args.timeout
    rc = 3
    try:
        for _ in range(100):  # QEMU creates the serial file lazily
            if os.path.exists(serial):
                break
            if proc.poll() is not None:
                print("[boot] QEMU exited early")
                return 1
            time.sleep(0.1)
        with open(serial, "rb") as f:
            while time.time() < deadline:
                line = f.readline()
                if not line:
                    if proc.poll() is not None:
                        break
                    time.sleep(0.2)
                    continue
                text = line.decode("utf-8", "replace").rstrip()
                if "LMBENCH" in text or "BW " in text or "Latencies" in text:
                    print("[probe] " + text)
                if "LMBENCH DONE" in text:
                    rc = 0
                    break
                if proc.poll() is not None and "DONE" not in text:
                    # drained
                    pass
    finally:
        try:
            proc.terminate()
            proc.wait(timeout=10)
        except Exception:
            try:
                proc.kill()
            except Exception:
                pass
    print("[boot] done rc=%d" % rc)
    print("[boot] serial log: %s" % serial)
    return rc


if __name__ == "__main__":
    sys.exit(main())
