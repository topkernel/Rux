#!/usr/bin/env python3
"""monkey.py — parallel random-workload stress for the Rux kernel.

Boots N QEMU instances in parallel on private copies of the musl rootfs,
feeds each a seeded-random stream of shell commands (toybox + mrsh), and
classifies every outcome. Goal: collect a bug list, not to fix it here.

Outcome classes per instance:
  CLEAN        shell exited or timeout hit with no anomaly markers
  PANIC        kernel panic markers in serial log (KERNPANIC / panic!)
  WEDGE        no serial progress for STALL_SEC seconds before timeout
  INIT-DEATH   userspace init died (SIGDEATH / init exited)
  QEMU-ABORT   QEMU itself died (internal error / triple fault report)

Discipline (from repo convention, CLAUDE.md):
  - private image copy per instance (-snapshot would also work; copies
    let destructive commands run with real writes and no cross-talk)
  - only the QEMU PIDs this script started are ever killed
  - detached process group so harness timeouts do not reap the wave

Usage:
  monkey.py --kernel target/riscv64gc-unknown-none-elf/debug/rux \
            --rootfs test/rootfs.img --instances 16 --minutes 10 \
            [--seed 0] [--smp 2] [--mem 1G] [--arch riscv64|x86_64] \
            [--out test/stress/runs/<tag>]
"""
import argparse
import os
import random
import re
import shutil
import signal
import subprocess
import sys
import time

# ---------------------------------------------------------------------------
# Workload pool: random command templates. Arguments get randomized per run.
# Kept deliberately "boring but nasty": high-churn file ops, process churn,
# pipes/redirection, kills, and the probe binaries when present.
# ---------------------------------------------------------------------------

CMD_POOL = [
    "mkdir -p /tmp/d{A}",
    "echo payload-{A} > /tmp/f{A}",
    "cat /tmp/f{A} > /dev/null",
    "for i in 1 2 3 4 5; do echo $i >> /tmp/l{A}; done",
    "cp /tmp/f{A} /tmp/g{A}",
    "mv /tmp/g{A} /tmp/h{A}",
    "rm -f /tmp/h{A}",
    "rmdir /tmp/d{A}",
    "ls -l / > /tmp/ls{A}",
    "dd if=/dev/zero of=/tmp/z{A} bs=4096 count={B}",
    "sync",
    "sleep 0.{B}",
    "yes | head -{C} > /tmp/y{A}",
    "cat /tmp/ls{A} | grep tmp > /dev/null",
    "wc -c /tmp/f{A}",
    "(sleep 1; echo bg{A}) &",
    "kill -9 %1 2>/dev/null",
    "mkfifo /tmp/p{A} 2>/dev/null; (echo x > /tmp/p{A} &) ; cat /tmp/p{A}",
    "ln -s /tmp/f{A} /tmp/s{A}; rm -f /tmp/s{A}",
    "stat /tmp/f{A}",
    "touch -c /tmp/f{A}",
    "du /tmp > /dev/null",
    "find /tmp -name '*{A}*' > /dev/null",
    "echo $$",
    "env | wc -l",
    "date",
    "uname -a",
    "cat /proc/meminfo > /dev/null",
    "cat /proc/cpuinfo > /dev/null",
    "free 2>/dev/null",
    "test/smoke_test/run_all.sh > /dev/null 2>&1",
    "test/jchurn 5 200 > /dev/null 2>&1",
    "test/execloop 20 > /dev/null 2>&1",
]

PANIC_MARKERS = [
    b"KERNPANIC", b"kernel panic", b"Panic in", b"double fault",
    b"KernelPanic", b"BUG:",
]
STALL_MARKERS_RESET = re.compile(rb"[\x00-\x08\x0b-\x1f\x80-\xff]")  # ignore binary noise


def gen_script(rng: random.Random, lines: int) -> bytes:
    out = []
    for _ in range(lines):
        tpl = rng.choice(CMD_POOL)
        cmd = tpl.format(A=rng.randrange(0, 32), B=rng.randrange(1, 64), C=rng.randrange(10, 2000))
        out.append(cmd)
    return ("\n".join(out) + "\n").encode()


def classify(log: bytes, stalled: bool, qemu_rc) -> str:
    for m in PANIC_MARKERS:
        if m in log:
            return "PANIC"
    if b"SIGDEATH: pid=1" in log or b"init exited" in log:
        return "INIT-DEATH"
    if qemu_rc is not None and qemu_rc != 0 and qemu_rc != -signal.SIGTERM:
        return "QEMU-ABORT"
    if stalled:
        return "WEDGE"
    return "CLEAN"


