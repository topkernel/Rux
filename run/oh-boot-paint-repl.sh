#!/bin/bash
# oh-boot-paint-repl.sh — scripted interactive serial session on the
# private painter ramdisk, rdinit=/bin/sh (NO OH init). Serial is a unix
# SOCKET (input) + logfile. Python drives: inspect env, run the painter
# in background, cat its log, while the monitor takes screendumps.
# Purpose: isolate the userspace->drm->scanout chain from OH entirely.
cd /home/william/rux-agents/oh-launcher || exit 1
TAG="${1:-repl1}"
SECS="${2:-150}"
ROOT="/home/william/rux-agents/oh-launcher"
IMGDIR="$ROOT/run/oh-img"
MON="$ROOT/run/oh-$TAG-mon.sock"
SOCK="$ROOT/run/oh-$TAG-serial.sock"
SERIAL="$ROOT/run/oh-$TAG-serial.log"
for f in "$MON" "$SOCK" "$SERIAL" "$ROOT"/run/oh-$TAG-shot-*.ppm; do rm -f "$f"; done

APPEND="ip=dhcp loglevel=7 console=ttyS0,115200 rdinit=/bin/sh"
APPEND="$APPEND root=/dev/ram0 rw"

qemu_drive() { local idx=$1 file=$2; [ -f "$file" ] || return 0
  printf ' -drive file=%s,if=none,id=ohd%d,format=raw,file.locking=off,snapshot=on -device virtio-blk-pci,drive=ohd%d' "$file" "$idx" "$idx"; }
DISKS="$(qemu_drive 1 $IMGDIR/system.img)"

timeout --signal=TERM --kill-after=10 "${SECS}s" qemu-system-x86_64 \
    -M q35 -accel kvm -cpu max -m 4G -smp 2 -snapshot \
    -display none -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    -chardev socket,id=ser0,path="$SOCK",server=on,wait=off,logfile="$SERIAL" \
    -serial chardev:ser0 \
    -monitor unix:"$MON",server=on,wait=off \
    -kernel "$ROOT/target/x86_64-unknown-none/debug/bzImage" -initrd "$ROOT/run/ramdisk-paint.img" \
    -append "$APPEND" $DISKS &
QPID=$!

sleep 3
python3 - "$SOCK" "$SERIAL" "$MON" "$ROOT" "$TAG" <<'PY' &
import socket, sys, time, os
sockpath, logpath, monpath, root, tag = sys.argv[1:6]
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
for _ in range(60):
    try:
        s.connect(sockpath); break
    except Exception:
        time.sleep(1)
else:
    sys.exit(1)

def send(cmd, wait=4):
    try:
        s.sendall((cmd + "\n").encode())
    except Exception as e:
        print("send failed:", e); return
    time.sleep(wait)
    return len(open(logpath, 'rb').read())

def shot(t):
    try:
        m = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        m.connect(monpath); m.settimeout(3)
        m.sendall(f"screendump {root}/run/oh-{tag}-shot-{t}.ppm\n".encode())
        m.recv(4096); m.close()
    except Exception as e:
        print("mon:", e)

time.sleep(20)                       # boot to prompt
send("", 2)
send("echo MARKER-ALIVE; ls -la /scanpaint", 3)
send("ls -la /dev/dri /dev/console /dev/kmsg 2>&1; echo MARKER-DEV", 3)
send("/scanpaint 720 1280 > /painter.log 2>&1 &", 3)
send("sleep 8; echo MARKER-PLOG; cat /painter.log", 12)
shot(45)
send("sleep 10; cat /painter.log; echo MARKER-PLOG2", 14)
shot(65)
send("cat /proc/kmsg 2>/dev/null | tail -c 0; dmesg 2>/dev/null | tail -20; echo MARKER-DMESG", 5)
shot(80)
time.sleep(20)
shot(100)
print("REPL script done")
PY
REPLPID=$!
wait $QPID 2>/dev/null
kill $REPLPID 2>/dev/null
echo "repl boot done (log: $SERIAL)"
