#!/usr/bin/env python3
"""bindmkdir_boot.py — boot the Rux kernel (riscv64) with the bind+chroot
mkdir probe as init and report the verdict.

  BMD-DONE printed and no rc!=0 line that matters -> PASS (rc 0)
  BMD-DONE absent at timeout                  -> TIMEOUT (rc 1)
  kernel panic                                -> rc 2

Usage: ./bindmkdir_boot.py [--smp 2] [--timeout 180] [--kernel ELF]
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
QEMU = "/usr/bin/qemu-system-riscv64"
CC = "riscv64-linux-gnu-gcc"
MKFS = "/usr/sbin/mkfs.ext4"
DEFAULT_KERNEL = os.path.join(
    WT, "target/riscv64gc-unknown-none-elf/debug/rux")


def build_image(img):
    staging = os.path.join(WT, "work", "bindmkdir_probe_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    # Rootfs skeleton the probe expects: it stats /dev and /proc, binds
    # /dev recursively and opens /dev/null through a jail of that bind.
    for d in ("dev", "proc", "sys", "run", "mnt"):
        os.makedirs(os.path.join(staging, d), exist_ok=True)
    open(os.path.join(staging, "dev", "null"), "wb").close()
    probe = os.path.join(WT, "work", "bindmkdir_probe")
    # Use the project musl SDK (toolchain/riscv64-rux-linux-musl): the
    # host distro gcc 15.2's static glibc contains ifunc-selected RVV
    # (vsetivli) memset variants, and the probe boots on a QEMU cpu
    # without the V extension -> SIGILL at the first memset. The SDK
    # build does not install a musl-gcc wrapper for cross builds, so
    # link musl's crt/libc explicitly through the distro gcc driver.
    sdk = os.path.join(WT, "toolchain", "riscv64-rux-linux-musl")
    if os.path.exists(os.path.join(sdk, "lib", "libc.a")):
        cc = [CC, "-static", "-O2", "-nostdinc",
              "-isystem", os.path.join(sdk, "include"),
              "-nostdlib",
              os.path.join(sdk, "lib", "crt1.o"),
              os.path.join(sdk, "lib", "crti.o")]
        libs = ["-L" + os.path.join(sdk, "lib"), "-lc",
                os.path.join(sdk, "lib", "crtn.o"), "-lgcc"]
    else:
        cc = [CC, "-static", "-O2", "-Wall", "-march=rv64gc"]
        libs = []
    subprocess.run(cc + ["-o", probe, os.path.join(HERE, "bindmkdir.c")] + libs,
                   check=True)
    shutil.copyfile(probe, os.path.join(staging, "test", "bindmkdir_probe"))
    os.chmod(os.path.join(staging, "test", "bindmkdir_probe"), 0o755)
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^flex_bg",
                        "-d", staging, img, "64M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--thread", default="multi", choices=["multi", "single"])
    ap.add_argument("--timeout", type=int, default=180)
    ap.add_argument("--mem", default="1G")
    ap.add_argument("--kernel", default=DEFAULT_KERNEL)
    args = ap.parse_args()

    os.makedirs(os.path.join(WT, "work"), exist_ok=True)
    img = os.path.join(WT, "work", "bindmkdir_probe.img")
    if not build_image(img):
        return 2
    if not os.path.exists(args.kernel):
        print("[boot] kernel missing: %s" % args.kernel)
        return 2

    append = "root=/dev/vda rw init=/test/bindmkdir_probe console=ttyS0"
    cmd = [QEMU,
           "-M", "virt", "-accel", "tcg,thread=" + args.thread,
           "-cpu", "rv64,zbb=true,zba=true,zbs=true",
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none", "-serial", "stdio",
           "-drive", "file=%s,if=none,id=rootfs,format=raw" % img,
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", args.kernel,
           "-append", append]
    print("[boot] qemu smp=%d thread=%s mem=%s" % (args.smp, args.thread, args.mem))

    ser = open(os.path.join(WT, "work", "bindmkdir_serial.log"), "wb")
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

    buf = bytearray()
    t0 = time.time()
    done = False
    panicked = False
    rc = 3
    try:
        while time.time() - t0 < args.timeout:
            r, _, _ = select.select([proc.stdout], [], [], 1.0)
            if r:
                chunk = os.read(proc.stdout.fileno(), 4096)
                if not chunk:
                    break
                buf.extend(chunk)
                ser.write(chunk)
                ser.flush()
                text = bytes(buf[-8192:])
                if b"BMD-DONE" in text:
                    done = True
                    break
                if b"KERNPANIC" in text or b"kernel panic" in text:
                    panicked = True
                    break
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
        ser.close()

    out = buf.decode("utf-8", "replace")
    if panicked:
        print("[verdict] kernel panic")
        rc = 2
    elif done:
        fails = [ln for ln in out.splitlines()
                 if ln.startswith("BMD[") and "rc=0" not in ln
                 and "openmiss probe" not in ln]
        # Steps whose FAILURE is the expected outcome: the openmiss
        # probe reports the missing file, the negative-dentry primes
        # must fail ENOENT, and the re-mkdir wants EEXIST (errno=17).
        hard = [ln for ln in fails
                if "missing" not in ln
                and "(prime)" not in ln
                and not ("want EEXIST" in ln and "errno=17" in ln)]
        if hard:
            print("[verdict] BMD-DONE with failing steps:")
            for ln in hard:
                print("  " + ln)
            rc = 1
        else:
            print("[verdict] PASS (BMD-DONE, no unexpected failures)")
            rc = 0
    else:
        print("[verdict] TIMEOUT waiting for BMD-DONE")
        rc = 1
    return rc


if __name__ == "__main__":
    sys.exit(main())