def run_one(idx: int, args, rng_seed: int, workdir: str) -> dict:
    os.makedirs(workdir, exist_ok=True)
    img = os.path.join(workdir, f"rootfs-{idx}.img")
    shutil.copyfile(args.rootfs, img)

    rng = random.Random(rng_seed)
    script = gen_script(rng, args.cmds)
    script_path = os.path.join(workdir, f"script-{idx}.sh")
    with open(script_path, "wb") as f:
        f.write(script)

    if args.arch == "x86_64":
        qemu = "qemu-system-x86_64"
        cmdline = [qemu, "-M", "q35", "-accel", "tcg,thread=single", "-cpu", "max",
                   "-m", args.mem, "-smp", str(args.smp), "-nographic", "-serial", "stdio"]
        kernel_args = ["-kernel", args.kernel]
        append = "root=/dev/vda rw init=/bin/sh console=ttyS0"
        disk = ["-drive", f"file={img},if=virtio,format=raw"]
        full = cmdline + disk + kernel_args + ["-append", append]
    else:
        qemu = "qemu-system-riscv64"
        full = [qemu, "-M", "virt", "-accel", "tcg,thread=single", "-cpu", "rv64",
                "-m", args.mem, "-smp", str(args.smp), "-nographic", "-serial", "mon:stdio",
                "-drive", f"file={img},if=none,id=rootfs{idx},format=raw",
                "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs" + str(idx),
                "-device", "virtio-gpu-pci",
                "-kernel", args.kernel,
                "-append", "root=/dev/vda rw init=/bin/sh console=ttyS0"]

    log_path = os.path.join(workdir, f"serial-{idx}.log")
    logf = open(log_path, "wb")
    start = time.time()
    with open(script_path, "rb") as stdin_f:
        proc = subprocess.Popen(full, stdin=stdin_f, stdout=logf,
                                stderr=subprocess.STDOUT, start_new_session=True)
    deadline = start + args.minutes * 60
    stall_deadline = None
    last_size = 0
    last_progress = start
    stalled = False
    rc = None
    while True:
        rc = proc.poll()
        now = time.time()
        if rc is not None:
            break
        if now > deadline:
            break
        # stall detection on log growth
        try:
            size = os.path.getsize(log_path)
        except OSError:
            size = last_size
        if size != last_size:
            last_size = size
            last_progress = now
            stall_deadline = None
        elif now - last_progress > args.stall_sec:
            stalled = True
            break
        time.sleep(2)
    if proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()
    rc = proc.returncode
    logf.close()

    with open(log_path, "rb") as f:
        log = f.read()
    verdict = classify(log, stalled, rc)
    # keep the image only for failures (disk space discipline)
    if verdict == "CLEAN":
        os.unlink(img)
    return {
        "idx": idx, "seed": rng_seed, "verdict": verdict, "rc": rc,
        "log": log_path, "img": img if verdict != "CLEAN" else None,
        "seconds": round(time.time() - start, 1),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kernel", required=True)
    ap.add_argument("--rootfs", required=True)
    ap.add_argument("--instances", type=int, default=16)
    ap.add_argument("--minutes", type=float, default=10)
    ap.add_argument("--cmds", type=int, default=120)
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="1G")
    ap.add_argument("--arch", default="riscv64", choices=["riscv64", "x86_64"])
    ap.add_argument("--stall-sec", type=float, default=180)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    tag = time.strftime("wave-%Y%m%d-%H%M%S")
    outdir = args.out or os.path.join("test", "stress", "runs", tag)
    os.makedirs(outdir, exist_ok=True)

    import concurrent.futures as cf
    results = []
    seeds = [args.seed * 100000 + i for i in range(args.instances)]
    with cf.ThreadPoolExecutor(max_workers=args.instances) as ex:
        futs = {ex.submit(run_one, i, args, seeds[i], outdir): i for i in range(args.instances)}
        for fut in cf.as_completed(futs):
            r = fut.result()
            results.append(r)
            print(f"[{r['idx']:02d}] {r['verdict']:11s} rc={r['rc']} {r['seconds']}s "
                  f"seed={r['seed']}", flush=True)

    counts = {}
    for r in results:
        counts[r["verdict"]] = counts.get(r["verdict"], 0) + 1
    summary = " ".join(f"{k}={v}" for k, v in sorted(counts.items()))
    print(f"\nwave summary: {summary}")
    with open(os.path.join(outdir, "summary.txt"), "w") as f:
        f.write(f"kernel={args.kernel}\narch={args.arch}\ninstances={args.instances}\n"
                f"minutes={args.minutes}\ncmds={args.cmds}\nseed={args.seed}\n{summary}\n")
        for r in sorted(results, key=lambda x: x["idx"]):
            f.write(f"{r['idx']:02d} {r['verdict']:11s} seed={r['seed']} rc={r['rc']} "
                    f"sec={r['seconds']} log={r['log']}\n")
    bad = [r for r in results if r["verdict"] != "CLEAN"]
    return 0 if not bad else 1


if __name__ == "__main__":
    sys.exit(main())
