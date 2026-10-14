#!/usr/bin/env python3
"""fire.py — headless ignition driver for the x86_64 GNOME Ubuntu image.

Boots work/gnome-x86.img under QEMU q35 with the serial console on a unix
socket and QMP on another, then:
  - streams serial to runs/gnome-x86-<tag>/serial.log
  - takes periodic QMP screendumps (PPM) into the same dir
  - prints pixel stats per dump (non-black ratio, dominant colors)

QEMU shape mirrors test/run-x86-ub.sh with two changes from the R86
handover (commit 1bffb8a7):
  - -cpu Haswell: with -cpu max, TCG reports AuthenticAMD family=0
    model=0 and llvmpipe's JIT fatally aborts ("can't map a 64-bit host
    CPU") — Haswell gives llvmpipe a CPUID it accepts.
  - -vga none: QMP `screendump` dumps the first console; without this it
    grabs the default std-VGA text console instead of virtio-gpu scanout.

Usage:
  python3 test/gnome-x86/fire.py [--tag T] [--seconds 240] [--img PATH]
         [--bzimage PATH] [--init /sbin/runit] [--mem 2G] [--smp 1]
         [--extra-append "k=v ..."]
Ctrl-C or SIGTERM stops the VM cleanly (only the PID we started).
"""
import argparse
import json
import os
import signal
import socket
import subprocess
import sys
import threading
import time

WT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

p = argparse.ArgumentParser()
p.add_argument("--tag", default="run")
p.add_argument("--seconds", type=int, default=240)
p.add_argument("--img", default=os.path.join(WT, "work/gnome-x86.img"))
p.add_argument("--bzimage", default=os.path.join(WT, "target/x86_64-unknown-none/debug/bzImage"))
p.add_argument("--init", default="/sbin/runit")
p.add_argument("--mem", default="2G")
p.add_argument("--smp", default="1")
p.add_argument("--cpu", default="Haswell")
p.add_argument("--extra-append", default="")
p.add_argument("--dump-every", type=int, default=20)
args = p.parse_args()

OUT = os.path.join(WT, "runs", f"gnome-x86-{args.tag}")
os.makedirs(OUT, exist_ok=True)
SER_SOCK = os.path.join(OUT, "serial.sock")
QMP_SOCK = os.path.join(OUT, "qmp.sock")
for s in (SER_SOCK, QMP_SOCK):
    if os.path.exists(s):
        os.unlink(s)

append = f"root=/dev/vda rw init={args.init} console=ttyS0 {args.extra_append}".strip()
qemu_cmd = [
    "qemu-system-x86_64",
    "-M", "q35", "-m", args.mem, "-smp", args.smp,
    "-accel", "tcg,thread=single",
    "-cpu", args.cpu,
    "-snapshot",
    "-display", "none",
    "-vga", "none",
    "-chardev", f"socket,id=ser0,path={SER_SOCK},server=on,wait=off",
    "-serial", "chardev:ser0",
    "-qmp", f"unix:{QMP_SOCK},server=on,wait=off",
    "-drive", f"file={args.img},if=none,id=rootfs,format=raw,file.locking=off",
    "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
    "-netdev", "user,id=net0",
    "-device", "virtio-net-pci,disable-legacy=on,netdev=net0",
    "-device", "virtio-gpu-pci,xres=1280,yres=800",
    "-device", "virtio-keyboard-pci,disable-legacy=on",
    "-device", "virtio-tablet-pci,disable-legacy=on",
    "-kernel", args.bzimage,
    "-append", append,
]
print("[fire] qemu:", " ".join(qemu_cmd), flush=True)
logf = open(os.path.join(OUT, "serial.log"), "wb")
vm = subprocess.Popen(qemu_cmd, stdout=logf, stderr=subprocess.STDOUT)

stop = threading.Event()


def bye(signum, frame):
    stop.set()


signal.signal(signal.SIGTERM, bye)
signal.signal(signal.SIGINT, bye)


