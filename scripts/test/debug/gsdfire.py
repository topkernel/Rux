#!/usr/bin/env python3
"""gsdfire.py — fast probe booter for the gsd-discovery investigation.

Copies the GNOME image, injects /root/gsdprobe + /root/glibprobe (+ a
start-probe-gnome.sh scenario script in --full mode), boots the Rux
kernel, streams serial to a log, optionally sends the "DUMP!" uart magic
(syscall-ring + task dump), and kills only its own QEMU pid.

Usage:
  ./gsdfire.py --kernel ../target/riscv64gc-unknown-none-elf/debug/rux \
               [--full] [--tag run1] [--send-dump-at 260]
"""
import argparse
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
IMG = "/home/william/rux-agents/gnome-img/gnome.img"
QEMU = "/usr/bin/qemu-system-riscv64"
DEBUGFS = "/usr/sbin/debugfs"

PROBE_AUTO = """# gsd-discovery probe chain (replaces the Xorg chain)
/bin/mkdir /run/dbus
/root/gsdprobe
/root/glibprobe
/bin/echo GSD-PROBE-DONE
/bin/sh
"""

FULL_AUTO = """# gsd-discovery full-scenario chain: Xorg + dbus + env + real gnome-session
/bin/mkdir /run/user
/bin/mkdir /run/user/0
/bin/mkdir /run/dbus
/bin/mkdir /tmp/.X11-unix
BG /usr/bin/Xorg -config /root/xorg-fbdev.conf -logfile /root/Xorg.0.log :0
BG /root/start-probe-gnome.sh
/bin/echo GSD-PROBE-DONE
/bin/sh
"""

STREAM_KEYS = ("PROBE:", "GLIB:", "REQ ", "PANIC", "GSD-PROBE", "GSD-RING",
               "PROBE-GNOME", "PROBE-ENV", "PROBE-SUBCHILD", "Unable to find",
               "sent DUMP", "WARNING", "version", "runit", "GSD-")


def log(msg):
    print(f"[gsdfire {time.strftime('%H:%M:%S')}] {msg}", flush=True)


def debugfs(img, *cmds, write=False):
    out = []
    for c in cmds:
        args = [DEBUGFS] + (["-w"] if write else []) + ["-R", c, img]
        p = subprocess.run(args, capture_output=True, text=True, timeout=120)
        out.append(p.stdout + p.stderr)
    return "".join(out)


