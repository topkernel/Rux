#!/usr/bin/env python3
"""pbench_boot.py — quiet-boot the Rux x86_64 kernel with the P1-P7
microbench probe (test/pbench.c, perf-report.md section 8.2) as init and
collect the PBENCH result lines.

Builds the static musl probe, packs a small ext4 rootfs (mkfs -d, same
recipe as faultlat_boot), boots q35 + virtio-blk-pci with
init=/test/pbench, and tails the serial log: every `PBENCH ...` line is
printed as it appears; success = `PBENCH DONE rc=0`.

  rc 0  full run (DONE seen)
  rc 1  kernel panic before DONE
  rc 3  no DONE within --timeout
  rc 2  build/image failure

Usage: ./pbench_boot.py [--timeout 1200] [--bzimage PATH] [--tag NAME]
       [--accel tcg,thread=multi] [--smp 2] [--mem 1G]
The defaults match the pa-series machine class (q35, -cpu max,
SMP2/1G per repo discipline; P2 needs SMP>=2).
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
MUSL_CC = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl", "bin", "musl-gcc")
DEFAULT_BZIMAGE = os.path.join(WT, "run", "pa31-main-bzImage")
WORK = os.path.join(WT, "work")
SIZE_MB = 192          # probe writes ~30 MB (fault file + tree) into /test


def foreign_qemus():
    try:
        out = subprocess.run(["pgrep", "-c", "qemu-system"], capture_output=True,
                             text=True).stdout.strip()
        return int(out) if out else 0
    except Exception:
        return -1


def build_probe(out, src=None):
    if src is None:
        src = os.path.join(HERE, "pbench.c")
    cmd = [MUSL_CC, "-static", "-O2", "-Wall", "-o", out, src]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    print("[build] %s (%d KB)" % (out, os.path.getsize(out) // 1024))
    return True


def build_image(img, probe, arm=None, nojournal=False, mkfs_features=None,
                arm_name="arm"):
    staging = os.path.join(WORK, "pbench_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    shutil.copyfile(probe, os.path.join(staging, "test", "pbench"))
    os.chmod(os.path.join(staging, "test", "pbench"), 0o755)
    if arm:
        shutil.copyfile(arm, os.path.join(staging, "test", arm_name))
        os.chmod(os.path.join(staging, "test", arm_name), 0o755)
    # --nojournal (leg 44, F4 arm-1): mkfs WITHOUT has_journal so the fs
    # rows exercise the no-journal path (route-1 null-journal handle
    # deferral) — the OH/GNOME userdata image class. The standing
    # protocol (journal image) is unchanged; this flag exists only for
    # the journal-vs-nojournal A/B.
    # --mkfs-features (leg 45): generic passthrough for feature-bisect
    # repros (e.g. "^metadata_csum,^orphan_file" localizes the
    # no-journal root-lookup failure to a feature bit). Takes precedence
    # over --nojournal when both are given.
    if mkfs_features:
        feats = mkfs_features
    else:
        feats = "^metadata_csum,^has_journal" if nojournal else "^metadata_csum"
    r = subprocess.run([MKFS, "-q", "-F", "-O", feats,
                        "-d", staging, img, "%dM" % SIZE_MB],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=int, default=1200)
    ap.add_argument("--bzimage", default=DEFAULT_BZIMAGE)
    ap.add_argument("--tag", default="pb1")
    ap.add_argument("--accel", default="tcg,thread=multi")
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="2G")  # 1G panics: kernel-heap alloc
    # error (4MB layout, main.rs OOM path) during early boot
    ap.add_argument("--arm-schedstat", action="store_true",
                    help="A-side of the schedstat cost A-B: stage the "
                         "pbench_arm wrapper and boot init=/test/arm "
                         "(arms /proc/sys/kernel/sched_schedstats then "
                         "execs the probe); the B side is the stock boot")
    ap.add_argument("--p3d", action="store_true",
                    help="stage the pbench_p3d wrapper (sets P3D=1 in the "
                         "probe's env then execs it) — enables the P3d "
                         "concurrent-fault contention row (ledger 8.10.27)")
    ap.add_argument("--nojournal", action="store_true",
                    help="F4 arm-1 (leg 44): mkfs the probe rootfs WITHOUT "
                         "has_journal so the fs rows run the no-journal "
                         "path (route-1 deferral) — OH/GNOME image class")
    ap.add_argument("--mkfs-features", default=None,
                    help="leg 45: full -O feature string for the probe "
                         "rootfs (feature-bisect repros; overrides "
                         "--nojournal when given)")
    ap.add_argument("--long-p5", default=None, metavar="MODE",
                    choices=["cold", "warm", "pin"],
                    help="leg 46: stage the p5long wrapper and boot it as "
                         "init — pbench runs duration-bounded p5c2/p5c3 "
                         "windows with serial markers (pair with "
                         "--append-extra dfx=wwcensus so the WW heartbeats "
                         "bracket the windows). cold = windows only; warm = "
                         "the board's p5 section first (regime driver), "
                         "then windows + pinned spread/samecpu variants; "
                         "pin = the serialized-cycle discriminators "
                         "(samecpu read 64K/4K, samecpu recvfrom, spread)")
    ap.add_argument("--probe-bin", default=None,
                    help="ship this prebuilt probe as init instead of building pbench.c")
    ap.add_argument("--append-extra", default=None,
                    help="leg 46: extra kernel cmdline tokens appended "
                         "to -append (e.g. 'dfx=wwcensus')")
    args = ap.parse_args()

    os.makedirs(WORK, exist_ok=True)
    if not os.path.exists(args.bzimage):
        print("[boot] bzImage missing: %s" % args.bzimage)
        return 2
    probe = os.path.join(WORK, "pbench")
    if args.probe_bin:
        import shutil as _sh
        _sh.copy(args.probe_bin, probe)
    elif not build_probe(probe):
        return 2
    img = os.path.join(WORK, "pbench-%s.img" % args.tag)
    arm = None
    init_path = "/test/pbench"
    if args.arm_schedstat:
        arm = os.path.join(WORK, "pbench_arm")
        if not build_probe(arm, src=os.path.join(HERE, "pbench_arm.c")):
            return 2
        init_path = "/test/arm"
    if args.p3d:
        arm = os.path.join(WORK, "pbench_p3d")
        if not build_probe(arm, src=os.path.join(HERE, "pbench_p3d_arm.c")):
            return 2
        init_path = "/test/pbench_p3d"
    if args.long_p5:
        # leg 46: stage the wrapper as an extra image file and boot it
        # (the probe itself is still staged — the wrapper execve's it).
        # Staged NAME selects the mode (p5long -> longp5,
        # p5longwarm -> longp5warm, p5longpin -> longp5pin).
        arm = os.path.join(WORK, "p5long")
        if not build_probe(arm, src=os.path.join(HERE, "p5long.c")):
            return 2
        staged = {None: "p5long", "cold": "p5long",
                  "warm": "p5longwarm", "pin": "p5longpin"}[args.long_p5]
        shutil.copyfile(arm, os.path.join(WORK, staged))
        init_path = "/test/" + staged
    if not build_image(img, probe, arm, nojournal=args.nojournal,
                       mkfs_features=args.mkfs_features,
                       arm_name=os.path.basename(init_path)):
        return 2

    serial = os.path.join(WORK, "pbench-%s-serial.log" % args.tag)
    if os.path.exists(serial):
        os.remove(serial)

    append = "root=/dev/vda rw init=%s console=ttyS0" % init_path
    if args.append_extra:
        append += " " + args.append_extra
    cmd = [QEMU, "-M", "q35", "-accel", args.accel, "-cpu", "max",
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none",
           "-serial", "file:" + serial,
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", args.bzimage, "-append", append]
    print("[boot] tag=%s accel=%s smp=%s mem=%s foreign_qemus=%d"
          % (args.tag, args.accel, args.smp, args.mem, foreign_qemus()))
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        t0 = time.time()
        seen = 0
        done = False
        while time.time() - t0 < args.timeout:
            time.sleep(3)
            if proc.poll() is not None and done:
                break
            try:
                with open(serial, "rb") as f:
                    data = f.read()
            except FileNotFoundError:
                continue
            lines = data.split(b"\n")
            for line in lines[seen:]:
                if line.startswith(b"PBENCH "):
                    print("[probe] %s" % line.decode(errors="replace"))
            seen = len(lines)
            if b"PBENCH DONE rc=0" in data:
                done = True
                print("[boot] DONE marker at t=%ds" % (time.time() - t0))
                time.sleep(3)   # let poweroff settle
                break
            if b"panicked" in data or b"Kernel panic" in data:
                print("[boot] KERNEL PANIC at t=%ds (no DONE)"
                      % (time.time() - t0))
                return 1
        if not done:
            print("[boot] no DONE within %ds" % args.timeout)
            return 3
        return 0
    finally:
        proc.terminate()
        try:
            proc.wait(5)
        except Exception:
            proc.kill()
        print("[boot] serial log: %s" % serial)


if __name__ == "__main__":
    sys.exit(main())
