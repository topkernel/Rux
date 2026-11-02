#!/usr/bin/env python3
# sp-ref-u.py — reference-kernel (OHOS 6.6.101 bzImage) OH boot, A/B-matched
# to the lb300 class boots: SAME system image (system-lb5), SAME userdata
# (USERDATA env, default userdata-warm), same machine class (q35+KVM, -cpu
# max, 4G, smp2), timestamped serial. 15th successor A/B for the useriam /
# accountmgr store-load window.
import os, socket, subprocess, sys, time

TAG = sys.argv[1] if len(sys.argv) > 1 else "refu"
SECS = int(sys.argv[2]) if len(sys.argv) > 2 else 700
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.chdir(ROOT)

REF = "/home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images"
SERIAL = f"run/oh-{TAG}-serial.log"
MON = f"run/oh-{TAG}-mon.sock"
for f in (SERIAL, MON):
    try: os.unlink(f)
    except FileNotFoundError: pass

def drive(path, i):
    if not os.path.exists(path):
        return ""
    return (f" -drive file={path},if=none,id=ohd{i},format=raw,"
            f"file.locking=off,snapshot=on -device virtio-blk-pci,drive=ohd{i}")

DISKS = (drive(f"{REF}/updater.img", 0)
         + drive(os.environ.get("SYSIMG", "run/oh-img/system.img"), 1)
         + drive("run/oh-img/vendor.img", 2) + drive(f"{REF}/sys_prod.img", 3)
         + drive("run/oh-img/chip_prod.img", 4)
         + drive(os.environ.get("USERDATA", "run/userdata-fresh.img"), 5))
APPEND = ("ip=dhcp loglevel=6 console=ttyS0,115200 init=init root=/dev/ram0 rw "
          "ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio "
          "ohos.boot.sn=01234567890 "
          "ohos.required_mount.system=/dev/block/vdb@/usr@ext4@ro,barrier=1@wait,required "
          "ohos.required_mount.vendor=/dev/block/vdc@/vendor@ext4@ro,barrier=1@wait,required")

cmd = ["qemu-system-x86_64", "-M", "q35", "-accel", "kvm", "-cpu", "max",
       "-m", "4G", "-smp", os.environ.get("SMP", "2"), "-snapshot",
       "-display", "none", "-vga", "none",
       "-device", "virtio-gpu-pci,id=ohgpu,xres=720,yres=1280",
       "-serial", "stdio", "-monitor", f"unix:{MON},server=on,wait=off",
       "-kernel", f"{REF}/bzImage",
       "-initrd", "run/oh-img/ramdisk.img", "-append", APPEND] + DISKS.split() + [
       "-nic", "user,model=virtio-net-pci",
       "-device", "virtio-keyboard-pci", "-device", "virtio-tablet-pci"]

t0 = time.time()
log = open(SERIAL, "w", buffering=1)
proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

MARKERS = [
    "bootevent.bootanimation.ready",
    "bootevent.bootanimation.finished",
    "ServiceStart starting:appspawn",
    "ServiceStart starting:useriam",
    "ServiceStart starting:accountmgr",
    "bootevent.launcher.ready",
    "bootevent.launcher.finished",
    "start launcher",
    "com.ohos.launcher",
    "ExecReboot",
]
seen = {}
buf = b""
next_shot = 120

while True:
    now = time.time()
    el = now - t0
    if el > SECS or proc.poll() is not None:
        break
    if el >= next_shot:
        next_shot += 120
        try:
            m = socket.socket(socket.AF_UNIX)
            m.settimeout(5)
            m.connect(MON)
            m.sendall(f"screendump run/oh-{TAG}-shot-{int(el)}.ppm\n".encode())
            m.recv(4096)
            m.close()
        except Exception:
            pass
    try:
        chunk = proc.stdout.read1(65536)
    except Exception:
        break
    if not chunk:
        time.sleep(0.02)
        continue
    buf += chunk
    while b"\n" in buf:
        line, buf = buf.split(b"\n", 1)
        stamp = time.time() - t0
        text = line.decode("utf-8", "replace")
        log.write(f"[{stamp:9.3f}] {text}\n")
        for mk in MARKERS:
            if mk in text and "not set" not in text and mk not in seen:
                seen[mk] = stamp
                log.write(f"[{stamp:9.3f}] ===MARKER {mk}===\n")

log.write(f"===MARKERS {seen}===\n")
log.close()
try:
    proc.terminate()
    proc.wait(10)
except Exception:
    proc.kill()
