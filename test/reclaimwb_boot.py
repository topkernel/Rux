#!/usr/bin/env python3
# reclaimwb_boot.py — boot the kernel with the reclaim-time dirty
# writeback probe as init (wildwrite lane session 28). Single boot; the
# verdict comes from the pressure child's outcome:
#
#   RWB-DONE + RWB-PRESSURE-COMPLETE -> rc 0  (v2 writeback effective)
#   RWB-DONE + RWB-PRESSURE-OOMKILL  -> rc 3  (pre-fix signature: the
#                                             dirty frames stayed stuck;
#                                             the boot survived the OOM)
#   panic / no RWB-DONE               -> rc 2 / 1
#
# Usage: ./reclaimwb_boot.py [--platform riscv64|x86_64] [--timeout 1800]
import argparse
import os
import subprocess
import sys
import time

WT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
WORK = "/tmp/reclaimwb-work"

# dirty = abandoned MAP_SHARED dirty region; anon = child pressure
# target; img = ext4 image size (dirty + metadata + journal headroom).
X86 = {
    "arch": "x86_64",
    "qemu": "qemu-system-x86_64",
    "gcc": "gcc",
    "sdk": os.path.join(WT, "toolchain/x86_64-rux-linux-musl"),
    "kernel": os.path.join(WT, "target/x86_64-unknown-none/debug/bzImage"),
    "ldso": "/lib/ld-musl-x86_64.so.1",
    "ldso_src": "/tmp/ww-loader/ld-musl.so",
    "mem": "2G",  # kernel heap region sits at 0x40000000: needs >=2G
    "dirty_mb": 256,
    "anon_mb": 1600,
    "img_mb": 512,
}
RV64 = {
    "arch": "riscv64",
    "qemu": "/usr/bin/qemu-system-riscv64",
    "gcc": "riscv64-linux-gnu-gcc",
    "sdk": os.path.join(WT, "toolchain/riscv64-rux-linux-musl"),
    "kernel": os.path.join(
        WT, "target/riscv64gc-unknown-none-elf/debug/rux"),
    "ldso": "/lib/ld-musl-riscv64.so.1",
    "ldso_src": None,  # filled from the SDK libc.so
    "mem": "1G",
    "dirty_mb": 128,
    "anon_mb": 600,
    "img_mb": 256,
}


