#!/usr/bin/env python3
# MIT License
# Copyright (c) 2026 Fei Wang

"""perfsample_boot.py — end-to-end driver for the kernel tick-sampler
baseline report (test/perf_sample.c + scripts/perf_report.py).

Boots the Rux kernel with test/perf_sample as init; the probe arms
per-CPU cpu-clock sampling events (10 ms period, kernel-mode only, with
callchains), runs the pbench load as a child for the sampling window,
streams the ring records into /test/perf.data (reference perf.data
container), prints PERFSAMPLE markers and powers off. The driver then
extracts the dump from the ext4 image (debugfs) and runs the report.

The symbol table is snapshotted from kernel/core/src/debug/
(ksyms_generated.rs + ksyms.bin) right after the kernel build — that is
the exact table embedded in the booted kernel, and it is regenerated
per-platform on the next build, so the snapshot keeps dump and symbols
paired.

  rc 0  sample + report produced
  rc 1  kernel panic before the DONE marker
  rc 3  no DONE within --timeout
  rc 2  build/image failure

Usage: ./perfsample_boot.py [--arch x86_64|riscv64] [--seconds 45]
       [--accel kvm] [--smp 2] [--mem 2G] [--top 20] [--tag NAME]
       [--bzimage PATH (x86)] [--kernel PATH (riscv)] [--no-report]
Artifacts land in run/perfsample-<tag>/ (never committed).
"""
import argparse
import os
import shutil
import subprocess
import sys
import time
from ruxroot import repo_root

HERE = os.path.dirname(os.path.abspath(__file__))
WT = repo_root(__file__)
MKFS = "/usr/sbin/debugfs"
MKFS_EXT4 = "/usr/sbin/mkfs.ext4"
DEBUGFS = MKFS
KSYSMS_DIR = os.path.join(WT, "kernel", "core", "src", "debug")
REPORT = os.path.join(WT, "scripts", "perf_report.py")
SIZE_MB = 192


def x86_musl_cc():
    return os.path.join(WT, "toolchain", "x86_64-rux-linux-musl", "bin",
                        "musl-gcc")


