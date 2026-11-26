#!/usr/bin/env python3
# MIT License
# Copyright (c) 2026 Fei Wang

"""hotmap_boot.py — sampling-window driver for the p23-hotmap lane.

Boots a kernel with an INJECTED test image (probe binaries + conf files
already staged into /test by inject.sh on a worktree-local image copy),
with the perf_sample recorder as init. The recorder arms 1 ms cpu-clock
sampling events, execs /test/pbench (the injected multi-case load
wrapper), drains the rings for the window, streams perf.data, prints
PERFSAMPLE markers and powers off. The driver extracts the dump and
runs scripts/perf_report.py against the ksyms snapshot taken at boot
start (the exact table embedded in the booted kernel — the build
regenerates it per platform, so snapshots keep dump and symbols paired
across later rebuilds).

  rc 0  sample + report produced
  rc 1  kernel panic before DONE
  rc 3  no DONE within --timeout
  rc 2  missing kernel/image/report tool

Usage: hotmap_boot.py --img PATH --kernel PATH --tag NAME
       [--smp 2] [--mem 2G] [--top 100] [--timeout 600] [--no-report]
Artifacts land in run/hotmap/out-<tag>/.
"""
import argparse
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
REPORT = os.path.join(WT, "scripts", "perf_report.py")
KSYMS_DIR = os.path.join(WT, "kernel", "core", "src", "debug")
DEBUGFS = "/usr/sbin/debugfs"


def debugfs_dump(img, guest_path, host_path):
    r = subprocess.run([DEBUGFS, "-R", "dump %s %s" % (guest_path, host_path),
                        img], capture_output=True, text=True)
    return r.returncode == 0 and os.path.exists(host_path)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--img", required=True, help="injected test image copy")
    ap.add_argument("--kernel", required=True, help="bzImage to boot")
    ap.add_argument("--tag", required=True)
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--top", type=int, default=100)
    ap.add_argument("--timeout", type=int, default=600)
    ap.add_argument("--no-report", action="store_true")
    ap.add_argument("--extra-append", default="")
    args = ap.parse_args()

    for p in (args.img, args.kernel, REPORT):
        if not os.path.exists(p):
            print("[boot] missing: %s" % p)
            return 2

    outdir = os.path.join(HERE, "out-%s" % args.tag)
    os.makedirs(outdir, exist_ok=True)

    # Snapshot the booted kernel's own symbol table NOW (before any
    # later build can regenerate it).
    for fname in ("ksyms_generated.rs", "ksyms.bin"):
        src = os.path.join(KSYMS_DIR, fname)
        if os.path.exists(src):
            shutil.copyfile(src, os.path.join(outdir, fname))
        else:
            print("[symtab] WARNING %s missing" % src)

    serial = os.path.join(outdir, "serial.log")
    append = ("root=/dev/vda rw init=/test/perfsample console=ttyS0 "
              + args.extra_append)
    cmd = ["qemu-system-x86_64", "-M", "q35", "-cpu", "host",
           "-accel", "kvm", "-m", args.mem, "-smp", str(args.smp),
           "-display", "none", "-serial", "file:" + serial,
           "-drive", "file=%s,if=none,id=rootfs,format=raw,"
                     "file.locking=off" % args.img,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", args.kernel, "-append", append]
    print("[boot] tag=%s smp=%s mem=%s img=%s" %
          (args.tag, args.smp, args.mem, os.path.basename(args.img)))
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)
    rc = 3
    try:
        t0 = time.time()
        seen = 0
        while time.time() - t0 < args.timeout:
            time.sleep(3)
            if proc.poll() is not None:
                rc = 0 if os.path.exists(serial) else 3
                break
            try:
                with open(serial, "rb") as fh:
                    data = fh.read()
            except FileNotFoundError:
                continue
            for line in data.split(b"\n")[seen:]:
                if line.startswith(b"PERFSAMPLE") or \
                   line.startswith(b"HOTMAPLOAD"):
                    print("[guest] %s" % line.decode(errors="replace"))
            seen = len(data.split(b"\n"))
            if b"PERFSAMPLE DONE rc=0" in data:
                print("[boot] DONE at t=%ds" % (time.time() - t0))
                try:
                    proc.wait(90)
                except subprocess.TimeoutExpired:
                    proc.terminate()
                rc = 0
                break
            if b"panicked" in data or b"Kernel panic" in data:
                print("[boot] KERNEL PANIC at t=%ds" % (time.time() - t0))
                rc = 1
                proc.terminate()
                break
        else:
            print("[boot] no DONE within %ds" % args.timeout)
            proc.terminate()
    finally:
        try:
            proc.wait(5)
        except Exception:
            proc.kill()

    if rc != 0:
        return rc

    dump = os.path.join(outdir, "perf.data")
    if not debugfs_dump(args.img, "/test/perf.data", dump):
        print("[extract] perf.data missing from image")
        return 3
    debugfs_dump(args.img, "/test/pbench-results.txt",
                 os.path.join(outdir, "load-results.txt"))
    print("[extract] %s (%d KiB)" % (dump, os.path.getsize(dump) // 1024))

    if args.no_report:
        return 0
    r = subprocess.run([sys.executable, "-I", REPORT, dump,
                        "--ksyms-dir", outdir, "--top", str(args.top),
                        "--sort", "total"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print(r.stdout)
        print(r.stderr)
        return r.returncode
    with open(os.path.join(outdir, "report-top%d-total.txt" % args.top),
              "w") as fh:
        fh.write(r.stdout)
    r2 = subprocess.run([sys.executable, "-I", REPORT, dump,
                         "--ksyms-dir", outdir, "--json", "--all"],
                        capture_output=True, text=True)
    if r2.returncode == 0:
        with open(os.path.join(outdir, "report.json"), "w") as fh:
            fh.write(r2.stdout)
    r3 = subprocess.run([sys.executable, "-I", REPORT, dump,
                         "--ksyms-dir", outdir, "--all", "--sort", "self"],
                        capture_output=True, text=True)
    if r3.returncode == 0:
        with open(os.path.join(outdir, "report-full-self.txt"), "w") as fh:
            fh.write(r3.stdout)
    print("[report] %s (+ report.json + report-full-self.txt)" %
          os.path.join(outdir, "report-top%d-total.txt" % args.top))
    return 0


if __name__ == "__main__":
    sys.exit(main())
