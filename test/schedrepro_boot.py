#!/usr/bin/env python3
"""schedrepro_boot.py — boot the Rux riscv64 kernel with the standalone
scheduler-starvation reproducer as init and report the verdict.

Builds a tiny ext4 probe rootfs (just /test/schedrepro + /proc), boots
qemu virt TCG (thread=multi unless overridden) with
init=/test/schedrepro and `schedrepro=<cfg>` on the cmdline, tails the
serial log for SREPRO lines.

  SREPRO DONE verdict=REPRO max_ms=<n>  -> rc 0  (starvation reproduced)
  SREPRO DONE verdict=CLEAN max_ms=<n>  -> rc 1
  no DONE within timeout                -> rc 3

Usage:
  ./schedrepro_boot.py --tag sc281 --cfg "W8,x2:P4:H2:F2:R4:T60" \
      [--smp 2] [--accel tcg,thread=multi] [--timeout 180] [--append-extra "dfx=wwcensus"]
"""
import argparse
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(HERE)
KERNEL = os.path.join(WT, "target/riscv64gc-unknown-none-elf/debug/rux")
QEMU = "qemu-system-riscv64"
CC = "riscv64-linux-gnu-gcc"
SRC = os.path.join(HERE, "schedrepro.c")
MUSL = os.path.join(WT, "toolchain", "riscv64-rux-linux-musl")
WORK = os.path.join(WT, "work", "schedrepro")
DONE = "SREPRO DONE"


def build_probe(out):
    # Static glibc ships ifunc vector variants that SIGILL on this CPU;
    # link against the in-tree riscv64 musl instead (faultlat pattern).
    r = subprocess.run([CC, "-static", "-nostdlib", "-O2", "-w",
                        "-march=rv64gc_zba_zbb_zbs",
                        "-I", os.path.join(MUSL, "include"),
                        "-o", out, SRC,
                        os.path.join(MUSL, "lib/crt1.o"),
                        os.path.join(MUSL, "lib/libc.a"), "-lgcc"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    return True


def build_image(img, probe):
    staging = os.path.join(WORK, "root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    os.makedirs(os.path.join(staging, "proc"))
    shutil.copyfile(probe, os.path.join(staging, "test/schedrepro"))
    os.chmod(os.path.join(staging, "test/schedrepro"), 0o755)
    r = subprocess.run(["mkfs.ext4", "-q", "-F", "-O", "^metadata_csum",
                        "-d", staging, img, "64M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--tag", required=True)
    ap.add_argument("--cfg", default="W8,x2:P4:H2:F2:R4:T60")
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="1G")
    ap.add_argument("--accel", default="tcg,thread=multi")
    ap.add_argument("--timeout", type=int, default=180)
    ap.add_argument("--append-extra", default="dfx=wwcensus")
    ap.add_argument("--kernel", default=KERNEL)
    args = ap.parse_args()

    os.makedirs(WORK, exist_ok=True)
    probe = os.path.join(WORK, "schedrepro-rv")
    if not build_probe(probe):
        return 4
    img = os.path.join(WORK, "rootfs-%s.img" % args.tag)
    if not build_image(img, probe):
        return 4

    serial = os.path.join(WORK, "%s-serial.log" % args.tag)
    if os.path.exists(serial):
        os.remove(serial)
    append = ("root=/dev/vda rw init=/test/schedrepro console=ttyS0 "
              "schedrepro=%s %s" % (args.cfg, args.append_extra))
    cmd = [QEMU, "-M", "virt", "-accel", args.accel,
           "-cpu", "rv64,zbb=true,zba=true,zbs=true",
           "-m", args.mem, "-smp", str(args.smp),
           "-nographic", "-serial", "file:" + serial,
           "-monitor", "none",
           "-drive", "file=%s,if=none,id=rootfs,format=raw,snapshot=on" % img,
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", args.kernel, "-append", append]
    with open(os.path.join(WORK, "%s-qemu.log" % args.tag), "w") as qlog:
        p = subprocess.Popen(cmd, stdout=qlog, stderr=subprocess.STDOUT)
    print("[boot] qemu pid=%d tag=%s cfg=%s smp=%d" %
          (p.pid, args.tag, args.cfg, args.smp), flush=True)

    deadline = time.time() + args.timeout
    verdict = None

    def scan_done():
        if not os.path.exists(serial):
            return None
        with open(serial, "r", errors="replace") as f:
            txt = f.read()
        for line in txt.splitlines():
            if line.startswith(DONE):
                return line
        return None

    try:
        while time.time() < deadline:
            if p.poll() is not None:
                time.sleep(1.0)
                verdict = scan_done()
                break
            verdict = scan_done()
            if verdict is not None:
                break
            time.sleep(1.0)
    finally:
        if p.poll() is None:
            p.terminate()
            try:
                p.wait(5)
            except subprocess.TimeoutExpired:
                p.kill()

    with open(serial, "r", errors="replace") as f:
        txt = f.read()
    for line in txt.splitlines():
        if "SREPRO" in line or "STALL-" in line or "TIMER-SOFTIRQ-STALL" in line \
           or "PREEMPT-LEAK" in line or "PREEMPT-HEAL" in line or "PANIC" in line:
            print("[serial] " + line, flush=True)
    if verdict is None:
        print("SREPRO-TIMEOUT tag=%s" % args.tag)
        return 3
    time.sleep(1.0)
    print("[%s] %s" % (args.tag, verdict))
    return 0 if "verdict=REPRO" in verdict else 1


if __name__ == "__main__":
    sys.exit(main())
