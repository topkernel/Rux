#!/usr/bin/env python3
"""soak.py — long-run stability soak for the Ubuntu GUI (udesk) session.

Boots the Ubuntu GUI image (smp4, MTTCG multi, dfx watchdog+taskdump),
logs into the desktop, starts a guest-side churn loop in the Terminal's
dash (fork/exec + batch reads + /tmp write churn — the GNOME-session
shape), then for N minutes:
  - heartbeats a shell command through the udesk terminal every 60s
    (a missing echo inside the timeout window = serial/terminal wedge),
  - grabs a QMP screendump every 60s and pixel-checks the top panel and
    terminal body (paint liveness, catches a frozen scanout),
  - re-checksums a fixed window of the churn data file every 5 minutes
    (data integrity through the virtio-bk/ext4 read path),
  - scans the serial log for panic/wedge markers.

Usage: soak.py [--minutes 30] [--smp 4] [--kernel PATH] [--img PATH]
               [--outdir work/soak-r1] [--no-churn]
"""
import argparse
import os
import shutil
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import verify  # Serial, Qmp, read_ppm, get, close, AUB_DARK, TERM_BG, wingeo

WT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

PANIC_MARKERS = [
    b"KERNPANIC", b"Kernel panic", b"panicked", b"Segfault", b"pagefault:",
    b"deadlock", b"soft lockup", b"SOFT-LOCKUP", b"ALLOCTHROW",
    b"trap: stack:", b"lost completion", b"buffer io wait timeout",
    b"watchdog bite", b"WATCHDOG",
]


