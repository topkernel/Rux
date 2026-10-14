#!/usr/bin/env python3
"""LTP chunked scan driver for the Rux kernel (riscv64).

Boots QEMU per chunk of tests, runs each LTP binary as PID-1's child with a
30s wall-clock timeout, parses RES|name|rc|tail lines from the serial log,
classifies each test, and appends to a results file.

Usage:
  python3 scan.py --image test/rootfs.img --kernel target/.../rux \
      --tests tests.txt --out results.txt [--range a-k] [--chunk 100] \
      [--qemu-timeout 4200] [--attempts 5]

tests.txt: one test name per line (ordered). Results file format:
  idx|test|class|rc|tail
Wedged chunks are retried with the UNREPORTED tests only (auto-resume),
up to --attempts times; tests still missing after that are WEDGE.
"""
import argparse
import os
import re
import subprocess
import sys
import tempfile
import time

QEMU = "qemu-system-riscv64"


def classify(rc):
    if rc == 0:
        return "PASS"
    if rc in (124, 125):
        return "TIMEOUT"
    if rc in (126, 127):
        return "EXECFAIL"
    # 143 = SIGTERM, 158 = SIGKILL: the ltpscan wall-clock kill (30s/40s)
    # — TIMEOUT, not a crash. Other signal deaths are real CRASHes.
    if rc in (143, 158):
        return "TIMEOUT"
    if rc >= 128:
        return "CRASH"
    if rc == 4:
        return "WARN"
    if rc & 32:
        return "TCONF"
    if rc & 1 or rc & 2:
        return "FAIL"
    return "FAIL"


def debugfs_write(image, local_path, remote_path):
    """Replace remote_path in the ext4 image with local file (no loop mount)."""
    rdir, rname = os.path.split(remote_path)
    cmdfile = tempfile.NamedTemporaryFile(mode="w", suffix=".dbfs", delete=False)
    cmdfile.write(f"cd {rdir}\nrm {rname}\nwrite {local_path} {rname}\n")
    cmdfile.close()
    try:
        p = subprocess.run(["debugfs", "-w", "-f", cmdfile.name, image],
                           capture_output=True, text=True, timeout=120)
        # debugfs exits 0 even on some errors; grep stderr for hard failures
        if "File not found" in p.stderr or "Cannot" in p.stderr:
            print(f"[scan] debugfs stderr: {p.stderr.strip()[:300]}", file=sys.stderr)
    finally:
        os.unlink(cmdfile.name)


def run_chunk(image, kernel, tests, qemu_timeout, smp, mem, log_path):
    """Boot once and run `tests`. Returns {name: (rc, tail)} for reported tests."""
    chunk_local = tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False)
    chunk_local.write("\n".join(tests) + "\n")
    chunk_local.close()
    debugfs_write(image, chunk_local.name, "/test/linux-ltp/chunk.txt")
    os.unlink(chunk_local.name)

    cmd = [
        QEMU, "-M", "virt", "-accel", "tcg,thread=single",
        "-cpu", "rv64,zbb=true,zba=true,zbs=true",
        "-m", mem, "-smp", str(smp), "-nographic", "-snapshot",
        "-serial", "mon:stdio",
        "-drive", f"file={image},if=none,id=rootfs,format=raw,file.locking=off",
        "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
        "-kernel", kernel,
        "-append", "root=/dev/vda rw init=/test/ltpscan console=ttyS0",
    ]
    with open(log_path, "wb") as log:
        proc = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT)
        deadline = time.time() + qemu_timeout
        while time.time() < deadline and proc.poll() is None:
            time.sleep(3)
            try:
                with open(log_path, "rb") as lf:
                    data = lf.read()
                if b"SCAN_DONE" in data or b"Power down" in data \
                        or b"kernel panic" in data:
                    time.sleep(2)  # let the last serial bytes drain
                    break
                # Kernel spinlock watchdog: repeated DEADLOCK lines mean the
                # kernel is wedged — bail out now instead of burning the
                # whole chunk budget.
                if data.count(b"DEADLOCK: spinlock stuck") >= 3:
                    time.sleep(2)
                    break
                # Hung-task detector storm: tasks blocked >120s with no RES
                # progress (observed: heap-exhaustion SIGKILL leaves the
                # sweep runner stuck in waitpid forever; retrying the tail
                # in a fresh boot recovers everything after the culprit).
                if data.count(b"blocked for more than") >= 3:
                    time.sleep(2)
                    break
            except OSError:
                pass
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
    return parse_serial(log_path)


