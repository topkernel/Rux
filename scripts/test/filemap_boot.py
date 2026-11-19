#!/usr/bin/env python3
# filemap_boot.py — boot the kernel (riscv64 default) with the filemap
# probe as init and report the verdict for the MAP_PRIVATE file-fault
# page-cache mapping port (wildwrite lane session 24; the riscv64 twin
# of the x86_64 faultaround FA legs).
#
# Usage: ./filemap_boot.py [--platform riscv64|x86_64] [--timeout 180]
#
# FMAP-PASS -> rc 0; FMAP-FAIL or timeout -> rc 1; panic -> rc 2.
import argparse
import os
import subprocess
import sys
import time

WT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
WORK = "/tmp/fmap-work"

X86 = {
    "arch": "x86_64",
    "qemu": "qemu-system-x86_64",
    "gcc": "gcc",
    "sdk": os.path.join(WT, "toolchain/x86_64-rux-linux-musl"),
    "kernel": os.path.join(WT, "target/x86_64-unknown-none/debug/bzImage"),
    "ldso": "/lib/ld-musl-x86_64.so.1",
    "ldso_src": "/tmp/ww-loader/ld-musl.so",
    "mem": "2G",  # kernel heap region sits at 0x40000000: needs >=2G
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
}


def build_probe(cfg, path):
    ldso_src = cfg["ldso_src"]
    if ldso_src is None:
        ldso_src = os.path.join(cfg["sdk"], "lib/libc.so")
    # riscv64: pin -march=rv64gc — the distro cross-gcc emits Zce
    # code-size insns (C.SB et al.) by default, and the QEMU cpu
    # (rv64,zbb,zba,zbs) SIGILLs on them (bindmkdir_boot.py lesson,
    # session 7).
    march = ["-march=rv64gc"] if cfg["arch"] == "riscv64" else []
    cmd = [cfg["gcc"], "-static-libgcc", "-O2", "-Wall"] + march + [
           "-nostdinc", "-isystem", os.path.join(cfg["sdk"], "include"),
           "-nostdlib", "-o", path,
           os.path.join(WT, "test/filemap.c"),
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
    os.makedirs(os.path.join(staging, "tmp"))
    subprocess.run(["cp", probe, os.path.join(staging, "test/filemap")],
                   check=True)
    subprocess.run(["chmod", "+x", os.path.join(staging, "test/filemap")],
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
    # cache=none: the persistence pass terminates QEMU right after the
    # probe prints PASS — guest writes must be on the host image file, not
    # in a writeback cache that a SIGTERM can drop.
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
                "-append", "root=/dev/vda rw init=/test/filemap "
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
            "-append", "root=/dev/vda rw init=/test/filemap console=ttyS0"]


def boot_once(cfg, img, serial, timeout, markers):
    """Boot once; return (verdict, panicked). Success = every marker seen."""
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
            time.sleep(2)
            if proc.poll() is not None:
                break
            try:
                data = open(serial, "rb").read()
            except FileNotFoundError:
                continue
            if b"FMAP-FAIL" in data or b"FMAP-PERSIST-FAIL" in data:
                verdict = "FAIL"
                break
            if b"KERNPANIC" in data or b"Kernel panic" in data:
                verdict = "PANIC"
                panicked = True
                break
            if all(m in data for m in markers):
                verdict = "PASS"
                break
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(5)
            except subprocess.TimeoutExpired:
                proc.kill()
    # Final check after guest exit: the probe powers the system down right
    # after printing its verdict — the poll loop can lose that race (known
    # filedrain class). The serial file survives; read it once more.
    if verdict is None:
        try:
            data = open(serial, "rb").read()
            if b"FMAP-FAIL" in data or b"FMAP-PERSIST-FAIL" in data:
                verdict = "FAIL"
            elif b"KERNPANIC" in data or b"Kernel panic" in data:
                verdict = "PANIC"
                panicked = True
            elif all(m in data for m in markers):
                verdict = "PASS"
        except FileNotFoundError:
            pass
    return verdict, panicked


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--platform", default="riscv64",
                    choices=["x86_64", "riscv64"])
    ap.add_argument("--timeout", type=int, default=180)
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
    img = os.path.join(WORK, "fmap-%s.img" % tag)
    probe = os.path.join(WORK, "filemap-%s" % tag)
    serial = os.path.join(WORK, "fmap-%s-serial.log" % tag)
    serial2 = os.path.join(WORK, "fmap-%s-serial2.log" % tag)

    ldso_src = build_probe(cfg, probe)
    if ldso_src is None:
        return 2
    if not build_image(cfg, img, probe, ldso_src):
        print("[img] mkfs failed")
        return 2

    # Pass 1: the full FC/FS suite; the probe leaves /filemap-persist.dat
    # (written through a MAP_SHARED mapping + msync + fsync) in the image.
    verdict, panicked = boot_once(cfg, img, serial, args.timeout, [b"FMAP-PASS"])
    print("[boot] pass1 verdict: %s" % verdict)
    if verdict != "PASS":
        if os.path.exists(serial):
            sys.stdout.write(open(serial, errors="replace").read()[-6000:])
        return 2 if panicked else 1

    # Pass 2 (session 25): same image, cold page cache — the persist
    # pattern read back here proves the msync/fsync writeback reached the
    # DISK, not just the in-memory cache frames.
    verdict2, panicked2 = boot_once(
        cfg, img, serial2, args.timeout,
        [b"FMAP-PASS", b"FMAP-PERSIST-PASS"])
    print("[boot] pass2 verdict: %s" % verdict2)
    for s in (serial2, serial):
        if os.path.exists(s):
            sys.stdout.write("---- %s ----\n" % s)
            sys.stdout.write(open(s, errors="replace").read()[-6000:])
    if panicked2:
        return 2
    return 0 if verdict2 == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
