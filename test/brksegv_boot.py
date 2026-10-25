#!/usr/bin/env python3
"""brksegv_boot.py — boot the x86_64 Rux kernel with the brksegv_churn
probe as init and judge the do_exit-hang / preempt-count-leak fix
(PREEMPT-HEAL round 3e: pure-HARDIRQ phantom levels join the heal).

Verdict discipline (exitwalk_boot.py family):
  BRKSEG V-PASS (churn completed, no PREEMPT-LEAK outliving the heal
                 window, or a PREEMPT-HEAL cleared every leak)
                                            -> rc 0
  BRKSEGFAIL-leak-survived (a PREEMPT-LEAK whose held_nonzero_for grew
                 past 1200 jiffies — the 10 s heal never fired)
                                            -> rc 1
  kernel panic / trap / DEADLOCK            -> rc 2
  timeout (no CHURN-DONE line)              -> rc 3

Usage:
  ./brksegv_boot.py [--smp 2] [--secs 60] [--mem 512M]
      [--kernel bzImage] [--accel kvm|tcg]
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(HERE)
QEMU = "qemu-system-x86_64"
MKFS = "/usr/sbin/mkfs.ext4"
MUSL_CC = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl", "bin",
                       "musl-gcc")

DONE_LINE = re.compile(rb"BRKSEQ-CHURN-DONE")
LEAK_LINE = re.compile(
    rb"PREEMPT-LEAK cpu=\S+ pid=\S+ pc=\S+ "
    rb"held_nonzero_for=([0-9a-f]+)jiffies")
HEAL_LINE = re.compile(rb"PREEMPT-HEAL cpu=")
FAIL_LINE = re.compile(
    rb"Kernel panic|panic: |DEADLOCK: spinlock stuck|BUG: soft lockup|"
    rb"trap: #GP|general protection fault")


def build_image(img, probe_bin, name):
    staging = tempfile.mkdtemp(prefix="brksegv_root")
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
    ap.add_argument("--smp", default="2")
    ap.add_argument("--secs", default="60")
    ap.add_argument("--mem", default="2048M",
                    help="Kernel.toml ships physical_memory=2048; a "
                         "smaller guest leaves the kernel heap carve "
                         "empty (boot PANIC: Allocation error 4MiB)")
    ap.add_argument("--kernel", default=os.path.join(
        WT, "target/x86_64-unknown-none/debug/bzImage"))
    ap.add_argument("--accel", default="kvm")
    ap.add_argument("--arch", default="x86_64",
                    choices=["x86_64", "riscv64"],
                    help="riscv64 boots a raw ELF kernel on the virt "
                         "machine (the ltp lane's frozen kernels are "
                         "riscv64 ELF builds)")
    ap.add_argument("--rounds", default="12")
    ap.add_argument("--keep-img", action="store_true")
    ap.add_argument("--show", action="store_true")
    args = ap.parse_args()

    src = os.path.join(HERE, "brksegv_churn.c")
    arch = args.arch
    if arch == "riscv64":
        musl = os.path.join(WT, "toolchain", "riscv64-rux-linux-musl",
                            "bin", "musl-gcc")
        if not os.path.exists(musl):
            # The riscv64 musl wrapper is not built in every worktree;
            # a static glibc binary works equally as init.
            musl = "riscv64-linux-gnu-gcc"
    else:
        musl = MUSL_CC
    binp = os.path.join(WT, "work", f"brksegv_churn-{arch}")
    os.makedirs(os.path.join(WT, "work"), exist_ok=True)
    env = dict(os.environ)
    env.setdefault("REALGCC", "gcc")
    subprocess.run([musl, "-static", "-O1", "-Wall",
                    f"-DROUNDS={args.rounds}",
                    "-o", binp, src],
                   check=True, env=env)

    img = os.path.join(WT, "work", f"brksegv_churn-{arch}.img")
    build_image(img, binp, "brksegv_churn")

    if arch == "riscv64":
        qemu = "qemu-system-riscv64"
        kernel = args.kernel or os.path.join(
            WT, "target/riscv64gc-unknown-none-elf/debug/rux")
        accel = ["-accel", "tcg,thread=multi"]
        mach = ["-M", "virt", "-cpu", "rv64,zbb=true,zba=true,zbs=true,zcb=true"]
        bootdev = ["-device",
                   "virtio-blk-pci,disable-legacy=on,drive=rootfs"]
    else:
        qemu = QEMU
        kernel = args.kernel or os.path.join(
            WT, "target/x86_64-unknown-none/debug/bzImage")
        accel = ["-accel", "kvm", "-accel", "tcg"] \
            if args.accel == "kvm" else ["-accel", "tcg,thread=multi"]
        mach = ["-M", "q35", "-cpu", "max"]
        bootdev = ["-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs"]
    cmd = [qemu, *mach, *accel,
           "-m", args.mem, "-smp", args.smp,
           "-display", "none", "-serial", "stdio", "-monitor", "none",
           "-no-reboot", "-snapshot",
           "-drive", f"file={img},if=none,id=rootfs,format=raw,file.locking=off",
           *bootdev,
           "-kernel", kernel,
           "-append",
           "root=/dev/vda rw init=/test/brksegv_churn console=ttyS0"]

    if args.show:
        print("+", " ".join(cmd), file=sys.stderr)
    try:
        p = subprocess.run(cmd, capture_output=True, timeout=int(args.secs))
        out = p.stdout
        rc = "exit=%d" % p.returncode
    except subprocess.TimeoutExpired as e:
        out = e.stdout or b""
        rc = "timeout"
    finally:
        if not args.keep_img:
            for f in (img,):
                try:
                    os.unlink(f)
                except OSError:
                    pass

    open(os.path.join(WT, "run", "brksegv-serial.log"), "wb").write(out)

    if FAIL_LINE.search(out):
        print("BRKSEGFAIL-kernel-witness", rc)
        return 2
    if not DONE_LINE.search(out):
        print("BRKSEGFAIL-no-done-line", rc)
        return 3
    # Leak verdict: any leak that outlived the 10 s (1000 jiffy) heal
    # window — 1200 leaves margin for the print cadence.
    worst = 0
    for m in LEAK_LINE.finditer(out):
        worst = max(worst, int(m.group(1), 16))
    nheal = len(HEAL_LINE.findall(out))
    if worst > 1200 and nheal == 0:
        print(f"BRKSEGFAIL-leak-survived held={worst} heals={nheal}")
        return 1
    print(f"BRKSEQ-PASS worst_leak_held={worst}jif heals={nheal} ({rc})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