class Soak:
    def __init__(self, args):
        self.args = args
        self.run_dir = args.outdir
        shutil.rmtree(self.run_dir, ignore_errors=True)
        os.makedirs(self.run_dir)
        self.ser_sock = os.path.join(self.run_dir, "serial.sock")
        self.qmp_sock = os.path.join(self.run_dir, "qmp.sock")
        for p in (self.ser_sock, self.qmp_sock):
            if os.path.exists(p):
                os.unlink(p)
        self.shots = os.path.join(self.run_dir, "shots")
        os.makedirs(self.shots)
        self.marker_counts = {m: 0 for m in PANIC_MARKERS}
        self.hb_ok = 0
        self.hb_fail = 0
        self.paint_fail = 0
        self.integrity_ref = None
        self.integrity_fails = 0
        self.wedged = False
        self.ser = None
        self.qmp = None

    def start_qemu(self):
        kernel = self.args.kernel or os.path.join(
            WT, "target", "riscv64gc-unknown-none-elf", "debug", "rux")
        img = self.args.img or os.path.join(WT, "work", "ubuntu-gui.img")
        cmd = [
            "qemu-system-riscv64", "-M", "virt", "-accel", "tcg,thread=multi",
            "-cpu", "rv64", "-m", str(self.args.mem), "-smp", str(self.args.smp),
            "-snapshot",
            "-display", "none",
            "-serial", f"unix:{self.ser_sock},server,nowait",
            "-qmp", f"unix:{self.qmp_sock},server,nowait",
            "-drive", f"file={img},if=none,id=rootfs,format=raw,file.locking=off",
            "-device", "virtio-blk-pci,drive=rootfs",
            "-device", "virtio-gpu-pci",
            "-kernel", kernel,
            "-append", "root=/dev/vda rw init=/sbin/init console=ttyS0 "
                       "dfx=watchdog,taskdump",
        ]
        print("qemu:", " ".join(cmd), flush=True)
        qemu_log = open(os.path.join(self.run_dir, "qemu.log"), "wb")
        self.proc = subprocess.Popen(cmd, stdout=qemu_log,
                                     stderr=subprocess.STDOUT,
                                     start_new_session=True)
        print("qemu pid:", self.proc.pid, flush=True)

    def scan_markers(self):
        text = self.ser.text().encode()
        for m in PANIC_MARKERS:
            self.marker_counts[m] = text.count(m)

    def clear_serial(self):
        """Drop backlog so wait_for only matches fresh output (locked: the
        reader thread appends under the same lock)."""
        with self.ser.lock:
            self.ser.buf = bytearray()

    def run(self):
        a = self.args
        self.start_qemu()
        try:
            self.ser = verify.Serial(self.ser_sock,
                                     os.path.join(self.run_dir, "serial.log"))
            self.qmp = verify.Qmp(self.qmp_sock)
            self.ser.wait_for(b"[udesk] login screen up", 300)
            print("login screen up", flush=True)

            self.ser.send(b"root\r")
            self.ser.wait_for(b"login user='root' -> password")
            self.ser.send(b"rux\r")
            self.ser.wait_for(b"LOGIN OK -> desktop", 60)
            print("desktop up, soak clock starts", flush=True)
            t0 = time.time()

            # churn data: ~64MB of real file bytes (binaries), fixed window
            self.ser.send(
                b"(cat /usr/bin/* /bin/* 2>/dev/null | head -c 67108864) > /tmp/rdata; "
                b"echo PRIME-DONE\r")
            self.ser.wait_for(b"PRIME-DONE", 300)
            print("churn data primed", flush=True)

            # reference checksum of the fixed window dd reads will hit
            self.ser.send(b"dd if=/tmp/rdata bs=1048576 skip=7 count=8 2>/dev/null "
                          b"| cksum; echo REF-DONE\r")
            self.ser.wait_for(b"REF-DONE", 120)
            for l in reversed(self.ser.text().splitlines()):
                parts = l.split()
                if len(parts) == 2 and all(p.isdigit() for p in parts):
                    self.integrity_ref = l.strip()
                    break
            print("integrity reference:", self.integrity_ref, flush=True)

            if not a.no_churn:
                # fork/exec + batch-read + /tmp write churn, backgrounded in
                # the terminal's dash; every iteration reads a rotating 1MB
                # window (cold batch reads through ext4/virtio), writes and
                # removes an 8MB file (dirty writeback churn).
                churn = (b"(i=0; while [ $i -lt 1000000 ]; do "
                         b"dd if=/tmp/rdata bs=1048576 skip=$((i%56)) count=1 "
                         b"2>/dev/null | cksum >/dev/null; "
                         b"head -c 8388608 /tmp/rdata > /tmp/fx; rm -f /tmp/fx; "
                         b"i=$((i+1)); done) >/dev/null 2>&1 & "
                         b"echo CHURN-STARTED\r")
                self.ser.send(churn)
                self.ser.wait_for(b"CHURN-STARTED", 60)
                print("churn loop started", flush=True)

            n = 0
            while time.time() - t0 < a.minutes * 60:
                n += 1
                time.sleep(60)
                el = int(time.time() - t0)
                self.scan_markers()
                bad = [m for m, c in self.marker_counts.items() if c]
                if bad:
                    print(f"t={el}s MARKERS PRESENT: "
                          f"{[(m.decode(), c) for m, c in self.marker_counts.items() if c]}",
                          flush=True)
                    return self.verdict(t0, early=True)
                # heartbeat through the whole stack (serial -> udesk -> dash)
                self.clear_serial()
                self.ser.send(f"echo SOAK-HB-{n}-$(date +%s)\r".encode())
                try:
                    self.ser.wait_for(f"SOAK-HB-{n}-".encode(), 120)
                    self.hb_ok += 1
                except TimeoutError:
                    self.hb_fail += 1
                    self.wedged = True
                    print(f"t={el}s HEARTBEAT {n} TIMED OUT — wedge", flush=True)
                    return self.verdict(t0, early=True)
                # paint liveness
                shot = os.path.join(self.shots, f"s{n:03d}-t{el}.ppm")
                try:
                    self.qmp.screendump(shot)
                    w, h, pix = verify.read_ppm(shot)
                    term, info, about = verify.wingeo(w)
                    okp = (verify.close(verify.get(pix, w, w // 3, 10), verify.AUB_DARK)
                           and verify.close(
                               verify.get(pix, w, term[0] + 10,
                                          term[1] + term[3] - 6), verify.TERM_BG))
                    if okp:
                        print(f"t={el}s hb={n} ok, paint ok", flush=True)
                    else:
                        self.paint_fail += 1
                        print(f"t={el}s hb={n} ok, PAINT FAIL (panel/terminal "
                              f"colors wrong)", flush=True)
                        if self.paint_fail >= 3:
                            return self.verdict(t0, early=True)
                except Exception as e:
                    self.paint_fail += 1
                    print(f"t={el}s screendump/pixel error: {e}", flush=True)
                # data integrity every 5th heartbeat
                if n % 5 == 0:
                    self.clear_serial()
                    self.ser.send(b"dd if=/tmp/rdata bs=1048576 skip=7 count=8 "
                                  b"2>/dev/null | cksum; echo INTOK\r")
                    try:
                        self.ser.wait_for(b"INTOK", 120)
                        got = None
                        for l in reversed(self.ser.text().splitlines()):
                            parts = l.split()
                            if len(parts) == 2 and all(p.isdigit() for p in parts):
                                got = l.strip()
                                break
                        if got and got != self.integrity_ref:
                            self.integrity_fails += 1
                            print(f"t={el}s INTEGRITY FAIL got={got} "
                                  f"want={self.integrity_ref}", flush=True)
                        else:
                            print(f"t={el}s integrity ok", flush=True)
                    except TimeoutError:
                        self.integrity_fails += 1
                        print(f"t={el}s integrity check timed out", flush=True)
            return self.verdict(t0)
        finally:
            try:
                if self.qmp:
                    self.qmp.cmd("quit")
            except Exception:
                pass
            time.sleep(1.0)
            if self.proc.poll() is None:
                self.proc.terminate()
                try:
                    self.proc.wait(10)
                except subprocess.TimeoutExpired:
                    self.proc.kill()
            print("qemu exited rc=", self.proc.poll(), flush=True)

    def verdict(self, t0, early=False):
        self.scan_markers()
        elapsed = int(time.time() - t0)
        bad = {m.decode(): c for m, c in self.marker_counts.items() if c}
        ok = (not bad and not self.wedged and self.paint_fail == 0
              and self.integrity_fails == 0 and not early)
        lines = [
            f"tag={os.path.basename(self.args.outdir)} dur={elapsed}s "
            f"requested={self.args.minutes * 60}s",
            f"early_end={early} wedged={self.wedged}",
            f"heartbeats ok={self.hb_ok} failed={self.hb_fail}",
            f"paint_fails={self.paint_fail} integrity_fails={self.integrity_fails}",
            f"markers={bad if bad else 'none'}",
            f"serial_bytes={len(self.ser.text()) if self.ser else 0}",
            f"overall: {'PASS' if ok else 'FAIL'}",
        ]
        text = "\n".join(lines)
        print(text, flush=True)
        with open(os.path.join(self.run_dir, "verdict.txt"), "w") as f:
            f.write(text + "\n")
        return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--minutes", type=int, default=30)
    ap.add_argument("--smp", type=int, default=4)
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--kernel")
    ap.add_argument("--img")
    ap.add_argument("--outdir", default=os.path.join(WT, "work", "soak-r1"))
    ap.add_argument("--no-churn", action="store_true")
    args = ap.parse_args()
    s = Soak(args)
    rc = s.run()
    sys.exit(rc)


if __name__ == "__main__":
    main()
