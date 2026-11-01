#!/usr/bin/env python3
"""fb_boot.py — boot the forkbench attribution probe (test/forkbench.c)
as init on the strong-profile x86_64 kernel under KVM and print the
PBENCH lines. Same shape as pbench_boot.py but init=/test/forkbench.

Usage: ./fb_boot.py [--bzimage PATH] [--tag NAME] [--repeats N]
"""
import argparse
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(HERE)
QEMU = "qemu-system-x86_64"
MKFS = "/usr/sbin/mkfs.ext4"
MUSL_CC = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl", "bin", "musl-gcc")
WORK = os.path.join(WT, "work")


def foreign_qemus():
    try:
        out = subprocess.run(["pgrep", "-c", "qemu-system"], capture_output=True,
                             text=True).stdout.strip()
        return int(out) if out else 0
    except Exception:
        return -1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bzimage", default=os.path.join(WT, "run", "base-bzImage"))
    ap.add_argument("--tag", default="fb")
    ap.add_argument("--repeats", type=int, default=1)
    ap.add_argument("--timeout", type=int, default=180)
    ap.add_argument("--append", default="",
                    help="extra kernel cmdline tokens (e.g. 'forkprof=1')")
    ap.add_argument("--trace", action="store_true",
                    help="build the probe in FB_TRACE mode (mm-only, N=400)")
    args = ap.parse_args()

    probe = os.path.join(WORK, "forkbench")
    src = os.path.join(WT, "test", "forkbench.c")
    cc = [MUSL_CC, "-static", "-O2"]
    probe = os.path.join(WORK, "forkbench")
    if args.trace:
        cc.append("-DFB_TRACE")
        probe = os.path.join(WORK, "forkbench-trace")
    r = subprocess.run(cc + ["-o", probe, src],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] failed: %s" % r.stderr)
        return 2

    staging = os.path.join(WORK, "fb_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    shutil.copyfile(probe, os.path.join(staging, "test", "forkbench"))
    os.chmod(os.path.join(staging, "test", "forkbench"), 0o755)  # noqa: same name
    img = os.path.join(WORK, "fb.img")
    r = subprocess.run([MKFS, "-q", "-F", "-O",
                        "^metadata_csum,^orphan_file", "-d", staging, img, "16M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[mkfs] failed: %s" % r.stderr)
        return 2

    rc = 0
    for i in range(args.repeats):
        log = os.path.join(WORK, "fb-%s-%d.log" % (args.tag, i))
        cmd = [QEMU, "-M", "q35", "-accel", "kvm", "-cpu", "host",
               "-m", "2G", "-smp", "2", "-display", "none", "-vga", "none",
               "-serial", "file:%s" % log, "-monitor", "none",
               "-kernel", args.bzimage,
               "-append", "root=/dev/vda rw init=/test/forkbench console=ttyS0 "
                          + args.append,
               "-drive", "file=%s,if=none,id=rootfs,format=raw,snapshot=on,file.locking=off" % img,
               "-device", "virtio-blk-pci,drive=rootfs"]
        p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL)
        import time as _t
        deadline = _t.time() + args.timeout
        while _t.time() < deadline and p.poll() is None:
            _t.sleep(2)
            try:
                with open(log, "rb") as f:
                    tail = f.read()[-4096:]
                if b"Power down" in tail or b"PANIC" in tail:
                    _t.sleep(1)
                    break
            except OSError:
                pass
        if p.poll() is None:
            p.kill()
        p.wait()
        print("=== run %d (foreign_qemus=%d)" % (i, foreign_qemus()))
        with open(log, encoding="utf-8", errors="replace") as f:
            for line in f:
                line = line.rstrip("\r\n")
                if "PBENCH" in line or "PANIC" in line or "panic" in line.lower():
                    print(line)
    return rc


if __name__ == "__main__":
    sys.exit(main())
