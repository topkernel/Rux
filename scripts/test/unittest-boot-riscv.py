#!/usr/bin/env python3
"""Run the in-kernel unit-test boot and watch for pass/fail/timeout."""
import os, subprocess, sys, time, select

HERE = os.path.dirname(os.path.abspath(__file__))
# Repo root: walk up until the Cargo.toml+kernel marker (move-immune,
# same discipline as ruxroot.py — the scripts/test relocation broke the
# old one-level dirname guess).
_d = HERE
while _d != os.path.dirname(_d):
    if os.path.isfile(os.path.join(_d, "Cargo.toml")) and os.path.isdir(os.path.join(_d, "kernel")):
        break
    _d = os.path.dirname(_d)
ROOT = _d
KERNEL = os.environ.get("RUX_KERNEL", os.path.join(ROOT, "target/riscv64gc-unknown-none-elf/debug/rux"))
ROOTFS = os.environ.get("RUX_ROOTFS", os.path.join(HERE, "rootfs.img"))
SECS = int(sys.argv[1]) if len(sys.argv) > 1 else 420
LOG = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, "unittest.log")

logf = open(LOG, "wb")
cmd = ["qemu-system-riscv64", "-M", "virt", "-accel", "tcg,thread=single",
       "-cpu", "rv64,zbb=true,zba=true,zbs=true", "-m", "1G", "-smp", "2",
       "-nographic",
       "-drive", f"file={ROOTFS},if=none,id=rootfs,format=raw,snapshot=on,file.locking=off",
       "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
       "-kernel", KERNEL]
proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                        stderr=subprocess.STDOUT, bufsize=0)
buf = bytearray()
start = time.time()
try:
    while time.time() - start < SECS:
        r, _, _ = select.select([proc.stdout], [], [], 0.5)
        if r:
            c = os.read(proc.stdout.fileno(), 4096)
            if not c:
                print("EOF", flush=True); break
            buf.extend(c); logf.write(c); logf.flush()
            if len(buf) > 200000: del buf[:-20000]
            sys.stdout.buffer.write(c); sys.stdout.buffer.flush()
        if b"all tests" in bytes(buf[-2000:]) or b"Power down" in buf or b"KERNPANIC" in buf:
            time.sleep(5)
            break
finally:
    print("\n=== unittest summary ===", flush=True)
    import re
    m = re.findall(rb"(passed|failed|PASS|FAIL|ok)\S*", bytes(buf[-5000:]))
    tail = bytes(buf[-3000:])
    for line in tail.split(b"\n")[-25:]:
        print(line.decode(errors="replace"), flush=True)
    logf.close(); proc.kill(); proc.wait()
