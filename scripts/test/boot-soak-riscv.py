#!/usr/bin/env python3
"""Scripted riscv64 repro boot for the sched fix set.

Boots the minimal musl rootfs with init=/bin/sh, feeds shell commands over
serial, and watches for POISONED/wedge/panic. Usage:

    python3 repro.py [--smp 2] [--seconds 120] [--kernel PATH] [--dfx schedtrace]
"""
import argparse
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
KERNEL = os.path.join(ROOT, "target/riscv64gc-unknown-none-elf/debug/rux")
ROOTFS = os.environ.get("RUX_ROOTFS", os.path.join(HERE, "rootfs.img"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--seconds", type=int, default=120)
    ap.add_argument("--kernel", default=KERNEL)
    ap.add_argument("--dfx", default="")
    ap.add_argument("--cpuext", default="", help="extra -cpu extension bits, e.g. ,zcmop=true")
    ap.add_argument("--log", default=None)
    args = ap.parse_args()

    log_path = args.log or os.path.join(HERE, "repro.log")
    logf = open(log_path, "wb")

    cmd = [
        "qemu-system-riscv64",
        "-M", "virt",
        "-accel", os.environ.get("RUX_TCG", "tcg,thread=single"),
        "-cpu", "rv64,zbb=true,zba=true,zbs=true" + args.cpuext,
        "-m", "1G",
        "-smp", str(args.smp),
        "-nographic",
        "-snapshot",
        "-drive", f"file={ROOTFS},if=none,id=rootfs,format=raw,snapshot=on,file.locking=off",
        "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
        "-kernel", args.kernel,
        "-append", f"root=/dev/vda rw init=/bin/sh console=ttyS0 dfx={args.dfx}".rstrip("="),
    ]
    print("CMD:", " ".join(cmd), flush=True)

    proc = subprocess.Popen(
        cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, bufsize=0,
    )

    markers = {
        b"POISONED": 0,
        b"KERNPANIC": 0,
        b"panicked": 0,
        b"Welcome to Rux": 0,
        b"R83": 0,
        b"R56": 0,
        b"EXITNOTIFY": 0,
        b"ORPHAN": 0,
        b"PICK-SKIP": 0,
        b"WEDGE-DETECT": 0,
        b"watchdog": 0,
    }
    buf = bytearray()
    last_output_time = time.time()
    start = time.time()
    wedge_reported = False
    boot_done = False
    commands_sent = False
    last_cmd_time = 0.0
    cmd_queue = []
    cur_cmd_idx = 0

    def feed(line):
        proc.stdin.write(line.encode() + b"\n")
        proc.stdin.flush()

    try:
        while time.time() - start < args.seconds:
            # non-blocking-ish read via select
            import select
            r, _, _ = select.select([proc.stdout], [], [], 0.2)
            if r:
                chunk = os.read(proc.stdout.fileno(), 4096)
                if not chunk:
                    print("EOF from qemu stdout", flush=True)
                    break
                buf.extend(chunk)
                logf.write(chunk)
                logf.flush()
                last_output_time = time.time()
                for m, c in list(markers.items()):
                    n = chunk.count(m)
                    if n:
                        markers[m] += n
                # keep buf bounded
                if len(buf) > 40000:
                    del buf[:-8000]
                sys.stdout.buffer.write(chunk)
                sys.stdout.buffer.flush()

            now = time.time()
            if not boot_done and b"init task (PID 1) enqueued" in buf:
                boot_done = True
            if boot_done and not commands_sent:
                time.sleep(2.0)
                feed("cd /proc")
                feed("pwd")
                feed("ls > /dev/null")
                feed("i=0; while [ $i -lt 300 ]; do ls /proc >/dev/null; i=$((i+1)); done; echo LOOP-DONE")
                feed("i=0; while [ $i -lt 3000 ]; do /bin/true; i=$((i+1)); done; echo CHURN-DONE")
                feed("j=0; while [ $j -lt 8000 ]; do j=$((j+1)); done; echo ARITH-DONE")
                commands_sent = True
                last_cmd_time = now
            # idle detection: shell wedged = no output for 20s while loop
            # should be producing serial traffic
            if commands_sent and now - last_output_time > 25.0 and not wedge_reported:
                # check whether the loop finished before going quiet
                if b"LOOP-DONE" in buf:
                    print("\n=== loop finished, then quiet (shell exited?) ===", flush=True)
                else:
                    print("\n=== WEDGE: no serial output for 25s, LOOP-DONE not seen ===", flush=True)
                wedge_reported = True
    finally:
        print("\n=== summary ===", flush=True)
        print("elapsed: %.1fs" % (time.time() - start), flush=True)
        for m, c in markers.items():
            if c:
                print("marker %-16s x%d" % (m.decode(), c), flush=True)
        print("wedge_reported:", wedge_reported, flush=True)
        print("loop_done:", b"LOOP-DONE" in buf, flush=True)
        logf.close()
        proc.kill()
        proc.wait()


if __name__ == "__main__":
    main()
