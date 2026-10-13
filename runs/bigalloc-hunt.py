#!/usr/bin/env python3
"""bigalloc-hunt — boot gp3.img on the integration kernel (poll fix +
fbdev fix + R75 reject + ALLOCTHROW raw-frame dump) and capture the
caller of the 64 MB kernel-heap allocation that kills gnome-shell.

Serial → serial.log; QMP screendump every 60 s → shots/sNNNN.png.
Stops after --timeout (default 1200 s). Kills only its own QEMU PID.
"""
import argparse, json, os, socket, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
QEMU = "/usr/bin/qemu-system-riscv64"

ap = argparse.ArgumentParser()
ap.add_argument("--kernel", default=os.path.join(
    HERE, "..", "target", "riscv64gc-unknown-none-elf", "debug", "rux"))
ap.add_argument("--img", default="/home/william/rux-agents/gnome-paint/work/gp3.img")
ap.add_argument("--out", default=os.path.join(HERE, "bigalloc1"))
ap.add_argument("--smp", default="1")
ap.add_argument("--mem", default="2G")
ap.add_argument("--timeout", type=int, default=1200)
args = ap.parse_args()

os.makedirs(args.out, exist_ok=True)
shots = os.path.join(args.out, "shots")
os.makedirs(shots, exist_ok=True)
qmp_sock = os.path.join(args.out, "qmp.sock")
serial_sock = os.path.join(args.out, "serial.sock")
serial_path = os.path.join(args.out, "serial.log")

cmd = [QEMU, "-M", "virt", "-accel", "tcg,thread=multi", "-cpu", "rv64",
       "-m", args.mem, "-smp", args.smp, "-display", "none",
       "-serial", f"unix:{serial_sock},server,nowait",
       "-qmp", f"unix:{qmp_sock},server,nowait",
       "-drive", f"file={args.img},if=none,id=rootfs,format=raw,"
                 f"file.locking=off,snapshot=on",
       "-device", "virtio-blk-pci,drive=rootfs",
       "-device", "virtio-gpu-pci",
       "-device", "virtio-keyboard-pci", "-device", "virtio-tablet-pci",
       "-kernel", args.kernel,
       "-append", "root=/dev/vda rw init=/sbin/runit console=ttyS0 "
                  "dfx=watchdog,taskdump"]

with open(os.path.join(args.out, "cmdline.txt"), "w") as f:
    f.write(" ".join(cmd) + "\n")
log = open(os.path.join(args.out, "driver.log"), "w", buffering=1)

def say(msg):
    log.write(f"[{time.time()-t0:7.1f}s] {msg}\n")

t0 = time.time()
proc = subprocess.Popen(cmd, stdout=log, stderr=log)
say(f"qemu pid {proc.pid}")
with open(os.path.join(args.out, "qemu.pid"), "w") as f:
    f.write(str(proc.pid))

qmp = None
def qmp_send(obj):
    global qmp
    if qmp is None:
        return None
    try:
        qmp.sendall((json.dumps(obj) + "\n").encode())
        # A screendump under host contention can take >5 s; a timeout here
        # must skip the reply wait, never crash the driver loop (bigalloc4).
        while True:
            try:
                line = qmp.recv(65536).decode(errors="replace")
            except socket.timeout:
                return None
            for l in line.splitlines():
                try:
                    r = json.loads(l)
                except Exception:
                    continue
                if "return" in r or "error" in r:
                    return r
    except Exception:
        qmp = None
        return None
    return None

shot_i = 0
seen = set()
last_size = 0
ser = None          # serial console socket (bidirectional)
ser_f = open(serial_path, "wb", buffering=0)

def serial_send(data: bytes):
    global ser
    if ser is not None:
        try:
            ser.sendall(data)
        except Exception:
            pass

def drain_serial():
    global ser
    if ser is None:
        try:
            ser = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            ser.settimeout(0.2)
            ser.connect(serial_sock)
            say("serial socket up")
        except Exception:
            ser = None
            return
    try:
        while True:
            chunk = ser.recv(65536)
            if not chunk:
                ser = None
                return
            ser_f.write(chunk)
    except socket.timeout:
        pass
    except Exception:
        ser = None

next_dump_t = 240   # first DUMP! at 240 s, then every 180 s
try:
    while time.time() - t0 < args.timeout:
        time.sleep(5)
        drain_serial()
        if proc.poll() is not None:
            say(f"qemu exited rc={proc.returncode}")
            break
        if qmp is None and os.path.exists(qmp_sock):
            try:
                qmp = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                qmp.settimeout(5)
                qmp.connect(qmp_sock)
                qmp.recv(65536)  # greeting
                qmp_send({"execute": "qmp_capabilities"})
                say("qmp up")
            except Exception as e:
                qmp = None
        now = time.time() - t0
        with open(serial_path, "rb") as f:
            text = f.read().decode(errors="replace")
        for marker in ("ALLOCTHROW", "bigalloc", "KERNPANIC", "GS-SHELL-BG",
                       "GS-BUSES-UP", "X-OK"):
            if marker in text and marker not in seen:
                seen.add(marker)
                say(f"serial marker: {marker}")
        if now >= next_dump_t:
            serial_send(b"\nDUMP!\n")
            say("sent DUMP!")
            next_dump_t = now + 180
        if qmp is not None and int(now) % 60 < 5:
            shot_i += 1
            png = os.path.join(shots, f"s{shot_i:03d}-t{int(now)}.png")
            qmp_send({"execute": "screendump", "arguments": {"filename": png}})
            say(f"shot {png}")
finally:
    say("stopping")
    if proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except Exception:
            proc.kill()
say(f"done elapsed={time.time()-t0:.0f}s serial={serial_path}")
