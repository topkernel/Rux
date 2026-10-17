#!/usr/bin/env python3
"""vma_storm_boot.py — boot the Rux kernel with the vma_storm VMA-race
probe as init, stream the serial log, report the verdict.

Builds a tiny private ext4 image containing only /test/vma_storm
(mkfs.ext4 -d, no root privileges needed) and boots it with
init=/test/vma_storm. The probe's own output is the verdict:

  VMASTORM-PASS              -> rc 0
  VMASTORM-FAIL / VMAFAIL-*  -> rc 1
  kernel panic               -> rc 2

Any "VMAFAIL-" line in the log is reported even if a verdict line was
missed, so a crashed probe still yields rc 1.

Usage:
  ./vma_storm_boot.py [--smp 2] [--thread multi|single] [--secs 30]
      [--stack-threads 3] [--mem 1G] [--kernel ELF] [--keep-img]
"""
import argparse
import os
import re
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
# Newer ISA bits: the cross-gcc's static glibc emits zbb/zba/zbs/zicond/
# zfa/zca/zcb instructions (mrsh on the shared image needs the same set).
CPU = ("rv64,zbb=true,zba=true,zbs=true,zcmop=true,zimop=true,"
       "zicond=true,zfa=true,zca=true,zcb=true")


def build_image(img, secs, stack_threads):
    staging = os.path.join(WT, "work", "vma_storm_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    probe = os.path.join(WT, "work", "vma_storm")
    subprocess.run([CC, "-static", "-O2", "-Wall", "-pthread",
                    "-o", probe, os.path.join(HERE, "vma_storm.c")],
                   check=True)
    shutil.copyfile(probe, os.path.join(staging, "test", "vma_storm"))
    os.chmod(os.path.join(staging, "test", "vma_storm"), 0o755)
    # init string carries the runtime args
    init_link = os.path.join(staging, "test", "init")
    os.symlink("/test/vma_storm", init_link)
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^flex_bg",
                        "-d", staging, img, "64M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return None
    return "init=/test/vma_storm %d %d" % (secs, stack_threads)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--thread", default="multi", choices=["multi", "single"])
    ap.add_argument("--secs", type=int, default=30)
    ap.add_argument("--stack-threads", type=int, default=3)
    ap.add_argument("--timeout", type=int, default=0,
                    help="wall-clock cap; default secs + 240")
    ap.add_argument("--mem", default="1G")
    ap.add_argument("--kernel", default=DEFAULT_KERNEL)
    ap.add_argument("--keep-img", action="store_true")
    args = ap.parse_args()
    if not args.timeout:
        args.timeout = args.secs + 240

    os.makedirs(os.path.join(WT, "work"), exist_ok=True)
    img = os.path.join(WT, "work", "vma_storm.img")
    initline = build_image(img, args.secs, args.stack_threads)
    if not initline:
        return 2
    if not os.path.exists(args.kernel):
        print("[boot] kernel missing: %s" % args.kernel)
        return 2

    append = ("root=/dev/vda rw %s console=ttyS0" % initline)
    cmd = [QEMU,
           "-M", "virt", "-accel", "tcg,thread=" + args.thread,
           "-cpu", CPU,
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none", "-serial", "stdio",
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", args.kernel,
           "-append", append]
    print("[boot] qemu smp=%d thread=%s mem=%s secs=%d" %
          (args.smp, args.thread, args.mem, args.secs))

    ser = open(os.path.join(WT, "work", "vma_storm_serial.log"), "wb")
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

    buf = bytearray()
    fails = []
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
                text = bytes(buf[-65536:])
                if b"panicked" in text or b"Kernel panic" in text:
                    print("[boot] KERNEL PANIC detected")
                    rc = 2
                    break
                for m in re.finditer(rb"VMAFAIL-[A-Za-z0-9_-]+[^\n]*", text):
                    line = m.group(0).decode(errors="replace").strip()
                    if line not in fails:
                        fails.append(line)
                for line in text.split(b"\n")[:-1]:
                    sline = line.decode(errors="replace").strip()
                    if sline.startswith("VMASTORM-"):
                        print("[probe] %s" % sline)
                    if sline == "VMASTORM-PASS":
                        verdict = sline
                        rc = 0
                    elif sline == "VMASTORM-FAIL":
                        verdict = sline
                        rc = 1
                if verdict:
                    break
            if proc.poll() is not None:
                break
    finally:
        proc.kill()
        proc.wait()
        ser.close()

    if fails:
        for f in fails[:20]:
            print("[probe] %s" % f)
        if rc != 2:
            rc = 1
    if rc == 3 and not verdict:
        print("[boot] no verdict (timeout or silent exit)")
    if not args.keep_img:
        try:
            os.unlink(img)
        except OSError:
            pass
    return rc


if __name__ == "__main__":
    sys.exit(main())