def build_probe(out, src, arch):
    if arch == "x86_64":
        cc = x86_musl_cc()
        if not os.path.exists(cc):
            print("[build] x86 musl toolchain missing: %s "
                  "(tar xzf toolchain/x86_64-rux-linux-musl.tar.gz)" % cc)
            return False
        cmd = [cc, "-static", "-O2", "-Wall", "-pthread", "-o", out, src]
    else:
        # riscv64: in-tree musl (scalar-safe; glibc-static carries RVV),
        # same recipe as pbench_boot_rv.py.
        musl = os.path.join(WT, "toolchain", "riscv64-rux-linux-musl")
        if not os.path.exists(os.path.join(musl, "lib", "libc.a")):
            print("[build] riscv64 musl missing: run toolchain/build-musl.sh")
            return False
        cmd = ["/usr/bin/riscv64-linux-gnu-gcc", "-static", "-nostdlib",
               "-march=rv64gc_zicsr", "-O2", "-Wall",
               # the distro gcc defaults to -D_FORTIFY_SOURCE=2; the
               # in-tree musl carries no __*_chk shims
               "-U_FORTIFY_SOURCE", "-D_FORTIFY_SOURCE=0",
               "-I", os.path.join(musl, "include"), "-o", out, src,
               os.path.join(musl, "lib", "crt1.o"),
               os.path.join(musl, "lib", "libc.a"), "-lgcc"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed:\n%s" % r.stderr)
        return False
    print("[build] %s (%d KB)" % (out, os.path.getsize(out) // 1024))
    return True


def build_image(img, probes, conf_text):
    staging = os.path.join(WT, "run", "perfsample_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    for name, path in probes.items():
        shutil.copyfile(path, os.path.join(staging, "test", name))
        os.chmod(os.path.join(staging, "test", name), 0o755)
    with open(os.path.join(staging, "test", "perfsample.conf"), "w") as fh:
        fh.write(conf_text)
    r = subprocess.run([MKFS_EXT4, "-q", "-F", "-O", "^metadata_csum",
                        "-d", staging, img, "%dM" % SIZE_MB],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def debugfs_dump(img, guest_path, host_path):
    r = subprocess.run([DEBUGFS, "-R",
                        "dump %s %s" % (guest_path, host_path), img],
                       capture_output=True, text=True)
    if r.returncode != 0 or not os.path.exists(host_path):
        print("[extract] debugfs dump %s failed: %s" % (guest_path,
                                                        r.stderr.strip()))
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--arch", choices=["x86_64", "riscv64"],
                    default="x86_64")
    ap.add_argument("--seconds", type=int, default=40,
                    help="sampling window; staged via /test/perfsample.conf "
                         "(the kernel passes no argv to init). Must stay "
                         "below the pbench battery duration (~40 s on x86 "
                         "KVM) so the recorder kills pbench at or before "
                         "its final reboot(POWER_OFF)")
    ap.add_argument("--accel", default=None,
                    help="default: kvm (x86_64) / tcg,thread=multi (riscv64)")
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--tag", default=None)
    ap.add_argument("--bzimage", default=None, help="x86_64 kernel image")
    ap.add_argument("--kernel", default=None, help="riscv64 kernel ELF")
    ap.add_argument("--top", type=int, default=20)
    ap.add_argument("--no-report", action="store_true")
    ap.add_argument("--timeout", type=int, default=1800)
    args = ap.parse_args()

    tag = args.tag or ("ps-x86" if args.arch == "x86_64" else "ps-rv")
    outdir = os.path.join(WT, "run", "perfsample-%s" % tag)
    os.makedirs(outdir, exist_ok=True)

    if args.arch == "x86_64":
        kernel = args.bzimage or os.path.join(
            WT, "target", "x86_64-unknown-none", "debug", "bzImage")
        accel = args.accel or "kvm"
        qemu = "qemu-system-x86_64"
        machine = ["-M", "q35", "-cpu", "max"]
    else:
        kernel = args.kernel or os.path.join(
            WT, "target", "riscv64gc-unknown-none-elf", "debug", "rux")
        accel = args.accel or "tcg,thread=multi"
        qemu = "qemu-system-riscv64"
        machine = ["-M", "virt",
                   "-cpu", "rv64,zbb=true,zba=true,zbs=true,"
                           "zicond=true,v=true,vlen=256,elen=64"]
    if not os.path.exists(kernel):
        print("[boot] kernel missing: %s" % kernel)
        return 2

    # Snapshot the booted kernel's own symbol table (regenerated per
    # platform at every build — pair it with THIS kernel now).
    for fname in ("ksyms_generated.rs", "ksyms.bin"):
        src = os.path.join(KSYSMS_DIR, fname)
        if os.path.exists(src):
            shutil.copyfile(src, os.path.join(outdir, fname))
        else:
            print("[symtab] WARNING %s missing" % src)

    # Build the recorder + the load.
    recorder = os.path.join(outdir, "perfsample")
    load = os.path.join(outdir, "pbench")
    if not build_probe(recorder,
                       os.path.join(WT, "test", "perf_sample.c"), args.arch):
        return 2
    if not build_probe(load, os.path.join(WT, "test", "pbench.c"), args.arch):
        return 2

    img = os.path.join(outdir, "rootfs.img")
    if not build_image(img, {"perfsample": recorder, "pbench": load},
                       "%d\n" % args.seconds):
        return 2

    serial = os.path.join(outdir, "serial.log")
    append = "root=/dev/vda rw init=/test/perfsample console=ttyS0"
    blk = "virtio-blk-pci,drive=rootfs"
    if args.arch == "riscv64":
        blk = "virtio-blk-pci,disable-legacy=on,drive=rootfs"
    cmd = [qemu] + machine + ["-accel", accel,
                              "-m", args.mem, "-smp", str(args.smp),
                              "-display", "none",
                              "-serial", "file:" + serial,
                              "-drive",
                              "file=%s,if=none,id=rootfs,format=raw,"
                              "file.locking=off" % img,
                              "-device", blk,
                              "-kernel", kernel, "-append", append]
    print("[boot] tag=%s arch=%s accel=%s smp=%s mem=%s window=%ss"
          % (tag, args.arch, accel, args.smp, args.mem, args.seconds))
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
                if line.startswith(b"PERFSAMPLE") or line.startswith(b"PBENCH"):
                    print("[guest] %s" % line.decode(errors="replace"))
            seen = len(data.split(b"\n"))
            if b"PERFSAMPLE DONE rc=0" in data:
                print("[boot] DONE marker at t=%ds" % (time.time() - t0))
                # wait for the guest's own poweroff, then reap
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

    # Extract the dump + results from the image (guest poweroff flushed).
    dump = os.path.join(outdir, "perf.data")
    if not debugfs_dump(img, "/test/perf.data", dump):
        print("[extract] perf.data missing from image")
        return 3
    debugfs_dump(img, "/test/pbench-results.txt",
                 os.path.join(outdir, "pbench-results.txt"))
    print("[extract] %s (%d KiB)" % (dump, os.path.getsize(dump) // 1024))

    if args.no_report:
        return 0
    r = subprocess.run([sys.executable, "-I", REPORT, dump,
                        "--ksyms-dir", outdir, "--top", str(args.top)],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print(r.stdout)
        print(r.stderr)
        return r.returncode
    print(r.stdout)
    # Keep the report + json next to the artifacts (run/, not git).
    with open(os.path.join(outdir, "report.txt"), "w") as fh:
        fh.write(r.stdout)
    subprocess.run([sys.executable, "-I", REPORT, dump,
                    "--ksyms-dir", outdir, "--json"],
                   stdout=open(os.path.join(outdir, "report.json"), "w"),
                   text=True, check=True)
    print("[report] %s (+ .json)" % os.path.join(outdir, "report.txt"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
