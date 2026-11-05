#!/usr/bin/env python3
"""gnome-ledger-run.py — udesk/GNOME acceptance boot with the accounting
ledger armed (oom-accounting lane).

Boots the shared Ubuntu GUI image read-only (-snapshot, file.locking=off),
logs in over serial (root/rux), then keeps the session alive while:
  - sending the "LEDGER!" UART magic every LEDGER_EVERY seconds (dfx=ledger
    census of where every managed page went), and
  - watching the serial for the fake-OOM signature family:
    pagefault OOM / PF-OOM / OOM-FORENSIC / MEMLEDGER / ALLOCTHROW /
    oom-killer / ENOMEM storms.

Verdict printed at the end:
  FAKE-OOM <line>   — a fake-OOM signature fired (capture in <log>)
  SESSION-ALIVE     — no OOM to --secs; ledger lines captured: N
Everything is logged verbatim to --log.
"""
import argparse
import os
import re
import select
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(HERE)
QEMU = "/usr/bin/qemu-system-riscv64"

OOM_PATTERNS = [
    re.compile(p) for p in [
        r"pagefault: Out of memory",
        r"PF-OOM",
        r"OOM-FORENSIC",
        r"ALLOCTHROW",
        r"oom-killer: Killed process",
        r"oom: out of memory",
        r"free_pages: REFUSED",
        r"errno=12",
        r"Kernel panic",
    ]
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--img", default="/home/william/workspace/Rux/work/ubuntu-gui.img")
    ap.add_argument("--kernel",
                    default=os.path.join(WT, "target",
                                         "riscv64gc-unknown-none-elf",
                                         "debug", "rux"))
    ap.add_argument("--secs", type=int, default=2400)
    ap.add_argument("--smp", default="4")
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--dfx", default="watchdog,taskdump,memwatch,ledger")
    ap.add_argument("--ledger-every", type=float, default=180.0)
    ap.add_argument("--login-at", type=float, default=None,
                    help="seconds to wait for the login screen "
                         "(default: auto-detect '[udesk] login screen up')")
    ap.add_argument("--log", default=None)
    args = ap.parse_args()

    log_path = args.log or os.path.join(
        WT, "work", "gnome-ledger-%d.log" % int(time.time()))
    logf = open(log_path, "wb")

    cmd = [QEMU, "-M", "virt", "-accel", "tcg,thread=multi", "-cpu", "rv64",
           "-m", args.mem, "-smp", args.smp,
           "-nographic", "-serial", "mon:stdio", "-monitor", "none",
           "-snapshot",
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % args.img,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-device", "virtio-gpu-pci",
           "-device", "virtio-keyboard-pci", "-device", "virtio-tablet-pci",
           "-kernel", args.kernel,
           "-append", "root=/dev/vda rw init=/sbin/init console=ttyS0 "
                      "dfx=%s" % args.dfx]
    print("[qemu] %s" % " ".join(cmd[:9]) + " ...")
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT,
                            stdin=subprocess.PIPE)

    t0 = time.time()
    deadline = t0 + args.secs
    logged_in = False
    next_ledger = None
    verdict = None
    ledgers = 0
    oom_hits = []

    def send(s):
        try:
            proc.stdin.write(s.encode())
            proc.stdin.flush()
            print("[sent %7.1fs] %r" % (time.time() - t0, s))
        except OSError as e:
            print("[send failed] %s" % e)

    try:
        buf = b""
        while time.time() < deadline:
            r, _, _ = select.select([proc.stdout], [], [], 5.0)
            if r:
                chunk = os.read(proc.stdout.fileno(), 65536)
                if not chunk:
                    break
                logf.write(chunk)
                logf.flush()
                sys.stdout.buffer.write(chunk)
                sys.stdout.buffer.flush()
                buf = (buf + chunk)[-8192:]
                text = buf.decode("utf-8", "replace")

                if not logged_in and "login screen up" in text:
                    # login screen just appeared; give the greeter a moment
                    time.sleep(5)
                    send("root\n")
                    time.sleep(3)
                    send("rux\n")
                    logged_in = True
                    next_ledger = time.time() + 30
                    print("[login] credentials sent at %.1fs" % (time.time() - t0))

                for pat in OOM_PATTERNS:
                    m = pat.search(text)
                    if m and verdict is None:
                        verdict = ("FAKE-OOM", m.group(0))
                        oom_hits.append((time.time() - t0, m.group(0)))
                ledgers += chunk.count(b"MEMLEDGER @")
                if "Kernel panic" in text:
                    verdict = verdict or ("PANIC", "kernel panic")
                    break

            if logged_in and next_ledger and time.time() >= next_ledger:
                send("LEDGER!")
                next_ledger = time.time() + args.ledger_every

            if proc.poll() is not None:
                break
    finally:
        proc.kill()
        proc.wait()
        logf.close()

    print("\n[verdict] %s (login=%s, ledgers=%d, log: %s)" %
          (verdict or ("SESSION-ALIVE", "no OOM signature to %ds" % args.secs),
           logged_in, ledgers, log_path))
    if verdict and verdict[0] == "FAKE-OOM":
        return 3
    if verdict and verdict[0] == "PANIC":
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
