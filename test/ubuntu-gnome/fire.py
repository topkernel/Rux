#!/usr/bin/env python3
"""fire.py — headless ignition driver for the riscv64 GNOME Ubuntu image.

Rebuild of the lost fire6.py driver (same contract, adapted from the
surviving x86 mirror test/gnome-x86/fire.py). Boots the image under
QEMU riscv64 virt with serial on a unix socket and QMP on another, then:
  - streams serial to runs/gnome-<tag>/serial.log
  - takes periodic QMP screendumps (PPM) and prints pixel stats
  - tracks chain milestones from the serial stream (runit prints, Xorg
    banner, dfx taskdump comm listings, kernel panic signatures)
  - writes runs/gnome-<tag>/summary.txt + markers.json at exit

Milestones (mirrors the old fire-r8x family):
  M2_gsd_daemon          a gsd-* comm appears in a taskdump
  M3_gnome_shell         gnome-shell comm appears (RUNNING or any)
  M4_screen_lit          screendump nonblack ratio >= 0.05
  M5_desktop_visible     nonblack >= 0.30 and >= 4 significant colors

Usage:
  python3 test/ubuntu-gnome/fire.py [--tag T] [--seconds 2400]
         [--img work/gnome-riscv64.img] [--kernel target/.../debug/rux]
         [--init /sbin/runit] [--mem 2G] [--smp 2] [--dfx "watchdog,taskdump"]
Ctrl-C or SIGTERM stops the VM cleanly (only the PID we started).
"""
import argparse
import json
import os
import re
import signal
import socket
import subprocess
import sys
import threading
import time

WT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

p = argparse.ArgumentParser()
p.add_argument("--tag", default="run")
p.add_argument("--seconds", type=int, default=2400)
p.add_argument("--img", default=os.path.join(WT, "work/gnome-riscv64.img"))
p.add_argument("--kernel", default=os.path.join(
    WT, "target/riscv64gc-unknown-none-elf/debug/rux"))
p.add_argument("--init", default="/sbin/runit")
p.add_argument("--mem", default="2G")
p.add_argument("--smp", default="2")
p.add_argument("--thread", default="multi", choices=["multi", "single"])
p.add_argument("--dfx", default="")
p.add_argument("--extra-append", default="")
p.add_argument("--dump-every", type=int, default=30)
p.add_argument("--keyprobe", default="",
               help="comma list of elapsed seconds for a single QMP 'a' keypress")
p.add_argument("--mousemove", default="",
               help="comma list of elapsed seconds for QMP abs pointer moves")
args = p.parse_args()

OUT = os.path.join(WT, "runs", f"gnome-{args.tag}")
os.makedirs(OUT, exist_ok=True)
SER_SOCK = os.path.join(OUT, "serial.sock")
QMP_SOCK = os.path.join(OUT, "qmp.sock")
for s in (SER_SOCK, QMP_SOCK):
    if os.path.exists(s):
        os.unlink(s)

append = (f"root=/dev/vda rw init={args.init} console=ttyS0 loglevel=3 "
          + (f"dfx={args.dfx} " if args.dfx else "")
          + args.extra_append).strip()
qemu_cmd = [
    "qemu-system-riscv64",
    "-M", "virt", "-accel", f"tcg,thread={args.thread}", "-cpu", "rv64",
    "-m", args.mem, "-smp", args.smp,
    "-snapshot",
    "-display", "none",
    "-chardev", f"socket,id=ser0,path={SER_SOCK},server=on,wait=off",
    "-serial", "chardev:ser0",
    "-qmp", f"unix:{QMP_SOCK},server=on,wait=off",
    "-drive", f"file={args.img},if=none,id=rootfs,format=raw,file.locking=off",
    "-device", "virtio-blk-pci,drive=rootfs",
    "-device", "virtio-gpu-pci,xres=1280,yres=800",
    "-device", "virtio-keyboard-pci",
    "-device", "virtio-tablet-pci",
    "-kernel", args.kernel,
    "-append", append,
]
with open(os.path.join(OUT, "cmdline.txt"), "w") as f:
    f.write(" ".join(qemu_cmd) + "\n")
