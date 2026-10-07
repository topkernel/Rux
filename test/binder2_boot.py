#!/usr/bin/env python3
"""binder2_boot.py — boot the Rux kernel with the binder S2 samgr-lifecycle
probe as init and report the verdict.

  B2 PROBE RESULT: PASS ... -> rc 0
  B2 PROBE RESULT: FAIL ... -> rc 1
  B2 PROBE RESULT: TIMEOUT  -> rc 1
  kernel panic             -> rc 2

Usage: ./binder2_boot.py [--smp 2] [--timeout 240] [--kernel ELF]
"""
import argparse
import os
import select
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(HERE)
QEMU = "/usr/bin/qemu-system-riscv64"
CC = "riscv64-linux-gnu-gcc"
MKFS = "/usr/sbin/mkfs.ext4"
DEFAULT_KERNEL = os.path.join(
    WT, "target/riscv64gc-unknown-none-elf/debug/rux")
MARKER = "B2 PROBE RESULT:"


def build_image(img):
    staging = os.path.join(WT, "work", "binder2_probe_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    probe = os.path.join(WT, "work", "binder2_probe")
    subprocess.run([CC, "-static", "-O2", "-Wall",
                    "-o", probe, os.path.join(HERE, "binder2_probe.c")],
                   check=True)
    shutil.copyfile(probe, os.path.join(staging, "test", "binder2_probe"))
    os.chmod(os.path.join(staging, "test", "binder2_probe"), 0o755)
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^flex_bg",
                        "-d", staging, img, "64M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--thread", default="multi", choices=["multi", "single"])
    ap.add_argument("--timeout", type=int, default=240)
    ap.add_argument("--mem", default="1G")
    ap.add_argument("--kernel", default=DEFAULT_KERNEL)
    args = ap.parse_args()

    os.makedirs(os.path.join(WT, "work"), exist_ok=True)
    img = os.path.join(WT, "work", "binder2_probe.img")
    if not build_image(img):
        return 2
    if not os.path.exists(args.kernel):
        print("[boot] kernel missing: %s" % args.kernel)
        return 2

    append = "root=/dev/vda rw init=/test/binder2_probe console=ttyS0"
    cmd = [QEMU,
           "-M", "virt", "-accel", "tcg,thread=" + args.thread,
           "-cpu", "rv64,zbb=true,zba=true,zbs=true",
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none", "-serial", "stdio",
           "-drive", "file=%s,if=none,id=rootfs,format=raw" % img,
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", args.kernel,
           "-append", append]
    print("[boot] qemu smp=%d thread=%s mem=%s" % (args.smp, args.thread, args.mem))

    ser = open(os.path.join(WT, "work", "binder2_serial.log"), "wb")
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

    buf = bytearray()
    t0 = time.time()
    verdict = None
    rc = 3
    try:
        while time.time() - t0 < args.timeout:
            r, _, _ = select.select([proc.stdout], [], [], 1.0)
            if r:
                chunk = os.read(proc.stdout.fileno(), 65536)
                if not chunk:
                    break
                buf.extend(chunk)
                ser.write(chunk)
                ser.flush()
                text = bytes(buf[-32768:])
                if b"panicked" in text or b"Kernel panic" in text:
                    print("[boot] KERNEL PANIC detected")
                    rc = 2
                    break
                for line in text.split(b"\n"):
                    if line.startswith(MARKER.encode()):
                        verdict = line.decode(errors="replace").strip()
                        print("[boot] %s" % verdict)
                        rc = 0 if " PASS " in verdict or verdict.endswith("PASS") or "PASS" in verdict else 1
                if verdict:
                    break
            if proc.poll() is not None:
                break
    finally:
        proc.kill()
        proc.wait()
        ser.close()
    if rc == 3 and not verdict:
        print("[boot] no verdict (timeout or silent exit)")
    return rc


if __name__ == "__main__":
    sys.exit(main())