RES_RE = re.compile(r"^RES\|([^|]+)\|(-?\d+)\|(.*)$", re.M)


def parse_serial(log_path):
    with open(log_path, "r", errors="replace") as f:
        text = f.read()
    out = {}
    for m in RES_RE.finditer(text):
        name, rc, tail = m.group(1), int(m.group(2)), m.group(3)
        # Last occurrence wins (retry duplicates)
        out[name] = (rc, tail.strip())
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--image", default="test/rootfs.img")
    ap.add_argument("--kernel", default="target/riscv64gc-unknown-none-elf/debug/rux")
    ap.add_argument("--tests", required=True, help="file with one test name per line")
    ap.add_argument("--only", help="comma-separated test names to run (overrides --tests)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--range", dest="letter_range", help="e.g. a-k: first-letter filter")
    ap.add_argument("--chunk", type=int, default=100)
    ap.add_argument("--qemu-timeout", type=int, default=4200)
    ap.add_argument("--attempts", type=int, default=5)
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--keep-logs", help="directory to keep per-chunk serial logs")
    args = ap.parse_args()

    if args.only:
        tests = [t.strip() for t in args.only.split(",") if t.strip()]
    else:
        with open(args.tests) as f:
            tests = [l.strip() for l in f if l.strip() and not l.startswith("#")]
    if args.letter_range:
        lo, hi = args.letter_range.split("-")
        tests = [t for t in tests if lo <= t[0] <= hi]
    print(f"[scan] {len(tests)} tests, chunk={args.chunk}", flush=True)

    results = {}  # name -> (class, rc, tail)
    t0 = time.time()
    for ci in range(0, len(tests), args.chunk):
        chunk = tests[ci:ci + args.chunk]
        remaining = list(chunk)
        for attempt in range(1, args.attempts + 1):
            if not remaining:
                break
            log_path = os.path.join(args.keep_logs, f"chunk{ci:04d}-a{attempt}.log") \
                if args.keep_logs else tempfile.mktemp(suffix=".log")
            got = run_chunk(args.image, args.kernel, remaining,
                            args.qemu_timeout, args.smp, args.mem, log_path)
            for name in list(remaining):
                if name in got:
                    rc, tail = got[name]
                    results[name] = (classify(rc), rc, tail)
            remaining = [t for t in remaining if t not in got]
            done = len(chunk) - len(remaining)
            print(f"[scan] chunk@{ci}: attempt {attempt} reported {len(got)}, "
                  f"cumulative {done}/{len(chunk)}, elapsed {time.time()-t0:.0f}s",
                  flush=True)
            if remaining:
                print(f"[scan] chunk@{ci}: WEDGE retry with: {remaining[:10]}{'...' if len(remaining)>10 else ''}", flush=True)
        for name in remaining:
            results[name] = ("WEDGE", "", "")

    with open(args.out, "w") as f:
        f.write("# idx|test|class|rc|tail\n")
        for i, t in enumerate(tests):
            cls, rc, tail = results.get(t, ("NOTRUN", "", ""))
            f.write(f"{i}|{t}|{cls}|{rc}|{tail}\n")

    from collections import Counter
    c = Counter(v[0] for v in results.values())
    print(f"[scan] done in {time.time()-t0:.0f}s: " +
          " ".join(f"{k}={v}" for k, v in sorted(c.items())), flush=True)


if __name__ == "__main__":
    main()
