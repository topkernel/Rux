#!/usr/bin/env python3
"""verify.py — boot the Ubuntu GUI image under QEMU and verify it end to end.

Drives the serial console (unix socket) and QMP (unix socket):
  boot -> login screen (screendump) -> log in -> desktop + terminal
  -> open SysInfo / About -> cycle focus -> run a shell command in the
  Terminal window -> close windows, all confirmed by QMP screendump pixel
  checks plus serial log markers. Runs the whole flow for N consecutive
  boots (default 2) and fails on any kernel panic / deadlock marker.

Usage: verify.py [--runs 2] [--kernel PATH] [--img PATH] [--outdir DIR]
"""
import argparse
import json
import os
import shutil
import socket
import subprocess
import sys
import threading
import time

WT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))  # worktree root

# ---------------------------------------------------------------- serial side
class Serial:
    def __init__(self, path, log):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        for _ in range(100):
            try:
                self.sock.connect(path)
                break
            except (FileNotFoundError, ConnectionRefusedError):
                time.sleep(0.1)
        else:
            raise RuntimeError("cannot connect serial socket " + path)
        self.sock.settimeout(0.5)
        self.buf = bytearray()
        self.logf = open(log, "wb")
        self.lock = threading.Lock()
        self.alive = True
        threading.Thread(target=self._reader, daemon=True).start()

    def _reader(self):
        while self.alive:
            try:
                d = self.sock.recv(4096)
            except socket.timeout:
                continue
            except OSError:
                break
            if not d:
                break
            with self.lock:
                self.buf += d
                self.logf.write(d)
                self.logf.flush()

    def send(self, data: bytes):
        self.sock.sendall(data)

    def wait_for(self, needle: bytes, timeout=240.0):
        deadline = time.time() + timeout
        while time.time() < deadline:
            with self.lock:
                if needle in self.buf:
                    return True
            time.sleep(0.3)
        with self.lock:
            tail = bytes(self.buf[-2000:]).decode("utf-8", "replace")
        raise TimeoutError(f"timeout waiting for {needle!r}; serial tail:\n{tail}")

    def text(self):
        with self.lock:
            return bytes(self.buf).decode("utf-8", "replace")


class Qmp:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        for _ in range(100):
            try:
                self.sock.connect(path)
                break
            except (FileNotFoundError, ConnectionRefusedError):
                time.sleep(0.1)
        else:
            raise RuntimeError("cannot connect qmp socket " + path)
        self.f = self.sock.makefile("rwb")
        self._read()  # greeting
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

    def cmd(self, name, **args):
        req = {"execute": name}
        if args:
            req["arguments"] = args
        self.f.write(json.dumps(req).encode() + b"\n")
        self.f.flush()
        return self._read()

    def screendump(self, path):
        r = self.cmd("screendump", filename=path)
        if "error" in r:
            raise RuntimeError(f"screendump failed: {r}")
        for _ in range(100):
            if os.path.exists(path) and os.path.getsize(path) > 100:
                return
            time.sleep(0.1)


# ------------------------------------------------------------------ ppm tools
def read_ppm(path):
    with open(path, "rb") as f:
        data = f.read()
    if not data.startswith(b"P6"):
        raise ValueError("not a P6 ppm: " + path)
    parts = data.split(b"\n", 3)
    w, h = map(int, parts[1].split())
    if parts[2] != b"255":
        raise ValueError("unexpected maxval " + repr(parts[2]))
    pix = parts[3][: w * h * 3]
    return w, h, pix


def get(pix, w, x, y):
    o = (y * w + x) * 3
    return pix[o], pix[o + 1], pix[o + 2]


def close(c, hexcol, tol=28):
    r, g, b = (hexcol >> 16) & 255, (hexcol >> 8) & 255, hexcol & 255
    return abs(c[0] - r) <= tol and abs(c[1] - g) <= tol and abs(c[2] - b) <= tol


def count_near(pix, hexcol, tol=30):
    r, g, b = (hexcol >> 16) & 255, (hexcol >> 8) & 255, hexcol & 255
    n = 0
    for o in range(0, len(pix), 3):
        if abs(pix[o] - r) <= tol and abs(pix[o + 1] - g) <= tol and abs(pix[o + 2] - b) <= tol:
            n += 1
    return n


# palette (see udesk.c)
AUB_DARK, AUB_MID, AUB_DEEP = 0x2C001E, 0x772953, 0x1B0410
ORANGE, WHITE, TERM_BG, TITLE_A = 0xE95420, 0xFFFFFF, 0x300A24, 0x5E2750

TITLE_H = 22


