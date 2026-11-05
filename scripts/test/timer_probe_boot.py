#!/usr/bin/env python3
"""timer_probe_boot.py — boot the Rux kernel with timer_probe on a cold
copy of the Ubuntu disk, stream the serial log, report the verdict.

Adapted from unix-repro's unix_wedge_boot.py. Extra: --dfx cmdline switch
appended to the kernel command line (e.g. "dfx=watchdog,periodic").

Usage:
  ./timer_probe_boot.py [--dur 600] [--sleepers 6] [--dfx "watchdog,periodic"]
      [--kernel ELF] [--img-base PATH] [--timeout SECS] [--out DIR]
      [--smp 4] [--reuse-img]
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
CC = "riscv64-linux-gnu-gcc"
DEBUGFS = "/usr/sbin/debugfs"
DEFAULT_IMG_BASE = "/home/william/rux-gate/ubuntu/ubuntu2.img"
DEFAULT_KERNEL = os.path.join(
    WT, "target/riscv64gc-unknown-none-elf/debug/rux")
MARKER = "TIMER_PROBE_ALL_DONE"


def build_and_inject(args, img):
    subprocess.run([CC, "-static", "-O2", "-Wall",
                    "-o", os.path.join(WT, "work", "timer_probe"),
                    os.path.join(HERE, "timer_probe.c")], check=True)
    subprocess.run([CC, "-static", "-O2", "-Wall",
                    "-o", os.path.join(WT, "work", "runit"),
                    os.path.join(HERE, "timer_runit.c")], check=True)

    auto = (f"/root/timer_probe {args.dur} {args.sleepers}"
            + f"\n/bin/echo {MARKER}\n")
    with open(os.path.join(WT, "work", "auto.sh"), "w") as f:
        f.write(auto)

    def dbg(cmd):
        r = subprocess.run([DEBUGFS, "-w", "-R", cmd, img],
                           capture_output=True, text=True)
        if r.returncode != 0:
            print("[inject] debugfs '%s' rc=%d %s" %
                  (cmd, r.returncode, r.stderr.strip()))

    for guest in ("/root/timer_probe", "/usr/sbin/runit", "/root/auto.sh"):
        dbg("rm %s" % guest)
    dbg("write %s /root/timer_probe" % os.path.join(WT, "work", "timer_probe"))
    dbg("write %s /usr/sbin/runit" % os.path.join(WT, "work", "runit"))
    dbg("write %s /root/auto.sh" % os.path.join(WT, "work", "auto.sh"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dur", type=int, default=600)
    ap.add_argument("--sleepers", type=int, default=6)
    ap.add_argument("--dfx", default="")
    ap.add_argument("--kernel", default=DEFAULT_KERNEL)
    ap.add_argument("--img-base", default=DEFAULT_IMG_BASE)
    ap.add_argument("--timeout", type=int, default=None)
    ap.add_argument("--out", default=os.path.join(WT, "work", "run"))
    ap.add_argument("--smp", type=int, default=4)
    ap.add_argument("--reuse-img", action="store_true")
    args = ap.parse_args()

    if args.timeout is None:
        args.timeout = args.dur + 420

    os.makedirs(args.out, exist_ok=True)
    os.makedirs(os.path.join(WT, "work"), exist_ok=True)
    img = os.path.join(WT, "work", "timer_probe.img")

    if not args.reuse_img:
        print("[boot] cold-copying %s -> %s" % (args.img_base, img))
        shutil.copyfile(args.img_base, img)
    build_and_inject(args, img)

    if not os.path.exists(args.kernel):
        print("[boot] kernel missing: %s" % args.kernel)
        return 2

    append = "root=/dev/vda rw init=/usr/sbin/runit console=ttyS0"
    if args.dfx:
        append += " dfx=" + args.dfx
    cmd = [QEMU,
           "-M", "virt", "-accel", "tcg,thread=single", "-cpu", "rv64",
           "-m", "2G", "-smp", str(args.smp),
           "-display", "none", "-serial", "stdio",
           "-drive", f"file={img},if=none,id=rootfs,format=raw",
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", args.kernel,
           "-append", append]
    print("[boot] qemu dur=%d sleepers=%d dfx='%s' smp=%d" %
          (args.dur, args.sleepers, args.dfx, args.smp))

    ser = open(os.path.join(args.out, "serial.log"), "wb")
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

    buf = bytearray()
    t0 = time.time()
    last_out = t0
    test_started = False
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
                last_out = time.time()
                text = bytes(buf[-16384:])
                if b"TIMER_PROBE CONFIG" in text:
                    test_started = True
                if b"panicked" in text or b"Kernel panic" in text:
                    print("[boot] KERNEL PANIC detected")
                    rc = 2
                    break
                for line in text.split(b"\n"):
                    if line.startswith(b"TIMER_PROBE RESULT:"):
                        verdict = line.decode(errors="replace").strip()
                        print("[boot] %s" % verdict)
                if MARKER.encode() in text:
                    print("[boot] DONE marker seen")
                    rc = 0 if verdict and "PASS" in verdict else 1
                    break
            if proc.poll() is not None:
                break
            if test_started and time.time() - last_out > 120:
                print("[boot] SERIAL-SILENCE >120s after test start")
                rc = 4
                break
    finally:
        try:
            proc.terminate()
            proc.wait(timeout=10)
        except Exception:
            proc.kill()
        ser.close()

    print("[boot] elapsed %.0fs rc=%d log=%s" %
          (time.time() - t0, rc, os.path.join(args.out, "serial.log")))
    if verdict:
        print("[boot] %s" % verdict)
    return rc


if __name__ == "__main__":
    sys.exit(main())
