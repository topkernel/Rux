#!/usr/bin/env python3
# arena_boot.py — boot the x86_64 kernel with the FFRT-arena probe as
# init and report the verdict (arena-mmap lane, TASK 110.7 item 2).
#
# The probe (test/arena.c, from userspace/probes/probes.tar.gz) is a
# fully-static musl binary: no loader needed, so the image is just
# mkfs.ext4 -d staging with /test/arena.
#
# Usage: ./arena_boot.py [--kernel <bzImage>] [--accel kvm|tcg]
#                        [--timeout 120] [--log <serial-log>]
# ARENA-PASS -> rc 0; ARENA-FAIL/panic/timeout -> rc 1.
import argparse
import os
import subprocess
import sys
import time

WT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
WORK = os.path.join(WT, "run", "arena-work")


def build_image(cfg):
    staging = os.path.join(WORK, "staging")
    if os.path.exists(staging):
        subprocess.run(["rm", "-rf", staging], check=True)
    os.makedirs(os.path.join(staging, "test"))
    subprocess.run(["cp", os.path.join(WT, "run", "arena-musl"),
                    os.path.join(staging, "test", "arena")], check=True)
    subprocess.run(["chmod", "+x", os.path.join(staging, "test", "arena")],
                   check=True)
    img = os.path.join(WORK, "arena.img")
    if os.path.exists(img):
        os.remove(img)
    r = subprocess.run(["mkfs.ext4", "-q", "-F",
                        "-O", "^metadata_csum,^flex_bg",
                        "-d", staging, img, "16M"],
                       capture_output=True, text=True)
    return img if r.returncode == 0 else None


def qemu_cmd(args, img, serial):
    accel = ("kvm" if args.accel == "kvm" else "tcg,thread=multi")
    return ["qemu-system-x86_64",
            "-M", "q35", "-accel", accel, "-cpu", "max",
            "-m", "8G", "-smp", "8",
            "-display", "none",
            "-serial", "file:" + serial,
            "-drive", "file=%s,if=none,id=rootfs,format=raw,"
                      "file.locking=off" % img,
            "-device", "virtio-blk-pci,drive=rootfs",
            "-kernel", args.kernel,
            "-append", "root=/dev/vda rw init=/test/arena console=ttyS0"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kernel",
                    default=os.path.join(
                        WT, "target/x86_64-unknown-none/debug/bzImage"))
    ap.add_argument("--accel", default="kvm", choices=["kvm", "tcg"])
    ap.add_argument("--timeout", type=int, default=150)
    ap.add_argument("--log", default=os.path.join(WT, "run",
                                                  "arena-rux-serial.log"))
    args = ap.parse_args()

    if not os.path.exists(args.kernel):
        print("[boot] kernel missing: %s" % args.kernel)
        return 2
    bin_path = os.path.join(WT, "run", "arena-musl")
    if not os.path.exists(bin_path):
        print("[boot] probe binary missing: %s" % bin_path)
        return 2

    os.makedirs(WORK, exist_ok=True)
    if os.path.exists(args.log):
        os.remove(args.log)
    img = build_image(args)
    if img is None:
        print("[img] mkfs failed")
        return 2

    proc = subprocess.Popen(qemu_cmd(args, img, args.log),
                            stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)
    print("[boot] qemu pid=%d accel=%s" % (proc.pid, args.accel))
    verdict = None
    t0 = time.time()
    try:
        while time.time() - t0 < args.timeout:
            time.sleep(2)
            if proc.poll() is not None:
                break
            try:
                data = open(args.log, "rb").read()
            except FileNotFoundError:
                continue
            if b"ARENA-PASS" in data or b"ARENA-FAIL" in data:
                verdict = "DONE"
                time.sleep(2)  # drain serial
                break
            if b"KERNPANIC" in data or b"Kernel panic" in data:
                verdict = "PANIC"
                break
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(5)
            except subprocess.TimeoutExpired:
                proc.kill()
    print("[boot] verdict: %s" % verdict)
    if os.path.exists(args.log):
        sys.stdout.write(open(args.log, errors="replace").read()[-8000:])
    return 0 if verdict == "DONE" else 1


if __name__ == "__main__":
    sys.exit(main())
