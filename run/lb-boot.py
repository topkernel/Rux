#!/usr/bin/env python3
# lb-boot.py — oh-launcher timestamped boot driver (clean images, no probes).
# Same machine class as the stockperf sp-boot.py / oh-svc gc boots: q35+KVM,
# 4G/2CPU, virtio-gpu 720x1280, snapshot on every drive (userdata-fresh is
# protected; each boot pays the first-boot /data population).
# Markers track the launcher ladder: bundle install stream -> appspawn
# spawn of com.ohos.launcher -> launcher.ready.
import os, socket, subprocess, sys, time

TAG = sys.argv[1] if len(sys.argv) > 1 else "lb1"
SECS = int(sys.argv[2]) if len(sys.argv) > 2 else 1500
SYSIMG = sys.argv[3] if len(sys.argv) > 3 else "run/oh-img/system.img"
KERNEL = sys.argv[4] if len(sys.argv) > 4 else "target/x86_64-unknown-none/debug/bzImage"
EXTRA = sys.argv[5] if len(sys.argv) > 5 else ""   # extra kernel cmdline (e.g. "syscost=1")
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.chdir(ROOT)

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

DISKS = (drive("run/oh-img/updater.img", 0) + drive(SYSIMG, 1)
         + drive("run/oh-img/vendor.img", 2) + drive("run/oh-img/sys_prod.img", 3)
         + drive("run/oh-img/chip_prod.img", 4) + drive("run/userdata-fresh.img", 5))
APPEND = ("ip=dhcp loglevel=" + os.environ.get("LOGLEVEL", "7") + " console=ttyS0,115200 init=init root=/dev/ram0 rw "
          "ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio "
          "ohos.boot.sn=01234567890 "
          "ohos.required_mount.system=/dev/block/vdb@/usr@ext4@ro,barrier=1@wait,required "
          "ohos.required_mount.vendor=/dev/block/vdc@/vendor@ext4@ro,barrier=1@wait,required" + EXTRA)

cmd = ["qemu-system-x86_64", "-M", "q35", "-accel", "kvm", "-cpu", "max",
       "-m", "4G", "-smp", os.environ.get("SMP", "2"), "-snapshot", "-display", "none", "-vga", "none",
       "-device", "virtio-gpu-pci,id=ohgpu,xres=720,yres=1280",
       "-serial", "stdio", "-monitor", f"unix:{MON},server=on,wait=off",
       "-kernel", KERNEL,
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
    "bootevent.launcher.ready",
    "bootevent.launcher.finished",
    "start launcher",
    "ServiceExec launcher",
    "appspawn start com.ohos.launcher",
    "AppSpawn start com.ohos.launcher",
    "com.ohos.launcher",
    "ExecReboot",
]
seen = {}
buf = b""
next_shot = 180
SHOT_STEP = 120

while True:
    now = time.time()
    el = now - t0
    if el > SECS or proc.poll() is not None:
        break
    if el >= next_shot:
        next_shot += SHOT_STEP
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
            if mk in text and mk not in seen:
                seen[mk] = stamp
                log.write(f"[{stamp:9.3f}] ===MARKER {mk}===\n")
                log.flush()
log.close()
try:
    proc.terminate()
    proc.wait(timeout=5)
except Exception:
    proc.kill()

with open(f"run/oh-{TAG}-markers.txt", "w") as f:
    for mk, st in sorted(seen.items(), key=lambda kv: kv[1]):
        f.write(f"{st:9.3f} {mk}\n")
print("markers:")
for mk, st in sorted(seen.items(), key=lambda kv: kv[1]):
    print(f"  {st:9.3f} {mk}")
