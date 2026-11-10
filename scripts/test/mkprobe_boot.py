#!/usr/bin/env python3
"""mkprobe_boot.py — boot a kernel with the mkprobe rootfs (mkdir/rebuild
loop replicating pbench's p6 tree build) and report failures.

Usage: mkprobe_boot.py --bzimage PATH --tag NAME [--reps N]
Builds the probe with the same musl toolchain, packs the same mkfs -d
ext4 recipe as pbench_boot.py, boots q35 KVM smp2 2G, tails serial for
MKLOOP-DONE. Prints MKRESULT lines.
"""
import argparse, os, shutil, subprocess, sys, re

WT = "/home/william/rux-agents/perf-gap-hunt"
MUSL_CC = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl", "bin", "musl-gcc")
MKFS = "/usr/sbin/mkfs.ext4"
WORK = os.path.join(WT, "work")


def build_probe(out):
    cmd = [MUSL_CC, "-static", "-O2", "-Wall", "-o", out, os.path.join(WT, "test", "mkprobe.c")]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[probe] build failed:", r.stderr)
        sys.exit(2)


def build_image(img, probe):
    staging = os.path.join(WORK, "mkprobe_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    shutil.copyfile(probe, os.path.join(staging, "test", "mkprobe"))
    os.chmod(os.path.join(staging, "test", "mkprobe"), 0o755)
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum", "-d", staging, img, "192M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed:", r.stderr)
        sys.exit(2)


def boot(bzimage, img, log):
    cmd = ["qemu-system-x86_64", "-M", "q35", "-accel", "kvm", "-cpu", "max",
           "-m", "2G", "-smp", "2", "-display", "none", "-vga", "none",
           "-serial", f"file:{log}", "-monitor", "none", "-snapshot",
           "-kernel", bzimage,
           "-append", "root=/dev/vda rw init=/test/mkprobe console=ttyS0",
           "-drive", f"file={img},if=none,id=rootfs,format=raw,snapshot=on,file.locking=off",
           "-device", "virtio-blk-pci,drive=rootfs"]
    p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(150):
        import time; time.sleep(1)
        if os.path.exists(log):
            data = open(log, "rb").read()
            if b"MKLOOP-DONE" in data or b"KERNPANIC" in data:
                break
        if p.poll() is not None:
            break
    p.kill(); p.wait()
    data = open(log, "rb").read().decode(errors="replace") if os.path.exists(log) else ""
    m = re.search(r"MKLOOP-DONE fails=(\d+)", data)
    panic = "KERNPANIC" in data
    return (int(m.group(1)) if m else None), panic, data


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bzimage", required=True)
    ap.add_argument("--tag", required=True)
    ap.add_argument("--reps", type=int, default=4)
    args = ap.parse_args()
    probe = os.path.join(WORK, "mkprobe")
    build_probe(probe)
    img = os.path.join(WORK, f"mkprobe-{args.tag}.img")
    build_image(img, probe)
    for rep in range(1, args.reps + 1):
        log = os.path.join(WORK, f"mkprobe-{args.tag}-{rep}-serial.log")
        if os.path.exists(log):
            os.remove(log)
        fails, panic, data = boot(args.bzimage, img, log)
        load = open("/proc/loadavg").read().split(" ")[0]
        print(f"MKRESULT tag={args.tag} rep={rep} load={load} fails={fails} panic={panic} "
              f"mkdirfail={data.count('mkdirfail')} ooo={data.count('out-of-order completion')}")


if __name__ == "__main__":
    main()
