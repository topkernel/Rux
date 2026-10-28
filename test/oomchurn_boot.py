#!/usr/bin/env python3
"""oomchurn_boot.py — boot the Rux kernel with oomchurn as init and
stream the serial log, reporting the verdict (oom-accounting lane).

Builds a tiny private ext4 image containing only /test/oomchurn (mkfs.ext4
-d) and boots it with init=/test/oomchurn. The probe's own output carries
the verdict:

  OOMCHURN-*-PASS / "ALL ... touched ok"  -> rc 0
  pagefault: Out of memory / OOM-FORENSIC  -> rc 3 (fake OOM reproduced)
  kernel panic                            -> rc 2
  probe error                             -> rc 1

Usage:
  ./oomchurn_boot.py [--smp 2] [--thread multi|single] [--mem 2G]
      [--secs 600] [--mode anon] [--mb 1200] [--fork-n 8] [--rounds 4]
      [--kernel ELF] [--extra-append "dfx=memwatch"] [--keep-img]
"""
import argparse
import os
import re
import select
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(HERE)
QEMU = "/usr/bin/qemu-system-riscv64"
CC = "riscv64-linux-gnu-gcc"
MKFS = "/usr/sbin/mkfs.ext4"
MUSL = "/home/william/workspace/Rux/toolchain/riscv64-rux-linux-musl"
DEFAULT_KERNEL = os.path.join(
    WT, "target/riscv64gc-unknown-none-elf/debug/rux")
CPU = ("rv64,zbb=true,zba=true,zbs=true,zcmop=true,zimop=true,"
       "zicond=true,zfa=true,zca=true,zcb=true")

FAIL_PATTERNS = [
    re.compile(p) for p in [
        r"pagefault: Out of memory",
        r"OOM-FORENSIC",
        r"oom-killer: Killed process",
        r"ALLOCTHROW",
        r"oom: out of memory",
        r"SEED-ALLOC-BAD-POP",
        r"RAW-FREE-OF-LIVE-TABLE",
        r"oomchurn: mmap .* failed",
        r"OOMCHURN-.*-FAIL",
        r"free_pages: REFUSED",
    ]
]


