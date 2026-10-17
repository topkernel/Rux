#!/usr/bin/env python3
"""Slow-feed variant: one command at a time, wait for the prompt."""
import os, subprocess, sys, time, select

HERE = os.path.dirname(os.path.abspath(__file__))
KERNEL = os.path.join(HERE, "target/riscv64gc-unknown-none-elf/debug/rux")
ROOTFS = os.path.join(HERE, "test/rootfs.img")
SMP = int(sys.argv[1]) if len(sys.argv) > 1 else 2
SECS = int(sys.argv[2]) if len(sys.argv) > 2 else 120
LOG = sys.argv[3] if len(sys.argv) > 3 else os.path.join(HERE, "repro2.log")

logf = open(LOG, "wb")
cmd = ["qemu-system-riscv64","-M","virt","-accel","tcg,thread=single",
 "-cpu","rv64,zbb=true,zba=true,zbs=true","-m","1G","-smp",str(SMP),
 "-nographic","-drive",f"file={ROOTFS},if=none,id=rootfs,format=raw,snapshot=on,file.locking=off",
 "-device","virtio-blk-pci,disable-legacy=on,drive=rootfs",
 "-kernel",KERNEL,
 "-append","root=/dev/vda rw init=/bin/sh console=ttyS0"]
proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, bufsize=0)

buf = bytearray()
start = time.time()
state = "boot"
sent = 0
last_prompt = 0.0

def drain(tmo=0.2):
    r,_,_ = select.select([proc.stdout],[],[],tmo)
    if not r: return False
    c = os.read(proc.stdout.fileno(), 4096)
    if not c: return False
    buf.extend(c); logf.write(c); logf.flush()
    if len(buf) > 60000: del buf[:-10000]
    sys.stdout.buffer.write(c); sys.stdout.buffer.flush()
    return True

try:
    while time.time() - start < SECS:
        drain(0.2)
        tail = bytes(buf[-300:])
        if state == "boot" and b"# " in tail:
            state = "shell"; time.sleep(1.0)
            proc.stdin.write(b"cd /proc\n"); proc.stdin.flush(); sent += 1
            time.sleep(2.0)
            proc.stdin.write(b"pwd\n"); proc.stdin.flush(); sent += 1
            time.sleep(2.0)
            proc.stdin.write(b"i=0\n"); proc.stdin.flush()
            time.sleep(1.0)
            proc.stdin.write(b"while [ $i -lt 300 ]; do ls /proc >/dev/null; i=$((i+1)); done\n"); proc.stdin.flush()
            proc.stdin.write(b"echo LOOP-DONE\n"); proc.stdin.flush()
            state = "loop"
            last_prompt = time.time()
        elif state == "loop":
            pass
        if b"LOOP-DONE" in buf and b"echo LOOP-DONE" not in bytes(buf[-500:]):
            state = "done"
            print("\n=== LOOP-DONE seen ===", flush=True)
            break
        if b"SIGILL" in buf or b"KERNPANIC" in buf or b"SIGDEATH" in buf:
            print("\n=== FATAL marker seen ===", flush=True)
            break
    print("=== state:", state, "elapsed %.1f" % (time.time()-start), flush=True)
finally:
    logf.close()
    proc.kill(); proc.wait()
