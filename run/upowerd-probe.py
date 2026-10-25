#!/usr/bin/env python3
"""upowerd probe (stdio-serial variant): quiet boot (init=/bin/sh) on
the r3 image, start the system bus, launch upowerd by hand, measure the
time to acquire org.freedesktop.UPower. The 25s service_start_timeout
was blown under the session storm in r6setuid — this decides slow-start
vs broken-activation. Only the QEMU PID this script starts is killed."""
import os, subprocess, sys, time

WT = "/home/william/rux-agents/gnome-bringup"
OUT = os.path.join(WT, "run", "upowerd-probe")
os.makedirs(OUT, exist_ok=True)

cmd = [
    "qemu-system-riscv64", "-M", "virt", "-accel", "tcg,thread=multi",
    "-cpu", "rv64", "-m", "2G", "-smp", "2", "-snapshot", "-display", "none",
    "-serial", "stdio", "-monitor", "none",
    "-drive", f"file={WT}/work/gnome-r3.img,if=none,id=rootfs,format=raw,file.locking=off",
    "-device", "virtio-blk-pci,drive=rootfs",
    "-kernel", f"{WT}/target/riscv64gc-unknown-none-elf/debug/rux",
    "-append", "root=/dev/vda rw init=/bin/sh console=ttyS0 loglevel=3 quiet",
]
log = open(os.path.join(OUT, "serial.log"), "wb")
proc = subprocess.Popen(cmd, cwd=WT, stdin=subprocess.PIPE,
                        stdout=log, stderr=subprocess.STDOUT)
print("[probe] qemu pid", proc.pid, flush=True)

buf = b""
def pump(dur):
    global buf
    with open(os.path.join(OUT, "serial.log"), "rb") as f:
        f.seek(0, 2)
        sz = f.tell()
    # read what's new via the file (stdio goes to the log)
    time.sleep(dur)
    with open(os.path.join(OUT, "serial.log"), "rb") as f:
        data = f.read()
    return data

def wait_for(pat, dur):
    t0 = time.time()
    while time.time() - t0 < dur:
        data = pump(2)
        if pat.encode() in data:
            return data, True
    return data, False

def send(line):
    proc.stdin.write(line.encode() + b"\n")
    proc.stdin.flush()

data, ok = wait_for("#", 300)
print("[probe] shell prompt:", ok, flush=True)
if not ok:
    proc.kill()
    sys.exit(1)

CMDS = """export PATH=/usr/bin:/usr/sbin:/bin:/sbin
mkdir -p /run/dbus /tmp
dbus-daemon --system --fork --print-address
sleep 2
dbus-send --system --print-reply --dest=org.freedesktop.DBus / org.freedesktop.DBus.ListNames
echo MARKER_LISTNAMES_DONE
(/usr/libexec/upowerd > /tmp/upowerd.log 2>&1 &)
i=0; while [ $i -lt 90 ]; do i=$((i+1)); sleep 1; if dbus-send --system --print-reply --dest=org.freedesktop.DBus / org.freedesktop.DBus.NameHasOwner string:org.freedesktop.UPower 2>/dev/null | grep -q true; then break; fi; done; echo UPOWERD_NAME_ACQUIRED_AFTER=${i}s
cat /tmp/upowerd.log
echo MARKER_UPD_DONE
"""
proc.stdin.write(CMDS.encode())
proc.stdin.flush()

data, ok = wait_for("MARKER_UPD_DONE", 600)
time.sleep(5)
data = pump(3)
print("[probe] upd done:", ok, flush=True)

send("poweroff")
try:
    proc.wait(30)
except subprocess.TimeoutExpired:
    proc.kill()
print("[probe] done", flush=True)
