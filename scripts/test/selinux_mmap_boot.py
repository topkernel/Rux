#!/usr/bin/env python3
# selinux_mmap_boot.py — boot the x86_64 kernel with the selinux_mmap
# probe as init (cfi_shadow_boot.py shape) and report the verdict.
#
# Reproduces in seconds the OH cfi5/cfi6 deterministic KERNPANIC
# (badaddr 0xa7 in the page-fault path when libselinux mmaps
# /sys/fs/selinux/status): pass the BASE kernel to see the panic, the
# fixed kernel to see SELINUX-MMAP: PASS.
import os
import subprocess
import sys
import time

WT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
QEMU = "qemu-system-x86_64"
BZIMAGE = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    WT, "target/x86_64-unknown-none/debug/bzImage")
WORK = "/tmp/selinux-mmap-work"
SDK = os.path.join(WT, "toolchain/x86_64-rux-linux-musl")


def build_probe(path):
    cmd = ["gcc", "-static", "-O2", "-Wall",
           "-nostdinc", "-isystem", os.path.join(SDK, "include"),
           "-nostdlib", "-o", path,
           os.path.join(WT, "test/selinux_mmap.c"),
           os.path.join(SDK, "lib/crt1.o"),
           os.path.join(SDK, "lib/crti.o"),
           os.path.join(SDK, "lib/crtn.o"),
           "-L" + os.path.join(SDK, "lib"), "-lc",
           subprocess.check_output(["gcc", "-print-libgcc-file-name"], text=True).strip()]
    return subprocess.run(cmd).returncode == 0


def build_image(path, probe):
    staging = os.path.join(WORK, "staging")
    os.makedirs(os.path.join(staging, "test"), exist_ok=True)
    subprocess.run(["cp", probe, os.path.join(staging, "test/selinux_mmap")], check=True)
    subprocess.run(["chmod", "+x", os.path.join(staging, "test/selinux_mmap")], check=True)
    if os.path.exists(path):
        os.remove(path)
    return subprocess.run(["mkfs.ext4", "-q", "-F", "-O", "^metadata_csum,^flex_bg", "-d", staging, path, "16M"]).returncode == 0


def main():
    os.makedirs(WORK, exist_ok=True)
    if not os.path.exists(BZIMAGE):
        print("[boot] bzImage missing: %s" % BZIMAGE)
        return 2
    img = os.path.join(WORK, "probe.img")
    probe = os.path.join(WORK, "selinux_mmap_x64")
    if not build_probe(probe):
        return 2
    if not build_image(img, probe):
        return 2

    serial = os.path.join(WORK, "serial.log")
    mon = os.path.join(WORK, "mon.sock")
    for p in (serial, mon):
        if os.path.exists(p):
            os.remove(p)

    append = "root=/dev/vda rw init=/test/selinux_mmap console=ttyS0"
    cmd = [QEMU,
           "-M", "q35", "-accel", "tcg,thread=multi", "-cpu", "max",
           "-m", "2G", "-smp", "2",
           "-display", "none",
           "-serial", "file:" + serial,
           "-monitor", "unix:" + mon + ",server=on,wait=off",
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", BZIMAGE,
           "-append", append]
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    print("[boot] qemu started pid=%d kernel=%s" % (proc.pid, BZIMAGE))
    verdict = None
    t0 = time.time()
    try:
        while time.time() - t0 < 120:
            time.sleep(2)
            try:
                data = open(serial, "rb").read()
            except FileNotFoundError:
                continue
            if b"SELINUX-MMAP: PASS" in data:
                verdict = "PASS"
                break
            if b"SELINUX-MMAP: FAIL" in data:
                verdict = "FAIL"
                break
            if b"KERNPANIC" in data:
                verdict = "KERNPANIC"
                break
            if proc.poll() is not None:
                break
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(5)
            except subprocess.TimeoutExpired:
                proc.kill()
    print("[boot] verdict: %s" % verdict)
    if os.path.exists(serial):
        sys.stdout.write(open(serial, errors="replace").read()[-4000:])
    return 0 if verdict == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