def build_image(img, args):
    staging = os.path.join(WT, "work", "oomchurn_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    probe = os.path.join(WT, "work", "oomchurn")
    if not os.path.exists(probe) or os.path.getmtime(
            os.path.join(HERE, "oomchurn.c")) > os.path.getmtime(probe):
        subprocess.run([CC, "-static", "-O2", "-Wall",
                        "-march=rv64gc_zicsr", "-nostdinc", "-nostartfiles",
                        "-U_FORTIFY_SOURCE",
                        "-isystem", "/usr/lib/gcc-cross/riscv64-linux-gnu/15/include",
                        "-isystem", os.path.join(MUSL, "include"),
                        "-o", probe, os.path.join(HERE, "oomchurn.c"),
                        os.path.join(MUSL, "lib", "crt1.o"),
                        os.path.join(MUSL, "lib", "libc.a"),
                        "-lgcc"],
                       check=True)
    shutil.copyfile(probe, os.path.join(staging, "test", "oomchurn"))
    os.chmod(os.path.join(staging, "test", "oomchurn"), 0o755)
    os.symlink("/test/oomchurn", os.path.join(staging, "test", "init"))
    # write-storm modes need a directory to churn files in
    os.makedirs(os.path.join(staging, "storm"), exist_ok=True)
    if args.mode == "anon":
        argline = "%s %d" % (args.mode, args.mb)
    elif args.mode == "fork":
        argline = "%s %d %d" % (args.mode, args.fork_n, args.mb)
    elif args.mode == "churn":
        argline = "%s %d %d" % (args.mode, args.mb, args.rounds)
    elif args.mode == "exec":
        argline = "%s %d %d" % (args.mode, args.fork_n, args.mb)
    elif args.mode == "wrstorm":
        argline = "%s %d %d /storm/wrfile" % (args.mode, args.mb, args.rounds)
    elif args.mode == "mix":
        argline = "%s %d %d" % (args.mode, args.rounds, args.mb)
    elif args.mode == "fake":
        argline = "%s %d %d" % (args.mode, args.mb, args.mb2)
    else:
        argline = "%s %d" % (args.mode, args.mb)
    with open(os.path.join(staging, "test", "args"), "w") as f:
        f.write(argline + "\n")
    # fake mode writes a cache-filling file of --mb MB inside the guest:
    # size the image to hold it (default 128M as before).
    img_mbs = "%dM" % (args.mb + 64) if args.mode == "fake" else "128M"
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^flex_bg",
                        "-d", staging, img, img_mbs],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return None
    return "init=/test/oomchurn"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--thread", default="multi", choices=["multi", "single"])
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--secs", type=int, default=600)
    ap.add_argument("--mode", default="anon",
                    choices=["anon", "fork", "churn", "exec", "mapfile",
                             "wrstorm", "mix", "fake"])
    ap.add_argument("--mb", type=int, default=1200)
    ap.add_argument("--fork-n", type=int, default=8)
    ap.add_argument("--rounds", type=int, default=4)
    ap.add_argument("--mb2", type=int, default=0,
                    help="fake mode: anon MB after the cache fill")
    ap.add_argument("--kernel", default=DEFAULT_KERNEL)
    ap.add_argument("--extra-append", default="")
    ap.add_argument("--keep-img", action="store_true")
    ap.add_argument("--log", default=None)
    ap.add_argument("--accel", default="tcg")
    ap.add_argument("--magic", default=None,
                    help="raw bytes written to guest serial stdin after boot "
                         "(e.g. 'LEDGER!' with dfx=ledger)")
    ap.add_argument("--magic-at", type=float, default=25.0,
                    help="seconds before writing --magic")
    ap.add_argument("--magic-every", type=float, default=0.0,
                    help="repeat --magic every N seconds (drift trace)")
    args = ap.parse_args()

    img = os.path.join(WT, "work", "oomchurn.img")
    init_str = build_image(img, args)
    if not init_str:
        return 1

    append = "root=/dev/vda rw console=ttyS0 %s %s" % (init_str, args.extra_append)
    cmd = [QEMU, "-M", "virt", "-accel", "%s,thread=%s" % (args.accel,
                                                           args.thread),
           "-cpu", CPU, "-m", args.mem,
           "-smp", str(args.smp), "-nographic", "-serial", "mon:stdio",
           "-monitor", "none",
           "-drive", "file=%s,if=none,id=rootfs,format=raw,snapshot=on" % img,
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", args.kernel, "-append", append]
    print("[qemu] %s" % " ".join(cmd[:8]) + " ...")

    log_path = args.log or os.path.join(
        WT, "work", "oomchurn-%s.log" % int(time.time()))
    logf = open(log_path, "wb")
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT,
                            stdin=subprocess.PIPE if args.magic else subprocess.DEVNULL)
    deadline = time.time() + args.secs
    verdict = None
    touched_max = 0
    magic_done = args.magic is None
    magic_at = time.time() + args.magic_at
    drift_trace = []
    memfree_trace = []
    try:
        while time.time() < deadline:
            if not magic_done and time.time() >= magic_at:
                # -serial mon:stdio: QEMU's stdin IS the guest serial RX.
                try:
                    proc.stdin.write(args.magic.encode())
                    proc.stdin.flush()
                    print("[magic] sent %r" % args.magic)
                except OSError as e:
                    print("[magic] failed: %s" % e)
                if args.magic_every > 0:
                    magic_at = time.time() + args.magic_every
                else:
                    magic_done = True
            r, _, _ = select.select([proc.stdout], [], [], 5.0)
            if not r:
                if proc.poll() is not None:
                    break
                continue
            chunk = os.read(proc.stdout.fileno(), 65536)
            if not chunk:
                break
            logf.write(chunk)
            logf.flush()
            sys.stdout.buffer.write(chunk)
            sys.stdout.buffer.flush()
            text = chunk.decode("utf-8", "replace")
            m = re.findall(r"anon: (\d+) MB touched", text)
            if m:
                touched_max = max(touched_max, int(m[-1]))
            for dm in re.findall(r"free_census=(\d+) vs zone_free_cnt=(\d+) "
                                 r"drift=(-?\d+)", text):
                drift_trace.append((int(dm[0]), int(dm[1]), int(dm[2])))
                print("[drift] census=%d zone=%d drift=%d"
                      % drift_trace[-1])
            for mm_ in re.findall(r"MemFree:\s+(\d+) kB", text):
                memfree_trace.append(int(mm_))
            for pat in FAIL_PATTERNS:
                if pat.search(text) and verdict is None:
                    verdict = ("FAKE-OOM", pat.pattern)
            if re.search(r"OOMCHURN-\S+-PASS", text) and verdict is None:
                verdict = ("PASS", "probe pass marker")
            if "Kernel panic" in text:
                verdict = ("PANIC", "kernel panic")
                break
            if "Power down" in text or "powering system down" in text:
                if verdict is None:
                    verdict = ("PASS-EXIT", "init exited cleanly")
                break
            if proc.poll() is not None:
                # drain
                tail = proc.stdout.read()
                if tail:
                    logf.write(tail)
                    sys.stdout.buffer.write(tail)
                break
    finally:
        proc.kill()
        proc.wait()
        logf.close()
        if not args.keep_img:
            pass  # keep image for iteration speed

    if memfree_trace:
        print("[memfree] first=%d kB last=%d kB min=%d kB"
              % (memfree_trace[0], memfree_trace[-1], min(memfree_trace)))
    if drift_trace:
        print("[drift-trace] %d sample(s): %s"
              % (len(drift_trace),
                 " -> ".join("d=%d" % d[2] for d in drift_trace)))

    print("\n[verdict] %s (log: %s)" % (verdict or ("UNKNOWN", "timeout"),
                                        log_path))
    if verdict:
        if verdict[0] == "PASS":
            return 0
        if verdict[0] == "FAKE-OOM":
            return 3
        if verdict[0] == "PANIC":
            return 2
    return 1


if __name__ == "__main__":
    sys.exit(main())
