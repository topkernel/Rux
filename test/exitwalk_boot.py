#!/usr/bin/env python3
"""exitwalk_boot.py — boot the x86_64 Rux kernel with the exitwalk_churn
probe as init (musl static binary) and judge the exit/teardown race fix.

Verdict discipline (x86_probe_boot.py family):
  EXITWALK-PASS                        -> rc 0
  EXITWALKFAIL-*                       -> rc 1
  kernel panic / trap / FUT-STALE /
  PTLEDGER DOUBLE-FREE / MM-DROP-CTX
  witness / DEADLOCK / soft lockup     -> rc 2
  timeout (no PASS line)               -> rc 3

Usage:
  ./exitwalk_boot.py [--smp 2] [--secs 30] [--mem 512M]
      [--kernel bzImage] [--accel kvm|tcg] [--keep-img] [--show]

Kernel forensics notes: FUT-STALE-TREE / PTLEDGER DOUBLE-FREE prints are
the smoking gun of the double-teardown family (two teardowns of one
page-table root); MM-DROP-CTX is the new witness for a teardown in a
non-owner context. Any of them is a verdict even without a panic.
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
MKFS = "/usr/sbin/mkfs.ext4"
MUSL_CC = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl", "bin",
                       "musl-gcc")

PASS_LINE = re.compile(rb"EXITWALK-PASS")
FAIL_LINE = re.compile(rb"EXITWALKFAIL-")
WEDGE_LINE = re.compile(
    rb"Kernel panic|panic: |DEADLOCK: spinlock stuck|BUG: soft lockup|"
    rb"trap: #GP|trap: #PF|general protection fault")
FORENSIC_LINE = re.compile(
    rb"FUT-STALE|PTLEDGER: DOUBLE-FREE|FUT: REPEAT teardown|MM-DROP-CTX|"
    rb"EARLY-ROOT-FREE|PT-WALK-STALE")


def build_image(img, probe_bin, name):
    staging = tempfile.mkdtemp(prefix="exitwalk_root")
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
    ap.add_argument("--secs", default="30")
    ap.add_argument("--mem", default="512M")
    ap.add_argument("--kernel", default=os.path.join(
        WT, "target/x86_64-unknown-none/debug/bzImage"))
    ap.add_argument("--accel", default="kvm",
                    help="kvm (default; falls back to tcg) or tcg")
    ap.add_argument("--churn-delay-us", default="0",
                    help="per-spawner pacing in us (0 = pathological rate)")
    ap.add_argument("--smaps-every", default="7",
                    help="smaps every Nth pid (1 = every pid)")
    ap.add_argument("--dfx", default="",
                    help="dfx= boot flags (default: none — the watchdog's "
                         "periodic taskdumps are a crash discriminator)")
    ap.add_argument("--keep-img", action="store_true")
    ap.add_argument("--show", action="store_true",
                    help="stream raw serial to stdout too")
    args = ap.parse_args()

    src = os.path.join(HERE, "exitwalk_churn.c")
    binp = os.path.join(WT, "work", "exitwalk_churn-x86")
    os.makedirs(os.path.join(WT, "work"), exist_ok=True)
    env = dict(os.environ)
    env.setdefault("REALGCC", "gcc")
    subprocess.run([MUSL_CC, "-static", "-O1", "-Wall",
                    f"-DPROBE_SECS={args.secs}",
                    f"-DCHURN_DELAY_US={args.churn_delay_us}",
                    f"-DSMAPS_EVERY={args.smaps_every}",
                    "-o", binp, src],
                   check=True, env=env)

    img = os.path.join(WT, "work", "exitwalk_churn-x86.img")
    build_image(img, binp, "exitwalk_churn")

    # QEMU 10.x: repeated -accel entries form a fallback list.
    accel = ["-accel", "kvm", "-accel", "tcg"] if args.accel == "kvm" else \
            ["-accel", "tcg,thread=multi"]
    cmd = [QEMU, "-M", "q35", *accel,
           "-cpu", "max", "-m", args.mem, "-smp", args.smp,
           "-display", "none", "-serial", "stdio", "-monitor", "none",
           "-no-reboot", "-snapshot",
           "-drive", f"file={img},if=none,id=rootfs,format=raw,file.locking=off",
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", args.kernel,
           "-append", f"root=/dev/vda rw init=/test/exitwalk_churn "
                      f"console=ttyS0{(' ' + args.dfx) if args.dfx else ''}"]
    print("[boot]", " ".join(cmd[1:]), flush=True)
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE,
                         stderr=subprocess.STDOUT)

    deadline = time.time() + float(args.secs) + 300
    out = bytearray()
    wedge = None
    forensic = None
    verdict = None
    logp = os.path.join(WT, "runs", "exitwalk-x86.log")
    os.makedirs(os.path.join(WT, "runs"), exist_ok=True)
    logf = open(logp, "wb")
    try:
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
                sys.stdout.write(line.decode("latin1"))
                sys.stdout.flush()
            if FAIL_LINE.search(line):
                verdict = "probe-fail"
            elif WEDGE_LINE.search(line):
                wedge = line.decode("latin1").strip()
                verdict = "wedge"
            elif FORENSIC_LINE.search(line):
                forensic = line.decode("latin1").strip()
                verdict = "forensic"
            elif PASS_LINE.search(line):
                verdict = "pass"
            if verdict is not None:
                if verdict != "pass":
                    # Keep streaming so the kernel panic/trap dump (stack
                    # words, KERNPANIC block) lands in the log before we
                    # kill QEMU.
                    drain_until = time.time() + 12
                    while time.time() < drain_until:
                        line = p.stdout.readline()
                        if not line:
                            if p.poll() is not None:
                                break
                            continue
                        out += line
                        logf.write(line)
                        logf.flush()
                        if args.show:
                            sys.stdout.write(line.decode("latin1"))
                break
    finally:
        logf.close()
        if p.poll() is None:
            p.terminate()
            try:
                p.wait(10)
            except subprocess.TimeoutExpired:
                p.kill()
    if not args.keep_img:
        try:
            os.unlink(img)
        except OSError:
            pass

    tail = out[-4000:].decode("latin1")
    print(f"--- verdict: {verdict} (log: {logp}) ---")
    if forensic:
        print("forensic line:", forensic)
    if wedge:
        print("wedge line:", wedge)
    print(tail[-1500:])
    if verdict == "pass":
        return 0
    if verdict in ("probe-fail", "wedge", "forensic"):
        return 2
    return 3


if __name__ == "__main__":
    sys.exit(main())
