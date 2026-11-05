#!/usr/bin/env python3
"""futex_requeue_boot.py — boot the x86_64 Rux kernel with the
FUTEX_REQUEUE chain probe (test/futex_requeue_probe.c, musl-built) as
init and report the verdict.

  FRQ PROBE RESULT: PASS -> rc 0
  FRQ PROBE RESULT: FAIL -> rc 1
  kernel panic        -> rc 2
  timeout             -> rc 3

Usage: ./futex_requeue_boot.py [--smp 2] [--timeout 300] [--kernel bzImage]
       [--rounds 10000] [--kick]
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
QEMU = "/usr/bin/qemu-system-x86_64"
MKFS = "/usr/sbin/mkfs.ext4"
SDK = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl")
DEFAULT_KERNEL = os.path.join(
    WT, "target/x86_64-unknown-none/debug/bzImage")
MARKER = "FRQ PROBE RESULT:"


def build_probe(out, arch):
    src = os.path.join(HERE, "futex_requeue_probe.c")
    if arch == "riscv64":
        cc = "riscv64-linux-gnu-gcc"
        sdk = os.path.join(WT, "toolchain", "riscv64-rux-linux-musl")
        march = ["-march=rv64gc_zba_zbb_zbs"]
    else:
        cc = "gcc"
        sdk = SDK
        march = []
    cmd = [cc, "-static", "-O2", "-Wall", "-pthread"] + march + [
        "-nostdinc", "-isystem", os.path.join(sdk, "include"),
        "-nostdlib", "-o", out, src,
        os.path.join(sdk, "lib/crt1.o"),
        os.path.join(sdk, "lib/crti.o"),
        os.path.join(sdk, "lib/crtn.o"),
        "-L" + os.path.join(sdk, "lib"), "-lc"]
    if arch == "riscv64":
        # fp128 helper symbols live in libgcc (same as probe-common.sh)
        cmd.append(subprocess.run(
            [cc] + march + ["-print-libgcc-file-name"],
            capture_output=True, text=True).stdout.strip())
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    return True


def build_image(img, probe):
    staging = os.path.join(WT, "work", "futex_requeue_probe_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    shutil.copyfile(probe, os.path.join(staging, "test", "futex_requeue_probe"))
    os.chmod(os.path.join(staging, "test", "futex_requeue_probe"), 0o755)
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^flex_bg",
                        "-d", staging, img, "32M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--arch", default="x86_64", choices=["x86_64", "riscv64"])
    ap.add_argument("--smp", type=int, default=1)
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--kernel", default=None)
    ap.add_argument("--rounds", type=int, default=10000)
    ap.add_argument("--kick", action="store_true")
    ap.add_argument("--accel", default="auto")
    args = ap.parse_args()

    if args.kernel is None:
        if args.arch == "riscv64":
            args.kernel = os.path.join(
                WT, "target/riscv64gc-unknown-none-elf/debug/rux")
        else:
            args.kernel = DEFAULT_KERNEL

    os.makedirs(os.path.join(WT, "work"), exist_ok=True)
    probe = os.path.join(WT, "work", "futex_requeue_probe_%s" % args.arch)
    if not build_probe(probe, args.arch):
        return 2
    img = os.path.join(WT, "work", "futex_requeue_probe_%s.img" % args.arch)
    if not build_image(img, probe):
        return 2
    if not os.path.exists(args.kernel):
        print("[boot] kernel missing: %s" % args.kernel)
        return 2

    # NOTE: the kernel does not yet pass extra init args from the cmdline,
    # so the probe's built-in defaults (10000 rounds, kick on) decide the
    # run parameters; init_args is kept for when argv passing lands.
    init_args = "/test/futex_requeue_probe"
    append = ("root=/dev/vda rw init=%s console=ttyS0" % init_args)
    if args.arch == "riscv64":
        if args.accel == "auto":
            args.accel = "tcg"  # riscv64 has no KVM on an x86 host
        qemu = "/usr/bin/qemu-system-riscv64"
        machine = "-M virt -accel tcg,thread=multi -cpu rv64"
        dev = "-device virtio-blk-pci,drive=rootfs"
    else:
        if args.accel == "auto":
            args.accel = "kvm"
        qemu = QEMU
        machine = ("-M q35 -accel kvm -cpu max" if args.accel == "kvm"
                   else "-M q35 -accel tcg,thread=multi -cpu max")
        dev = "-device virtio-blk-pci,disable-legacy=on,drive=rootfs"
    cmd = ("%s %s -m %s -smp %d -display none -serial stdio "
           "-drive file=%s,if=none,id=rootfs,format=raw,file.locking=off "
           "%s -kernel %s -append \"%s\""
           % (qemu, machine, args.mem, args.smp, img, dev,
              args.kernel, append))
    print("[boot] qemu arch=%s smp=%d accel=%s rounds=%d"
          % (args.arch, args.smp, args.accel, args.rounds))

    ser = open(os.path.join(WT, "work", "futex_requeue_serial.log"), "wb")
    proc = subprocess.Popen(cmd, shell=True, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

    buf = bytearray()
    t0 = time.time()
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
                text = bytes(buf[-8192:])
                if b"KERNPANIC" in text or b"Kernel panic" in text:
                    verdict = "PANIC"
                    rc = 2
                    break
                idx = text.find(MARKER.encode())
                if idx >= 0:
                    verdict = text[idx:].split(b"\n")[0].decode(
                        "utf-8", "replace")
                    rc = 0 if b"PASS" in text[idx:idx + 80] else 1
                    break
    finally:
        proc.terminate()
        try:
            proc.wait(5)
        except subprocess.TimeoutExpired:
            proc.kill()
        ser.close()

    tail = bytes(buf[-4096:]).decode("utf-8", "replace")
    print("[serial tail]\n%s" % tail[-2000:])
    if verdict:
        print("%s" % verdict)
    else:
        print("[boot] no verdict within %ds" % args.timeout)
    return rc


if __name__ == "__main__":
    sys.exit(main())
