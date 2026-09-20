#!/usr/bin/env python3
"""Deterministic B-form prober: drives the icount QEMU by its OUTPUT
(prompt-synced), and when output stalls (the silent pipe hang), sends
the DUMP! magic so the guest's RX-IRQ DFX trigger prints the task
snapshot. icount makes guest timing instruction-deterministic, so this
script's waits cannot perturb reproduction."""
import subprocess, sys, time, os, select

WORK = "/tmp/rux-hunt"
os.makedirs(WORK, exist_ok=True)
KRN = "target/riscv64gc-unknown-none-elf/debug/rux"
LOG = open(f"{WORK}/probe.log", "wb")

cmd = ["/usr/bin/qemu-system-riscv64", "-M", "virt", "-accel", "tcg,thread=single",
       "-cpu", "rv64", "-m", "2G", "-smp", "4", "-icount", "shift=2",
       "-nographic", "-serial", "mon:stdio",
       "-drive", "file=/tmp/r32gate/rfs.img,if=none,id=rootfs,format=raw",
       "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
       "-kernel", KRN,
       "-append", "root=/dev/vda rw init=/bin/sh console=ttyS0 dfx=watchdog,taskdump"]
p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.STDOUT)

def read_for(sec):
    out = b""
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([p.stdout], [], [], 0.5)
        if r:
            chunk = os.read(p.stdout.fileno(), 65536)
            if not chunk:
                break
            out += chunk
            LOG.write(chunk); LOG.flush()
    return out

def send(s):
    p.stdin.write(s.encode() + b"\n")
    p.stdin.flush()

def wait_prompt(timeout=240):
    """Wait for the shell prompt '# ' with stall detection."""
    end = time.time() + timeout
    last_data = time.time()
    while time.time() < end:
        r, _, _ = select.select([p.stdout], [], [], 1.0)
        if r:
            chunk = os.read(p.stdout.fileno(), 65536)
            if not chunk:
                return "eof"
            LOG.write(chunk); LOG.flush()
            last_data = time.time()
            if chunk.rstrip().endswith(b"#") or b":/#" in chunk[-40:]:
                return "prompt"
        else:
            if time.time() - last_data > 30:
                return "stall"
    return "timeout"

# Boot
send("")  # wake
time.sleep(5)
read_for(3)
send("sleep 2")
time.sleep(4); read_for(1)
send("/test/smoke_test")
wait_prompt(120)
send("/test/nettest")
wait_prompt(300)
print("nettest done, starting pipe loop", flush=True)

for i in range(1, 11):
    send("echo PP | cat")
    st = wait_prompt(60)
    print(f"pipe {i}: {st}", flush=True)
    if st == "stall":
        print(">>> STALL detected — injecting DUMP! magic", flush=True)
        p.stdin.write(b"DUMP!\n"); p.stdin.flush()
        out = read_for(10)
        if b"DFX TASK DUMP" in out:
            print(">>> taskdump captured", flush=True)
        time.sleep(3)
        p.stdin.write(b"DUMP!\n"); p.stdin.flush()
        read_for(10)
        break
    send("sleep 5"); wait_prompt(30)

print("finishing", flush=True)
p.kill()
