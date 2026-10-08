#!/usr/bin/env python3
"""oh2_probe_boot.py — boot the Rux kernel with the OpenHarmony Phase 2
pre-work probe (memfd + ashmem + access_token_id) as init, stream the
serial log, report the verdict.

Builds a tiny private ext4 image containing only the probe (mkfs.ext4 -d,
no root privileges needed) and boots it with init=/test/oh2_probe. The
probe's own output is the verdict:

  OH2_PROBE RESULT: PASS n/n   -> rc 0
  OH2_PROBE RESULT: FAIL p/n   -> rc 1
  kernel panic                 -> rc 2

Usage:
  ./oh2_probe_boot.py [--smp 4] [--thread multi|single] [--timeout 180]
      [--kernel ELF]
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
MUSL = os.path.join(WT, "toolchain", "riscv64-rux-linux-musl")
CC = "riscv64-linux-gnu-gcc"
MKFS = "/usr/sbin/mkfs.ext4"
DEFAULT_KERNEL = os.path.join(
    WT, "target/riscv64gc-unknown-none-elf/debug/rux")
MARKER = "OH2_PROBE RESULT:"


def build_probe(out):
    """Compile the probe as a static musl binary (toolchain/README recipe)."""
    cmd = [CC, "-static", "-nostdlib", "-O2", "-Wall",
           "-I", os.path.join(MUSL, "include"),
           "-o", out, os.path.join(HERE, "oh2_probe.c"),
           os.path.join(MUSL, "lib", "crt1.o"),
           os.path.join(MUSL, "lib", "libc.a"),
           "-lgcc"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    return True


def build_image(img):
    staging = os.path.join(WT, "work", "oh2_probe_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    probe = os.path.join(WT, "work", "oh2_probe")
    if not build_probe(probe):
        return False
    shutil.copyfile(probe, os.path.join(staging, "test", "oh2_probe"))
    os.chmod(os.path.join(staging, "test", "oh2_probe"), 0o755)
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^flex_bg",
                        "-d", staging, img, "64M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--smp", type=int, default=4)
    ap.add_argument("--thread", default="multi", choices=["multi", "single"])
    ap.add_argument("--timeout", type=int, default=180)
    ap.add_argument("--mem", default="1G")
    ap.add_argument("--kernel", default=DEFAULT_KERNEL)
    args = ap.parse_args()

    os.makedirs(os.path.join(WT, "work"), exist_ok=True)
    img = os.path.join(WT, "work", "oh2_probe.img")
    if not build_image(img):
        return 2
    if not os.path.exists(args.kernel):
        print("[boot] kernel missing: %s" % args.kernel)
        return 2

    append = "root=/dev/vda rw init=/test/oh2_probe console=ttyS0"
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

    ser = open(os.path.join(WT, "work", "oh2_probe_serial.log"), "wb")
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
                        # A chunk boundary can deliver the marker prefix
                        # before the PASS/FAIL word arrives — only accept
                        # the line once it is complete.
                        if not (verdict.endswith("PASS") or " PASS " in verdict
                                or "FAIL" in verdict.split("RESULT:")[-1]):
                            verdict = None
                            continue
                        print("[boot] %s" % verdict)
                        rc = 0 if ("RESULT: PASS" in verdict) else 1
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
