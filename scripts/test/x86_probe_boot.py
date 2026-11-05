#!/usr/bin/env python3
"""x86_probe_boot.py — boot the x86_64 Rux kernel with a test probe as
init on a tiny private ext4 image (mkfs.ext4 -d — no root needed), stream
the serial log, report a verdict.

The x86_64 mirror of vma_storm_boot.py: same verdict discipline.

  <PROBE>-PASS               -> rc 0
  <PROBE>-FAIL / FAIL-*      -> rc 1
  kernel DEADLOCK / panic /
  soft lockup                -> rc 2
  timeout                    -> rc 3

Usage:
  ./x86_probe_boot.py writestorm|gwp_churn [--smp 2] [--secs 30]
      [--mem 512M] [--kernel bzImage] [--keep-img] [--show]
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(HERE)
QEMU = "qemu-system-x86_64"
CC = "gcc"
MKFS = "/usr/sbin/mkfs.ext4"

PASS_LINE = re.compile(rb"(WRITESTORM|GWP)-PASS")
FAIL_LINE = re.compile(rb"(WRITESTORM|GWP)FAIL-|GWP-FAIL|WRITESTORM-FAIL|GWPFAIL")
WEDGE_LINE = re.compile(rb"DEADLOCK: spinlock stuck|BUG: soft lockup|Kernel panic|panic: ")


def build_image(img, probe_bin, name):
    staging = tempfile.mkdtemp(prefix="x86probe_root")
    os.makedirs(os.path.join(staging, "test"))
    shutil.copy(probe_bin, os.path.join(staging, "test", name))
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^64bit",
                        "-d", staging, img, "64M"],
                       capture_output=True)
    shutil.rmtree(staging)
    if r.returncode != 0:
        print("mkfs failed:", r.stderr.decode(), file=sys.stderr)
        sys.exit(4)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("probe", choices=["writestorm", "gwp_churn"])
    ap.add_argument("--smp", default="2")
    ap.add_argument("--secs", default="30")
    ap.add_argument("--mem", default="512M")
    ap.add_argument("--kernel", default=os.path.join(
        WT, "target/x86_64-unknown-none/debug/bzImage"))
    ap.add_argument("--keep-img", action="store_true")
    ap.add_argument("--show", action="store_true",
                    help="stream raw serial to stdout too")
    args = ap.parse_args()

    src = os.path.join(HERE, args.probe + ".c")
    binp = os.path.join(WT, "work", args.probe + "-x86")
    os.makedirs(os.path.join(WT, "work"), exist_ok=True)
    subprocess.run([CC, "-static", "-O1", "-Wall",
                    f"-DPROBE_SECS={args.secs}", "-o", binp, src],
                   check=True)

    img = os.path.join(WT, "work", args.probe + "-x86.img")
    build_image(img, binp, args.probe)

    cmd = [QEMU, "-M", "q35", "-accel", "tcg,thread=multi",
           "-cpu", "max", "-m", args.mem, "-smp", args.smp,
           "-display", "none", "-serial", "stdio", "-monitor", "none",
           "-no-reboot", "-snapshot",
           "-drive", f"file={img},if=none,id=rootfs,format=raw,file.locking=off",
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", args.kernel,
           "-append", f"root=/dev/vda rw init=/test/{args.probe} "
                      f"console=ttyS0 dfx=watchdog"]
    print("[boot]", " ".join(cmd[1:]), flush=True)
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE,
                         stderr=subprocess.STDOUT)

    deadline = time.time() + float(args.secs) + 300
    out = bytearray()
    wedge = False
    verdict = None
    logp = os.path.join(WT, "runs", f"x86probe-{args.probe}.log")
    os.makedirs(os.path.join(WT, "runs"), exist_ok=True)
    logf = open(logp, "wb")
    while time.time() < deadline:
        line = p.stdout.readline()
        if not line:
            if p.poll() is not None:
                break
            continue
        out += line
        logf.write(line)
        logf.flush()
        if args.show:
            sys.stdout.write(line.decode(errors="replace"))
            sys.stdout.flush()
        if WEDGE_LINE.search(line):
            wedge = True
        if PASS_LINE.search(line):
            verdict = 0
            break
        if FAIL_LINE.search(line):
            verdict = 1
            break
    p.kill()
    p.wait()
    logf.close()

    if verdict is None:
        verdict = 2 if wedge else 3
    elif wedge:
        verdict = 2
    print(f"verdict={verdict} log={logp} "
          f"({'PASS' if verdict == 0 else 'FAIL' if verdict == 1 else 'WEDGE' if verdict == 2 else 'TIMEOUT'})")
    if not args.keep_img:
        os.unlink(img)
    return verdict


if __name__ == "__main__":
    sys.exit(main())
