#!/usr/bin/env python3
"""fm24-gnome.py — GNOME riscv64 long boot of the filemap_fault port
(wildwrite session 24).

Boots the session-24 kernel (MAP_PRIVATE file faults map page-cache
frames) on a PRIVATE cold copy of the Ubuntu 22.04 riscv64 GNOME image,
auto-starting Xorg + start-gnome.sh (riscv-gnome repro-full.py shape),
and captures serial for the full budget. The verdict question: does the
main session's fake-OOM (page allocator reports OOM at ~342MB/2G, oom
kills through the session startup) still fire?

Discipline: one QEMU, private cold copy (sparse) + file.locking=off,
kills only its own pid (SIGTERM to the QEMU process it spawned).
"""
import os
import subprocess
import sys
import time

WT = "/home/william/rux-agents/wildwrite-hunt"
SRC_IMG = "/home/william/rux-agents/oom-accounting/work/gnome-riscv.img"
DEBUGFS = "/usr/sbin/debugfs"
KERNEL = os.path.join(
    WT, "target/riscv64gc-unknown-none-elf/debug/rux")
OUT = os.path.join(WT, "run", "fm24-gnome")
BUDGET = int(sys.argv[1]) if len(sys.argv) > 1 else 2700


def debugfs(img, *cmds, write=False):
    for c in cmds:
        args = [DEBUGFS] + (["-w"] if write else []) + ["-R", c, img]
        subprocess.run(args, capture_output=True, text=True, timeout=180)


def log(msg):
    print(f"[fm24 {time.strftime('%H:%M:%S')}] {msg}", flush=True)


FAST_AUTO = """# /root/auto.sh - fast session chain (repro-full shape)
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
    os.makedirs(OUT, exist_ok=True)
    fire_img = os.path.join(OUT, "fire.img")
    if os.path.exists(fire_img):
        os.unlink(fire_img)
    log(f"cold-copy {SRC_IMG} -> {fire_img}")
    if subprocess.run(["cp", "--sparse=always", SRC_IMG, fire_img]).returncode:
        sys.exit("image copy failed")
    debugfs(fire_img, "rm /tmp/.X0-lock", "rm /tmp/.X11-unix/X0", write=True)
    auto_path = "/tmp/fm24-auto.sh"
    with open(auto_path, "w") as f:
        f.write(FAST_AUTO)
    debugfs(fire_img, "rm /root/auto.sh", f"write {auto_path} /root/auto.sh",
            "sif /root/auto.sh mode 0100644", write=True)

    ser_path = os.path.join(OUT, "serial.log")
    if os.path.exists(ser_path):
        os.remove(ser_path)
    cmd = ["/usr/bin/qemu-system-riscv64", "-M", "virt",
           "-accel", "tcg,thread=multi", "-cpu", "rv64", "-m", "2G",
           "-smp", "4", "-display", "none", "-serial", "file:" + ser_path,
           "-drive", f"file={fire_img},if=none,id=rootfs,format=raw,"
                     f"file.locking=off",
           "-device", "virtio-blk-pci,drive=rootfs",
           "-device", "virtio-gpu-pci",
           "-device", "virtio-keyboard-pci",
           "-device", "virtio-tablet-pci",
           "-kernel", KERNEL,
           "-append", "root=/dev/vda rw init=/sbin/runit console=ttyS0 "
                      "dfx=taskdump,watchdog,periodic,memwatch,ledger"]
    with open(os.path.join(OUT, "cmdline.txt"), "w") as f:
        f.write(" ".join(cmd) + "\n")
    log("qemu starting")
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)
    with open(os.path.join(OUT, "qemu.pid"), "w") as f:
        f.write(str(proc.pid))
    log(f"qemu pid {proc.pid}, budget {BUDGET}s")

    t0 = time.time()
    try:
        while time.time() - t0 < BUDGET:
            time.sleep(20)
            if proc.poll() is not None:
                log(f"qemu exited rc={proc.returncode} at "
                    f"{int(time.time() - t0)}s")
                break
    finally:
        if proc.poll() is None:
            log("budget spent - terminating own qemu")
            proc.terminate()
            try:
                proc.wait(10)
            except subprocess.TimeoutExpired:
                proc.kill()
    log(f"done, serial at {ser_path}")


if __name__ == "__main__":
    main()
