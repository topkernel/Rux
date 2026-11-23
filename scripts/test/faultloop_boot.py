#!/usr/bin/env python3
# faultloop_boot.py — stage run/faultprobe.c on a small ext4 image and
# boot it as init on x86_64 KVM; every FAULTLOOP line is the
# measurement. The fs-substrate lane's interface-cost receipt
# (crate-plan 3a rule 2): tight MAP_SHARED fault loops over an ext4
# file, a tmpfs file (/dev/shm) and a devfs regular file.
import os
import subprocess
import sys
import time

WT = "/home/william/rux-agents/fs-substrate"
QEMU = "qemu-system-x86_64"
MKFS = "/usr/sbin/mkfs.ext4"
MUSL_CC = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl", "bin", "musl-gcc")
WORK = os.path.join(WT, "work")
TAG = sys.argv[1] if len(sys.argv) > 1 else "fl1"
BZIMAGE = sys.argv[2] if len(sys.argv) > 2 else os.path.join(
    WT, "target", "x86_64-unknown-none", "debug", "bzImage")

probe = os.path.join(WORK, "faultprobe")
img = os.path.join(WORK, "faultloop-%s.img" % TAG)
serial = os.path.join(WORK, "faultloop-%s-serial.log" % TAG)

src = os.path.join(WT, "run", "faultprobe.c")
if not os.path.exists(src):
    # extracted from userspace/probes/probes.tar.gz (the archive rule)
    src = os.path.join(WT, "test", "faultprobe.c")
r = subprocess.run([MUSL_CC, "-static", "-O2", "-Wall", "-o", probe, src],
                   capture_output=True, text=True)
if r.returncode != 0:
    print("[build] compile failed: %s" % r.stderr)
    sys.exit(2)
print("[build] %d KB" % (os.path.getsize(probe) // 1024))

staging = os.path.join(WORK, "faultloop_root")
subprocess.run(["rm", "-rf", staging], check=True)
os.makedirs(os.path.join(staging, "test"))
subprocess.run(["cp", probe, os.path.join(staging, "test", "faultprobe")], check=True)
os.chmod(os.path.join(staging, "test", "faultprobe"), 0o755)
r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum", "-d", staging,
                    img, "64M"], capture_output=True, text=True)
if r.returncode != 0:
    print("[img] mkfs failed: %s" % r.stderr)
    sys.exit(2)

cmd = [QEMU, "-M", "q35", "-accel", "kvm", "-cpu", "max",
       "-m", "2G", "-smp", "2", "-display", "none",
       "-serial", "file:" + serial,
       "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
       "-device", "virtio-blk-pci,drive=rootfs",
       "-kernel", BZIMAGE,
       "-append", "root=/dev/vda rw init=/test/faultprobe console=ttyS0"]
if os.path.exists(serial):
    os.remove(serial)
print("[boot] tag=%s bzImage=%s" % (TAG, BZIMAGE))
t0 = time.time()
try:
    p = subprocess.run(cmd, timeout=240)
    rc = p.returncode
except subprocess.TimeoutExpired:
    rc = 124  # the probe halts the guest; a timeout after DONE is fine
print("[boot] rc=%d t=%.0fs serial=%s" % (rc, time.time() - t0, serial))
ok = "FAULTLOOP DONE rc=0" in open(serial, errors="replace").read()
sys.exit(0 if ok else 1)
