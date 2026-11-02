#!/usr/bin/env python3
"""r3b_enospc_boot.py — boot the Rux kernel (riscv64) with the ENOSPC
partial-write probe as init and report the verdict (review-r3b A/B rig,
cloned from test/bindmkdir_boot.py).

  EP-RESULT: PASS + EP-DONE -> PASS (rc 0)
  EP-RESULT: FAIL           -> FAIL (rc 1)
  timeout / panic           -> rc 2 / 1
"""
import argparse
import os
import select
import shutil
import subprocess
import sys
import time

WT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
RUN = os.path.join(WT, "run")
QEMU = "/usr/bin/qemu-system-riscv64"
CC = "riscv64-linux-gnu-gcc"
MKFS = "/usr/sbin/mkfs.ext4"
DEFAULT_KERNEL = os.path.join(
    WT, "target/riscv64gc-unknown-none-elf/debug/rux")


def build_image(img):
    staging = os.path.join(RUN, "r3b_enospc_stage")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    for d in ("dev", "proc", "sys", "run", "mnt"):
        os.makedirs(os.path.join(staging, d), exist_ok=True)
    open(os.path.join(staging, "dev", "null"), "wb").close()
    probe = os.path.join(RUN, "r3b_enospc_probe")
    sdk = os.path.join(WT, "toolchain", "riscv64-rux-linux-musl")
    if os.path.exists(os.path.join(sdk, "lib", "libc.a")):
        cc = [CC, "-static", "-O2", "-nostdinc",
              "-isystem", os.path.join(sdk, "include"),
              "-nostdlib",
              os.path.join(sdk, "lib", "crt1.o"),
              os.path.join(sdk, "lib", "crti.o")]
        libs = ["-L" + os.path.join(sdk, "lib"), "-lc",
                os.path.join(sdk, "lib", "crtn.o"), "-lgcc"]
    else:
        cc = [CC, "-static", "-O2", "-Wall", "-march=rv64gc"]
        libs = []
    subprocess.run(cc + ["-o", probe,
                         os.path.join(RUN, "r3b_enospc.c")] + libs,
                   check=True)
    shutil.copyfile(probe, os.path.join(staging, "test", "r3b_enospc"))
    os.chmod(os.path.join(staging, "test", "r3b_enospc"), 0o755)
    # Host-side pre-fill (the guest-side fill storm trips the pre-existing
    # dirty-eviction wedge — filed separately; this rig needs the disk
    # FULL, not filled live). Grow the staging junk until mkfs leaves the
    # filesystem at a handful of free blocks (mkfs -d copies until it
    # fits; when content overflows it errors or skips — probe the size).
    for junk_mb in (7, 8, 9, 10, 12):
        for old in os.listdir(staging):
            if old.startswith("junk"):
                os.unlink(os.path.join(staging, old))
        with open(os.path.join(staging, "junk00"), "wb") as f:
            # NON-zero pattern: an all-zero source is copied sparsely
            # (holes) and consumes no blocks.
            blk = bytes(range(1, 256)) * 16  # 4096 non-zero bytes
            for _ in range(junk_mb * 256):
                f.write(blk)
        r = subprocess.run([MKFS, "-q", "-F", "-O",
                            "^metadata_csum,^flex_bg,^64bit,^has_journal",
                            "-m", "0", "-d", staging, img, "8M"],
                           capture_output=True, text=True)
        if r.returncode != 0:
            continue  # too big — try a smaller junk on next loop? no:
            # junk_mb ascends; if the FIRST size fails, shrink instead
        d = subprocess.run(["dumpe2fs", "-h", img],
                           capture_output=True, text=True)
        free = None
        for ln in (d.stderr + d.stdout).splitlines():
            if "Free blocks:" in ln:
                free = int(ln.split(":")[1].strip())
        if free is not None and free < 256:
            print("[img] junk=%dMB free_blocks=%d" % (junk_mb, free))
            return True
        if r.returncode == 0 and free is not None:
            print("[img] junk=%dMB free_blocks=%d (not full)" % (junk_mb, free))
    print("[img] could not brim-fill via mkfs")
    return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--thread", default="single", choices=["multi", "single"])
    ap.add_argument("--timeout", type=int, default=240)
    ap.add_argument("--mem", default="1G")
    ap.add_argument("--kernel", default=DEFAULT_KERNEL)
    args = ap.parse_args()

    img = os.path.join(RUN, "r3b_enospc.img")
    if not build_image(img):
        return 2
    if not os.path.exists(args.kernel):
        print("[boot] kernel missing: %s" % args.kernel)
        return 2

    append = "root=/dev/vda rw init=/test/r3b_enospc console=ttyS0 dfx=taskdump"
    cmd = [QEMU,
           "-M", "virt", "-accel", "tcg,thread=" + args.thread,
           "-cpu", "rv64,zbb=true,zba=true,zbs=true",
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none", "-serial", "stdio",
           "-snapshot",
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", args.kernel,
           "-append", append]
    print("[boot] qemu smp=%d thread=%s mem=%s" % (args.smp, args.thread, args.mem))

    ser = open(os.path.join(RUN, "r3b_enospc_serial.log"), "wb")
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

    buf = bytearray()
    t0 = time.time()
    done = False
    panicked = False
    rc = 3
    try:
        while time.time() - t0 < args.timeout:
            r, _, _ = select.select([proc.stdout], [], [], 1.0)
            if r:
                chunk = os.read(proc.stdout.fileno(), 4096)
                if not chunk:
                    break
                buf.extend(chunk)
                ser.write(chunk)
                ser.flush()
                text = bytes(buf[-8192:])
                if b"EP-DONE" in text:
                    done = True
                    break
                if b"KERNPANIC" in text or b"kernel panic" in text:
                    panicked = True
                    break
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
        ser.close()

    out = buf.decode("utf-8", "replace")
    for ln in out.splitlines():
        if ln.startswith("EP[") or ln.startswith("EP-"):
            print("  " + ln)
    if panicked:
        print("[verdict] kernel panic")
        rc = 2
    elif done:
        if "EP-RESULT: PASS" in out:
            print("[verdict] PASS (partial count reported, data intact)")
            rc = 0
        else:
            print("[verdict] FAIL (see EP lines)")
            rc = 1
    else:
        print("[verdict] TIMEOUT waiting for EP-DONE")
        rc = 1
    return rc


if __name__ == "__main__":
    sys.exit(main())
