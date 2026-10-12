#!/usr/bin/env python3
"""cpufull - full GNOME session CPU verification.

Boots a cold copy of the shared image, runs the ORIGINAL chain
(Xorg -> start-gnome.sh -> gnome-session), samples per-PID utime/stime
every 30s for all session processes, and checks milestones: Xorg socket,
gsd daemons, gnome-shell process.

Discipline: one QEMU, cold copy, file.locking=off, kills only its own pid.
Final validation: smp4 / 2G (reference r74j shape).
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

FULL_SH = r"""#!/bin/bash
# fullsampler: watch the whole GNOME chain and sample CPU.
# Output goes to the serial console (stdout) so the driver captures it
# even on a hard QEMU terminate.
export HOME=/root
echo "FULL start t=$(date +%s)"
round=0
while [ $round -lt 40 ]; do
  round=$((round+1))
  echo "SAMPLE t=$(date +%s)"
  if [ $round -eq 8 ]; then
    /root/pollburn wake 1 &
  fi
  for p in /proc/[0-9]*; do
    pid=${p#/proc/}
    comm=$(cat $p/comm 2>/dev/null) || continue
    case "$comm" in
      Xorg|dbus-daemon|InputThread|gnome-shell*|gnome-session*|gsd-*|gmain|gdbus|dconf*|goa*|evolution*|polkit*|Xwayland|at-spi*) ;;
      *) continue ;;
    esac
    line=$(awk '{print $3, $14, $15}' $p/stat 2>/dev/null) || continue
    echo "P $pid $comm $line"
  done
  sleep 30
done
echo "FULL done t=$(date +%s)"
"""

AUTO_SH = """# /root/auto.sh - cpufull full GNOME session chain
/bin/mkdir /run/dbus
/bin/mkdir /tmp/.X11-unix
BG /usr/bin/Xorg -config /root/xorg-fbdev.conf -logfile /root/Xorg.0.log :0
BG /root/start-gnome.sh
BG /root/fullsampler.sh
SLEEP 7200
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
    print(f"[cpufull {time.strftime('%H:%M:%S')}] {msg}", flush=True)


def main():
    out = os.path.join(HERE, sys.argv[1] if len(sys.argv) > 1 else
                       f"full{time.strftime('%H%M%S')}")
    timeout = int(sys.argv[2]) if len(sys.argv) > 2 else 1500
    smp = os.environ.get("CPU_SMP", "4")
    mem = os.environ.get("CPU_MEM", "2G")
    dfx = os.environ.get("CPU_DFX", "taskdump")
    os.makedirs(out, exist_ok=True)
    fire_img = os.path.join(out, "fire.img")

    if os.path.exists(fire_img):
        os.unlink(fire_img)
    log(f"cold-copy {SRC_IMG} -> {fire_img}")
    if subprocess.run(["cp", "--sparse=always", SRC_IMG, fire_img]).returncode:
        sys.exit("image copy failed")
    debugfs(fire_img, "rm /tmp/.X0-lock", "rm /tmp/.X11-unix/X0", write=True)
    for path, body, mode in [("/root/fullsampler.sh", FULL_SH, "0100755"),
                             ("/root/auto.sh", AUTO_SH, "0100644"),
                             ("/root/pollburn", open(POLLBURN, "rb").read(), "0100755")]:
        tmp = f"/tmp/cpufull{os.path.basename(path)}"
        with open(tmp, "wb") as f:
            f.write(body if isinstance(body, bytes) else body.encode())
        debugfs(fire_img, f"rm {path}", f"write {tmp} {path}",
                f"sif {path} mode {mode}", write=True)
    log("image patched (fullsampler.sh + auto.sh + pollburn)")

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
    milestones = {}

    def pump(dur):
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
        return True

    def send(line):
        try:
            proc.stdin.write(line.encode() + b"\n")
            proc.stdin.flush()
        except Exception as e:
            log(f"send failed: {e}")

    try:
        sent_dump = False
        while time.time() - t0 < timeout:
            pump(2)
            data = bytes(buf)
            # milestones from serial output
            if b"X.Org X Server" in data or b"Xorg.0.log" in data:
                milestones.setdefault("xorg_started", time.time() - t0)
            for pat, key in [
                (b"gnome-session", "gnome_session_seen"),
                (b"gnome-shell", "gnome_shell_seen"),
                (b"gsd-", "gsd_seen"),
            ]:
                if pat in data:
                    milestones.setdefault(key, time.time() - t0)
            if b"KERNPANIC" in data[-8000:] or b"Kernel panic" in data[-8000:]:
                log("kernel panic detected")
                pump(60)
                break

            # every 120s: status line
            el = time.time() - t0
            if int(el) % 120 < 3:
                log(f"t={el:.0f}s milestones={sorted(milestones.keys())}")
                time.sleep(3)

            # near the end: manual taskdump (state check with full session)
            if el > timeout - 150 and not sent_dump:
                sent_dump = True
                log("sending manual DUMP! (uart-magic)")
                send("DUMP!")
                pump(20)
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

    # samples live in the serial log (sampler echoes to console)
    data = bytes(buf).decode(errors="replace")
    with open(os.path.join(out, "samples.txt"), "w") as f:
        for ln in data.splitlines():
            if ln.startswith(("SAMPLE", "P ", "FULL", "POLLBURN")):
                f.write(ln + "\n")
    dumps = re.findall(r"=== DFX TASK DUMP \(uart-magic\).*?(?=\n=== \d+ tasks)", data, re.S)
    with open(os.path.join(out, "dump_uartmagic.txt"), "w") as f:
        f.write("\n\n=====\n\n".join(dumps))
    log(f"done: out={out} milestones={ {k: round(v) for k, v in milestones.items()} }")
    return 0


if __name__ == "__main__":
    sys.exit(main())
