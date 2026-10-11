#!/usr/bin/env python3
"""gnome-sigill reproduction driver.

Shortened chain: Xorg + start-gnome.sh start immediately (auto.sh SLEEPs
removed - start-gnome.sh polls for the X0 socket itself), so the
gnome-session SIGILL window is reached minutes earlier. The kernel's
SIGILL-FORENSIC trap dump (VMA table + PTE + page words) is captured from
serial and saved.

Discipline: one QEMU, -snapshot-equivalent cold copy, file.locking=off,
kills only its own pid.
"""
import os
import re
import select
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
SRC_IMG = "/home/william/rux-agents/gnome-img/gnome.img"
DEBUGFS = "/usr/sbin/debugfs"
KERNEL = os.environ.get(
    "REPRO_KERNEL",
    os.path.join(HERE, "target/riscv64gc-unknown-none-elf/debug/rux"))


def debugfs(img, *cmds, write=False):
    out = []
    for c in cmds:
        args = [DEBUGFS] + (["-w"] if write else []) + ["-R", c, img]
        p = subprocess.run(args, capture_output=True, text=True, timeout=180)
        out.append(p.stdout + p.stderr)
    return "".join(out)


def log(msg):
    print(f"[repro {time.strftime('%H:%M:%S')}] {msg}", flush=True)


FAST_AUTO = """# /root/auto.sh - fast SIGILL repro chain
/bin/mkdir /run/user
/bin/mkdir /run/user/0
/bin/mkdir /run/dbus
/bin/mkdir /tmp/.X11-unix
BG /usr/bin/Xorg -config /root/xorg-fbdev.conf -logfile /root/Xorg.0.log :0
BG /root/start-gnome.sh
SLEEP 1800
/bin/sh
"""


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
        HERE, "runs", f"r{time.strftime('%H%M%S')}")
    timeout = int(sys.argv[2]) if len(sys.argv) > 2 else 1500
    os.makedirs(out, exist_ok=True)
    fire_img = os.path.join(out, "fire.img")

    # cold copy + patched auto.sh (no fixed sleeps) + core dumps enabled
    if os.path.exists(fire_img):
        os.unlink(fire_img)
    log(f"cold-copy {SRC_IMG} -> {fire_img}")
    if subprocess.run(["cp", "--sparse=always", SRC_IMG, fire_img]).returncode:
        sys.exit("image copy failed")
    debugfs(fire_img, "rm /tmp/.X0-lock", "rm /tmp/.X11-unix/X0", write=True)
    auto_path = "/tmp/repro-auto.sh"
    with open(auto_path, "w") as f:
        f.write(FAST_AUTO)
    debugfs(fire_img, "rm /root/auto.sh", f"write {auto_path} /root/auto.sh",
            "sif /root/auto.sh mode 0100644", write=True)
    # start-gnome.sh: enable core dumps for the whole session tree so a
    # SIGILL in gnome-session writes /core.<pid> into the image
    sg = debugfs(fire_img, "cat /root/start-gnome.sh")
    if "ulimit -c unlimited" not in sg:
        sg2 = sg.replace("export NO_AT_BRIDGE=1",
                         "export NO_AT_BRIDGE=1\nulimit -c unlimited")
        sg_path = "/tmp/repro-start-gnome.sh"
        with open(sg_path, "w") as f:
            f.write(sg2)
        debugfs(fire_img, "rm /root/start-gnome.sh",
                f"write {sg_path} /root/start-gnome.sh",
                "sif /root/start-gnome.sh mode 0100755", write=True)
        log("start-gnome.sh patched: ulimit -c unlimited")

    qmp_sock = os.path.join(out, "qmp.sock")
    ser_path = os.path.join(out, "serial.log")
    cmd = ["/usr/bin/qemu-system-riscv64", "-M", "virt",
           "-accel", "tcg,thread=multi", "-cpu", "rv64", "-m", "2G",
           "-smp", "4", "-display", "none", "-serial", "stdio",
           "-qmp", f"unix:{qmp_sock},server,nowait",
           "-drive", f"file={fire_img},if=none,id=rootfs,format=raw,"
                     f"file.locking=off",
           "-device", "virtio-blk-pci,drive=rootfs",
           "-device", "virtio-gpu-pci",
           "-device", "virtio-keyboard-pci",
           "-device", "virtio-tablet-pci",
           "-kernel", KERNEL,
           "-append", "root=/dev/vda rw init=/sbin/runit console=ttyS0 "
                      "dfx=watchdog,periodic,taskdump,mmforensics"]
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
    forensic_seen = False
    try:
        while time.time() - t0 < timeout:
            r, _, _ = select.select([proc.stdout], [], [], 1.0)
            if r:
                chunk = os.read(proc.stdout.fileno(), 65536)
                if not chunk:
                    log("qemu stdout EOF")
                    break
                buf.extend(chunk)
                ser.write(chunk)
                ser.flush()
            if b"SIGILL-FORENSIC" in bytes(buf[-40000:]) or \
                    b"trap: user SIGILL" in bytes(buf[-40000:]):
                forensic_seen = True
                # give the dump/core a few seconds to finish
                deadline = time.time() + 60
                while time.time() < deadline:
                    r, _, _ = select.select([proc.stdout], [], [], 1.0)
                    if r:
                        chunk = os.read(proc.stdout.fileno(), 65536)
                        if not chunk:
                            break
                        buf.extend(chunk)
                        ser.write(chunk)
                        ser.flush()
                    if b"SIGDEATH" in bytes(buf[-8000:]):
                        # let the core write finish (SIGDEATH lines print
                        # before do_coredump in some paths; wait more)
                        deadline = max(deadline, time.time() + 30)
                break
            if b"KERNPANIC" in bytes(buf[-8000:]) or b"Kernel panic" in bytes(buf[-8000:]):
                log("kernel panic - letting the trap dump finish")
                deadline = time.time() + 90
                while time.time() < deadline:
                    r, _, _ = select.select([proc.stdout], [], [], 1.0)
                    if r:
                        chunk = os.read(proc.stdout.fileno(), 65536)
                        if not chunk:
                            break
                        buf.extend(chunk)
                        ser.write(chunk)
                        ser.flush()
                    if b"Rux kernel panic" in bytes(buf[-4000:]) or b"=== end panic" in bytes(buf[-4000:]):
                        break
                break
        elapsed = time.time() - t0
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
    # extract the forensic block(s)
    blocks = re.findall(
        r"trap: user SIGILL.*?(?=\n(?:=== DFX|MEM heap|\Z))", data, re.S)
    with open(os.path.join(out, "forensic.txt"), "w") as f:
        f.write("\n\n=====\n\n".join(blocks))
    sigill = "trap: user SIGILL" in data
    deaths = re.findall(r"SIGDEATH: .*", data)
    with open(os.path.join(out, "deaths.txt"), "w") as f:
        f.write("\n".join(deaths))
    log(f"done: elapsed={elapsed:.0f}s sigill={sigill} "
        f"forensic_blocks={len(blocks)} deaths={len(deaths)}")
    # pull any core file the kernel wrote into the image
    if sigill:
        ls = debugfs(fire_img, "ls -l /")
        core_names = re.findall(r"(core\S*)", ls)
        for cn in core_names:
            dst = os.path.join(out, cn)
            debugfs(fire_img, f"dump /{cn} {dst}")
            if os.path.exists(dst):
                log(f"core extracted: {dst} ({os.path.getsize(dst)}B)")
    log(f"out={out}")
    return 0 if sigill else 1


if __name__ == "__main__":
    sys.exit(main())
