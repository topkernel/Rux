#!/usr/bin/env python3
"""lostwake_boot.py — boot the x86_64 Rux kernel with the lostwake probe
as init on a tiny private ext4 image, under KVM SMP, and grade the run.

Rig (mirrors scripts/test/run-oh.sh flags):
    qemu-system-x86_64 -M q35 -accel kvm -cpu max -m 1G -smp 8
    -snapshot + file.locking=off, serial to a log file.

Verdict:
    rc 0 — clean run (LW: DONE seen, zero LW: LOSTWAKE lines)
    rc 1 — userspace lost wakes observed
    rc 2 — kernel-side WAKE-*/W2/W3/STALE- diagnostic fired (even with
           no userspace LOSTWAKE — kernel forensics count as a repro)
    rc 3 — probe never reached DONE (wedge / boot failure)
    rc 4 — setup failure

Usage:
  ./run/lostwake_boot.py [--secs 30] [--smp 8] [--tag NAME]
      [--kernel PATH] [--probe PATH] [--accel kvm] [--mem 1G]
      [--append "extra tokens"] [--show]
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

WT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
QEMU = "qemu-system-x86_64"
MKFS = "/usr/sbin/mkfs.ext4"

LOSTWAKE = re.compile(rb"LW: LOSTWAKE(?:-STUCK)? tid=(\S+) surf=(\S+)")
DONE = re.compile(rb"LW: DONE")
BOOT = re.compile(rb"LW: BOOT")
WEDGE = re.compile(rb"KERNPANIC|Kernel panic|panic: |DEADLOCK|soft lockup")
KERNEL_DIAG = re.compile(
    rb"WAKE-STRANDED|WAKE-REFUSED|WAKE-PID-MISS|WAKE-RUN-LEAK|"
    rb"W2-|W3-RUNWAKE|STALE-CLAIM|STALE-ONCPU|LOSTWAKE-K")


def build_image(img, probe_bin):
    staging = tempfile.mkdtemp(prefix="lw_root")
    os.makedirs(os.path.join(staging, "test"))
    shutil.copy(probe_bin, os.path.join(staging, "test", "lostwake"))
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^64bit",
                        "-d", staging, img, "64M"],
                       capture_output=True)
    shutil.rmtree(staging)
    if r.returncode != 0:
        print("mkfs failed:", r.stderr.decode(errors="replace"),
              file=sys.stderr)
        sys.exit(4)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--secs", default="30")
    ap.add_argument("--smp", default="8")
    ap.add_argument("--mem", default="1G")
    ap.add_argument("--accel", default="kvm")
    ap.add_argument("--tag", default="run")
    ap.add_argument("--kernel", default=os.path.join(
        WT, "target/x86_64-unknown-none/debug/bzImage"))
    ap.add_argument("--probe", default=os.path.join(WT, "work/lostwake-x86"))
    ap.add_argument("--append", default="")
    ap.add_argument("--surfs", default="",
                    help="surface subset letters: f p n s k i")
    ap.add_argument("--show", action="store_true")
    args = ap.parse_args()

    for f in (args.kernel, args.probe):
        if not os.path.isfile(f):
            print(f"missing: {f}", file=sys.stderr)
            return 4

    img = os.path.join(WT, "run", f"lostwake-{args.tag}.img")
    log = os.path.join(WT, "run", f"lostwake-{args.tag}.log")
    os.makedirs(os.path.join(WT, "run"), exist_ok=True)
    build_image(img, args.probe)

    accel = args.accel
    if accel == "kvm" and not os.path.exists("/dev/kvm"):
        print("NOTE: /dev/kvm missing, falling back to tcg", file=sys.stderr)
        accel = "tcg,thread=multi"
    accel_arg = f"-accel {accel}" if accel else "-accel kvm"

    append = (f"root=/dev/vda rw init=/test/lostwake console=ttyS0 "
              f"loglevel=7 lw.secs={args.secs}"
              + (f" lw.surfs={args.surfs}" if args.surfs else "")
              + f" {args.append}").strip()

    cmd = (f"{QEMU} -M q35 {accel_arg} -cpu max -m {args.mem} "
           f"-smp {args.smp} -display none -serial file:{log} "
           f"-monitor none -no-reboot -snapshot "
           f"-drive file={img},if=none,id=rootfs,format=raw,file.locking=off "
           f"-device virtio-blk-pci,disable-legacy=on,drive=rootfs "
           f"-kernel {args.kernel} -append \"{append}\"")

    print(f"[boot] {cmd}", flush=True)
    p = subprocess.Popen(cmd, shell=True,
                         stdout=subprocess.DEVNULL,
                         stderr=subprocess.STDOUT,
                         start_new_session=True)
    qpid = p.pid

    verdict = None
    lost = 0
    kernel_diag = 0
    deadline = time.time() + float(args.secs) + 420
    size = 0
    try:
        while time.time() < deadline:
            time.sleep(2)
            if p.poll() is not None:
                break
            try:
                with open(log, "rb") as f:
                    f.seek(size)
                    chunk = f.read()
                size += len(chunk)
            except FileNotFoundError:
                continue
            if not chunk:
                continue
            if args.show:
                sys.stdout.write(chunk.decode(errors="replace"))
                sys.stdout.flush()
            lost += len(LOSTWAKE.findall(chunk))
            kernel_diag += len(KERNEL_DIAG.findall(chunk))
            if WEDGE.search(chunk):
                verdict = 3
                break
            if DONE.search(chunk):
                verdict = 0
                break
    finally:
        # kill ONLY the qemu we started (shell wrapper + qemu share the
        # session; kill the process group)
        try:
            os.killpg(os.getpgid(p.pid), 15)
        except ProcessLookupError:
            pass
        time.sleep(1)
        try:
            os.killpg(os.getpgid(p.pid), 9)
        except ProcessLookupError:
            pass
        p.wait()

    # final full-log pass (missed tail)
    try:
        with open(log, "rb") as f:
            tail = f.read()[size:]
        lost += len(LOSTWAKE.findall(tail))
        kernel_diag += len(KERNEL_DIAG.findall(tail))
        if verdict is None and DONE.search(tail):
            verdict = 0
    except FileNotFoundError:
        pass

    if lost:
        verdict = 1
    elif kernel_diag and verdict == 0:
        verdict = 2
    if verdict is None:
        verdict = 3

    names = {"0": "CLEAN", "1": "LOSTWAKE", "2": "KERNEL-DIAG",
             "3": "NOWEDGE/WEDGE", "4": "SETUP"}
    print(f"[lostwake-{args.tag}] verdict={names[str(verdict)]} "
          f"userspace_lostwakes={lost} kernel_diag_lines={kernel_diag} "
          f"log={log}")
    return verdict


if __name__ == "__main__":
    sys.exit(main())