def prepare(outdir, keep_auto, full):
    fire_img = os.path.join(outdir, "probe.img")
    if os.path.exists(fire_img):
        os.unlink(fire_img)
    log(f"cold-copy {IMG} -> {fire_img}")
    r = subprocess.run(["cp", "--sparse=always", IMG, fire_img])
    if r.returncode != 0:
        sys.exit("image copy failed")
    inject = ["gsdprobe", "glibprobe"]
    if full:
        inject.append("start-probe-gnome.sh")
    for name in inject:
        src = os.path.join(HERE, name)
        debugfs(fire_img,
                f"rm /root/{name}",
                f"write {src} /root/{name}",
                f"sif /root/{name} mode 0100755", write=True)
        got = debugfs(fire_img, f"stat /root/{name}")
        if "Inode" not in got:
            sys.exit(f"injection of {name} failed: {got}")
    if not keep_auto:
        new = os.path.join(outdir, "auto.new")
        with open(new, "w") as f:
            f.write(FULL_AUTO if full else PROBE_AUTO)
        debugfs(fire_img, "rm /root/auto.sh", f"write {new} /root/auto.sh",
                "sif /root/auto.sh mode 0100644", write=True)
    return fire_img


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kernel", required=True)
    ap.add_argument("--tag", default="run")
    ap.add_argument("--timeout", type=int, default=420)
    ap.add_argument("--keep-auto", action="store_true",
                    help="keep original auto.sh (full Xorg+GNOME chain)")
    ap.add_argument("--full", action="store_true",
                    help="full scenario: Xorg + dbus + start-gnome env + real gnome-session")
    ap.add_argument("--append", default="")
    ap.add_argument("--send-dump-at", type=float, default=0,
                    help="seconds after boot to send the DUMP! uart magic")
    ap.add_argument("--send-cmd", action="append", default=[],
                    help="delay:command to type into the guest shell")
    ap.add_argument("--dump-count", type=int, default=1,
                    help="how many DUMP! bursts to send (each +90s apart)")
    args = ap.parse_args()

    outdir = os.path.join(HERE, f"gsdfire-{args.tag}")
    os.makedirs(outdir, exist_ok=True)
    fire_img = prepare(outdir, args.keep_auto, args.full)

    qmp = os.path.join(outdir, "qmp.sock")
    if os.path.exists(qmp):
        os.unlink(qmp)
    ser_path = os.path.join(outdir, "serial.log")
    ser = open(ser_path, "wb")
    append = ("root=/dev/vda rw init=/sbin/runit console=ttyS0 "
              "dfx=watchdog,periodic,taskdump " + args.append).strip()
    cmd = [QEMU, "-M", "virt", "-accel", "tcg,thread=single", "-cpu", "rv64",
           "-m", "2G", "-smp", "4", "-display", "none", "-serial", "stdio",
           "-qmp", f"unix:{qmp},server,nowait",
           "-drive", f"file={fire_img},if=none,id=rootfs,format=raw",
           "-device", "virtio-blk-pci,drive=rootfs",
           "-device", "virtio-gpu-pci",
           "-device", "virtio-keyboard-pci",
           "-device", "virtio-tablet-pci",
           "-kernel", args.kernel, "-append", append]
    with open(os.path.join(outdir, "cmdline.txt"), "w") as f:
        f.write(" ".join(cmd) + "\n")
    proc = subprocess.Popen(cmd, stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    with open(os.path.join(outdir, "qemu.pid"), "w") as f:
        f.write(str(proc.pid))
    log(f"qemu pid {proc.pid}")

    t0 = time.time()
    seen = set()
    dumps_sent = 0
    ring_pids = {}
    follow_sent = [False]
    cmds = []
    for spec in args.send_cmd:
        d, _, c = spec.partition(":")
        cmds.append((float(d), c))
    cmds_i = 0
    try:
        while time.time() - t0 < args.timeout:
            while cmds_i < len(cmds) and time.time() - t0 > cmds[cmds_i][0]:
                try:
                    proc.stdin.write(cmds[cmds_i][1].encode() + b"\n")
                    proc.stdin.flush()
                    log(f"sent cmd: {cmds[cmds_i][1]}")
                except Exception as e:
                    log(f"cmd send failed: {e}")
                cmds_i += 1
            if args.send_dump_at and dumps_sent < args.dump_count and \
               time.time() - t0 > args.send_dump_at + 90 * dumps_sent:
                try:
                    proc.stdin.write(b"DUMP!\n")
                    proc.stdin.flush()
                    log(f"sent DUMP! uart magic (#{dumps_sent + 1})")
                    dumps_sent += 1
                except Exception as e:
                    log(f"DUMP! send failed: {e}")
                    dumps_sent = args.dump_count
            line = proc.stdout.readline()
            if not line:
                log("serial EOF")
                break
            ser.write(line)
            ser.flush()
            try:
                s = line.decode(errors="replace").rstrip("\r\n")
            except Exception:
                continue
            if follow_sent[0] or any(k in s for k in STREAM_KEYS):
                print(f"  [{time.time()-t0:6.1f}s] {s}", flush=True)
            # ring-follow: count spinner pids from streamed GSD-SYS lines;
            # once the dump burst is over, query the top spinner's fd table
            if s.startswith("GSD-SYS pid="):
                parts = dict(x.split("=", 1) for x in s.split()[1:] if "=" in x)
                ring_pids[parts.get("pid", "?")] = ring_pids.get(parts.get("pid", "?"), 0) + 1
            if ring_pids and not follow_sent[0] and \
               time.time() - t0 > args.send_dump_at + 45:
                top = max(ring_pids, key=ring_pids.get)
                for c in (f"ls -l /proc/{top}/fd",):
                    try:
                        proc.stdin.write(c.encode() + b"\n")
                        proc.stdin.flush()
                        log(f"sent follow cmd: {c}")
                    except Exception:
                        pass
                follow_sent[0] = True
            if b"PROBE-GNOME: DONE" in line:
                seen.add(b"done")
            if b"GSD-PROBE-DONE" in line and not args.full:
                seen.add(b"done")
            stop_mark = b"PROBE-GNOME: DONE" if args.full else b"GSD-PROBE-DONE"
            if stop_mark in seen and (time.time() - t0) > 30:
                log("probe done; giving 10s grace then stopping")
                time.sleep(10)
                break
    finally:
        try:
            proc.stdin.write(b"\x03")
            proc.stdin.flush()
        except Exception:
            pass
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except Exception:
            proc.kill()
            proc.wait()
        ser.close()
        log(f"done in {time.time()-t0:.0f}s; serial at {ser_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
