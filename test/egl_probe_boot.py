#!/usr/bin/env python3
"""egl_probe_boot.py — boot the Rux x86_64 kernel with the OH image's own
EGL/GLES/KMS stack as init and report the verdict (gbm_probe pattern).

Same staging as gbm_probe_boot.py (OH musl loader + chipsetsdk libs) plus
the GLES libraries, driving test/egl_probe.c:

  B ladder: SET_CLIENT_CAP -> GETRESOURCES -> CREATE_DUMB -> MAP_DUMB ->
            mmap paint -> ADDFB2 -> MODE_ATOMIC flip  (pixels!)
  A ladder: eglGetDisplay(DEFAULT) -> eglInitialize -> choose/pbuffer/
            context/makeCurrent -> glClear (llvmpipe)

The QEMU gets a monitor socket; after the marker a screendump is taken to
/nonblack check the flip actually lit the scanout.

Verdict: EGL_PROBE RESULT: PASS n/n -> rc 0; FAIL -> 1; panic -> 2; timeout -> 3.

Usage: ./egl_probe_boot.py [--timeout 420] [--smp 1] [--mem 2G]
"""
import argparse
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(HERE)
QEMU = "qemu-system-x86_64"
MKFS = "/usr/sbin/mkfs.ext4"
BZIMAGE = os.path.join(WT, "target", "x86_64-unknown-none", "debug", "bzImage")
MUSL = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl")
OHROOT = "/tmp/ohroot"
MARKER = "EGL_PROBE RESULT:"
WORK = os.path.join(WT, "work")


def build_probe(out):
    cmd = ["gcc", "-O1", "-nostartfiles", "-o", out,
           os.path.join(HERE, "egl_probe.c"),
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
    staging = os.path.join(WORK, "egl_probe_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    os.makedirs(os.path.join(staging, "lib"))
    os.makedirs(os.path.join(staging, "lib64"))
    os.makedirs(os.path.join(staging, "etc"))
    shutil.copyfile(os.path.join(OHROOT, "test_egl_probe"),
                    os.path.join(staging, "test", "egl_probe"))
    os.chmod(os.path.join(staging, "test", "egl_probe"), 0o755)
    shutil.copyfile(os.path.join(OHROOT, "lib64", "ld-musl-x86_64.so.1"),
                    os.path.join(staging, "lib", "ld-musl-x86_64.so.1"))
    shutil.copyfile(os.path.join(OHROOT, "lib64", "libc.so"),
                    os.path.join(staging, "lib64", "libc.so"))
    shutil.copyfile("/tmp/rd64/etc/ld-musl-x86_64.path",
                    os.path.join(staging, "etc", "ld-musl-x86_64.path"))
    src = os.path.join(OHROOT, "vendor", "lib64", "chipsetsdk")
    dst = os.path.join(staging, "lib")
    os.makedirs(dst, exist_ok=True)
    for f in os.listdir(src):
        if os.path.islink(os.path.join(src, f)):
            continue
        shutil.copyfile(os.path.join(src, f), os.path.join(dst, f))
    for a, b in [("virtio_gpu_dri.so", "libgallium_dri.so"),
                 ("kms_swrast_dri.so", "libgallium_dri.so"),
                 ("swrast_dri.so", "libgallium_dri.so"),
                 ("libgbm.so", "libgbm.so.1.0.0"),
                 ("libEGL.so.1", "libEGL.so.1.0.0"),
                 ("libEGL.so", "libEGL.so.1.0.0"),
                 ("libglapi.so", "libglapi.so.0.0.0"),
                 ("libglapi.so.0", "libglapi.so.0.0.0"),
                 ("libGLESv2.so.2", "libGLESv2.so.2.0.0"),
                 ("libGLESv2.so", "libGLESv2.so.2.0.0"),
                 ("libGLESv1_CM.so.1", "libGLESv1_CM.so.1.1.0")]:
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


def mon_cmd(sock_path, cmd):
    import socket as s
    try:
        c = s.socket(s.AF_UNIX, s.SOCK_STREAM)
        c.settimeout(3)
        c.connect(sock_path)
        c.sendall((cmd + "\n").encode())
        time.sleep(0.5)
        try:
            c.recv(4096)
        except Exception:
            pass
        c.close()
        return True
    except Exception:
        return False


def shotstat(path):
    """Count distinct colors / non-black pixels of a PPM."""
    try:
        with open(path, "rb") as f:
            head = f.readline()  # P6
            dims = f.readline().split()
            f.readline()
            w, h = int(dims[0]), int(dims[1])
            data = f.read(w * h * 3)
        colors = set()
        nb = 0
        for i in range(0, len(data), 3 * 16):  # sample every 16th pixel
            px = data[i:i + 3]
            if px != b"\x00\x00\x00":
                nb += 1
            if len(colors) < 64:
                colors.add(px)
        return len(colors), nb
    except Exception:
        return 0, 0


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
    probe = os.path.join(OHROOT, "test_egl_probe")
    if not build_probe(probe):  # always rebuild: probe source changes often
        return 2
    img = os.path.join(WORK, "egl_probe.img")
    if not build_image(img):
        return 2

    serial = os.path.join(WORK, "egl_probe_serial.log")
    mon = os.path.join(WORK, "egl_probe-mon.sock")
    shot = os.path.join(WORK, "egl_probe-shot.ppm")
    for p in (serial, mon, shot):
        if os.path.exists(p):
            os.remove(p)

    append = "root=/dev/vda rw init=/test/egl_probe console=ttyS0"
    cmd = [QEMU,
           "-M", "q35", "-accel", "tcg,thread=multi", "-cpu", "max",
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none",
           "-device", "virtio-gpu-pci,id=gpu0,xres=720,yres=1280",
           "-serial", "file:" + serial,
           "-monitor", "unix:%s,server=on,wait=off" % mon,
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", BZIMAGE,
           "-append", append]
    print("[boot] qemu smp=%s mem=%s" % (args.smp, args.mem))
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    verdict = None
    try:
        t0 = time.time()
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
            if verdict:
                break
        if verdict is None:
            print("[probe] TIMEOUT (no verdict)")
            rc = 3
        else:
            print("[probe] %s" % verdict)
            rc = 0 if " PASS " in verdict + " " else 1
    finally:
        if verdict:
            # screendump while the guest still sleeps in the probe; name
            # the virtio-gpu device — the default console (720x400 dummy)
            # is NOT the virtio-gpu scanout surface.
            if mon_cmd(mon, "screendump %s gpu0" % shot):
                time.sleep(1)
                colors, nb = shotstat(shot)
                print("[shot] %s colors=%d nonblack=%d" % (shot, colors, nb))
            else:
                print("[shot] device screendump failed")
        proc.terminate()
        try:
            proc.wait(10)
        except Exception:
            proc.kill()
    return rc


if __name__ == "__main__":
    sys.exit(main())
