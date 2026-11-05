#!/usr/bin/env python3
"""taskcomm_probe_boot.py — boot the Rux x86_64 kernel with the
/proc/<pid>/task/<tid>/comm probe as init and report the verdict.

The OH hdf_devmgr death path: it spawns a CLONE_THREAD worker and opens
/proc/self/task/<tid>/comm; our kernel answers ENOENT and devmgr exits 1,
killing the whole HDF chain (composer_host -> no display HDI -> no UI).

  TASKCOMM RESULT: PASS n/n   -> rc 0
  TASKCOMM RESULT: FAIL p/n   -> rc 1
  kernel panic                  -> rc 2
  timeout                       -> rc 3

Usage:
  ./taskcomm_probe_boot.py [--timeout 180] [--smp 2] [--mem 2G]
"""
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
BZIMAGE = os.path.join(WT, "target/x86_64-unknown-none/debug/bzImage")
MUSL = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl")
MARKER = "TASKCOMM RESULT:"
WORK = os.path.join(WT, "work")


def build_probe(out):
    cmd = ["gcc", "-static", "-nostdlib", "-O2", "-Wall",
           "-I", os.path.join(MUSL, "include"),
           "-o", out, os.path.join(HERE, "taskcomm_probe.c"),
           os.path.join(MUSL, "lib", "crt1.o"),
           os.path.join(MUSL, "lib", "libc.a"), "-lgcc"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    return True


def build_image(img):
    staging = os.path.join(WORK, "taskcomm_probe_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    probe = os.path.join(WORK, "taskcomm_probe")
    shutil.copyfile(probe, os.path.join(staging, "test", "taskcomm"))
    os.chmod(os.path.join(staging, "test", "taskcomm"), 0o755)
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^has_journal",
                        "-d", staging, img, "64M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=int, default=180)
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="2G")
    args = ap.parse_args()

    probe = os.path.join(WORK, "taskcomm_probe")
    if not build_probe(probe):
        return 1
    img = os.path.join(WORK, "taskcomm_probe.img")
    if not build_image(img):
        return 1

    serial_path = os.path.join(WORK, "taskcomm_probe_serial.log")
    cmd = [QEMU, "-M", "q35", "-accel", "tcg,thread=multi", "-cpu", "max",
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none",
           "-no-reboot",
           "-serial", "file:" + serial_path,
           "-kernel", BZIMAGE,
           "-drive", "file=%s,if=none,id=pr,format=raw,file.locking=off" % img,
           "-device", "virtio-blk-pci,drive=pr",
           "-append", "console=ttyS0 init=/test/taskcomm rw"]
    with open(serial_path, "w") as ser:
        p = subprocess.Popen(cmd, stdout=ser, stderr=ser)
        verdict = None
        panicked = False
        deadline = time.time() + args.timeout
        while time.time() < deadline:
            if p.poll() is not None:
                break
            time.sleep(1)
            try:
                tail = open(serial_path, "rb").read()
            except OSError:
                continue
            t = tail.decode("utf-8", "replace")
            if MARKER in t:
                verdict = t.split(MARKER, 1)[1].splitlines()[0].strip()
                break
            if "KERNEL PANIC" in t or "panic" in t.lower():
                panicked = True
        if p.poll() is None:
            p.terminate()
            try:
                p.wait(5)
            except subprocess.TimeoutExpired:
                p.kill()
    tail = open(serial_path, "rb").read().decode("utf-8", "replace")
    if verdict is None:
        print("[probe] no verdict; serial tail:")
        print("\n".join(tail.splitlines()[-30:]))
        return 3 if not panicked else 2
    print("[probe] %s%s" % (MARKER, verdict))
    for line in tail.splitlines():
        if line.startswith(("ok   ", "FAIL ", "     chain", "taskcomm_probe", "note:", "child tid")):
            print("  " + line)
    return 0 if verdict.startswith("PASS") else 1


if __name__ == "__main__":
    sys.exit(main())
