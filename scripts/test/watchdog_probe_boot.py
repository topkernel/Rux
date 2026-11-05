#!/usr/bin/env python3
"""watchdog_probe_boot.py — boot the Rux x86_64 kernel with the
/dev/watchdog + stat-shape probe as init and report the verdict.

Builds a tiny private ext4 image containing only the probe (mkfs.ext4 -d,
no root privileges needed) and boots q35 with init=/test/watchdog_probe.
The probe checks (1) missing-leaf stat shapes (ENOENT never ENOTDIR —
the b151 watchdog_service forensics) and (2) the /dev/watchdog char
device ioctl round-trip (init_watchdog.c sequence).

  WDG_PROBE RESULT: PASS n/n   -> rc 0
  WDG_PROBE RESULT: FAIL p/n   -> rc 1
  kernel panic                  -> rc 2
  timeout                       -> rc 3

Usage:
  ./watchdog_probe_boot.py [--timeout 120] [--smp 1] [--mem 1G]
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
MARKER = "WDG_PROBE RESULT:"
WORK = os.path.join(WT, "work")


def build_probe(out):
    cmd = ["gcc", "-static", "-nostdlib", "-O2", "-Wall",
           "-I", os.path.join(MUSL, "include"),
           "-o", out, os.path.join(HERE, "watchdog_probe.c"),
           os.path.join(MUSL, "lib", "crt1.o"),
           os.path.join(MUSL, "lib", "libc.a"), "-lgcc"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    return True


def build_image(img):
    staging = os.path.join(WORK, "watchdog_probe_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    probe = os.path.join(WORK, "watchdog_probe")
    shutil.copyfile(probe, os.path.join(staging, "test", "watchdog_probe"))
    os.chmod(os.path.join(staging, "test", "watchdog_probe"), 0o755)
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^has_journal",
                        "-d", staging, img, "32M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=int, default=120)
    ap.add_argument("--smp", type=int, default=1)
    ap.add_argument("--mem", default="1G")
    args = ap.parse_args()

    os.makedirs(WORK, exist_ok=True)
    if not os.path.exists(BZIMAGE):
        print("[boot] bzImage missing: %s" % BZIMAGE)
        return 2
    probe = os.path.join(WORK, "watchdog_probe")
    if not build_probe(probe):
        return 2
    img = os.path.join(WORK, "watchdog_probe.img")
    if not build_image(img):
        return 2

    serial = os.path.join(WORK, "watchdog_probe_serial.log")
    if os.path.exists(serial):
        os.remove(serial)

    append = "root=/dev/vda rw init=/test/watchdog_probe console=ttyS0"
    cmd = [QEMU,
           "-M", "q35", "-accel", "tcg,thread=multi", "-cpu", "max",
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none",
           "-serial", "file:" + serial,
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", BZIMAGE,
           "-append", append]
    print("[boot] qemu smp=%s mem=%s" % (args.smp, args.mem))
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        t0 = time.time()
        verdict = None
        while time.time() - t0 < args.timeout:
            time.sleep(2)
            if proc.poll() is not None:
                break
            data = b""
            try:
                with open(serial, "rb") as f:
                    data = f.read()
            except FileNotFoundError:
                continue
            if b"panicked" in data or b"Kernel panic" in data:
                print("[boot] KERNEL PANIC detected")
                verdict = "PANIC"
                break
            idx = data.rfind(MARKER.encode())
            if idx >= 0:
                line = data[idx:].split(b"\n")[0].decode(errors="replace")
                print("[probe] %s" % line)
                verdict = line
                break
        if verdict is None:
            print("[boot] no verdict within %ds" % args.timeout)
            rc = 3
        elif verdict == "PANIC":
            rc = 2
        elif " PASS " in verdict:
            rc = 0
        else:
            rc = 1
        return rc
    finally:
        proc.terminate()
        try:
            proc.wait(5)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
