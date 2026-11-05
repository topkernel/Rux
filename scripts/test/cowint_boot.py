#!/usr/bin/env python3
"""cowint_boot.py — boot the x86_64 Rux kernel with the cow_int probe as
init and judge COW/fork CONTENT integrity (oh-crash lane, 50th agent).

Verdict:
  COWINT-PASS (COWINT-DONE seen, zero COWINT-*-FAIL lines)  -> rc 0
  COWINT-FAIL (any COWINT-*-FAIL line)                      -> rc 1
  kernel panic / trap                                       -> rc 2
  timeout (no COWINT-DONE)                                  -> rc 3

Usage:
  ./cowint_boot.py [--smp 2] [--secs 120] [--mem 1G]
      [--kernel bzImage] [--accel kvm|tcg] [--show] [--keep-img]
      [--loops N]   (repeat the probe body N times in one boot)
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from ruxroot import repo_root

HERE = os.path.dirname(os.path.abspath(__file__))
WT = repo_root(__file__)
QEMU = "qemu-system-x86_64"
MKFS = "/usr/sbin/mkfs.ext4"
MUSL_CC = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl", "bin",
                       "musl-gcc")

FAIL_RE = re.compile(rb"COWINT-\S*-FAIL|COWINT-DETAIL")
PANIC_RE = re.compile(
    rb"Kernel panic|panic: |DEADLOCK: spinlock stuck|BUG: soft lockup|"
    rb"trap: #GP|general protection fault|pagefault: Out of memory")


def build_probe(loops):
    src = os.path.join(WT, "test", "cowint", "cow_int.c")
    binp = os.path.join(WT, "work", "cow_int")
    os.makedirs(os.path.join(WT, "work"), exist_ok=True)
    env = dict(os.environ)
    env.setdefault("REALGCC", "gcc")
    subprocess.run([MUSL_CC, "-static", "-O2", "-Wall", "-pthread",
                    f"-DLOOPS={loops}", "-o", binp, src],
                   check=True, env=env)
    return binp


def build_image(img, probe_bin):
    staging = tempfile.mkdtemp(prefix="cowint_root")
    os.makedirs(os.path.join(staging, "test"))
    shutil.copy(probe_bin, os.path.join(staging, "test", "cow_int"))
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^64bit",
                        "-d", staging, img, "80M"],
                       capture_output=True)
    shutil.rmtree(staging)
    if r.returncode != 0:
        print("mkfs failed:", r.stderr.decode(), file=sys.stderr)
        sys.exit(4)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--smp", default="2")
    ap.add_argument("--secs", default="150")
    ap.add_argument("--mem", default="1024M")
    ap.add_argument("--kernel", default=os.path.join(
        WT, "target/x86_64-unknown-none/debug/bzImage"))
    ap.add_argument("--accel", default="kvm")
    ap.add_argument("--loops", default="1")
    ap.add_argument("--keep-img", action="store_true")
    ap.add_argument("--show", action="store_true")
    args = ap.parse_args()

    binp = build_probe(args.loops)
    img = os.path.join(WT, "work", "cow_int.img")
    build_image(img, binp)

    accel = (["-accel", "kvm", "-accel", "tcg"]
             if args.accel == "kvm" else ["-accel", "tcg,thread=multi"])
    cmd = [QEMU, "-M", "q35", *accel, "-cpu", "max",
           "-m", args.mem, "-smp", args.smp,
           "-display", "none", "-serial", "stdio", "-monitor", "none",
           "-no-reboot", "-snapshot",
           "-drive", f"file={img},if=none,id=rootfs,format=raw,file.locking=off",
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", args.kernel,
           "-append",
           "root=/dev/vda rw init=/test/cow_int console=ttyS0"]
    if args.show:
        print("+", " ".join(cmd), file=sys.stderr)

    out = bytearray()
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT)
    deadline = time.time() + int(args.secs)
    rc = "timeout"
    try:
        import select as _select
        fd = proc.stdout.fileno()
        while time.time() < deadline:
            r, _, _ = _select.select([fd], [], [], 1.0)
            if r:
                chunk = os.read(fd, 65536)
                if not chunk:
                    break
                out += chunk
                if b"COWINT-DONE" in bytes(out):
                    # give the final print a moment, then stop the VM
                    time.sleep(1)
                    rc = "done"
                    break
    finally:
        try:
            proc.kill()
            proc.wait(10)
        except OSError:
            pass
        if not args.keep_img:
            try:
                os.unlink(img)
            except OSError:
                pass

    logp = os.path.join(WT, "run", "cowint-serial.log")
    os.makedirs(os.path.dirname(logp), exist_ok=True)
    open(logp, "wb").write(bytes(out))
    print(f"serial log: {logp} ({len(out)} bytes)")

    if PANIC_RE.search(bytes(out)):
        print(f"COWINT-FAIL-kernel-witness ({rc})")
        return 2
    fails = FAIL_RE.findall(bytes(out))
    if fails:
        print(f"COWINT-FAIL probe reported {len(fails)} fail/detail lines ({rc})")
        return 1
    if b"COWINT-DONE" not in bytes(out):
        print(f"COWINT-FAIL-no-done-line ({rc})")
        return 3
    print(f"COWINT-PASS ({rc})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
