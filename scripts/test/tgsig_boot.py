#!/usr/bin/env python3
"""tgsig_boot.py — boot the rt_tgsigqueueinfo parity probe (tgsig.c).

Minimal q35 boot (KVM or TCG), init=/test/tgsig, verdict from the
TGSIG RESULT line. rc 0 = PASS, 1 = FAIL, 3 = timeout/panic.

Usage: tgsig_boot.py [--accel kvm] [--kernel PATH]
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
WORK = os.path.join(WT, "work")


def build_probe():
    out = os.path.join(WORK, "tgsig")
    cmd = ["gcc", "-static", "-nostdlib", "-O2", "-Wall",
           "-I", os.path.join(MUSL, "include"),
           "-o", out,
           os.path.join(HERE, "tgsig.c"),
           os.path.join(MUSL, "lib/crt1.o"),
           os.path.join(MUSL, "lib/libc.a"), "-lgcc"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    return True


def build_image():
    staging = os.path.join(WORK, "tgsig_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    shutil.copyfile(os.path.join(WORK, "tgsig"),
                    os.path.join(staging, "test/tgsig"))
    os.chmod(os.path.join(staging, "test/tgsig"), 0o755)
    img = os.path.join(WORK, "tgsig.img")
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum",
                        "-d", staging, img, "64M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--accel", default="kvm")
    ap.add_argument("--kernel", default=os.path.join(
        WT, "target/x86_64-unknown-none/debug/bzImage"))
    ap.add_argument("--tag", default="run")
    args = ap.parse_args()

    os.makedirs(WORK, exist_ok=True)
    if not os.path.exists(args.kernel):
        print("[boot] kernel missing: %s" % args.kernel)
        return 2
    if not build_probe() or not build_image():
        return 2

    serial = os.path.join(WORK, "tgsig_serial_%s.log" % args.tag)
    if os.path.exists(serial):
        os.remove(serial)
    append = "root=/dev/vda rw init=/test/tgsig console=ttyS0"
    cmd = [QEMU, "-M", "q35", "-accel", args.accel, "-cpu", "max",
           "-m", "2G", "-smp", "2", "-display", "none",
           "-serial", "file:" + serial,
           "-drive", "file=%s/tgsig.img,if=none,id=rootfs,format=raw,file.locking=off" % WORK,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", args.kernel, "-append", append]
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    verdict = None
    try:
        t0 = time.monotonic()
        while time.monotonic() - t0 < 180:
            if proc.poll() is not None:
                break
            try:
                with open(serial, "rb") as f:
                    data = f.read()
            except FileNotFoundError:
                time.sleep(1)
                continue
            if b"panicked" in data or b"Kernel panic" in data:
                print("[boot] KERNEL PANIC detected")
                return 3
            for line in data.split(b"\n"):
                if line.startswith(b"TGSIG"):
                    print("[probe] %s" % line.decode(errors="replace"))
                    if line.startswith(b"TGSIG RESULT:"):
                        verdict = line
            if verdict is not None:
                break
            time.sleep(1)
        if verdict is None:
            print("[boot] no verdict")
            return 3
        return 0 if b"PASS" in verdict else 1
    finally:
        proc.terminate()
        try:
            proc.wait(5)
        except Exception:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
