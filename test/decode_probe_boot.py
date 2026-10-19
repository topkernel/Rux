#!/usr/bin/env python3
"""decode_probe_boot.py — boot the decode oracle (probe as init).

Stages the OH lib2d_graphics dependency closure (flat, /lib) plus the
musl loader and the animation's own jpg into a minimal ext4, boots the
Rux x86_64 kernel with it as root+init, and reports the
"DECODE_PROBE RESULT:" line from serial.

Usage: ./decode_probe_boot.py [--timeout 240] [--smp 2] [--kvm]
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
SYSROOT = "/mnt/ccsys/system"  # loop-mounted read-only system.img
LOADER = "/tmp/ohroot/lib64/ld-musl-x86_64.so.1"
MARKER = "DECODE_PROBE RESULT:"
WORK = os.path.join(WT, "work")

LIB_DIRS = [
    os.path.join(SYSROOT, "lib64"),
    os.path.join(SYSROOT, "lib64", "platformsdk"),
    os.path.join(SYSROOT, "lib64", "chipset-sdk"),
    os.path.join(SYSROOT, "lib64", "chipset-sdk-sp"),
    os.path.join(SYSROOT, "lib64", "ndk"),
]


def find_lib(name):
    for d in LIB_DIRS:
        p = os.path.join(d, name)
        if os.path.exists(p):
            return p
    return None


def needed(path):
    out = subprocess.run(["readelf", "-d", path],
                         capture_output=True, text=True).stdout
    res = []
    for line in out.splitlines():
        if "NEEDED" in line:
            s = line.split("[")[-1].rstrip("]):-").strip("[]").split("：")[-1]
            res.append(s)
    return res


def closure(root_lib):
    seen, todo = {}, [root_lib]
    while todo:
        lib = todo.pop()
        if lib in seen:
            continue
        p = find_lib(lib)
        seen[lib] = p
        if not p:
            continue
        for n in needed(p):
            if n not in seen:
                todo.append(n)
    return seen


def build_probe(out):
    cmd = ["gcc", "-O1", "-nostartfiles", "-o", out,
           os.path.join(HERE, "decode_probe.c"),
           os.path.join(MUSL, "lib", "crt1.o"),
           "-Wl,-e,_start",
           "-Wl,--dynamic-linker,/lib/ld-musl-x86_64.so.1",
           "-L", "/tmp/ohroot/lib64", "-lc",
           "-I", os.path.join(MUSL, "include"), "-lgcc"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    return True


def build_image(img):
    staging = os.path.join(WORK, "decode_probe_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    os.makedirs(os.path.join(staging, "lib"))
    os.makedirs(os.path.join(staging, "etc"))

    shutil.copyfile(os.path.join(WORK, "decode_probe_bin"),
                    os.path.join(staging, "test", "decode_probe"))
    os.chmod(os.path.join(staging, "test", "decode_probe"), 0o755)
    shutil.copyfile(LOADER,
                    os.path.join(staging, "lib", "ld-musl-x86_64.so.1"))
    with open(os.path.join(staging, "etc", "ld-musl-x86_64.path"), "w") as f:
        f.write("/lib\n")

    libs = closure("lib2d_graphics.z.so")
    missing = [k for k, v in libs.items() if v is None]
    if missing:
        print("[img] missing libs: %s" % missing)
        return False
    for name, path in libs.items():
        shutil.copyfile(path, os.path.join(staging, "lib", name))
    print("[img] staged %d libs" % len(libs))

    # test media: produced by scripts/make_decode_media.py into work/
    for src, dst in [("logo40.jpg", "logo40.jpg"),
                     ("logo40.png", "logo40.png"),
                     ("corrupt.jpg", "corrupt.jpg")]:
        s = os.path.join(WORK, src)
        if os.path.exists(s):
            shutil.copyfile(s, os.path.join(staging, "test", dst))
        else:
            print("[img] missing media %s" % s)
            return False

    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^has_journal",
                        "-d", staging, img, "512M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=int, default=240)
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--kvm", action="store_true")
    args = ap.parse_args()

    os.makedirs(WORK, exist_ok=True)
    if not os.path.exists(BZIMAGE):
        print("[boot] bzImage missing")
        return 2
    probe = os.path.join(WORK, "decode_probe_bin")
    if not build_probe(probe):
        return 2
    img = os.path.join(WORK, "decode_probe.img")
    if not build_image(img):
        return 2

    serial = os.path.join(WORK, "decode_probe_serial.log")
    for p in (serial,):
        if os.path.exists(p):
            os.remove(p)
    accel = "kvm" if args.kvm else "tcg,thread=multi"
    append = "root=/dev/vda rw init=/test/decode_probe console=ttyS0"
    cmd = [QEMU, "-M", "q35", "-accel", accel, "-cpu", "max",
           "-m", "2G", "-smp", str(args.smp), "-display", "none",
           "-device", "virtio-gpu-pci,id=gpu0,xres=720,yres=1280",
           "-serial", "file:" + serial,
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", BZIMAGE, "-append", append]
    print("[boot] qemu %s smp=%d" % (accel, args.smp))
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    verdict = None
    try:
        t0 = time.time()
        while time.time() - t0 < args.timeout:
            time.sleep(2)
            if proc.poll() is not None:
                break
            try:
                with open(serial, "rb") as f:
                    data = f.read()
            except FileNotFoundError:
                continue
            for line in data.split(b"\n"):
                if MARKER.encode() in line or b"DECODE_PROBE:" in line:
                    txt = line.decode("utf-8", "replace").strip()
                    if MARKER in txt:
                        verdict = txt
                    print("[serial] %s" % txt)
            if verdict:
                break
        if verdict is None:
            print("[probe] TIMEOUT (no verdict)")
            rc = 3
        else:
            print("[probe] %s" % verdict)
            rc = 0 if "jpg=ok" in verdict else 1
    finally:
        proc.terminate()
        try:
            proc.wait(10)
        except Exception:
            proc.kill()
    return rc


if __name__ == "__main__":
    sys.exit(main())
