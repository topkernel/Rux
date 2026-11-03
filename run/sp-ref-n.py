#!/usr/bin/env python3
# sp-ref-n.py — reference-kernel (OHOS 6.6.101 bzImage) OH boot with a
# WRITABLE userdata drive (NOSNAP), for post-boot /data state extraction
# (the 28th successor's bmsdb.db A/B: does the reference's initial user-0
# scan populate /data/service/el1/public/bms/bundle_manager_service/).
# Same image set as lb-boot.py rolls: system-lb5 + USERDATA (reflink copy).
import os, subprocess, sys, time

TAG = sys.argv[1] if len(sys.argv) > 1 else "refn"
SECS = int(sys.argv[2]) if len(sys.argv) > 2 else 150
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.chdir(ROOT)

REF = "/home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images"
SERIAL = f"run/oh-{TAG}-serial.log"
for f in (SERIAL,):
    try: os.unlink(f)
    except FileNotFoundError: pass

def drive(path, i):
    if not os.path.exists(path):
        return ""
    # NOSNAP: userdata (i == 5) is booted WITHOUT snapshot so its writes
    # land in the (reflink) copy for host-side debugfs extraction.
    snap = "" if (os.environ.get("NOSNAP") == "1" and i == 5) else ",snapshot=on"
    return (f" -drive file={path},if=none,id=ohd{i},format=raw,"
            f"file.locking=off{snap} -device virtio-blk-pci,drive=ohd{i}")

DISKS = (drive(f"{REF}/updater.img", 0)
         + drive(os.environ.get("SYSIMG", "run/system-lb5.img"), 1)
         + drive(os.environ.get("VENDOR", "run/oh-img/vendor.img"), 2)
         + drive(f"{REF}/sys_prod.img", 3)
         + drive(os.environ.get("CHIP", "run/oh-img/chip_prod.img"), 4)
         + drive(os.environ.get("USERDATA", "run/userdata-fresh2.img"), 5))
APPEND = ("ip=dhcp loglevel=" + os.environ.get("LOGLEVEL", "7") + " console=ttyS0,115200 init=init root=/dev/ram0 rw "
          "ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio "
          "ohos.boot.sn=01234567890 "
          "ohos.required_mount.system=/dev/block/vdb@/usr@ext4@ro,barrier=1@wait,required "
          "ohos.required_mount.vendor=/dev/block/vdc@/vendor@ext4@ro,barrier=1@wait,required")

cmd = ["qemu-system-x86_64", "-M", "q35", "-accel", "kvm", "-cpu", "max",
       "-m", "4G", "-smp", os.environ.get("SMP", "2"),
       "-display", "none", "-vga", "none",
       "-device", "virtio-gpu-pci,id=ohgpu,xres=720,yres=1280",
       "-serial", "stdio", "-monitor", "none",
       "-kernel", f"{REF}/bzImage",
       "-initrd", "run/oh-img/ramdisk.img", "-append", APPEND] + DISKS.split() + [
       "-nic", "user,model=virtio-net-pci",
       "-device", "virtio-keyboard-pci", "-device", "virtio-tablet-pci"]
# The global -snapshot flag snapshots EVERY drive regardless of the
# per-drive setting — gate it on NOSNAP (lb-boot.py lesson).
if os.environ.get("NOSNAP") != "1":
    cmd.append("-snapshot")

t0 = time.time()
log = open(SERIAL, "w", buffering=1)
proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
while True:
    el = time.time() - t0
    if el > SECS or proc.poll() is not None:
        break
    try:
        chunk = proc.stdout.read1(65536)
    except Exception:
        break
    if not chunk:
        time.sleep(0.02)
        continue
    log.buffer.write(chunk)
    log.flush()
log.close()
try:
    proc.terminate()
    proc.wait(10)
except Exception:
    proc.kill()
