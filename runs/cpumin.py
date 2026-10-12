#!/usr/bin/env python3
"""cpumin - CPU-burn reproduction driver for the GNOME session storm.

Boots a cold copy of the shared GNOME image with a chosen kernel, starts a
minimal session (Xorg + session dbus-daemon only, or +gnome chain), samples
per-PID utime/stime via /proc/PID/stat over the serial console, and runs the
pollburn repro + DUMP! taskdump checks.

Discipline: one QEMU at a time, cold copy (never writes the shared image),
file.locking=off, kills only its own pid. -smp 2 -m 1G for iteration.
"""
import os
import re
import select
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
SRC_IMG = "/home/william/rux-agents/gnome-img/gnome.img"
DEBUGFS = "/usr/sbin/debugfs"
KERNEL = os.environ.get(
    "CPU_KERNEL",
    "/home/william/rux-agents/gnome-cpu/target/riscv64gc-unknown-none-elf/debug/rux")
POLLBURN = "/tmp/pollburn"

PHASE1_SH = r"""#!/bin/bash
# phase1: Xorg + session dbus only; sample their CPU.
export HOME=/root
for i in $(seq 1 90); do [ -S /tmp/.X11-unix/X0 ] && break; sleep 5; done
echo PHASE1 x0_socket=ready t=$(date +%s)
if [ ! -S /run/dbus/session_bus_socket ]; then
  /usr/bin/dbus-daemon --session --fork --nopidfile \
      --address=unix:path=/run/dbus/session_bus_socket >/root/dbus-session.log 2>&1
fi
sleep 30
echo PHASE1 settle_done t=$(date +%s)
for round in $(seq 1 5); do
  echo "SAMPLE t=$(date +%s)"
  for p in /proc/[0-9]*; do
    pid=${p#/proc/}
    comm=$(cat $p/comm 2>/dev/null) || continue
    case "$comm" in
      Xorg|dbus-daemon|InputThread|runit|bash|sh|pollburn|sleep|ksoftirqd*|kfbflush|khungtaskd|kswapd) ;;
      *) continue ;;
    esac
    line=$(awk '{print $1, $3, $14, $15}' $p/stat 2>/dev/null) || continue
    echo "P $pid $comm $line"
  done
  sleep 30
done
echo PHASE1 done t=$(date +%s)
"""

AUTO_SH = """# /root/auto.sh - cpumin minimal session
/bin/mkdir /run/dbus
/bin/mkdir /tmp/.X11-unix
BG /usr/bin/Xorg -config /root/xorg-fbdev.conf -logfile /root/Xorg.0.log :0
BG /root/phase1.sh
/bin/sh
"""


def debugfs(img, *cmds, write=False):
    out = []
    for c in cmds:
        args = [DEBUGFS] + (["-w"] if write else []) + ["-R", c, img]
        p = subprocess.run(args, capture_output=True, text=True, timeout=180)
        out.append(p.stdout + p.stderr)
    return "".join(out)


def log(msg):
    print(f"[cpumin {time.strftime('%H:%M:%S')}] {msg}", flush=True)


