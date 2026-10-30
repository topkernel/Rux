#!/bin/bash
# oh-boot-paint.sh — scanpaint proof boot: private ramdisk (painter +
# init shim) + timed screendumps. Images stay read-only (-snapshot).
# Usage: oh-boot-paint.sh <tag> <secs>
cd /home/william/rux-agents/oh-launcher || exit 1
TAG="${1:-paint1}"
SECS="${2:-200}"
ROOT="/home/william/rux-agents/oh-launcher"
IMGDIR="$ROOT/run/oh-img"
MON="$ROOT/run/oh-$TAG-mon.sock"
SERIAL="$ROOT/run/oh-$TAG-serial.log"
for f in "$MON" "$SERIAL" run/oh-$TAG-shot-*.ppm; do rm -f "$f"; done
USERDATA="${USERDATA:-$IMGDIR/userdata.img}"

APPEND="ip=dhcp loglevel=7 console=ttyS0,115200 rdinit=/rpaint"
APPEND="$APPEND init=init root=/dev/ram0 rw"
APPEND="$APPEND ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio"
APPEND="$APPEND ohos.boot.sn=01234567890"
APPEND="$APPEND ohos.required_mount.system=/dev/block/vdb@/usr@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ohos.required_mount.vendor=/dev/block/vdc@/vendor@ext4@ro,barrier=1@wait,required"

qemu_drive() { local idx=$1 file=$2; [ -f "$file" ] || return 0
  printf ' -drive file=%s,if=none,id=ohd%d,format=raw,file.locking=off,snapshot=on -device virtio-blk-pci,drive=ohd%d' "$file" "$idx" "$idx"; }
DISKS="$(qemu_drive 0 $IMGDIR/updater.img)$(qemu_drive 1 $IMGDIR/system.img)$(qemu_drive 2 $IMGDIR/vendor.img)$(qemu_drive 3 $IMGDIR/sys_prod.img)$(qemu_drive 4 $IMGDIR/chip_prod.img)$(qemu_drive 5 $USERDATA)"

timeout --signal=TERM --kill-after=10 "${SECS}s" qemu-system-x86_64 \
    -M q35 -accel kvm -cpu max -m 4G -smp 2 -snapshot \
    -display none -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    -serial file:"$SERIAL" \
    -monitor unix:"$MON",server=on,wait=off \
    -kernel "$ROOT/target/x86_64-unknown-none/debug/bzImage" -initrd "$ROOT/run/ramdisk-paint.img" \
    -append "$APPEND" $DISKS \
    -nic user,model=virtio-net-pci \
    -device virtio-keyboard-pci -device virtio-tablet-pci &
BOOTPID=$!

( sleep 25; python3 - "$MON" "$ROOT/run/oh-$TAG-shot-25.ppm" <<'PY'
import socket,sys
try:
    s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM); s.connect(sys.argv[1]); s.settimeout(3)
    s.sendall(f"screendump {sys.argv[2]}\n".encode()); s.recv(4096)
except Exception as e: print("mon:",e)
PY
  sleep 20; python3 - "$MON" "$ROOT/run/oh-$TAG-shot-45.ppm" <<'PY'
import socket,sys
try:
    s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM); s.connect(sys.argv[1]); s.settimeout(3)
    s.sendall(f"screendump {sys.argv[2]}\n".encode()); s.recv(4096)
except Exception as e: print("mon:",e)
PY
  sleep 25; python3 - "$MON" "$ROOT/run/oh-$TAG-shot-70.ppm" <<'PY'
import socket,sys
try:
    s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM); s.connect(sys.argv[1]); s.settimeout(3)
    s.sendall(f"screendump {sys.argv[2]}\n".encode()); s.recv(4096)
except Exception as e: print("mon:",e)
PY
  sleep 30; python3 - "$MON" "$ROOT/run/oh-$TAG-shot-100.ppm" <<'PY'
import socket,sys
try:
    s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM); s.connect(sys.argv[1]); s.settimeout(3)
    s.sendall(f"screendump {sys.argv[2]}\n".encode()); s.recv(4096)
except Exception as e: print("mon:",e)
PY
  sleep 30; python3 - "$MON" "$ROOT/run/oh-$TAG-shot-130.ppm" <<'PY'
import socket,sys
try:
    s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM); s.connect(sys.argv[1]); s.settimeout(3)
    s.sendall(f"screendump {sys.argv[2]}\n".encode()); s.recv(4096)
except Exception as e: print("mon:",e)
PY
) &
SHOTPID=$!
wait $SHOTPID 2>/dev/null
sleep 3
kill $BOOTPID 2>/dev/null
ls -la "$ROOT"/run/oh-$TAG-shot-*.ppm 2>/dev/null | awk '{print $5,$9}'
