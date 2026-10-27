#!/usr/bin/env python3
"""pbench_boot.py — quiet-boot the Rux x86_64 kernel with the P1-P7
microbench probe (test/pbench.c, perf-report.md section 8.2) as init and
collect the PBENCH result lines.

Builds the static musl probe, packs a small ext4 rootfs (mkfs -d, same
recipe as faultlat_boot), boots q35 + virtio-blk-pci with
init=/test/pbench, and tails the serial log: every `PBENCH ...` line is
printed as it appears; success = `PBENCH DONE rc=0`.

  rc 0  full run (DONE seen)
  rc 1  kernel panic before DONE
  rc 3  no DONE within --timeout
  rc 2  build/image failure

Usage: ./pbench_boot.py [--timeout 1200] [--bzimage PATH] [--tag NAME]
       [--accel tcg,thread=multi] [--smp 2] [--mem 1G]
The defaults match the pa-series machine class (q35, -cpu max,
SMP2/1G per repo discipline; P2 needs SMP>=2).
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
MUSL_CC = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl", "bin", "musl-gcc")
DEFAULT_BZIMAGE = os.path.join(WT, "run", "pa31-main-bzImage")
WORK = os.path.join(WT, "work")
SIZE_MB = 192          # probe writes ~30 MB (fault file + tree) into /test


def foreign_qemus():
    try:
        out = subprocess.run(["pgrep", "-c", "qemu-system"], capture_output=True,
                             text=True).stdout.strip()
        return int(out) if out else 0
    except Exception:
        return -1


def build_probe(out, src=None):
    if src is None:
        src = os.path.join(HERE, "pbench.c")
    cmd = [MUSL_CC, "-static", "-O2", "-Wall", "-o", out, src]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    print("[build] %s (%d KB)" % (out, os.path.getsize(out) // 1024))
    return True


def build_image(img, probe, arm=None):
    staging = os.path.join(WORK, "pbench_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    shutil.copyfile(probe, os.path.join(staging, "test", "pbench"))
    os.chmod(os.path.join(staging, "test", "pbench"), 0o755)
    if arm:
        shutil.copyfile(arm, os.path.join(staging, "test", "arm"))
        os.chmod(os.path.join(staging, "test", "arm"), 0o755)
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum",
                        "-d", staging, img, "%dM" % SIZE_MB],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=int, default=1200)
    ap.add_argument("--bzimage", default=DEFAULT_BZIMAGE)
    ap.add_argument("--tag", default="pb1")
    ap.add_argument("--accel", default="tcg,thread=multi")
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="2G")  # 1G panics: kernel-heap alloc
    # error (4MB layout, main.rs OOM path) during early boot
    ap.add_argument("--arm-schedstat", action="store_true",
                    help="A-side of the schedstat cost A-B: stage the "
                         "pbench_arm wrapper and boot init=/test/arm "
                         "(arms /proc/sys/kernel/sched_schedstats then "
                         "execs the probe); the B side is the stock boot")
    args = ap.parse_args()

    os.makedirs(WORK, exist_ok=True)
    if not os.path.exists(args.bzimage):
        print("[boot] bzImage missing: %s" % args.bzimage)
        return 2
    probe = os.path.join(WORK, "pbench")
    if not build_probe(probe):
        return 2
    img = os.path.join(WORK, "pbench-%s.img" % args.tag)
    arm = None
    init_path = "/test/pbench"
    if args.arm_schedstat:
        arm = os.path.join(WORK, "pbench_arm")
        if not build_probe(arm, src=os.path.join(HERE, "pbench_arm.c")):
            return 2
        init_path = "/test/arm"
    if not build_image(img, probe, arm):
        return 2

    serial = os.path.join(WORK, "pbench-%s-serial.log" % args.tag)
    if os.path.exists(serial):
        os.remove(serial)

    append = "root=/dev/vda rw init=%s console=ttyS0" % init_path
    cmd = [QEMU, "-M", "q35", "-accel", args.accel, "-cpu", "max",
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none",
           "-serial", "file:" + serial,
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", args.bzimage, "-append", append]
    print("[boot] tag=%s accel=%s smp=%s mem=%s foreign_qemus=%d"
          % (args.tag, args.accel, args.smp, args.mem, foreign_qemus()))
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        t0 = time.time()
        seen = 0
        done = False
        while time.time() - t0 < args.timeout:
            time.sleep(3)
            if proc.poll() is not None and done:
                break
            try:
                with open(serial, "rb") as f:
                    data = f.read()
            except FileNotFoundError:
                continue
            lines = data.split(b"\n")
            for line in lines[seen:]:
                if line.startswith(b"PBENCH "):
                    print("[probe] %s" % line.decode(errors="replace"))
            seen = len(lines)
            if b"PBENCH DONE rc=0" in data:
                done = True
                print("[boot] DONE marker at t=%ds" % (time.time() - t0))
                time.sleep(3)   # let poweroff settle
                break
            if b"panicked" in data or b"Kernel panic" in data:
                print("[boot] KERNEL PANIC at t=%ds (no DONE)"
                      % (time.time() - t0))
                return 1
        if not done:
            print("[boot] no DONE within %ds" % args.timeout)
            return 3
        return 0
    finally:
        proc.terminate()
        try:
            proc.wait(5)
        except Exception:
            proc.kill()
        print("[boot] serial log: %s" % serial)


if __name__ == "__main__":
    sys.exit(main())
