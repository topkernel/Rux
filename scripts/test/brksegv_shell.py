#!/usr/bin/env python3
"""brksegv_shell.py — the ltp lane's exact brkseq wedge shape, driven
through a REAL shell (mrsh as init, fork+exec per probe run).

Their brkseq1.log: /bin/sh runs `/brkseq ; echo M0` — the probe child
dies by SIGSEGV, TERM-DESCHED engages, M0 prints — and then the shell
never responds to the driver's next input line (marker timeout). The
churn-as-init harness (brksegv_boot.py) could not reproduce it; the
shell + exec path is the remaining delta.

Verdict:
  BRKSH-PASS          M0 and M1 both seen            -> rc 0
  BRKSH-FAIL-deaf     M0 seen, M1 never answered     -> rc 1 (the wedge)
  BRKSH-FAIL-m0       M0 itself never printed        -> rc 3
  BRKSH-FAIL-witness  kernel panic/trap              -> rc 2

Usage:
  ./brksegv_shell.py [--runs 6] [--secs 240] [--kernel PATH]
"""
import argparse
import os
import re
import select
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(HERE)
IMG = os.path.join(WT, "work", "rootfs-brkseq.img")

FAIL_LINE = re.compile(
    rb"Kernel panic|panic: |DEADLOCK: spinlock stuck|BUG: soft lockup|"
    rb"trap: #GP|general protection fault")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", default="6")
    ap.add_argument("--secs", default="240")
    ap.add_argument("--kernel", default=os.path.join(
        WT, "target/riscv64gc-unknown-none-elf/debug/rux"))
    ap.add_argument("--smp", default="2")
    ap.add_argument("--race", default=None,
                    help="seconds after cmd send to inject racing input")
    args = ap.parse_args()

    cmd = [
        "qemu-system-riscv64", "-M", "virt",
        "-cpu", "rv64,zbb=true,zba=true,zbs=true",
        "-accel", "tcg,thread=single", "-m", "1G", "-smp", args.smp,
        "-display", "none", "-serial", "stdio", "-monitor", "none",
        "-no-reboot", "-snapshot",
        "-drive", f"file={IMG},if=none,id=rootfs,format=raw,"
                  "file.locking=off",
        "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
        "-kernel", args.kernel,
        "-append", "root=/dev/vda rw init=/bin/sh console=ttyS0",
    ]

    out = bytearray()
    proc = subprocess.Popen(cmd, stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT)
    deadline = time.time() + int(args.secs)

    def pump(needle, window):
        """Read serial up to `window` seconds looking for needle."""
        end = time.time() + window
        while time.time() < end:
            r, _, _ = select.select([proc.stdout.fileno()], [], [], 1.0)
            if r:
                chunk = os.read(proc.stdout.fileno(), 65536)
                if not chunk:
                    return False
                out.extend(chunk)
                if needle and needle in bytes(out):
                    return True
        return False

    rc = 5
    try:
        # Shell prompt: mrsh prints "root:/# " once up.
        prompt = pump(b"# ", 120)
        if args.race:
            # The ltp wedge shape: their driver's next command arrived
            # WHILE the shell was still blocked in wait4 for a dying
            # child (no reader parked on the console). If those bytes
            # are dropped instead of queued, the shell waits forever.
            # One invocation; race M9 in mid-storm; M1 goes out only
            # after M0 (reader confirmed parked again).
            proc.stdin.write(b"/test/brkseq ; echo M0\n")
            proc.stdin.flush()
            time.sleep(float(args.race))
            proc.stdin.write(b"echo M9\n")
            proc.stdin.flush()
            m0 = pump(b"M0\r\n", 240) or pump(b"M0\n", 10)
            if FAIL_LINE.search(bytes(out)):
                rc = 2
            elif not m0:
                rc = 3
            else:
                m9 = pump(b"M9\r\n", 30) or pump(b"M9\n", 5)
                proc.stdin.write(b"echo M1\n")
                proc.stdin.flush()
                m1 = pump(b"M1\r\n", 60) or pump(b"M1\n", 10)
                time.sleep(20)
                rc = 0 if (m9 and m1) else 1
        else:
            m0_cmd = (f"/test/brkseq ; " * int(args.runs)) + "echo M0\n"
            proc.stdin.write(m0_cmd.encode())
            proc.stdin.flush()
            m0 = pump(b"M0\r\n", 180) or pump(b"M0\n", 10)
            if FAIL_LINE.search(bytes(out)):
                rc = 2
            elif not m0:
                rc = 3
            else:
                time.sleep(3)
                proc.stdin.write(b"echo M1\n")
                proc.stdin.flush()
                m1 = pump(b"M1\r\n", 60) or pump(b"M1\n", 10)
                # hold the window open for kernel-side forensics
                time.sleep(20)
                rc = 0 if m1 else 1
    finally:
        try:
            proc.kill()
            proc.wait(10)
        except OSError:
            pass
        open(os.path.join(WT, "run", "brksegv-shell-serial.log"),
             "wb").write(bytes(out))

    verdict = {0: "BRKSH-PASS", 1: "BRKSH-FAIL-deaf", 2: "BRKSH-FAIL-witness",
               3: "BRKSH-FAIL-m0", 5: "BRKSH-FAIL-driver"}
    print(f"{verdict[rc]} runs={args.runs}")
    return rc


if __name__ == "__main__":
    sys.exit(main())
