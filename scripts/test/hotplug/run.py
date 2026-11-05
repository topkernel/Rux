#!/usr/bin/env python3
"""run.py — drive the udev/hotplug verification boot end to end.

Boots the hotplug test image (test/hotplug/build-img.sh) with the worktree
kernel, then:

  A. coldplug: guest init deletes /dev/vda + /dev/input/event*, busybox
     `mdev -s` rebuilds them from the /sys scan → HP:COLD-OK.
  B. uevent-file trigger: `echo add > /sys/class/block/vda/uevent` — the
     netlink daemon (uevent_probe -m /bin/mdev) must receive the add
     uevent with the full env (ACTION/DEVPATH/SUBSYSTEM/MAJOR/MINOR/
     DEVNAME/SEQNUM) and mdev must recreate /dev/vda.
  C. hotplug: QMP `device_add virtio-blk-pci` attaches the predeclared
     hotdisk, guest `echo 1 > /sys/bus/pci/rescan` → kernel emits
     add@/class/block/vdb → daemon execs mdev → /dev/vdb exists
     (HP:HOT-OK from /bin/hpcheck).

Usage: run.py [--kernel PATH] [--img PATH] [--outdir DIR] [--keep]
"""
import argparse
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time

WT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
QEMU = "/usr/bin/qemu-system-riscv64"


