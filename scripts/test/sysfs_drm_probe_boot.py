#!/usr/bin/env python3
"""sysfs_drm_probe_boot.py — boot the Rux x86_64 kernel with the DRM/KMS probe
as init and report the verdict.

Builds a tiny private ext4 image containing only the probe (mkfs.ext4 -d,
no root privileges needed) and boots it q35 + virtio-gpu-pci 720x1280 —
the same display machine config the OH boots use — with
init=/test/sysfs_drm_probe. The probe walks the exact ioctl sequence the OH
composer + gralloc issue (see sysfs_drm_probe.c). A QEMU monitor socket allows
a screendump after the atomic commit so the visible pattern can be
verified.

  SYSFS_DRM_PROBE RESULT: PASS n/n   -> rc 0
  SYSFS_DRM_PROBE RESULT: FAIL p/n   -> rc 1
  kernel panic                  -> rc 2
  timeout                       -> rc 3

Usage:
  ./sysfs_drm_probe_boot.py [--timeout 240] [--smp 1] [--mem 2G]
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
BZIMAGE = os.path.join(WT, "target/x86_64-unknown-none/debug/bzImage")
MUSL = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl")
MARKER = "SYSFS_DRM_PROBE RESULT:"
WORK = os.path.join(WT, "work")


def build_probe(out):
    cmd = ["gcc", "-static", "-nostdlib", "-O2", "-Wall",
           "-I", os.path.join(MUSL, "include"),
           "-o", out, os.path.join(HERE, "sysfs_drm_probe.c"),
           os.path.join(MUSL, "lib", "crt1.o"),
           os.path.join(MUSL, "lib", "libc.a"), "-lgcc"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    return True


def build_image(img):
    staging = os.path.join(WORK, "sysfs_drm_probe_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    probe = os.path.join(WORK, "sysfs_drm_probe")
    shutil.copyfile(probe, os.path.join(staging, "test", "sysfs_drm_probe"))
    os.chmod(os.path.join(staging, "test", "sysfs_drm_probe"), 0o755)
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^has_journal",
                        "-d", staging, img, "64M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def mon_cmd(sock_path, cmd):
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(3)
        s.connect(sock_path)
        time.sleep(0.2)
        s.recv(65536)
        s.sendall((cmd + "\n").encode())
        time.sleep(0.6)
        try:
            s.recv(65536)
        except Exception:
            pass
        s.close()
        return True
    except Exception:
        return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=int, default=240)
    ap.add_argument("--smp", type=int, default=1)
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--shot", default=os.path.join(WORK, "sysfs_drm_probe_shot.ppm"))
    args = ap.parse_args()

    os.makedirs(WORK, exist_ok=True)
    if not os.path.exists(BZIMAGE):
        print("[boot] bzImage missing: %s" % BZIMAGE)
        return 2
    probe = os.path.join(WORK, "sysfs_drm_probe")
    if not build_probe(probe):
        return 2
    img = os.path.join(WORK, "sysfs_drm_probe.img")
    if not build_image(img):
        return 2

    serial = os.path.join(WORK, "sysfs_drm_probe_serial.log")
    mon = os.path.join(WORK, "sysfs_drm_probe_mon.sock")
    for p in (serial, mon):
        if os.path.exists(p):
            os.remove(p)

    append = "root=/dev/vda rw init=/test/sysfs_drm_probe console=ttyS0"
    cmd = [QEMU,
           "-M", "q35", "-accel", "tcg,thread=multi", "-cpu", "max",
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none",
           "-device", "virtio-gpu-pci,id=gpu0,xres=720,yres=1280",
           "-serial", "file:" + serial,
           "-monitor", "unix:" + mon + ",server=on,wait=off",
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
            time.sleep(2)
            if proc.poll() is not None:
                break
            data = b""
            try:
                with open(serial, "rb") as f:
                    data = f.read()
            except FileNotFoundError:
                continue
            if b"panicked" in data or b"Kernel panic" in data:
                print("[boot] KERNEL PANIC detected")
                verdict = "PANIC"
                break
            idx = data.rfind(MARKER.encode())
            if idx >= 0:
                line = data[idx:].split(b"\n")[0].decode(errors="replace")
                print("[probe] %s" % line)
                verdict = line
                break
        # give the flip a moment, then screendump the scanout
        time.sleep(2)
        mon_cmd(mon, "screendump %s" % args.shot)
        print("[shot] %s" % args.shot)
        alt = args.shot.replace(".ppm", "-gpu0.ppm")
        mon_cmd(mon, "screendump %s gpu0" % alt)
        print("[shot] %s (device gpu0)" % alt)
        if verdict is None:
            print("[boot] no verdict within %ds" % args.timeout)
            rc = 3
        elif verdict == "PANIC":
            rc = 2
        elif " PASS " in verdict:
            rc = 0
        else:
            rc = 1
        return rc
    finally:
        proc.terminate()
        try:
            proc.wait(5)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
