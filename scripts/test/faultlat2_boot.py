#!/usr/bin/env python3
"""faultlat2_boot.py — boot the Rux x86_64 kernel with the file-backed
page-fault latency probe as init and report the verdict.

Builds a tiny ext4 rootfs with a preallocated 32 MiB data file, boots
q35 + virtio-blk-pci (same device class the OH boots use) with
init=/test/faultlat2, and prints the FAULTLAT2 lines from serial.

  FAULTLAT2 RESULT: SLOW avg_page_us=<n>  -> rc 1
  FAULTLAT2 RESULT: FAST avg_page_us=<n>  -> rc 0
  timeout                            -> rc 3

Usage: ./faultlat2_boot.py [--timeout 240] [--accel kvm]
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
BZIMAGE = os.path.join(WT, "target/x86_64-unknown-none/debug/bzImage")
MUSL = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl")
MARKER = "FAULTLAT2 RESULT:"
WORK = os.path.join(WT, "work")
SIZE_MB = 64


def build_probe(out):
    cmd = ["gcc", "-static", "-nostdlib", "-O2", "-Wall",
           "-I", os.path.join(MUSL, "include"),
           "-o", out, os.path.join(HERE, "faultlat2.c"),
           os.path.join(MUSL, "lib/crt1.o"),
           os.path.join(MUSL, "lib/libc.a"), "-lgcc"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    return True


def build_image(img):
    staging = os.path.join(WORK, "faultlat2_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    probe = os.path.join(WORK, "faultlat2")
    shutil.copyfile(probe, os.path.join(staging, "test/faultlat2"))
    os.chmod(os.path.join(staging, "test/faultlat2"), 0o755)
    # deterministic non-trivial payload
    blk = bytes(range(256)) * 256 * 16  # 1 MiB
    for i in range(6):
        big = os.path.join(staging, "test/d%d.bin" % i)
        with open(big, "wb") as f:
            for _ in range(8):
                f.write(blk)
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum",
                        "-d", staging, img, "%dM" % (SIZE_MB + 32)],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=int, default=240)
    ap.add_argument("--accel", default="tcg,thread=multi")
    ap.add_argument("--smp", type=int, default=1)
    ap.add_argument("--mem", default="2G")
    args = ap.parse_args()

    os.makedirs(WORK, exist_ok=True)
    if not os.path.exists(BZIMAGE):
        print("[boot] bzImage missing: %s" % BZIMAGE)
        return 2
    if not build_probe(os.path.join(WORK, "faultlat2")):
        return 2
    img = os.path.join(WORK, "faultlat2.img")
    if not build_image(img):
        return 2

    serial = os.path.join(WORK, "faultlat2_serial.log")
    if os.path.exists(serial):
        os.remove(serial)

    append = "root=/dev/vda rw init=/test/faultlat2 console=ttyS0"
    cmd = [QEMU, "-M", "q35", "-accel", args.accel, "-cpu", "max",
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none",
           "-serial", "file:" + serial,
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", BZIMAGE, "-append", append]
    print("[boot] qemu accel=%s smp=%s mem=%s" % (args.accel, args.smp, args.mem))
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
                if line.startswith(b"FAULTLAT2 "):
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
