#!/usr/bin/env python3
"""segv-vma probe: quiet boot (init=/bin/sh) on a scratch image copy
carrying /segvprobe (static NULL-deref), run it, and capture the
kernel's user-SEGV report — verifying a8da458b's segv vma lines name
the faulting mappings. Only the QEMU PID this script starts is used."""
import os, subprocess, sys, time

WT = "/home/william/rux-agents/gnome-bringup"
OUT = os.path.join(WT, "run", "segv-vma-probe")
os.makedirs(OUT, exist_ok=True)

cmd = [
    "qemu-system-riscv64", "-M", "virt", "-accel", "tcg,thread=multi",
    "-cpu", "rv64", "-m", "2G", "-smp", "2", "-snapshot", "-display", "none",
    "-serial", "stdio", "-monitor", "none",
    "-drive", "file=/tmp/gnome-segv.img,if=none,id=rootfs,format=raw,file.locking=off",
    "-device", "virtio-blk-pci,drive=rootfs",
    "-kernel", f"{WT}/target/riscv64gc-unknown-none-elf/debug/rux",
    "-append", "root=/dev/vda rw init=/bin/sh console=ttyS0 loglevel=7 quiet",
]
log = open(os.path.join(OUT, "serial.log"), "wb")
proc = subprocess.Popen(cmd, cwd=WT, stdin=subprocess.PIPE,
                        stdout=log, stderr=subprocess.STDOUT)
print("[probe] qemu pid", proc.pid, flush=True)

def read_all():
    with open(os.path.join(OUT, "serial.log"), "rb") as f:
        return f.read()

def wait_for(pat, dur):
    t0 = time.time()
    last = b""
    while time.time() - t0 < dur:
        time.sleep(3)
        last = read_all()
        if pat.encode() in last:
            return last, True
    return last, False

def send(line):
    proc.stdin.write(line.encode() + b"\n")
    proc.stdin.flush()

_, ok = wait_for("#", 300)
print("[probe] shell prompt:", ok, flush=True)
if not ok:
    proc.kill(); sys.exit(1)

send("/segvprobe")
time.sleep(20)
_, ok = wait_for("SEGV-PROBE-DONE", 60)  # not used; just settle
data = read_all()
send("echo PROBE_WRAPPED")
time.sleep(5)
send("poweroff")
time.sleep(5)
try:
    proc.wait(30)
except subprocess.TimeoutExpired:
    proc.kill()
print("[probe] done", flush=True)
