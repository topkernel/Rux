#!/usr/bin/env python3
"""pbench_boot_rv.py — riscv64 twin of pbench_boot.py: boot the Rux
riscv64 kernel (strong or debug ELF) with the pbench probe (or the
p2fonly futex battery) as init and collect the PBENCH lines.

Machine class copies `make run` (test/run-qemu defaults): virt, TCG
thread=multi, rv64, 2G, SMP2, virtio-blk-pci, serial to file. The
probe is a glibc-static build (no riscv64 musl toolchain in-tree);
rdcycle fronts rdtsc_ (pbench.c guards by arch).

Usage: ./pbench_boot_rv.py [--kernel PATH] [--tag NAME] [--timeout 900]
       [--smp 2] [--mem 2G] [--p2f] [--append-extra "a=b"]
"""
import argparse
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(HERE)
QEMU = "qemu-system-riscv64"
MKFS = "/usr/sbin/mkfs.ext4"
RVCC = "/usr/bin/riscv64-linux-gnu-gcc"
MUSL = os.path.join(WT, "toolchain", "riscv64-rux-linux-musl")
# Probe links against the in-tree riscv64 musl (build-musl.sh; scalar,
# no RVV — Ubuntu's glibc-static carries unconditional RVV code the
# kernel cannot run: sstatus.VS is never enabled, so every vector
# instruction traps as SIGILL). -march pinned to match the libc build
# so gcc does not auto-vectorize RVV into the probe either.
MARCH = "-march=rv64gc_zicsr"
DEFAULT_KERNEL = os.path.join(WT, "target", "riscv64gc-unknown-none-elf",
                              "release", "rux")
WORK = os.path.join(WT, "work")
SIZE_MB = 256


def build_probe(out, src, extra=None):
    if not os.path.exists(os.path.join(MUSL, "lib", "libc.a")):
        print("[build] riscv64 musl missing: run toolchain/build-musl.sh")
        return False
    libs = [os.path.join(MUSL, "lib", "crt1.o"),
            os.path.join(MUSL, "lib", "libc.a")]
    cmd = [RVCC, "-static", "-nostdlib", MARCH, "-O2", "-Wall",
           "-I", os.path.join(MUSL, "include"),
           "-o", out, src] + libs + ["-lgcc"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    print("[build] %s (%d KB)" % (out, os.path.getsize(out) // 1024))
    return True


def build_image(img, probe, arm=None, arm_name="arm"):
    staging = os.path.join(WORK, "pbench_rv_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    shutil.copyfile(probe, os.path.join(staging, "test", "pbench"))
    os.chmod(os.path.join(staging, "test", "pbench"), 0o755)
    init_path = "/test/pbench"
    if arm:
        shutil.copyfile(arm, os.path.join(staging, "test", arm_name))
        os.chmod(os.path.join(staging, "test", arm_name), 0o755)
        init_path = "/test/" + arm_name
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum",
                        "-d", staging, img, "%dM" % SIZE_MB],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return None
    return init_path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=int, default=900)
    ap.add_argument("--kernel", default=DEFAULT_KERNEL)
    ap.add_argument("--tag", default="rvpb1")
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--p2f", action="store_true",
                    help="stage the p2fonly futex battery as init")
    ap.add_argument("--probe-bin", default=None,
                    help="ship this prebuilt probe instead of building pbench.c")
    ap.add_argument("--append-extra", default=None)
    args = ap.parse_args()

    os.makedirs(WORK, exist_ok=True)
    if not os.path.exists(args.kernel):
        print("[boot] kernel missing: %s" % args.kernel)
        return 2
    probe = os.path.join(WORK, "pbench-rv")
    if args.probe_bin:
        import shutil as _sh
        _sh.copyfile(args.probe_bin, probe)
    elif not build_probe(probe, os.path.join(HERE, "pbench.c")):
        return 2
    arm = None
    init_path = "/test/pbench"
    if args.p2f:
        arm = os.path.join(WORK, "pbench_p2f-rv")
        if not build_probe(arm, os.path.join(HERE, "pbench_p2f_arm.c")):
            return 2
    img = os.path.join(WORK, "pbench-rv-%s.img" % args.tag)
    init_path = build_image(img, probe, arm, "pbench_p2f")
    if not init_path:
        return 2

    serial = os.path.join(WORK, "pbench-rv-%s-serial.log" % args.tag)
    if os.path.exists(serial):
        os.remove(serial)
    append = "root=/dev/vda rw init=%s console=ttyS0" % init_path
    if args.append_extra:
        append += " " + args.append_extra
    # Ubuntu glibc-static carries zbb/zba/zbs/zicond AND RVV code (gcc
    # 15 / glibc 2.4x baseline: vse64.v in pthread start_thread paths);
    # make-run's zbb/zba/zbs class is not enough for the probe binary.
    cmd = [QEMU, "-M", "virt", "-accel", "tcg,thread=multi",
           "-cpu", "rv64,zbb=true,zba=true,zbs=true,zicond=true,v=true,vlen=256,elen=64",
           "-m", args.mem, "-smp", str(args.smp), "-nographic",
           "-serial", "file:" + serial,
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", args.kernel, "-append", append]
    try:
        foreign = subprocess.run(["pgrep", "-c", "qemu-system"],
                                 capture_output=True, text=True).stdout.strip()
    except Exception:
        foreign = -1
    print("[boot] tag=%s smp=%s mem=%s foreign_qemus=%s"
          % (args.tag, args.smp, args.mem, foreign))
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
                time.sleep(3)
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