class Serial:
    def __init__(self, path, log):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        for _ in range(200):
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

    def wait_for(self, needle, timeout):
        deadline = time.time() + timeout
        while time.time() < deadline:
            with self.lock:
                if needle.encode() in self.buf:
                    return True
            time.sleep(0.2)
        return False

    def send(self, line):
        self.sock.sendall(line.encode() + b"\n")

    def close(self):
        self.alive = False
        try:
            self.sock.close()
        except OSError:
            pass
        self.logf.close()


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
        self.sock.settimeout(5)
        self._f = self.sock.makefile("bw")
        self._readmsg()  # greeting
        self.cmd("qmp_capabilities")

    def _readmsg(self):
        buf = b""
        while not buf.endswith(b"\r\n") and not buf.endswith(b"\n\n"):
            chunk = self.sock.recv(4096)
            if not chunk:
                raise RuntimeError("qmp closed")
            buf += chunk
        return json.loads(buf.decode(errors="replace").strip().splitlines()[-1])

    def cmd(self, execute, **args):
        self._f.write((json.dumps({"execute": execute, "arguments": args}) + "\n").encode())
        self._f.flush()
        while True:
            resp = self._readmsg()
            if "return" in resp or "error" in resp:
                return resp


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kernel", default=os.path.join(
        WT, "target/riscv64gc-unknown-none-elf/debug/rux"))
    ap.add_argument("--img", default=os.path.join(WT, ".udev/hotplug.img"))
    ap.add_argument("--outdir", default=os.path.join(WT, ".udev/run"))
    ap.add_argument("--smp", default="4")
    ap.add_argument("--keep", action="store_true", help="keep outdir")
    args = ap.parse_args()

    outdir = os.path.abspath(args.outdir)
    shutil.rmtree(outdir, ignore_errors=True)
    os.makedirs(outdir)
    serial_sock = os.path.join(outdir, "serial.sock")
    qmp_sock = os.path.join(outdir, "qmp.sock")
    hot_img = os.path.join(outdir, "hotdata.img")
    demo_img = os.path.join(outdir, "hotdemo.img")

    # hotplug data disk (root-bus function, boot-deferred) + a demo drive
    # for the QMP device_add onto the pcie-root-port
    for img, mb in ((hot_img, 64), (demo_img, 16)):
        with open(img, "wb") as f:
            f.truncate(mb * 1024 * 1024)

    cmd = [
        QEMU, "-M", "virt", "-accel", "tcg,thread=multi", "-cpu", "rv64",
        "-m", "2G", "-smp", args.smp,
        "-display", "none",
        "-serial", f"unix:{serial_sock},server,nowait",
        "-qmp", f"unix:{qmp_sock},server,nowait",
        "-device", "virtio-keyboard-pci", "-device", "virtio-tablet-pci",
        # pcie.0 itself is not hotpluggable; a root port is
        "-device", "pcie-root-port,id=rp0,bus=pcie.0,chassis=0,slot=0",
        "-drive", f"file={os.path.abspath(args.img)},if=none,id=rootfs,format=raw",
        "-device", "virtio-blk-pci,drive=rootfs",
        # Second virtio-blk function on the root bus: the kernel boot
        # probe claims only the root disk (virtio-blk owns one vring);
        # this function is DEFERRED and discovered by the runtime
        # /sys/bus/pci/rescan — QEMU riscv/virt cannot deliver runtime
        # device_add into the guest (pcie.0 rejects hotplug; functions
        # behind pcie-root-ports are ECAM-invisible on gpex).
        "-drive", f"file={hot_img},if=none,id=hotdisk,format=raw",
        "-device", "virtio-blk-pci,drive=hotdisk",
        # demo drive for the QMP device_add (root port present at boot)
        "-drive", f"file={demo_img},if=none,id=hotdemo,format=raw",
        "-kernel", os.path.abspath(args.kernel),
        "-append", "root=/dev/vda rw init=/sbin/init console=ttyS0",
    ]
    print("+", " ".join(cmd))
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT,
                            start_new_session=True)
    ok = False
    try:
        serial = Serial(serial_sock, os.path.join(outdir, "serial.log"))
        print("booting (coldplug happens in init)...")
        if not serial.wait_for("HP:READY", 240):
            print("FAIL: no HP:READY within 240s")
            return 1
        with serial.lock:
            head = serial.buf.decode(errors="replace")
        cold_ok = "HP:COLD-OK" in head
        print(("PASS" if cold_ok else "FAIL"), "coldplug (mdev -s from /sys)")
        if not cold_ok:
            return 1

        # --- B: uevent-file trigger (udevadm-trigger equivalent) ---
        serial.send("rm -f /dev/vda")
        time.sleep(0.5)
        serial.send("echo add > /sys/class/block/vda/uevent")
        time.sleep(2.0)
        serial.send("[ -b /dev/vda ] && echo TRIG-NODE-OK || echo TRIG-NODE-FAIL")
        time.sleep(1.0)
        if not serial.wait_for("TRIG-NODE-OK", 10):
            print("FAIL: uevent-file trigger did not recreate /dev/vda")
            return 1
        print("PASS", "uevent-file trigger: add uevent -> mdev -> /dev/vda")

        # --- C: QMP device_add demo + PCI rescan hotplug ---
        qmp = Qmp(qmp_sock)
        r = qmp.cmd("device_add", driver="virtio-blk-pci", drive="hotdemo", id="hpblk", bus="rp0")
        if "error" in r:
            print("FAIL: device_add:", r)
            return 1
        print("device_add virtio-blk-pci ok")
        time.sleep(1.0)
        serial.send("echo 1 > /sys/bus/pci/rescan")
        time.sleep(3.0)
        serial.send("/bin/hpcheck")
        if serial.wait_for("HP:HOT-OK", 30):
            print("PASS", "hotplug: uevent -> mdev -> /dev/vdb")
            ok = True
        else:
            with serial.lock:
                tail = serial.buf.decode(errors="replace")
            print("FAIL hotplug; last console output:\n", tail[-3000:])
            return 1
        # let the console flush the log dump
        time.sleep(2.0)
        return 0
    finally:
        # discipline: kill ONLY our own qemu pid (never pkill)
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.wait(timeout=10)
        for s in (serial_sock, qmp_sock):
            try:
                os.unlink(s)
            except FileNotFoundError:
                pass
        print("serial log:", os.path.join(outdir, "serial.log"))
        if not ok and not args.keep:
            pass  # keep the log for diagnosis anyway


if __name__ == "__main__":
    sys.exit(main())
