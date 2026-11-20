#!/usr/bin/env python3
# MIT License
# Copyright (c) 2026 Fei Wang

"""garepro_boot.py — boot the x86_64 Rux kernel with a freeze probe as
init, stream serial, report a verdict. Two probes:

  --probe garepro (default): GLib gthreadpool spawn-protocol repro
    (test/garepro.c). Mimics GLib's global thread-pool spawner
    protocol with raw glibc pthreads and pre-arms its spawn mutex at a
    fixed mmap address (0x313370100) so the kernel RWFRZ-WATCH futex
    tracer can watch the word from t=0 (rwfrzwatch=0x313370100
    appended automatically, --rwfrz to add rwfrzbudget=<n>).

  --probe tsleep: timed-wait completion isolation (test/tsleep.c).
    Five groups (usleep / poll timeout / clock_nanosleep ABSTIME /
    futex timeout / timerfd), no locks, no clone machinery. Boots
    WITHOUT the rwfrz tracer (clean serial, no printk flood).

Verdicts (probe exit code on serial):
  <PROBE>-PASS                  -> rc 0  (no freeze in the window)
  GAREPRO-PHANTOM               -> rc 1  (word held, no user holder: kernel-suspect)
  GAREPRO-OWNER-PARKED          -> rc 1  (holder parked 15s+ per ledger)
  GAREPRO-STALL                 -> rc 1  (no M traffic 30s)
  GAREPRO-CANARY / -DOUBLE...   -> rc 1
  GAREPRO-REG-REWIND            -> rc 1 (monitor resumed from a stale
                                    context snapshot: page shadow and/or
                                    witness contradict its registers)
  TSLEEP-DEAD-G<n> / -DEAD-ALL  -> rc 1  (a timed-wait group froze)
  TSLEEP-SIG-WATCHDOG           -> rc 1  (monitor itself wedged)
  kernel wedge lines            -> rc 2
  timeout                       -> rc 3

Usage: garepro_boot.py [--probe garepro|tsleep] [--secs 300] [--smp 8]
       [--mem 4G] [--accel kvm|tcg] [--kernel PATH] [--keep-img]
       [--show] [--append-cmdline STR] [--rwfrz N] [--tag NAME]
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
CC = "gcc"
MKFS = "/usr/sbin/mkfs.ext4"

PASS_LINE = re.compile(rb"(GAREPRO|TSLEEP)-PASS")
FAIL_LINE = re.compile(
    rb"GAREPRO-(PHANTOM|OWNER-PARKED|STALL|CANARY|DOUBLE-HOLD|FAIL"
    rb"|REG-REWIND)"
    rb"|TSLEEP-(DEAD-G\d|DEAD-ALL|SIG-WATCHDOG|FAIL)")
WEDGE_LINE = re.compile(rb"DEADLOCK: spinlock stuck|BUG: soft lockup|Kernel panic|panic: ")


def build_image(img, probe_bin, guest_name):
    staging = tempfile.mkdtemp(prefix="garepro_root")
    os.makedirs(os.path.join(staging, "test"))
    shutil.copy(probe_bin, os.path.join(staging, "test", guest_name))
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^64bit",
                        "-d", staging, img, "64M"], capture_output=True)
    shutil.rmtree(staging)
    if r.returncode != 0:
        print("mkfs failed:", r.stderr.decode(), file=sys.stderr)
        sys.exit(4)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--probe", default="garepro", choices=["garepro", "tsleep"])
    ap.add_argument("--secs", default="300")
    ap.add_argument("--smp", default="8")
    ap.add_argument("--mem", default="4G")
    ap.add_argument("--accel", default="kvm", choices=["kvm", "tcg"])
    ap.add_argument("--pushers", default="6")
    ap.add_argument("--idle-ms", default="15000")
    ap.add_argument("--kernel", default=os.path.join(
        WT, "target/x86_64-unknown-none/debug/bzImage"))
    ap.add_argument("--keep-img", action="store_true")
    ap.add_argument("--show", action="store_true")
    ap.add_argument("--append-cmdline", default="",
                    help="extra kernel cmdline tokens (bisect levers)")
    ap.add_argument("--rwfrz", default="",
                    help="garepro only: rwfrzbudget value ('' = 20000, 'off' = no tracer)")
    ap.add_argument("--no-alarm", action="store_true",
                    help="garepro only: build with -DNO_ALARM (no SIGALRM/ualrm)")
    ap.add_argument("--alarm-us", default="3000000",
                    help="garepro only: ualarm period in us (default 3s)")
    ap.add_argument("--tag", default="",
                    help="serial log name prefix (default: probe name)")
    args = ap.parse_args()

    if args.probe == "tsleep":
        src = os.path.join(WT, "test", "tsleep.c")
        binp = os.path.join(WT, "work", "tsleep-x86")
        extra = []
        initpath = "/test/tsleep"
    else:
        src = os.path.join(WT, "test", "garepro.c")
        binp = os.path.join(WT, "work", "garepro-x86")
        extra = [f"-DN_PUSHERS={args.pushers}",
                 f"-DIDLE_MS={args.idle_ms}",
                 f"-DALARM_US={args.alarm_us}"]
        if args.no_alarm:
            extra.append("-DNO_ALARM")
        initpath = "/test/garepro"
    os.makedirs(os.path.join(WT, "work"), exist_ok=True)
    subprocess.run(
        [CC, "-static", "-O2", "-pthread", "-Wall",
         f"-DPROBE_SECS={args.secs}", *extra, "-o", binp, src],
        check=True)

    tag = args.tag or args.probe
    img = os.path.join(WT, "work", f"{tag}-x86.img")
    build_image(img, binp, args.probe)

    if args.probe == "tsleep":
        tracer = ""  # no watched word: keep serial clean of the flood
    elif args.rwfrz == "off":
        tracer = "rwfrzwatch=0x313370100"
    else:
        tracer = ("rwfrzwatch=0x313370100 rwfrzbudget="
                  + (args.rwfrz or "20000"))

    cmdline = (f"root=/dev/vda rw init={initpath} console=ttyS0 {tracer}"
               + (f" {args.append_cmdline}" if args.append_cmdline else ""))

    accel = ["-accel", "kvm"] if args.accel == "kvm" else \
            ["-accel", "tcg,thread=multi"]
    cmd = [QEMU, "-M", "q35", "-m", args.mem, "-smp", args.smp,
           *accel, "-cpu", "max",
           "-display", "none", "-serial", "stdio", "-monitor", "none",
           "-no-reboot", "-snapshot",
           "-drive", f"file={img},if=none,id=rootfs,format=raw,file.locking=off",
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", args.kernel,
           "-append", cmdline]
    print("[boot]", " ".join(cmd[1:]), flush=True)
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE,
                         stderr=subprocess.STDOUT)

    deadline = time.time() + float(args.secs) + 420
    verdict = None
    out = bytearray()
    logp = os.path.join(WT, "run", f"{tag}-serial.log")
    logf = open(logp, "wb")
    boot_t0 = time.time()
    while time.time() < deadline:
        line = p.stdout.readline()
        if not line:
            if p.poll() is not None:
                break
            continue
        out += line
        # host receive timestamp: separates guest clock behavior
        # (jumps/rate) from UART drain backlog — the boot8 lesson
        logf.write(("[+%8.3f] " % (time.time() - boot_t0)).encode()
                   + line)
        logf.flush()
        if args.show:
            sys.stdout.write(line.decode(errors="replace"))
            sys.stdout.flush()
        if WEDGE_LINE.search(line):
            verdict = verdict or 2
        if PASS_LINE.search(line):
            verdict = verdict if verdict is not None else 0
            break
        m = FAIL_LINE.search(line)
        if m:
            verdict = 1
            # keep draining a moment so the dump completes
            end = time.time() + 5
            while time.time() < end:
                l2 = p.stdout.readline()
                if not l2:
                    break
                logf.write(l2)
                logf.flush()
            break
    p.kill()
    p.wait()
    logf.close()
    if verdict is None:
        verdict = 3
    print(f"verdict={verdict} log={logp} "
          f"({'PASS' if verdict == 0 else 'REPRO' if verdict == 1 else 'WEDGE' if verdict == 2 else 'TIMEOUT'})")
    if not args.keep_img:
        os.unlink(img)
    return verdict


if __name__ == "__main__":
    sys.exit(main())
