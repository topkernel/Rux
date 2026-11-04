#!/usr/bin/env python3
# Boot lat_sem with serial on a unix socket; after N seconds send "DUMP!"
# to snapshot tasks DURING the wedge. Serial -> file + injected magic.
import socket, subprocess, sys, time, os

WT = "/home/william/rux-agents/lmbench-fix"
sock = f"{WT}/run/ser.sock"
log  = open(f"{WT}/run/dbg5.log", "wb")
delay = float(sys.argv[1]) if len(sys.argv) > 1 else 28.0
tmo   = float(sys.argv[2]) if len(sys.argv) > 2 else 70.0
case  = sys.argv[3] if len(sys.argv) > 3 else "lat_sem"
try: os.unlink(sock)
except FileNotFoundError: pass

import time as _t; _t.sleep(0.5)

p = subprocess.Popen([
    "qemu-system-x86_64", "-M", "q35", "-accel", "kvm", "-cpu", "host",
    "-m", "2G", "-smp", "2", "-display", "none", 
    "-monitor", "none",
    "-chardev", f"socket,id=ser0,path={sock},server=on,wait=off",
    "-serial", "chardev:ser0",
    "-kernel", f"{WT}/target/x86_64-unknown-none/debug/bzImage",
    "-append", f"root=/dev/vda rw init=/test/lmbench_run lmbench={case} console=ttyS0 dfx=taskdump",
    "-drive", f"file=/home/william/rux-agents/perf-lmbench/work/lmbench-x86.img,if=none,id=rootfs,format=raw,snapshot=on,file.locking=off",
    "-device", "virtio-blk-pci,drive=rootfs",
    "-device", "virtio-net-pci,netdev=n0", "-netdev", "user,id=n0",
], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

conn = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
for _ in range(40):
    try:
        conn.connect(sock); break
    except (FileNotFoundError, ConnectionRefusedError):
        _t.sleep(0.25)
conn.settimeout(1.0)
start = time.time(); sent = False; out = b""
while time.time() - start < tmo:
    try:
        d = conn.recv(65536)
        if d:
            out += d; log.write(d); log.flush()
    except socket.timeout:
        pass
    if not sent and time.time() - start > delay:
        try: conn.sendall(b"DUMP!")
        except Exception: pass
        sent = True
    if p.poll() is not None:
        break
try: conn.close()
except Exception: pass
p.terminate()
try: p.wait(5)
except Exception: p.kill()
try: os.unlink(sock)
except FileNotFoundError: pass
txt = out.decode("utf-8", "replace").replace("\r", "")
i = txt.find("DFX TASK DUMP")
print(txt[i:i+3500] if i >= 0 else "NO DUMP; tail:\n" + txt[-1500:])
