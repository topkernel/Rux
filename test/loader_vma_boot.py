#!/usr/bin/env python3
# loader_vma_boot.py — boot the x86_64 kernel with the dynamic musl
# loader_vma probe as init (cfi_shadow_boot.py shape) and report the
# verdict for the /proc/self/maps file-VMA parity fix.
import os
import subprocess
import sys
import time

WT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
QEMU = "qemu-system-x86_64"
BZIMAGE = os.path.join(WT, "target/x86_64-unknown-none/debug/bzImage")
WORK = "/tmp/lvma-work"
SDK = os.path.join(WT, "toolchain/x86_64-rux-linux-musl")
LDMUSL = "/tmp/ww-loader/ld-musl.so"


def build_probe(path):
    cmd = ["gcc", "-static-libgcc", "-O2", "-Wall",
           "-nostdinc", "-isystem", os.path.join(SDK, "include"),
           "-nostdlib", "-o", path,
           os.path.join(WT, "test/loader_vma.c"),
           os.path.join(SDK, "lib/crt1.o"),
           os.path.join(SDK, "lib/crti.o"),
           os.path.join(SDK, "lib/crtn.o"),
           LDMUSL,
           "-Wl,-dynamic-linker,/lib/ld-musl-x86_64.so.1",
           subprocess.check_output(["gcc", "-print-libgcc-file-name"], text=True).strip()]
    return subprocess.run(cmd).returncode == 0


def build_image(path, probe):
    staging = os.path.join(WORK, "staging")
    os.makedirs(os.path.join(staging, "test"), exist_ok=True)
    os.makedirs(os.path.join(staging, "lib"), exist_ok=True)
    subprocess.run(["cp", probe, os.path.join(staging, "test/loader_vma")], check=True)
    subprocess.run(["chmod", "+x", os.path.join(staging, "test/loader_vma")], check=True)
    subprocess.run(["cp", LDMUSL, os.path.join(staging, "lib/ld-musl-x86_64.so.1")], check=True)
    # data file for the mremap leg: 4 pages, V.. / W.. markers
    data = bytearray(4 * 4096)
    data[0:4] = b"V000"
    data[0x2000:0x2004] = b"W001"
    open(os.path.join(staging, "test/vmadata.bin"), "wb").write(bytes(data))
    if os.path.exists(path):
        os.remove(path)
    return subprocess.run(["mkfs.ext4", "-q", "-F", "-O", "^metadata_csum,^flex_bg", "-d", staging, path, "16M"]).returncode == 0


def main():
    os.makedirs(WORK, exist_ok=True)
    if not os.path.exists(BZIMAGE):
        print("[boot] bzImage missing: %s" % BZIMAGE)
        return 2
    img = os.path.join(WORK, "lvma.img")
    probe = os.path.join(WORK, "loader_vma")
    if not build_probe(probe):
        return 2
    if not build_image(img, probe):
        return 2

    serial = os.path.join(WORK, "serial.log")
    if os.path.exists(serial):
        os.remove(serial)

    append = "root=/dev/vda rw init=/test/loader_vma console=ttyS0 dfx=syscalltrace"
    cmd = [QEMU,
           "-M", "q35", "-accel", "tcg,thread=multi", "-cpu", "max",
           "-m", "2G", "-smp", "2",
           "-display", "none",
           "-serial", "file:" + serial,
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", BZIMAGE,
           "-append", append]
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    print("[boot] qemu started pid=%d" % proc.pid)
    verdict = None
    t0 = time.time()
    try:
        while time.time() - t0 < 120:
            time.sleep(2)
            if proc.poll() is not None:
                break
            try:
                data = open(serial, "rb").read()
            except FileNotFoundError:
                continue
            if b"LVMAP-PASS" in data:
                verdict = "PASS"
                break
            if b"LVMAP-FAIL" in data:
                verdict = "FAIL"
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
        sys.stdout.write(open(serial, errors="replace").read()[-6000:])
    return 0 if verdict == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
