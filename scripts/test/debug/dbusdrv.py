#!/usr/bin/env python3
"""dbusdrv.py - boot the gnome image with a minimal D-Bus test chain
(no Xorg, no GNOME), harvest RSLT: lines from serial, stop on
DBUSTEST-DONE / panic / timeout. One QEMU, cold-copied image, kills only
its own pid."""
import argparse
import os
import re
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
SRC_IMG = "/home/william/rux-agents/gnome-img/gnome.img"
QEMU = "/usr/bin/qemu-system-riscv64"
DEBUGFS = "/usr/sbin/debugfs"

AUTO_SH = """# dbus minimal test chain
/bin/mkdir /run/dbus
/bin/mkdir /root/xdg
/bin/mkdir /root/xdg/dbus-1
/bin/mkdir /root/xdg/dbus-1/services
SLEEP 3
BG /root/dbustest.sh
SLEEP 2400
"""

INJECT = [
    (os.path.join(HERE, "dbuswire.py"), "/root/dbuswire.py", 0o100755),
    (os.path.join(HERE, "dbustest.sh"), "/root/dbustest.sh", 0o100755),
    (os.path.join(HERE, "peer-wrap.sh"), "/root/peer-wrap.sh", 0o100755),
    (os.path.join(HERE, "obs.py"), "/root/obs.py", 0o100755),
    (os.path.join(HERE, "org.rux.Test.service"),
     "/root/xdg/dbus-1/services/org.rux.Test.service", 0o100644),
]


def debugfs(img, *cmds, write=False):
    out = []
    for c in cmds:
        args = [DEBUGFS] + (["-w"] if write else []) + ["-R", c, img]
        p = subprocess.run(args, capture_output=True, text=True, timeout=180)
        out.append(p.stdout + p.stderr)
    return "".join(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kernel", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--timeout", type=int, default=900)
    ap.add_argument("--smp", default="2")
    ap.add_argument("--mem", default="1G")
    ap.add_argument("--thread", default="multi")
    ap.add_argument("--keep", action="store_true",
                    help="keep the cold-copied image for postmortem")
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    img = os.path.join(args.out, "test.img")
    t0 = time.time()
    shutil.copyfile(SRC_IMG, img)
    log = lambda m: print("[drv %.0fs] %s" % (time.time() - t0, m), flush=True)
    log("image copied")

    for d in ("/root/xdg", "/root/xdg/dbus-1", "/root/xdg/dbus-1/services"):
        debugfs(img, "mkdir %s" % d, write=True)
    for src, dst, mode in INJECT:
        debugfs(img, "rm %s" % dst, "write %s %s" % (src, dst),
                "sif %s mode 0%o" % (dst, mode), write=True)
    with open("/tmp/dbusdrv-auto-%d" % os.getpid(), "w") as f:
        f.write(AUTO_SH)
    debugfs(img, "rm /root/auto.sh",
            "write /tmp/dbusdrv-auto-%d /root/auto.sh" % os.getpid(),
            "sif /root/auto.sh mode 0o100744", write=True)
    chk = debugfs(img, "cat /root/auto.sh")
    if "dbustest.sh" not in chk:
        log("FATAL: auto.sh patch roundtrip failed:\n" + chk)
        sys.exit(2)
    log("image patched")

    ser_path = os.path.join(args.out, "serial.log")
    cmd = [QEMU, "-M", "virt", "-accel", "tcg,thread=%s" % args.thread,
           "-cpu", "rv64", "-m", args.mem, "-smp", args.smp,
           "-display", "none", "-serial", "stdio",
           "-drive", "file=%s,if=none,id=rootfs,format=raw,"
           "file.locking=off" % img,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", args.kernel, "-append",
           "root=/dev/vda rw init=/sbin/runit console=ttyS0 "
           "dfx=taskdump"]
    with open(os.path.join(args.out, "cmdline.txt"), "w") as f:
        f.write(" ".join(cmd) + "\n")
    ser = open(ser_path, "wb")
    proc = subprocess.Popen(cmd, stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    with open(os.path.join(args.out, "qemu.pid"), "w") as f:
        f.write(str(proc.pid))
    log("qemu pid %d (budget %ds)" % (proc.pid, args.timeout))

    buf = b""
    results = []
    panic = done = False
    tail = b""
    while True:
        d = proc.stdout.read1(65536)
        if not d:
            log("qemu stdout eof")
            break
        ser.write(d)
        ser.flush()
        buf += d
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            try:
                t = line.decode(errors="replace")
            except Exception:
                continue
            if "RSLT:" in t or t.startswith("DBUSTEST"):
                stamp = "%.0f" % (time.time() - t0)
                results.append("[%ss] %s" % (stamp, t.strip()))
                log(t.strip())
            if "KERNPANIC" in t or "Kernel panic" in t:
                panic = True
            if "DBUSTEST-DONE" in t:
                done = True
            if len(tail) > 400:
                tail = tail[-400:]
            tail += t.encode() + b"\n"
        if panic or done:
            break
        if time.time() - t0 > args.timeout:
            log("budget exhausted")
            break
    try:
        proc.terminate()
        proc.wait(timeout=10)
    except Exception:
        proc.kill()
    ser.close()
    log("qemu stopped (done=%s panic=%s)" % (done, panic))

    with open(os.path.join(args.out, "results.txt"), "w") as f:
        f.write("\n".join(results) + "\n")
        f.write("--- serial tail ---\n")
        f.write(tail.decode(errors="replace"))
    log("results written; %d RSLT lines" % len(results))
    if not args.keep:
        for p in (img,):
            try:
                os.unlink(p)
            except OSError:
                pass
    return 0 if done and not panic else 1


if __name__ == "__main__":
    sys.exit(main())