class Qmp:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        for _ in range(200):
            try:
                self.sock.connect(path)
                break
            except (FileNotFoundError, ConnectionRefusedError):
                time.sleep(0.1)
        else:
            raise RuntimeError("cannot connect qmp socket " + path)
        self.f = self.sock.makefile("rwb")
        self._read()
        self.cmd("qmp_capabilities")

    def _read(self):
        while True:
            line = self.f.readline()
            if not line:
                raise EOFError("qmp closed")
            msg = json.loads(line)
            if "event" in msg:
                continue
            return msg

    def cmd(self, name, **kw):
        req = {"execute": name}
        if kw:
            req["arguments"] = kw
        self.f.write(json.dumps(req).encode() + b"\n")
        self.f.flush()
        return self._read()

    def screendump(self, path):
        r = self.cmd("screendump", filename=path)
        if "error" in r:
            raise RuntimeError(f"screendump failed: {r}")
        for _ in range(100):
            if os.path.exists(path) and os.path.getsize(path) > 100:
                return True
            time.sleep(0.1)
        return False


def ppm_stats(path):
    with open(path, "rb") as f:
        data = f.read()
    if not data.startswith(b"P6"):
        return None
    parts = data.split(b"\n", 3)
    w, h = map(int, parts[1].split())
    pix = parts[3][: w * h * 3]
    nonblack = 0
    counts = {}
    for o in range(0, len(pix), 3):
        c = (pix[o] // 32 << 5) | (pix[o + 1] // 32 << 2) | (pix[o + 2] // 64)
        if pix[o] | pix[o + 1] | pix[o + 2]:
            nonblack += 1
        counts[c] = counts.get(c, 0) + 1
    top = sorted(counts.items(), key=lambda kv: -kv[1])[:5]
    top_hex = ["#%06x:x%d" % (((c >> 5) & 7) * 32 << 16 | ((c >> 2) & 7) * 32 << 8 | (c & 3) * 64, n) for c, n in top]
    return w, h, nonblack, w * h, top_hex


# wait for sockets then connect serial (tee to our stdout as well)
for _ in range(200):
    if os.path.exists(SER_SOCK):
        break
    time.sleep(0.1)
ser = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
for _ in range(200):
    try:
        ser.connect(SER_SOCK)
        break
    except (FileNotFoundError, ConnectionRefusedError):
        time.sleep(0.1)
ser.settimeout(0.5)


def serial_reader():
    while not stop.is_set():
        try:
            d = ser.recv(4096)
        except socket.timeout:
            continue
        except OSError:
            break
        if not d:
            break
        logf.write(d)
        logf.flush()
        os.write(1, d)


threading.Thread(target=serial_reader, daemon=True).start()

t0 = time.time()
dump_i = 0
qmp = None
try:
    while not stop.is_set() and time.time() - t0 < args.seconds:
        if vm.poll() is not None:
            print(f"[fire] VM exited rc={vm.returncode} after {time.time()-t0:.0f}s", flush=True)
            break
        time.sleep(1)
        el = int(time.time() - t0)
        if qmp is None and os.path.exists(QMP_SOCK):
            try:
                qmp = Qmp(QMP_SOCK)
                print("[fire] qmp connected", flush=True)
            except Exception as e:
                print(f"[fire] qmp connect: {e}", flush=True)
        if qmp and el >= (dump_i + 1) * args.dump_every:
            dump_i += 1
            shot = os.path.join(OUT, f"shot-{el:04d}s.ppm")
            try:
                if qmp.screendump(shot):
                    st = ppm_stats(shot)
                    if st:
                        w, h, nb, tot, top = st
                        print(f"[fire] t={el}s shot {w}x{h} nonblack={nb}/{tot} ({100*nb//tot}%) top={top}", flush=True)
                else:
                    print(f"[fire] t={el}s screendump never materialized", flush=True)
            except Exception as e:
                print(f"[fire] t={el}s screendump error: {e}", flush=True)
finally:
    print("[fire] stopping", flush=True)
    stop.set()
    vm.terminate()
    try:
        vm.wait(timeout=10)
    except subprocess.TimeoutExpired:
        vm.kill()
    logf.close()
print(f"[fire] done, artifacts in {OUT}", flush=True)