print("[fire] qemu:", " ".join(qemu_cmd), flush=True)
logf = open(os.path.join(OUT, "serial.log"), "wb")
vm = subprocess.Popen(qemu_cmd, stdout=logf, stderr=subprocess.STDOUT)

stop = threading.Event()
t0 = time.time()
milestones = {}   # name -> (t_seconds, detail)
markers = []      # (t, text) raw events of interest
sigdeath = []     # (t, comm, sig) from dfx SIGDEATH lines


def bye(signum, frame):
    stop.set()


signal.signal(signal.SIGTERM, bye)
signal.signal(signal.SIGINT, bye)


def mark(name, detail=""):
    if name not in milestones:
        milestones[name] = (time.time() - t0, detail)
        print(f"[fire] MILESTONE {name} @ {milestones[name][0]:.0f}s {detail}",
              flush=True)


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
    top = sorted(counts.items(), key=lambda kv: -kv[1])[:6]
    top_hex = ["#%06x:x%d" % ((((c >> 5) & 7) * 32) << 16 | (((c >> 2) & 7) * 32) << 8
               | ((c & 3) * 64), n) for c, n in top]
    # significant colors = quantized colors covering >= 0.5% of pixels
    total = w * h
    sig = sum(1 for c, n in top if n >= total * 0.005)
    return w, h, nonblack, total, top_hex, sig


# ---------------------------------------------------------------- serial ---
serial_buf = b""


def note(text):
    el = time.time() - t0
    markers.append((round(el, 1), text))
    if len(markers) <= 400:
        print(f"[fire] marker @{el:.0f}s {text}", flush=True)


# Patterns that mark chain progress. taskdump comm listings come in two
# formats: the old "comm=gnome-shell pid=369 state=RUNNING" and the
# current dump's "pid=369 tgid=369 state=RUNNING ... comm=gnome-shell
# lastsys=..." (comm follows the state fields, no adjacency) — match
# both or reached milestones are silently under-reported.
PATTERNS = [
    (re.compile(rb"X\.Org X Server"), "xorg_banner"),
    (re.compile(rb"\[runit\] session bus: (unix:\S+)"), "session_bus"),
    (re.compile(rb"\[runit\] starting session chain"), "start_gnome"),
    (re.compile(rb"\[runit\] gnome-session died st=(\d+) after (\d+)s"), "gnome_session_died"),
    (re.compile(rb"\[runit\] falling back to minimal path"), "fallback_gs"),
    (re.compile(rb"comm=gnome-shell pid=(\d+) state=(\w+)"), "gnome_shell"),
    (re.compile(rb"pid=\d+ tgid=\d+ state=(\w+)[^\n]*comm=gnome-shell "), "gnome_shell"),
    (re.compile(rb"comm=(gsd-[\w-]+) pid=(\d+)"), "gsd"),
    (re.compile(rb"pid=\d+ tgid=\d+ state=\w+[^\n]*comm=(gsd-[\w-]+) "), "gsd"),
    (re.compile(rb"comm=mutter"), "mutter_wm"),
    (re.compile(rb"GNOME Shell started at"), "shell_started"),
    (re.compile(rb"comm=xterm"), "xterm_probe"),
    (re.compile(rb"comm=(gnome-session-failed)"), "session_failed"),
    (re.compile(rb"KERNPANIC|Kernel panic"), "kernpanic"),
    (re.compile(rb"SIGDEATH: (\S+) sig=(\d+)"), "sigdeath"),
    (re.compile(rb"alloc_pages.*[Oo][Mm]|Out of memory|oom-kill"), "oom"),
]