def main():
    out = os.path.join(HERE, sys.argv[1] if len(sys.argv) > 1 else
                       f"run{time.strftime('%H%M%S')}")
    timeout = int(sys.argv[2]) if len(sys.argv) > 2 else 900
    smp = os.environ.get("CPU_SMP", "2")
    mem = os.environ.get("CPU_MEM", "1G")
    dfx = os.environ.get("CPU_DFX", "taskdump")
    os.makedirs(out, exist_ok=True)
    fire_img = os.path.join(out, "fire.img")

    if os.path.exists(fire_img):
        os.unlink(fire_img)
    log(f"cold-copy {SRC_IMG} -> {fire_img}")
    if subprocess.run(["cp", "--sparse=always", SRC_IMG, fire_img]).returncode:
        sys.exit("image copy failed")
    debugfs(fire_img, "rm /tmp/.X0-lock", "rm /tmp/.X11-unix/X0", write=True)
    for path, body, mode in [("/root/phase1.sh", PHASE1_SH, "0100755"),
                             ("/root/auto.sh", AUTO_SH, "0100644"),
                             ("/root/pollburn", open(POLLBURN, "rb").read(), "0100755")]:
        tmp = f"/tmp/cpumin{os.path.basename(path)}"
        with open(tmp, "wb") as f:
            f.write(body if isinstance(body, bytes) else body.encode())
        debugfs(fire_img, f"rm {path}", f"write {tmp} {path}",
                f"sif {path} mode {mode}", write=True)
    log("image patched (phase1.sh + auto.sh + pollburn)")

    qmp_sock = os.path.join(out, "qmp.sock")
    ser_path = os.path.join(out, "serial.log")
    cmd = ["/usr/bin/qemu-system-riscv64", "-M", "virt",
           "-accel", "tcg,thread=multi", "-cpu", "rv64", "-m", mem,
           "-smp", smp, "-display", "none", "-serial", "stdio",
           "-qmp", f"unix:{qmp_sock},server,nowait",
           "-drive", f"file={fire_img},if=none,id=rootfs,format=raw,"
                     f"file.locking=off",
           "-device", "virtio-blk-pci,drive=rootfs",
           "-device", "virtio-gpu-pci",
           "-device", "virtio-keyboard-pci",
           "-device", "virtio-tablet-pci",
           "-kernel", KERNEL,
           "-append", f"root=/dev/vda rw init=/sbin/runit console=ttyS0 dfx={dfx}"]
    with open(os.path.join(out, "cmdline.txt"), "w") as f:
        f.write(" ".join(cmd) + "\n")
    log("qemu starting")
    proc = subprocess.Popen(cmd, stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    with open(os.path.join(out, "qemu.pid"), "w") as f:
        f.write(str(proc.pid))
    log(f"qemu pid {proc.pid}")

    ser = open(ser_path, "wb")
    buf = bytearray()
    t0 = time.time()
    last_out = 0.0

    def pump(dur):
        nonlocal buf, last_out
        end = time.time() + dur
        while time.time() < end:
            r, _, _ = select.select([proc.stdout], [], [], 0.2)
            if r:
                chunk = os.read(proc.stdout.fileno(), 65536)
                if not chunk:
                    log("qemu stdout EOF")
                    return False
                buf.extend(chunk)
                ser.write(chunk)
                ser.flush()
                last_out = time.time()
        return True

    def send(line):
        try:
            proc.stdin.write(line.encode() + b"\n")
            proc.stdin.flush()
        except Exception as e:
            log(f"send failed: {e}")

    def tail(n=4000):
        return bytes(buf[-n:]).decode(errors="replace")

    try:
        # Stage A: wait for the serial shell (auto.sh ends with /bin/sh).
        shell_at = None
        while time.time() - t0 < min(timeout, 600):
            pump(1)
            # The interactive shell echoes what we type: probe with a marker.
            if b"PHASE1 settle_done" in buf and shell_at is None:
                # components are up; find shell by probing
                send("echo CPUSHELL=$$")
                pump(3)
                if b"CPUSHELL=" in bytes(buf[-3000:]):
                    shell_at = time.time()
                    log(f"serial shell alive at t={time.time()-t0:.0f}s")
                    break
            if b"PHASE1 settle_done" in buf:
                send("echo CPUSHELL=$$")
        if shell_at is None:
            log(f"WARN: no shell marker by t={time.time()-t0:.0f}s; continuing anyway")

        # Stage B: wait for the 5 sampler rounds (5 x 30s + 30s settle).
        while time.time() - t0 < timeout:
            pump(1)
            if b"PHASE1 done" in buf:
                log(f"phase1 sampler finished at t={time.time()-t0:.0f}s")
                break
            if b"KERNPANIC" in bytes(buf[-8000:]):
                log("kernel panic detected")
                break

        # Stage C: quantified pollburn runs (foreground, 20s each) —
        # busy = control, poll/ppoll/epoll/select = idle-blocked waits.
        send("/root/pollburn busy 20")
        pump(28)
        send("/root/pollburn poll 20")
        pump(28)
        send("/root/pollburn ppoll 20")
        pump(28)
        send("/root/pollburn epoll 20")
        pump(28)
        send("/root/pollburn select 20")
        pump(28)

        # C2: taskdump-state check: sleeping tasks (sleep + pollburn blocked
        # in poll) + manual DUMP! (serial-RX context, NOT the timer softirq).
        send("/root/pollburn poll 90 &")
        send("/root/pollburn epoll 90 &")
        pump(4)
        send("sleep 60 &")
        pump(3)
        log("sending manual DUMP! (uart-magic)")
        send("DUMP!")
        pump(8)
        log("measurement stage complete; leaving QEMU up for a moment")
        pump(10)
    finally:
        try:
            proc.terminate()
            proc.wait(timeout=10)
        except Exception:
            try:
                proc.kill()
            except Exception:
                pass
        ser.close()

    data = bytes(buf).decode(errors="replace")
    # Extract and save the interesting bits.
    with open(os.path.join(out, "samples.txt"), "w") as f:
        for ln in data.splitlines():
            if ln.startswith(("SAMPLE", "P ", "PHASE1", "POLLBURN")) or "CPUSHELL" in ln:
                f.write(ln + "\n")
    dumps = re.findall(r"=== DFX TASK DUMP \(uart-magic\).*?(?=\n=== \d+ tasks)", data, re.S)
    with open(os.path.join(out, "dump_uartmagic.txt"), "w") as f:
        f.write("\n\n=====\n\n".join(dumps))
    log(f"done: out={out} serial={ser_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
