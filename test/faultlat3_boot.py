#!/usr/bin/env python3
"""faultlat3_boot.py — page-fault latency under the OH-style mixture.

Builds an ext4 rootfs with 6 x 16 MiB loader files (96 MiB aggregate —
larger than the old 32 MB page-cache cap), boots q35 + virtio-blk-pci
with init=/test/faultlat3 (6 mmap loaders + 2 journal-commit writers),
and prints the FAULTLAT3 lines from serial.

  FAULTLAT3 RESULT: SLOW wall_ms=<n>  -> rc 1
  FAULTLAT3 RESULT: FAST wall_ms=<n>  -> rc 0
  timeout                            -> rc 3

Usage: ./faultlat3_boot.py [--timeout 300] [--accel kvm] [--kernel PATH]
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
MUSL = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl")
MARKER = "FAULTLAT3 RESULT:"
WORK = os.path.join(WT, "work")
SIZE_MB = 64


def build_probe(out):
    cmd = ["gcc", "-static", "-nostdlib", "-O2", "-Wall",
           "-I", os.path.join(MUSL, "include"),
           "-o", out, os.path.join(HERE, "faultlat3.c"),
           os.path.join(MUSL, "lib/crt1.o"),
           os.path.join(MUSL, "lib/libc.a"), "-lgcc"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    return True


def build_image(img):
    staging = os.path.join(WORK, "faultlat3_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    shutil.copyfile(os.path.join(WORK, "faultlat3"),
                    os.path.join(staging, "test/faultlat3"))
    os.chmod(os.path.join(staging, "test/faultlat3"), 0o755)
    # deterministic non-trivial payload (1 MiB base block)
    blk = bytes(range(256)) * 256 * 16
    for i in range(6):
        big = os.path.join(staging, "test/l%d.bin" % i)
        with open(big, "wb") as f:
            for _ in range(16):
                f.write(blk)
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum",
                        "-d", staging, img, "%dM" % (SIZE_MB + 80)],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--accel", default="kvm")
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--kernel", default=os.path.join(
        WT, "target/x86_64-unknown-none/debug/bzImage"))
    ap.add_argument("--tag", default=None, help="suffix for serial log")
    args = ap.parse_args()

    os.makedirs(WORK, exist_ok=True)
    if not os.path.exists(args.kernel):
        print("[boot] kernel missing: %s" % args.kernel)
        return 2
    if not build_probe(os.path.join(WORK, "faultlat3")):
        return 2
    img = os.path.join(WORK, "faultlat3.img")
    if not build_image(img):
        return 2

    tag = args.tag or "run"
    serial = os.path.join(WORK, "faultlat3_serial_%s.log" % tag)
    if os.path.exists(serial):
        os.remove(serial)

    append = "root=/dev/vda rw init=/test/faultlat3 console=ttyS0"
    cmd = [QEMU, "-M", "q35", "-accel", args.accel, "-cpu", "max",
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none",
           "-serial", "file:" + serial,
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", args.kernel, "-append", append]
    print("[boot] qemu accel=%s smp=%s mem=%s kernel=%s"
          % (args.accel, args.smp, args.mem, args.kernel))
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        t0 = time.time()
        verdict = None
        while time.time() - t0 < args.timeout:
            time.sleep(2)
            if proc.poll() is not None:
                break
            try:
                with open(serial, "rb") as f:
                    data = f.read()
            except FileNotFoundError:
                continue
            if b"panicked" in data or b"Kernel panic" in data:
                print("[boot] KERNEL PANIC detected")
                verdict = "PANIC"
                break
            for line in data.split(b"\n"):
                if line.startswith(b"FAULTLAT3 "):
                    print("[probe] %s" % line.decode(errors="replace"))
                    if line.startswith(MARKER.encode()):
                        verdict = line.decode(errors="replace")
        if verdict is None:
            print("[boot] no verdict within %ds" % args.timeout)
            return 3
        if "SLOW" in verdict:
            return 1
        return 0
    finally:
        proc.terminate()
        try:
            proc.wait(5)
        except Exception:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
