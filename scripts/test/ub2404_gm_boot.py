#!/usr/bin/env python3
"""ub2404_gm3.py — headline boot, blind-login variant.

The serial console drops bytes under load ("UART TX stalled"), so waiting
for a visible "login:" prompt is unreliable. This driver waits for
multi-user.target, then periodically nudges the tty with CR and sends the
username, watching for the password prompt / shell in the RESPONSES.

Usage: UBIMG=path ub2404_gm3.py <tag> [wall_seconds]
Exit 0 on GM3-LOGIN-OK echo.
"""
import os
import socket
import subprocess
import sys
import time

WT = "/home/william/rux-agents/glib-mutex"
IMG = os.environ.get("UBIMG", "/home/william/rux-images/ubuntu/ub2404-gm.img")
BZ = os.path.join(WT, "target/x86_64-unknown-none/debug/bzImage")

tag = sys.argv[1] if len(sys.argv) > 1 else "gm3"
wall = int(sys.argv[2]) if len(sys.argv) > 2 else 1250

run = os.path.join(WT, "run")
sock_path = os.path.join(run, f"ub2404-{tag}.sock")
serial_log = os.path.join(run, f"ub2404-{tag}-serial.log")
try:
    os.unlink(sock_path)
except FileNotFoundError:
    pass

cmd = [
    "qemu-system-x86_64",
    "-M", "q35", "-m", "8G", "-smp", "8",
    "-accel", "kvm", "-cpu", "max",
    "-snapshot",
    "-display", "none",
    "-serial", f"unix:{sock_path},server,nowait",
    "-monitor", "none",
    "-no-reboot",
    "-drive", f"file={IMG},if=none,id=rootfs,format=raw,file.locking=off",
    "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
    "-netdev", "user,id=net0",
    "-device", "virtio-net-pci,disable-legacy=on,netdev=net0",
    "-device", "virtio-gpu-pci,xres=1280,yres=800",
    "-device", "virtio-keyboard-pci,disable-legacy=on",
    "-device", "virtio-tablet-pci,disable-legacy=on",
    "-kernel", BZ,
    "-append", "root=/dev/vda rw init=/sbin/init console=ttyS0",
]
qemu = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
print(f"[{time.strftime('%H:%M:%S')}] qemu pid {qemu.pid}", flush=True)

deadline = time.time() + 30
while not os.path.exists(sock_path):
    if time.time() > deadline or qemu.poll() is not None:
        print("FATAL: serial socket never appeared", flush=True)
        qemu.kill()
        sys.exit(1)
    time.sleep(0.2)
time.sleep(0.5)
sk = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sk.connect(sock_path)
sk.setblocking(False)

logf = open(serial_log, "wb")
buf = b""
t0 = time.time()
multiuser_at = None
next_attempt = None
stage = "wait_boot"  # wait_boot -> attempting -> logged_in
rc = 1
try:
    while time.time() - t0 < wall:
        try:
            data = sk.recv(65536)
            if data:
                buf += data
                logf.write(data)
                logf.flush()
        except BlockingIOError:
            pass
        now = time.time() - t0
        tail = buf[-600:]

        if stage == "wait_boot" and (
            b"multi-user.target" in buf
            or b"gdm.service" in buf
            or now >= 420
        ):
            multiuser_at = now
            next_attempt = now + 20
            stage = "armed"
            print(f"[+{now:.0f}s] boot marker (multi-user/gdm/420s); login attempts from +20s", flush=True)

        if stage == "armed" and next_attempt is not None and now >= next_attempt:
            # Nudge the tty (a getty reprints its prompt on CR) and send
            # the username; if a shell is already up, this is a harmless
            # newline on the command line.
            sk.send(b"\r")
            time.sleep(0.3)
            sk.send(b"root\r")
            print(f"[+{now:.0f}s] sent username attempt", flush=True)
            stage = "attempting"
            next_attempt = now + 60  # retry cadence
            time.sleep(0.5)
            continue

        if stage == "attempting":
            if b"assword" in tail:
                sk.send(b"rux\r")
                print(f"[+{now:.0f}s] password prompt seen; sent password", flush=True)
                time.sleep(0.5)
                sk.send(b"echo GM3-LOGIN-OK\r")
                stage = "confirming"
                time.sleep(0.3)
                continue
            if now >= (next_attempt or 0):
                stage = "armed"  # loop back into the attempt branch

        if stage in ("attempting", "confirming") and b"GM3-LOGIN-OK" in buf:
            m = buf.rfind(b"GM3-LOGIN-OK")
            print(f"[+{now:.0f}s] GM3-LOGIN-OK CONFIRMED", flush=True)
            print(buf[m : m + 160].decode(errors="replace"), flush=True)
            stage = "logged_in"
            next_attempt = now + (wall - now - 10)  # survive the rest
            continue

        if stage == "logged_in" and now >= wall - 8:
            sk.send(b"echo GM3-SURVIVED\r")
            time.sleep(3)
            rc = 0
            break

        if b"Kernel panic" in tail or b"DEADLOCK: spinlock stuck" in tail:
            print(f"[+{now:.0f}s] FATAL: panic in serial", flush=True)
            rc = 2
            break
        time.sleep(0.2)
finally:
    try:
        sk.close()
    except OSError:
        pass
    logf.close()
    try:
        qemu.terminate()
        qemu.wait(timeout=10)
    except Exception:
        qemu.kill()
print(f"result rc={rc} stage={stage} multiuser_at={multiuser_at}", flush=True)
sys.exit(rc)
