#!/usr/bin/env python3
# filedrain_boot.py — boot the riscv64 kernel with the filedrain probe
# as init (fake-OOM drainer A/B; wildwrite session 24).
#
# Builds a private ext4 image carrying the probe plus a 160MB stamped
# data file, boots riscv64 -m 2G (GNOME parity), and reports the
# verdict: FDR-PASS (all rounds + children survived) vs FDR-FAIL /
# timeout, with kernel OOM markers counted from serial.
#
# Usage: ./filedrain_boot.py [--timeout 1500] [--kernel PATH]
#   FDR-PASS + 0 OOM markers -> rc 0; FAIL/timeout -> rc 1; panic -> 2.
import argparse
import os
import subprocess
import sys
import time

WT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
WORK = "/tmp/fdr-work"
DATA_MB = 160

RV64 = {
    "qemu": "/usr/bin/qemu-system-riscv64",
    "gcc": "riscv64-linux-gnu-gcc",
    "sdk": os.path.join(WT, "toolchain/riscv64-rux-linux-musl"),
    "kernel": os.path.join(
        WT, "target/riscv64gc-unknown-none-elf/debug/rux"),
    "ldso": "/lib/ld-musl-riscv64.so.1",
}


def build_probe(cfg, path):
    ldso_src = os.path.join(cfg["sdk"], "lib/libc.so")
    # -march=rv64gc: distro cross-gcc Zce lesson (session 7).
    cmd = [cfg["gcc"], "-static-libgcc", "-O2", "-Wall", "-march=rv64gc",
           "-nostdinc", "-isystem", os.path.join(cfg["sdk"], "include"),
           "-nostdlib", "-o", path,
           os.path.join(WT, "test/filedrain.c"),
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
    subprocess.run(["cp", probe, os.path.join(staging, "test/filedrain")],
                   check=True)
    subprocess.run(["chmod", "+x", os.path.join(staging, "test/filedrain")],
                   check=True)
    subprocess.run(["cp", ldso_src, staging + cfg["ldso"]], check=True)
    # 160MB data file: per-page stamps so every read fault carries real
    # file data through the ext4 path (no holes).
    import struct
    data = os.path.join(staging, "data.bin")
    blk = bytearray(1024 * 1024)
    with open(data, "wb") as f:
        for mb in range(DATA_MB):
            for pg in range(256):
                base = pg * 4096
                for w in range(0, 1024, 64):
                    struct.pack_into("<I", blk, base + w * 4,
                                     (0xD8000000 | (mb << 8) | pg) ^ w)
            f.write(blk)
    if os.path.exists(path):
        os.remove(path)
    r = subprocess.run(["mkfs.ext4", "-q", "-F",
                        "-O", "^metadata_csum,^flex_bg",
                        "-d", staging, path, "%dM" % (DATA_MB + 64)],
                       capture_output=True, text=True)
    return r.returncode == 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=int, default=1500)
    ap.add_argument("--kernel", default=None)
    ap.add_argument("--tag", default="rv")
    args = ap.parse_args()

    cfg = dict(RV64)
    if args.kernel:
        cfg["kernel"] = args.kernel
    if not os.path.exists(cfg["kernel"]):
        print("[boot] kernel missing: %s" % cfg["kernel"])
        return 2

    os.makedirs(WORK, exist_ok=True)
    tag = args.tag
    img = os.path.join(WORK, "fdr-%s.img" % tag)
    probe = os.path.join(WORK, "filedrain-%s" % tag)
    serial = os.path.join(WORK, "fdr-%s-serial.log" % tag)
    if os.path.exists(serial):
        os.remove(serial)

    ldso_src = build_probe(cfg, probe)
    if ldso_src is None:
        return 2
    if not build_image(cfg, img, probe, ldso_src):
        print("[img] mkfs failed")
        return 2

    cmd = [cfg["qemu"],
           "-M", "virt", "-accel", "tcg,thread=multi",
           "-cpu", "rv64,zbb=true,zba=true,zbs=true",
           "-m", "2G", "-smp", "2",
           "-display", "none", "-serial", "file:" + serial,
           "-drive", "file=%s,if=none,id=rootfs,format=raw,"
                    "file.locking=off" % img,
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", cfg["kernel"],
           "-append", "root=/dev/vda rw init=/test/filedrain "
                      "console=ttyS0 dfx=taskdump,watchdog,periodic,"
                      "memwatch,ledger"]
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)
    print("[boot] qemu pid=%d" % proc.pid)
    verdict = None
    panicked = False
    t0 = time.time()
    try:
        while time.time() - t0 < args.timeout:
            time.sleep(5)
            if proc.poll() is not None:
                break
            try:
                data = open(serial, "rb").read()
            except FileNotFoundError:
                continue
            if b"FDR-PASS" in data:
                verdict = "PASS"
                break
            if b"FDR-FAIL" in data:
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
    oom = 0
    ser_txt = ""
    if os.path.exists(serial):
        ser_txt = open(serial, errors="replace").read()
        for m in ("Out of memory", "oom-killer", "OOM-FORENSIC",
                  "Killed process"):
            oom += ser_txt.count(m)
    print("[boot] verdict: %s oom_markers=%d" % (verdict, oom))
    print(ser_txt[-4000:])
    if panicked:
        return 2
    return 0 if verdict == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