def wingeo(w):
    """mirror udesk.c init_wge(): (x, y, w, h) for TERM/INFO/ABOUT"""
    if w >= 1240:
        x0 = (w - 1200) // 2
        return (x0, 110, 460, 330), (x0 + 500, 140, 360, 250), (x0 + 900, 170, 300, 230)
    return (70, 60, 460, 330), (120, 90, 360, 250), (180, 120, 300, 230)


def grad(y, h):
    """mirror paint_bg lerp(AUB_DARK -> AUB_MID)"""
    r = 0x2C + (0x77 - 0x2C) * y // h
    g = 0x00 + (0x29 - 0x00) * y // h
    b = 0x1E + (0x53 - 0x1E) * y // h
    return (r << 16) | (g << 8) | b


def check(cond, msg, results):
    print(("  [ok] " if cond else "  [FAIL] ") + msg)
    results.append((cond, msg))
    if not cond:
        raise AssertionError(msg)


def verify_shot(path, stage, res):
    w, h, pix = read_ppm(path)
    term, info, about = wingeo(w)
    print(f"  screendump {path} {w}x{h} {os.path.getsize(path)}B, stage={stage}")
    if stage == "login":
        y = int(h * 0.94)
        check(close(get(pix, w, w // 2, y), grad(y, h), 14),
              f"login: aubergine gradient bg @ ({w // 2},{y}) expect ~{grad(y, h):06x}", res)
        check(count_near(pix, WHITE, 26) > 2500, f"login: white glyph pixels (logo+wordmark) = {count_near(pix, WHITE, 26)}", res)
        check(count_near(pix, ORANGE, 26) > 800, f"login: orange accents (active field border) = {count_near(pix, ORANGE, 26)}", res)
    elif stage == "desktop":
        check(close(get(pix, w, w // 3, 10), AUB_DARK), "desktop: top panel aubergine", res)
        check(close(get(pix, w, 2, h - 2), AUB_DEEP), "desktop: dock dark @ bottom-left", res)
        check(close(get(pix, w, term[0] + 5, term[1] + 10), TITLE_A), "desktop: terminal title bar active", res)
        check(close(get(pix, w, term[0] + 10, term[1] + term[3] - 6), TERM_BG), "desktop: terminal body purple", res)
        check(count_near(pix, 0x8AE234, 30) > 30, f"desktop: green prompt/cursor pixels = {count_near(pix, 0x8AE234, 30)}", res)
    elif stage == "sysinfo":
        check(close(get(pix, w, info[0] + 5, info[1] + 8), TITLE_A), "sysinfo: focused title bar", res)
        n = 0
        for y in range(info[1] + TITLE_H, info[1] + info[3] - 20, 2):
            for x in range(info[0], info[0] + info[2], 2):
                if close(get(pix, w, x, y), ORANGE, 32):
                    n += 1
        check(n > 40, f"sysinfo: orange label glyphs in window = {n}", res)
    elif stage == "about":
        cx, cy = about[0] + about[2] // 2, about[1] + TITLE_H + 34  # logo centre
        check(close(get(pix, w, cx, cy - 18), ORANGE, 32), f"about: orange logo ring @ ({cx},{cy - 18})", res)
        n = 0
        for y in range(about[1] + TITLE_H, about[1] + about[3], 2):
            for x in range(about[0], about[0] + about[2], 2):
                if close(get(pix, w, x, y), WHITE, 32):
                    n += 1
        check(n > 60, f"about: white 'Ubuntu' wordmark pixels = {n}", res)
    elif stage == "closed":
        # info + about closed, terminal still open: sample a clean body pixel
        check(close(get(pix, w, term[0] + term[2] - 30, term[1] + term[3] - 6), TERM_BG),
              "closed: terminal body still present", res)
        check(close(get(pix, w, w // 3, 10), AUB_DARK), "closed: top panel intact", res)


def run_boot(idx, args, outdir):
    print(f"\n=== boot #{idx + 1}/{args.runs} ===")
    run_dir = os.path.join(outdir, f"boot{idx + 1}")
    shutil.rmtree(run_dir, ignore_errors=True)
    os.makedirs(run_dir)
    ser_sock = os.path.join(run_dir, "serial.sock")
    qmp_sock = os.path.join(run_dir, "qmp.sock")
    for p in (ser_sock, qmp_sock):
        if os.path.exists(p):
            os.unlink(p)

    kernel = args.kernel or os.path.join(
        WT, "target", "riscv64gc-unknown-none-elf", "debug", "rux")
    img = args.img or os.path.join(WT, "work", "ubuntu-gui.img")
    cmd = [
        "qemu-system-riscv64", "-M", "virt", "-accel", "tcg,thread=single",
        "-cpu", "rv64", "-m", "2G", "-smp", str(getattr(args, "smp", 1)),
        "-snapshot",
        "-display", "none",
        "-serial", f"unix:{ser_sock},server,nowait",
        "-qmp", f"unix:{qmp_sock},server,nowait",
        "-drive", f"file={img},if=none,id=rootfs,format=raw",
        "-device", "virtio-blk-pci,drive=rootfs",
        "-device", "virtio-gpu-pci",
        "-kernel", kernel,
        "-append", "root=/dev/vda rw init=/sbin/init console=ttyS0",
    ]
    print("qemu:", " ".join(cmd))
    qemu_log = open(os.path.join(run_dir, "qemu.log"), "wb")
    proc = subprocess.Popen(cmd, stdout=qemu_log, stderr=subprocess.STDOUT,
                            start_new_session=True)
    print("qemu pid:", proc.pid)
    res = []
    ser = qmp = None
    try:
        ser = Serial(ser_sock, os.path.join(run_dir, "serial.log"))
        qmp = Qmp(qmp_sock)
        ser.wait_for(b"[udesk] login screen up", 300)
        print("  login screen is up")
        time.sleep(1.5)

        shot = lambda name: qmp.screendump(os.path.join(run_dir, name))
        shot("01-login.ppm")
        verify_shot(os.path.join(run_dir, "01-login.ppm"), "login", res)

        # wrong password first (negative path, shows error line)
        ser.send(b"root\r")
        ser.wait_for(b"login user='root' -> password")
        ser.send(b"wrong\r")
        ser.wait_for(b"LOGIN FAIL")
        # after a FAIL the password field is cleared but still focused
        ser.send(b"rux\r")
        ser.wait_for(b"LOGIN OK -> desktop", 60)
        time.sleep(1.0)
        shot("02-desktop.ppm")
        verify_shot(os.path.join(run_dir, "02-desktop.ppm"), "desktop", res)

        # SysInfo
        ser.send(b"\x13")  # Ctrl-S
        ser.wait_for(b"app open+focus: 1")
        time.sleep(0.8)
        shot("03-sysinfo.ppm")
        verify_shot(os.path.join(run_dir, "03-sysinfo.ppm"), "sysinfo", res)

        # About
        ser.send(b"\x01")  # Ctrl-A
        ser.wait_for(b"app open+focus: 2")
        time.sleep(0.8)
        shot("04-about.ppm")
        verify_shot(os.path.join(run_dir, "04-about.ppm"), "about", res)

        # cycle focus back to terminal (term -> info -> about cycle: from about -> term)
        ser.send(b"\t")
        ser.wait_for(b"focus -> 0")
        time.sleep(0.5)

        # run a real command in the terminal window
        ser.send(b"echo GUITERM-OK-$(date +%s | wc -c)\r")
        ser.wait_for(b"GUITERM-OK-", 60)
        time.sleep(0.8)
        shot("05-terminal.ppm")
        verify_shot(os.path.join(run_dir, "05-terminal.ppm"), "desktop", res)

        # close About and SysInfo, terminal remains (focus auto-cycles)
        ser.send(b"\t"); ser.wait_for(b"focus -> 1")
        ser.send(b"\x17")  # Ctrl-W close sysinfo
        ser.wait_for(b"close win 1")
        ser.wait_for(b"focus -> 2")  # auto-cycled onto About
        ser.send(b"\x17")  # Ctrl-W close about
        ser.wait_for(b"close win 2")
        ser.wait_for(b"focus -> 0")  # auto-cycled onto Terminal
        time.sleep(0.8)
        shot("06-closed.ppm")
        verify_shot(os.path.join(run_dir, "06-closed.ppm"), "closed", res)

        # keep session alive a bit to catch late panics
        time.sleep(5)
        text = ser.text()
        for bad in ("KERNPANIC", "Segfault", "pagefault:", "deadlock"):
            check(bad not in text, f"no '{bad}' in serial log", res)

        ok = all(c for c, _ in res)
        print(f"boot #{idx + 1}: {'PASS' if ok else 'FAIL'} ({sum(1 for c, _ in res if c)}/{len(res)} checks)")
        return ok
    finally:
        try:
            if qmp:
                qmp.cmd("quit")
        except Exception:
            pass
        time.sleep(1.0)
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(10)
            except subprocess.TimeoutExpired:
                proc.kill()
        for s in (ser,):
            if s:
                s.alive = False
        print("qemu exited rc=", proc.poll())


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=2)
    ap.add_argument("--smp", type=int, default=1)
    ap.add_argument("--kernel")
    ap.add_argument("--img")
    ap.add_argument("--outdir", default=os.path.join(WT, "work", "verify"))
    args = ap.parse_args()
    os.makedirs(args.outdir, exist_ok=True)
    results = [run_boot(i, args, args.outdir) for i in range(args.runs)]
    print("\n=== overall:", "PASS" if all(results) else "FAIL", f"({sum(results)}/{len(results)} boots) ===")
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