def serial_reader():
    global serial_buf
    while not stop.is_set():
        try:
            d = ser.recv(65536)
        except socket.timeout:
            continue
        except OSError:
            break
        if not d:
            break
        logf.write(d)
        logf.flush()
        os.write(1, d)
        serial_buf += d
        if len(serial_buf) > 400_000:
            serial_buf = serial_buf[-200_000:]
        for m in re.finditer(rb"\n", d):
            pass  # (buffer scanned whole, below, on bounded window)
        for pat, name in PATTERNS:
            for mm in pat.finditer(serial_buf[-160_000:]):
                g = [x.decode(errors="replace") for x in mm.groups()] if mm.groups() else []
                detail = " ".join(g[:3])
                if name == "sigdeath":
                    sigdeath.append((round(time.time() - t0, 1), g[0] if g else "?",
                                     g[1] if len(g) > 1 else "?"))
                    note(f"SIGDEATH {g[0] if g else '?'} sig={g[1] if len(g)>1 else '?'}")
                elif name in ("gnome_shell", "gsd", "mutter_wm", "xorg_banner",
                              "session_bus", "start_gnome", "kernpanic", "oom",
                              "gnome_session_died", "fallback_gs",
                              "shell_started", "xterm_probe", "session_failed"):
                    note(f"{name} {detail}".rstrip())
                    if name == "gnome_shell":
                        mark("M3_gnome_shell", detail)
                    elif name == "gsd":
                        mark("M2_gsd_daemon", detail)
                    elif name == "kernpanic":
                        mark("KERNEL_PANIC", detail)
                    elif name == "oom":
                        mark("OOM_LINE", detail)
        # consume scanned window to avoid re-matching every poll
        serial_buf = serial_buf[-4000:] if len(serial_buf) > 160_000 else serial_buf


# wait for sockets then connect serial
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

threading.Thread(target=serial_reader, daemon=True).start()

best_nonblack = 0.0
dump_i = 0
keyprobe = [int(x) for x in args.keyprobe.split(",") if x.strip()]
mousemove = [int(x) for x in args.mousemove.split(",") if x.strip()]
qmp = None
exit_reason = "timeout"
try:
    while not stop.is_set() and time.time() - t0 < args.seconds:
        if vm.poll() is not None:
            print(f"[fire] VM exited rc={vm.returncode} after {time.time()-t0:.0f}s",
                  flush=True)
            exit_reason = f"vm-exit rc={vm.returncode}"
            break
        time.sleep(1)
        el = int(time.time() - t0)
        if qmp is None and os.path.exists(QMP_SOCK):
            try:
                qmp = Qmp(QMP_SOCK)
                print("[fire] qmp connected", flush=True)
            except Exception as e:
                print(f"[fire] qmp connect: {e}", flush=True)
        # --keyprobe 300,900: one 'a' keypress (press+release) via QMP
        # send-key at each listed elapsed second. Deterministic positive
        # control for dfx=inputtrace: the kernel INPTRC lines MUST show
        # exactly these events (kbd code 30) at these times; any other
        # INPTRC line in a hands-off window is a phantom event.
        while keyprobe and el >= keyprobe[0]:
            kt = keyprobe.pop(0)
            if qmp:
                try:
                    try:
                        # Qmp.cmd is keyword-only; hyphenated QMP arg
                        # names go through ** expansion
                        qmp.cmd("send-key",
                                **{"keys": [{"type": "qcode",
                                             "data": {"name": "a"}}],
                                   "hold-time": 50})
                    except Exception:
                        # send-key is deprecated; input-send-event is the
                        # supported form (explicit down+up pair)
                        qmp.cmd("input-send-event", events=[
                            {"type": "key", "data": {"down": True,
                             "key": {"type": "qcode", "data": "a"}}},
                            {"type": "key", "data": {"down": False,
                             "key": {"type": "qcode", "data": "a"}}}])
                    print(f"[fire] t={el}s keyprobe 'a' sent", flush=True)
                except Exception as e:
                    print(f"[fire] t={el}s keyprobe error: {e}", flush=True)
        # --mousemove 600: absolute pointer positioning via QMP
        # input-send-event at the listed second. The virtio-tablet is an
        # ABS device (kernel advertises 0..32767 per set_absinfo), so
        # axis values are device coords: 16384 = center -> screen
        # center ~ (640,400); the second pair jumps right-down to
        # 26214/21299 -> ~ (1024,520). With the leg-7 static Xorg input
        # sections the on-screen cursor sprite MUST appear near those
        # spots in the following screendumps and INPTRC must show the
        # ptr ABS_X/ABS_Y pushes — the end-to-end input positive
        # control (the pre-input cursor idled at (1154,728), far from
        # both targets).
        while mousemove and el >= mousemove[0]:
            mt = mousemove.pop(0)
            if qmp:
                try:
                    qmp.cmd("input-send-event", events=[
                        {"type": "abs", "data": {"axis": "x",
                                                 "value": 16384}},
                        {"type": "abs", "data": {"axis": "y",
                                                 "value": 16384}}])
                    time.sleep(2)
                    qmp.cmd("input-send-event", events=[
                        {"type": "abs", "data": {"axis": "x",
                                                 "value": 26214}},
                        {"type": "abs", "data": {"axis": "y",
                                                 "value": 21299}}])
                    print(f"[fire] t={el}s mousemove abs center+rightdown sent",
                          flush=True)
                except Exception as e:
                    print(f"[fire] t={el}s mousemove error: {e}", flush=True)
        if qmp and el >= (dump_i + 1) * args.dump_every:
            dump_i += 1
            shot = os.path.join(OUT, f"shot-{el:04d}s.ppm")
            try:
                if qmp.screendump(shot):
                    st = ppm_stats(shot)
                    if st:
                        w, h, nb, tot, top, sig = st
                        ratio = nb / tot
                        best_nonblack = max(best_nonblack, ratio)
                        print(f"[fire] t={el}s shot {w}x{h} nonblack={nb}/{tot} "
                              f"({100*nb//tot}%) sigcolors={sig} top={top}", flush=True)
                        if ratio >= 0.05:
                            mark("M4_screen_lit", f"nonblack={ratio:.3f}")
                        if ratio >= 0.30 and sig >= 4:
                            mark("M5_desktop_visible",
                                 f"nonblack={ratio:.3f} sigcolors={sig}")
                        # keep disk bounded: drop shots that are fully black
                        if nb == 0:
                            os.unlink(shot)
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
    if exit_reason == "timeout":
        exit_reason = f"stopped after {args.seconds}s"

