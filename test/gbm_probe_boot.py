#!/usr/bin/env python3
"""gbm_probe_boot.py — boot the Rux x86_64 kernel with the OH image's own
Mesa gbm stack as init and report the verdict.

Stages a private ext4 image carrying:
  /test/gbm_probe          (dynamic, OH musl loader)
  /lib/ld-musl-x86_64.so.1, /lib64/libc.so   (from the OH ramdisk)
  /vendor/lib64/chipsetsdk/{libgbm,libdrm,libgallium_dri,libEGL,libhilog,
   libc++,virtio_gpu_dri,...} (from the OH vendor/system images)
plus /etc/ld-musl-x86_64.path from the ramdisk.

Verdict: GBM_PROBE RESULT: PASS n/n -> rc 0, else 1; panic -> 2; timeout -> 3.

Usage: ./gbm_probe_boot.py [--timeout 420] [--smp 1] [--mem 2G]
"""
import argparse
import os
import shutil
import subprocess
import sys
import time
import socket

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(HERE)
QEMU = "qemu-system-x86_64"
MKFS = "/usr/sbin/mkfs.ext4"
BZIMAGE = os.path.join(WT, "target", "x86_64-unknown-none", "debug", "bzImage")
MUSL = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl")
OHROOT = "/tmp/ohroot"
MARKER = "GBM_PROBE RESULT:"
WORK = os.path.join(WT, "work")


def build_probe(out):
    cmd = ["gcc", "-O1", "-nostartfiles", "-o", out,
           os.path.join(HERE, "gbm_probe.c"),
           os.path.join(MUSL, "lib", "crt1.o"),
           "-Wl,-e,_start",
           "-Wl,--dynamic-linker,/lib/ld-musl-x86_64.so.1",
           "-L", os.path.join(OHROOT, "lib64"), "-lc",
           "-I", os.path.join(MUSL, "include"), "-lgcc"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    return True


def build_image(img):
    staging = os.path.join(WORK, "gbm_probe_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    os.makedirs(os.path.join(staging, "lib"))
    os.makedirs(os.path.join(staging, "lib64"))
    os.makedirs(os.path.join(staging, "etc"))
    shutil.copyfile(os.path.join(OHROOT, "test_gbm_probe"),
                    os.path.join(staging, "test", "gbm_probe"))
    os.chmod(os.path.join(staging, "test", "gbm_probe"), 0o755)
    shutil.copyfile(os.path.join(OHROOT, "lib64", "ld-musl-x86_64.so.1"),
                    os.path.join(staging, "lib", "ld-musl-x86_64.so.1"))
    shutil.copyfile(os.path.join(OHROOT, "lib64", "libc.so"),
                    os.path.join(staging, "lib64", "libc.so"))
    shutil.copyfile("/tmp/rd64/etc/ld-musl-x86_64.path",
                    os.path.join(staging, "etc", "ld-musl-x86_64.path"))
    # driver libs go to /lib (musl ldso default search dir) — the OH
    # path-file mechanism needs the namespace ini; /lib always works.
    src = os.path.join(OHROOT, "vendor", "lib64", "chipsetsdk")
    dst = os.path.join(staging, "lib")
    os.makedirs(dst, exist_ok=True)
    for f in os.listdir(src):
        if os.path.islink(os.path.join(src, f)):
            continue
        shutil.copyfile(os.path.join(src, f), os.path.join(dst, f))
    # recreate the driver-name symlinks
    for a, b in [("virtio_gpu_dri.so", "libgallium_dri.so"),
                 ("kms_swrast_dri.so", "libgallium_dri.so"),
                 ("swrast_dri.so", "libgallium_dri.so"),
                 ("libgbm.so", "libgbm.so.1.0.0"),
                 ("libEGL.so.1", "libEGL.so.1.0.0")]:
        try:
            os.symlink(b, os.path.join(dst, a))
        except FileExistsError:
            pass
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^has_journal",
                        "-d", staging, img, "512M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=int, default=420)
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="2G")
    args = ap.parse_args()

    os.makedirs(WORK, exist_ok=True)
    if not os.path.exists(BZIMAGE):
        print("[boot] bzImage missing: %s" % BZIMAGE)
        return 2
    probe = os.path.join(OHROOT, "test_gbm_probe")
    if not (os.path.exists(probe) or build_probe(probe)):
        return 2
    img = os.path.join(WORK, "gbm_probe.img")
    if not build_image(img):
        return 2

    serial = os.path.join(WORK, "gbm_probe_serial.log")
    for p in (serial,):
        if os.path.exists(p):
            os.remove(p)

    append = "root=/dev/vda rw init=/test/gbm_probe console=ttyS0"
    cmd = [QEMU,
           "-M", "q35", "-accel", "tcg,thread=multi", "-cpu", "max",
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none",
           "-device", "virtio-gpu-pci,id=gpu0,xres=720,yres=1280",
           "-serial", "file:" + serial,
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", BZIMAGE,
           "-append", append]
    print("[boot] qemu smp=%s mem=%s" % (args.smp, args.mem))
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        t0 = time.time()
        verdict = None
        while time.time() - t0 < args.timeout:
            time.sleep(3)
            if proc.poll() is not None:
                break
            try:
                with open(serial, "rb") as f:
                    data = f.read()
            except FileNotFoundError:
                continue
            for line in data.split(b"\n"):
                if MARKER.encode() in line:
                    verdict = line.decode("utf-8", "replace").strip()
        if verdict is None:
            print("[probe] TIMEOUT (no verdict)")
            rc = 3
        else:
            print("[probe] %s" % verdict)
            rc = 0 if " PASS " in verdict + " " else 1
    finally:
        proc.terminate()
        try:
            proc.wait(10)
        except Exception:
            proc.kill()
    return rc


if __name__ == "__main__":
    sys.exit(main())
