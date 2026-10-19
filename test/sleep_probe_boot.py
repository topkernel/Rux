#!/usr/bin/env python3
"""sleep_probe_boot.py — boot the sleep-precision probe as init (no OH,
musl libc only). Fast oracle for the vsync-worker usleep question."""
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
LOADER = "/tmp/ohroot/lib64/ld-musl-x86_64.so.1"
MARKER = "SLEEP_PROBE RESULT:"
WORK = os.path.join(WT, "work")


def build_probe(out):
    cmd = ["gcc", "-O1", "-nostartfiles", "-o", out,
           os.path.join(HERE, "sleep_probe.c"),
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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=int, default=180)
    ap.add_argument("--kvm", action="store_true")
    args = ap.parse_args()

    os.makedirs(WORK, exist_ok=True)
    probe = os.path.join(WORK, "sleep_probe_bin")
    if not build_probe(probe):
        return 2
    staging = os.path.join(WORK, "sleep_probe_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    os.makedirs(os.path.join(staging, "lib"))
    os.makedirs(os.path.join(staging, "etc"))
    shutil.copyfile(probe, os.path.join(staging, "test", "sleep_probe"))
    os.chmod(os.path.join(staging, "test", "sleep_probe"), 0o755)
    shutil.copyfile(LOADER,
                    os.path.join(staging, "lib", "ld-musl-x86_64.so.1"))
    with open(os.path.join(staging, "etc", "ld-musl-x86_64.path"), "w") as f:
        f.write("/lib\n")
    shutil.copyfile("/tmp/ohroot/lib64/libc.so",
                    os.path.join(staging, "lib", "libc.so"))
    img = os.path.join(WORK, "sleep_probe.img")
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^has_journal",
                        "-d", staging, img, "64M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return 2

    serial = os.path.join(WORK, "sleep_probe_serial.log")
    if os.path.exists(serial):
        os.remove(serial)
    accel = "kvm" if args.kvm else "tcg,thread=multi"
    cmd = [QEMU, "-M", "q35", "-accel", accel, "-cpu", "max",
           "-m", "2G", "-smp", "1", "-display", "none",
           "-serial", "file:" + serial,
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", BZIMAGE,
           "-append", "root=/dev/vda rw init=/test/sleep_probe %s console=ttyS0" % os.environ.get("SPIN", "")]
    print("[boot] qemu %s" % accel)
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
                if b"SLEEP_PROBE" in line:
                    print("[serial] %s" % line.decode("utf-8", "replace").strip())
                if MARKER.encode() in line:
                    verdict = line.decode("utf-8", "replace").strip()
            if verdict:
                break
        if verdict is None:
            print("[probe] TIMEOUT")
            rc = 3
        else:
            rc = 0
    finally:
        proc.terminate()
        try:
            proc.wait(10)
        except Exception:
            proc.kill()
    return rc


if __name__ == "__main__":
    sys.exit(main())