# ---------------------------------------------------------------- summary ---
elapsed = time.time() - t0
verdict_parts = []
for name in ("M2_gsd_daemon", "M3_gnome_shell", "M4_screen_lit", "M5_desktop_visible"):
    verdict_parts.append(f"{'REACHED' if name in milestones else 'not-reached'} {name}"
                         + (f" @{milestones[name][0]:.0f}s" if name in milestones else ""))
if "KERNEL_PANIC" in milestones:
    verdict_parts.append(f"KERNEL_PANIC @{milestones['KERNEL_PANIC'][0]:.0f}s")
if "OOM_LINE" in milestones:
    verdict_parts.append(f"OOM_LINE @{milestones['OOM_LINE'][0]:.0f}s")
ok = all(m in milestones for m in
         ("M2_gsd_daemon", "M3_gnome_shell", "M4_screen_lit", "M5_desktop_visible")) \
    and "KERNEL_PANIC" not in milestones

with open(os.path.join(OUT, "summary.txt"), "w") as f:
    f.write(f"verdict: {'FIRE-PASS' if ok else 'FIRE-PARTIAL'}\n"
            f"kernel: {args.kernel}\nimage:  {args.img}\n"
            f"elapsed: {elapsed:.0f}s (stop: {exit_reason})\n\n"
            "milestones:\n" + "\n".join("- " + v for v in verdict_parts)
            + f"\n\nbest nonblack ratio: {best_nonblack:.4f}\n\nmarkers:\n"
            + "\n".join(f"- @{t}s {x}" for t, x in markers)
            + "\n\nSIGDEATH events:\n" + ("\n".join(
                f"- t={t}s comm={c} sig={s}" for t, c, s in sigdeath[:60]) or " none\n"))
with open(os.path.join(OUT, "markers.json"), "w") as f:
    json.dump({"verdict": "FIRE-PASS" if ok else "FIRE-PARTIAL",
               "elapsed": round(elapsed),
               "milestones": {k: [round(v[0], 1), v[1]] for k, v in milestones.items()},
               "best_nonblack": round(best_nonblack, 4),
               "markers": markers, "sigdeath": sigdeath}, f, indent=1)

print(f"[fire] done ({'PASS' if ok else 'PARTIAL'}), artifacts in {OUT}", flush=True)
sys.exit(0 if ok else 3)
