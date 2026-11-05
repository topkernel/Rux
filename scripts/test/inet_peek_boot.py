#!/usr/bin/env python3
"""inet_peek_boot.py — boot the Rux kernel (x86_64) with the
inet_peek probe as init and judge the review-func network.rs round
(socket address write-back clamps + unix datagram all-or-nothing sends).

Verdict:
  INETPEEK-PASS  (INETPEEK-DONE fails=0)               -> rc 0
  INETPEEK-FAIL  (INETPEEK-DONE fails>0)               -> rc 1
  kernel panic / trap / deadlock                       -> rc 2
  timeout (no INETPEEK-DONE line)                      -> rc 3

Usage:
  ./inet_peek_boot.py [--secs 60] [--mem 2048M] [--kernel bzImage]
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
MKFS = "/usr/sbin/mkfs.ext4"
MUSL_CC = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl", "bin",
                       "musl-gcc")

DONE_LINE = re.compile(rb"INETPEEK-DONE fails=(\d+)")
CASE_LINE = re.compile(rb"INETPEEK-CASE (\S+) (PASS|FAIL) \(([^)]*)\)")
FAIL_LINE = re.compile(
    rb"Kernel panic|panic: |DEADLOCK: spinlock stuck|BUG: soft lockup|"
    rb"trap: #GP|general protection fault")


def build_image(img, probe_bin, name):
    staging = tempfile.mkdtemp(prefix="sockaddr_root")
    os.makedirs(os.path.join(staging, "test"))
    shutil.copy(probe_bin, os.path.join(staging, "test", name))
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^64bit",
                        "-d", staging, img, "72M"],
                       capture_output=True)
    shutil.rmtree(staging)
    if r.returncode != 0:
        print("mkfs failed:", r.stderr.decode(), file=sys.stderr)
        sys.exit(4)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--smp", default="2")
    ap.add_argument("--secs", default="90")
    ap.add_argument("--mem", default="2048M")
    ap.add_argument("--kernel", default=None)
    ap.add_argument("--accel", default="kvm")
    ap.add_argument("--arch", default="x86_64", choices=["x86_64", "riscv64"],
                    help="riscv64 boots a raw ELF kernel on the virt "
                         "machine (loopback TCP connect/accept works "
                         "there; the x86_64 loopback TCP handshake does "
                         "not complete yet, which wedges the accept case)")
    ap.add_argument("--skip-accept", action="store_true",
                    help="skip the TCP accept case (x86_64)")
    ap.add_argument("--keep-img", action="store_true")
    args = ap.parse_args()

    src = os.path.join(HERE, "inet_peek.c")
    arch = args.arch
    march = []
    linktail = []
    if arch == "riscv64":
        # Same recipe as brksegv_boot.py: the worktree's riscv64 musl is
        # headers+libs only, and the host cross-gcc defaults to a
        # RVV-capable -march the kernel cannot decode — pin the march,
        # use the musl headers, and link the musl crt/libc objects
        # directly.
        musl_root = os.path.join(WT, "toolchain",
                                 "riscv64-rux-linux-musl")
        gccdir = subprocess.run(
            ["riscv64-linux-gnu-gcc", "-print-libgcc-file-name"],
            capture_output=True, text=True).stdout.strip()
        gccdir = os.path.dirname(gccdir)
        march = ["-march=rv64gc_zicsr", "-mabi=lp64d",
                 "-nostdinc", "-isystem",
                 os.path.join(musl_root, "include"), "-nostdlib"]
        linktail = [os.path.join(musl_root, "lib", "crt1.o"),
                    os.path.join(musl_root, "lib", "crti.o"),
                    os.path.join(gccdir, "crtbegin.o"),
                    "-L" + os.path.join(musl_root, "lib"),
                    "-lc", "-lgcc",
                    os.path.join(gccdir, "crtend.o"),
                    os.path.join(musl_root, "lib", "crtn.o")]
        musl = "riscv64-linux-gnu-gcc"
    else:
        musl = MUSL_CC
    binp = os.path.join(WT, "work", f"inet_peek-{arch}")
    os.makedirs(os.path.join(WT, "run"), exist_ok=True)
    os.makedirs(os.path.join(WT, "work"), exist_ok=True)
    env = dict(os.environ)
    env.setdefault("REALGCC", "gcc")
    subprocess.run([musl, "-static", "-O1", "-Wall", *march,
                    "-o", binp, src, *linktail], check=True, env=env)

    img = os.path.join(WT, "work", f"inet_peek-{arch}.img")
    build_image(img, binp, "inet_peek")

    if arch == "riscv64":
        qemu = "qemu-system-riscv64"
        kernel = args.kernel or os.path.join(
            WT, "target/riscv64gc-unknown-none-elf/debug/rux")
        # thread=multi (MTTCG): the accept case runs a blocking parent
        # accept racing a fork()'d child connect; under thread=single
        # both sleep with nothing draining the loopback queue.
        accel = ["-accel", "tcg,thread=multi"]
        mach = ["-M", "virt",
                "-cpu", "rv64,zbb=true,zba=true,zbs=true"]
    else:
        qemu = QEMU
        kernel = args.kernel or os.path.join(
            WT, "target/x86_64-unknown-none/debug/bzImage")
        accel = ["-accel", "kvm", "-accel", "tcg"] \
            if args.accel == "kvm" else ["-accel", "tcg,thread=multi"]
        mach = ["-M", "q35", "-cpu", "max"]
    append = "root=/dev/vda rw init=/test/inet_peek console=ttyS0"
    if args.skip_accept:
        append += " sockaddr.skip_accept=1"
    cmd = [qemu, *mach, *accel,
           "-m", args.mem, "-smp", args.smp,
           "-display", "none", "-serial", "stdio", "-monitor", "none",
           "-no-reboot", "-snapshot",
           "-drive", f"file={img},if=none,id=rootfs,format=raw,file.locking=off",
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", kernel,
           "-append", append]

    out = bytearray()
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT)
    deadline = time.time() + int(args.secs)
    done = None
    try:
        import select as _select
        fd = proc.stdout.fileno()
        while time.time() < deadline:
            r, _, _ = _select.select([fd], [], [], 1.0)
            if r:
                chunk = os.read(fd, 65536)
                if not chunk:
                    break
                out += chunk
                m = DONE_LINE.search(bytes(out))
                if m:
                    done = int(m.group(1))
                    break
    finally:
        try:
            proc.kill()
            proc.wait(10)
        except OSError:
            pass
        if not args.keep_img:
            try:
                os.unlink(img)
            except OSError:
                pass

    open(os.path.join(WT, "run", "sockaddr-clamp-serial.log"), "wb") \
        .write(bytes(out))

    for c in CASE_LINE.findall(bytes(out)):
        print("case:", c[0].decode(), c[1].decode(), c[2].decode())

    if FAIL_LINE.search(bytes(out)):
        print("INETPEEK-FAIL-kernel-witness")
        return 2
    if done is None:
        print("INETPEEK-FAIL-no-done-line")
        return 3
    if done > 0:
        print(f"INETPEEK-FAIL fails={done}")
        return 1
    print("INETPEEK-PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
