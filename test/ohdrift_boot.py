#!/usr/bin/env python3
"""ohdrift_boot.py — boot the OH x86_64 image on the Rux kernel and watch
the freelist drift accumulate (freelist-drift lane).

Mirrors oh-display's oh-boot-tmp.sh (q35, six disks, patched ramdisk,
userdata-tok) but:
  - serial is a pipe, so the driver can send the "LEDGER!" UART magic
    periodically (dfx=ledger) and watch free_census vs zone_free_cnt over
    the whole first-boot write storm;
  - the log is parsed for drift / driftwatch attribution lines and a
    compact trace is printed at the end.

Usage:
  ./ohdrift_boot.py [--secs 1500] [--smp 2] [--accel kvm,thread=multi]
      [--kernel bzImage] [--magic-every 120] [--log PATH] [--tag NAME]
"""
import argparse
import os
import re
import select
import subprocess
import sys
import time

WT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IMGDIR = os.path.join(WT, "run", "oh-img")
QEMU = "/usr/bin/qemu-system-x86_64"
DEFAULT_KERNEL = os.path.join(
    WT, "target/x86_64-unknown-none/debug/bzImage")


def qemu_drive(idx, path):
    if not os.path.isfile(path):
        return []
    return ["-drive",
            "file=%s,if=none,id=ohd%d,format=raw,file.locking=off,snapshot=on"
            % (path, idx),
            "-device", "virtio-blk-pci,drive=ohd%d" % idx]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--secs", type=int, default=1500)
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--accel", default="kvm",
                    help="kvm | tcg,thread=multi | tcg,thread=single")
    ap.add_argument("--kernel", default=DEFAULT_KERNEL)
    ap.add_argument("--extra-append", default="dfx=ledger")
    ap.add_argument("--magic-every", type=float, default=120.0)
    ap.add_argument("--magic-at", type=float, default=45.0)
    ap.add_argument("--log", default=None)
    ap.add_argument("--tag", default="drift")
    args = ap.parse_args()

    # Blank userdata: the FIRST-BOOT write storm is the trigger — seeded
    # images skip it. ramdisk-x64: stock ramdisk lacks the x86_64 musl
    # default-library-path and setuid OH services die at loader stage.
    user = os.path.join(IMGDIR, "userdata-blank.img")
    ramdisk = os.path.join(IMGDIR, "ramdisk-x64.img")
    if not os.path.isfile(ramdisk):
        ramdisk = os.path.join(IMGDIR, "ramdisk.img")

    disks = []
    for i, f in enumerate(["updater.img", "system.img", "vendor.img",
                           "sys_prod.img", "chip_prod.img"]):
        disks += qemu_drive(i, os.path.join(IMGDIR, f))
    disks += qemu_drive(5, user)

    append = ("ip=dhcp loglevel=7 console=ttyS0,115200 init=init "
              "root=/dev/ram0 rw ohos.boot.hardware=virt "
              "default_boot_device=10007000.virtio_mmio "
              "ohos.boot.sn=01234567890 "
              "ohos.required_mount.system="
              "/dev/block/vdb@/usr@ext4@ro,barrier=1@wait,required "
              "ohos.required_mount.vendor="
              "/dev/block/vdc@/vendor@ext4@ro,barrier=1@wait,required "
              + args.extra_append)

    cmd = [QEMU, "-M", "q35", "-accel", args.accel, "-cpu", "max",
           "-m", args.mem, "-smp", str(args.smp), "-snapshot",
           "-display", "none", "-vga", "none",
           "-device", "virtio-gpu-pci,id=ohgpu,xres=720,yres=1280",
           "-serial", "mon:stdio", "-monitor", "none",
           "-kernel", args.kernel, "-initrd", ramdisk,
           "-append", append] + disks + [
        "-nic", "user,model=virtio-net-pci",
        "-device", "virtio-keyboard-pci", "-device", "virtio-tablet-pci"]

    log_path = args.log or os.path.join(
        WT, "work", "ohdrift-%s-%d.log" % (args.tag, int(time.time())))
    print("[qemu] %s" % " ".join(cmd[:6]) + " ...")
    print("[log] %s" % log_path)
    logf = open(log_path, "wb")
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT,
                            stdin=subprocess.PIPE)
    deadline = time.time() + args.secs
    magic_at = time.time() + args.magic_at
    drift_trace = []
    watch_lines = []
    killed = False
    try:
        while time.time() < deadline:
            if time.time() >= magic_at:
                try:
                    proc.stdin.write(b"LEDGER!")
                    proc.stdin.flush()
                except OSError:
                    pass
                magic_at = time.time() + args.magic_every
            r, _, _ = select.select([proc.stdout], [], [], 10.0)
            if not r:
                if proc.poll() is not None:
                    break
                continue
            chunk = os.read(proc.stdout.fileno(), 65536)
            if not chunk:
                break
            logf.write(chunk)
            logf.flush()
            text = chunk.decode("utf-8", "replace")
            for dm in re.findall(
                    r"free_census=(\d+) vs zone_free_cnt=(\d+) drift=(-?\d+)",
                    text):
                drift_trace.append((int(dm[0]), int(dm[1]), int(dm[2])))
                print("[drift] t=%.0fs census=%d zone=%d drift=%d"
                      % (args.secs - (deadline - time.time()), *drift_trace[-1]),
                      flush=True)
            for ln in re.findall(r"DRIFTWATCH \S+.*", text):
                watch_lines.append(ln)
            if "Kernel panic" in text:
                print("[!] kernel panic", flush=True)
                break
            if proc.poll() is not None:
                tail = proc.stdout.read()
                if tail:
                    logf.write(tail)
                break
    finally:
        proc.kill()
        proc.wait()
        logf.close()

    print("\n[drift-trace] %d sample(s)" % len(drift_trace))
    for (c, z, d) in drift_trace:
        print("  census=%d zone=%d drift=%d" % (c, z, d))
    seen = set()
    for ln in watch_lines:
        key = ln.split("=")[0]
        if key not in seen:
            seen.add(key)
            print("[watch-last] %s" % ln)
    # nonzero drift at any sample = repro
    bad = [d for (_, _, d) in drift_trace if d > 4096]
    print("[verdict] %s" % ("DRIFT" if bad else ("NO-DRIFT" if drift_trace
                                                 else "NO-SAMPLES")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