def build_probe(cfg, path):
    ldso_src = cfg["ldso_src"]
    if ldso_src is None:
        ldso_src = os.path.join(cfg["sdk"], "lib/libc.so")
    # riscv64: pin -march=rv64gc (Zce SIGILL lesson, session 7).
    march = ["-march=rv64gc"] if cfg["arch"] == "riscv64" else []
    cmd = [cfg["gcc"], "-static-libgcc", "-O2", "-Wall"] + march + [
           "-DRWB_DIRTY_MB=%d" % cfg["dirty_mb"],
           "-DRWB_ANON_MB=%d" % cfg["anon_mb"],
           "-nostdinc", "-isystem", os.path.join(cfg["sdk"], "include"),
           "-nostdlib", "-o", path,
           os.path.join(WT, "test/reclaimwb.c"),
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
    subprocess.run(["cp", probe, os.path.join(staging, "test/reclaimwb")],
                   check=True)
    subprocess.run(["chmod", "+x", os.path.join(staging, "test/reclaimwb")],
                   check=True)
    guest_ldso = staging + cfg["ldso"]
    subprocess.run(["cp", ldso_src, guest_ldso], check=True)
    if os.path.exists(path):
        os.remove(path)
    r = subprocess.run(["mkfs.ext4", "-q", "-F",
                        "-O", "^metadata_csum,^flex_bg",
                        "-d", staging, path, "%dM" % cfg["img_mb"]],
                       capture_output=True, text=True)
    return r.returncode == 0


def qemu_cmd(cfg, img, serial):
    if cfg["arch"] == "x86_64":
        return [cfg["qemu"],
                "-M", "q35", "-accel", "tcg,thread=multi", "-cpu", "max",
                "-m", cfg["mem"], "-smp", "2",
                "-display", "none",
                "-serial", "file:" + serial,
                "-drive", "file=%s,if=none,id=rootfs,format=raw,"
                         "cache=none,file.locking=off" % img,
                "-device", "virtio-blk-pci,drive=rootfs",
                "-kernel", cfg["kernel"],
                "-append", "root=/dev/vda rw init=/test/reclaimwb "
                           "console=ttyS0"]
    return [cfg["qemu"],
            "-M", "virt", "-accel", "tcg,thread=multi",
            "-cpu", "rv64,zbb=true,zba=true,zbs=true",
            "-m", cfg["mem"], "-smp", "2",
            "-display", "none", "-serial", "file:" + serial,
            "-drive", "file=%s,if=none,id=rootfs,format=raw,"
                     "cache=none,file.locking=off" % img,
            "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
            "-kernel", cfg["kernel"],
            "-append", "root=/dev/vda rw init=/test/reclaimwb console=ttyS0"]


def boot_once(cfg, img, serial, timeout):
    if os.path.exists(serial):
        os.remove(serial)
    proc = subprocess.Popen(qemu_cmd(cfg, img, serial),
                            stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)
    print("[boot] qemu pid=%d serial=%s" % (proc.pid, serial))
    verdict = None
    panicked = False
    t0 = time.time()
    try:
        while time.time() - t0 < timeout:
            time.sleep(3)
            if proc.poll() is not None:
                break
            try:
                data = open(serial, "rb").read()
            except FileNotFoundError:
                continue
            if b"KERNPANIC" in data or b"Kernel panic" in data:
                verdict = "PANIC"
                panicked = True
                break
            if b"RWB-FAIL" in data:
                verdict = "FAIL"
                break
            if b"RWB-DONE" in data:
                if b"RWB-PRESSURE-COMPLETE" in data:
                    verdict = "PASS"
                elif b"RWB-PRESSURE-OOMKILL" in data:
                    verdict = "OOMKILL"
                else:
                    verdict = "OTHER"
                break
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(5)
            except subprocess.TimeoutExpired:
                proc.kill()
    if verdict is None:
        try:
            data = open(serial, "rb").read()
            if b"KERNPANIC" in data or b"Kernel panic" in data:
                verdict, panicked = "PANIC", True
            elif b"RWB-FAIL" in data:
                verdict = "FAIL"
            elif b"RWB-DONE" in data:
                if b"RWB-PRESSURE-COMPLETE" in data:
                    verdict = "PASS"
                elif b"RWB-PRESSURE-OOMKILL" in data:
                    verdict = "OOMKILL"
                else:
                    verdict = "OTHER"
        except FileNotFoundError:
            pass
    return verdict, panicked


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--platform", default="riscv64",
                    choices=["x86_64", "riscv64"])
    ap.add_argument("--timeout", type=int, default=1800)
    ap.add_argument("--kernel", default=None)
    args = ap.parse_args()

    cfg = dict(X86 if args.platform == "x86_64" else RV64)
    if args.kernel:
        cfg["kernel"] = args.kernel
    if not os.path.exists(cfg["kernel"]):
        print("[boot] kernel missing: %s" % cfg["kernel"])
        return 2
    if cfg["ldso_src"] is not None and not os.path.exists(cfg["ldso_src"]):
        print("[boot] ldso source missing: %s" % cfg["ldso_src"])
        return 2

    os.makedirs(WORK, exist_ok=True)
    tag = args.platform
    img = os.path.join(WORK, "reclaimwb-%s.img" % tag)
    probe = os.path.join(WORK, "reclaimwb-%s" % tag)
    serial = os.path.join(WORK, "reclaimwb-%s-serial.log" % tag)

    ldso_src = build_probe(cfg, probe)
    if ldso_src is None:
        return 2
    if not build_image(cfg, img, probe, ldso_src):
        print("[img] mkfs failed")
        return 2

    verdict, panicked = boot_once(cfg, img, serial, args.timeout)
    print("[boot] verdict: %s" % verdict)
    # How far the pressure child got (the A/B discriminator).
    if os.path.exists(serial):
        reached = 0
        for line in open(serial, errors="replace"):
            if line.startswith("RWB child: ") and line.rstrip().endswith("MB"):
                try:
                    reached = int(line.split()[2].rstrip("MB"))
                except (IndexError, ValueError):
                    pass
        print("[boot] child reached: %dMB" % reached)
        sys.stdout.write(open(serial, errors="replace").read()[-6000:])
    if panicked or verdict == "PANIC":
        return 2
    if verdict == "PASS":
        return 0
    if verdict == "OOMKILL":
        return 3
    return 1


if __name__ == "__main__":
    sys.exit(main())
