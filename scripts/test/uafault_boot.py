#!/usr/bin/env python3
# uafault_boot.py — boot the kernel (x86_64 default) with the uafault
# probe as init and report the verdict for the bad-user-pointer EFAULT
# contract (x86_64 SMAP + dropped bootstrap identity map, commits
# bdc9234e + e22f72a8, 2026-10-31). Same driver skeleton as
# mmapfix_boot.py.
#
# Usage: ./uafault_boot.py [--platform x86_64] [--timeout 180]
#
# UAF-PASS -> rc 0; UAF-FAIL or timeout -> rc 1; panic -> rc 2.
import argparse
import os
import subprocess
import sys
import time

WT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
WORK = "/tmp/uaf-work"

X86 = {
    "arch": "x86_64",
    "qemu": "qemu-system-x86_64",
    "gcc": "gcc",
    "sdk": os.path.join(WT, "toolchain/x86_64-rux-linux-musl"),
    "kernel": os.path.join(WT, "target/x86_64-unknown-none/debug/bzImage"),
    "ldso": "/lib/ld-musl-x86_64.so.1",
    # staged by the faultaround harness at the fixed /tmp/ww-loader path
    # (musl: the dynamic loader IS libc.so)
    "ldso_src": "/tmp/ww-loader/ld-musl.so",
    "mem": "2G",  # kernel heap region sits at 0x40000000: needs >=2G
}


def build_probe(cfg, path):
    ldso_src = cfg["ldso_src"]
    cmd = [cfg["gcc"], "-static-libgcc", "-O2", "-Wall",
           "-nostdinc", "-isystem", os.path.join(cfg["sdk"], "include"),
           "-nostdlib", "-o", path,
           os.path.join(WT, "test/uafault.c"),
           os.path.join(cfg["sdk"], "lib/Scrt1.o"),
           os.path.join(cfg["sdk"], "lib/crti.o"),
           os.path.join(cfg["sdk"], "lib/crtn.o"),
           "-L" + os.path.join(cfg["sdk"], "lib"), "-lc",
           ldso_src,
           "-Wl,-dynamic-linker," + cfg["ldso"],
           subprocess.check_output(
               [cfg["gcc"], "-print-libgcc-file-name"],
               text=True).strip()]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] probe build failed: %s" % r.stderr[-2000:])
        return None
    return ldso_src


def build_image(cfg, path, probe, ldso_src):
    staging = os.path.join(WORK, "staging")
    if os.path.exists(staging):
        subprocess.run(["rm", "-rf", staging], check=True)
    os.makedirs(os.path.join(staging, "test"))
    os.makedirs(os.path.join(staging, "lib"))
    subprocess.run(["cp", probe, os.path.join(staging, "test/uafault")],
                   check=True)
    subprocess.run(["chmod", "+x", os.path.join(staging, "test/uafault")],
                   check=True)
    guest_ldso = staging + cfg["ldso"]
    subprocess.run(["cp", ldso_src, guest_ldso], check=True)
    if os.path.exists(path):
        os.remove(path)
    r = subprocess.run(["mkfs.ext4", "-q", "-F",
                        "-O", "^metadata_csum,^flex_bg",
                        "-d", staging, path, "16M"],
                       capture_output=True, text=True)
    return r.returncode == 0


def qemu_cmd(cfg, img, serial):
    return [cfg["qemu"],
            "-M", "q35", "-accel", "tcg,thread=multi", "-cpu", "max",
            "-m", cfg["mem"], "-smp", "2",
            "-display", "none",
            "-serial", "file:" + serial,
            "-drive", "file=%s,if=none,id=rootfs,format=raw,"
                     "file.locking=off" % img,
            "-device", "virtio-blk-pci,drive=rootfs",
            "-kernel", cfg["kernel"],
            "-append", "root=/dev/vda rw init=/test/uafault "
                       "console=ttyS0"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--platform", default="x86_64", choices=["x86_64"])
    ap.add_argument("--timeout", type=int, default=180)
    ap.add_argument("--kernel", default=None)
    args = ap.parse_args()

    cfg = dict(X86)
    if args.kernel:
        cfg["kernel"] = args.kernel
    if not os.path.exists(cfg["kernel"]):
        print("[boot] kernel missing: %s" % cfg["kernel"])
        return 2
    if not os.path.exists(cfg["ldso_src"]):
        print("[boot] ldso source missing: %s" % cfg["ldso_src"])
        return 2

    os.makedirs(WORK, exist_ok=True)
    img = os.path.join(WORK, "uaf-x86_64.img")
    probe = os.path.join(WORK, "uafault-x86_64")
    serial = os.path.join(WORK, "uaf-x86_64-serial.log")
    if os.path.exists(serial):
        os.remove(serial)

    ldso_src = build_probe(cfg, probe)
    if ldso_src is None:
        return 2
    if not build_image(cfg, img, probe, ldso_src):
        print("[img] mkfs failed")
        return 2

    proc = subprocess.Popen(qemu_cmd(cfg, img, serial),
                            stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)
    print("[boot] x86_64 qemu pid=%d" % proc.pid)
    verdict = None
    panicked = False
    t0 = time.time()
    try:
        while time.time() - t0 < args.timeout:
            time.sleep(2)
            if proc.poll() is not None:
                break
            try:
                data = open(serial, "rb").read()
            except FileNotFoundError:
                continue
            if b"UAF-PASS" in data:
                verdict = "PASS"
                break
            if b"UAF-FAIL" in data:
                verdict = "FAIL"
                break
            if b"KERNPANIC" in data or b"Kernel panic" in data:
                verdict = "PANIC"
                panicked = True
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
    if panicked:
        return 2
    return 0 if verdict == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
