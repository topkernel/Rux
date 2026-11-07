#!/bin/bash
# fbpaint-boot.sh — isolation probe for the "black host window" question.
#
# Boots the GNOME image with init=/bin/sh, runs the fbpaint probe (a
# direct write of a colour pattern into /dev/fb0) and takes a QMP
# screendump of the host console.
#
#   - a NON-black screendump  -> the kernel display chain is intact and
#     the blank screen belongs to the X/mutter side;
#   - a black screendump      -> the kernel's display path is the broken
#     link (mmap -> backing -> TRANSFER_TO_HOST_2D -> RESOURCE_FLUSH).
#
# Usage: bash scripts/test/gnome-diag/fbpaint-boot.sh [seconds]
set -u
# Repo root: walk up to the Cargo.toml+kernel marker (the one-level
# guess landed in scripts/ after the scripts/test relocation).
_d="$(cd "$(dirname "$0")" && pwd)"
while [ "$_d" != "/" ]; do
  if [ -f "$_d/Cargo.toml" ] && [ -d "$_d/kernel" ]; then break; fi
  _d="$(dirname "$_d")"
done
cd "$_d"

SECS="${1:-60}"
IMG="${IMG:-work/fbprobe.img}"
LOG=/tmp/fbpaint-console.log
QMP=/tmp/fbpaint-qmp.sock
OUT=/tmp/fbpaint-shot.ppm
rm -f "$LOG" "$QMP" "$OUT"

FIFO=/tmp/fbpaint.fifo
rm -f "$FIFO"; mkfifo "$FIFO"
qemu-system-x86_64 -M q35 -accel kvm -cpu max -m 2G -smp 8 \
  -display none -vga none -snapshot -no-reboot \
  -serial stdio < "$FIFO" \
  -qmp unix:"$QMP",server=on,wait=off \
  -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
  -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
  -device virtio-gpu-pci,xres=1280,yres=800 \
  -kernel target/x86_64-unknown-none/debug/bzImage \
  -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
  > "$LOG" 2>&1 &
QPID=$!

# Feed the probe to the guest shell once it is up (the fifo must be
# opened for writing AFTER qemu has it as its stdin).
exec 9>"$FIFO"
(
  sleep 25
  printf '/fbpaint\n'
  sleep 5
  printf 'echo FBPAINT-DONE\n'
  sleep 5
) >&9 2>/dev/null &
FEEDER=$!

# QMP screendump at the halfway mark.
(
  sleep $((SECS / 2))
  python3 - "$QMP" "$OUT" <<'PY'
import socket, sys, time
sock_path, out = sys.argv[1], sys.argv[2]
try:
    s = socket.socket(socket.AF_UNIX)
    s.connect(sock_path)
    f = s.makefile("rw", buffering=1)
    f.readline()  # greeting
    f.write('{"execute":"qmp_capabilities"}\n'); f.readline()
    f.write('{"execute":"screendump","arguments":{"filename":"%s"}}\n' % out)
    print("[fbpaint] screendump:", f.readline().strip())
    s.close()
except Exception as e:
    print("[fbpaint] screendump failed:", e)
PY
) &
DUMPER=$!

sleep "$SECS"
kill "$DUMPER" "$FEEDER" 2>/dev/null
exec 9>&-
kill "$QPID" 2>/dev/null
wait "$QPID" 2>/dev/null

echo "=== console (guest) ==="
grep -aE "FBPAINT|fbpaint" "$LOG" | head -6
if [ -f "$OUT" ]; then
  python3 - "$OUT" <<'PY'
import sys
data = open(sys.argv[1], "rb").read()
# PPM: "P6\n<w> <h>\n255\n" then RGB triples
parts = data.split(b"\n", 3)
if parts[0].strip() != b"P6":
    print("not a P6 ppm:", parts[0][:20]); raise SystemExit
w, h = map(int, parts[1].split())
px = parts[3] if len(parts) > 3 else b""
nonblack = 0
from collections import Counter
top = Counter()
for i in range(0, len(px) - 2, 3):
    c = (px[i], px[i+1], px[i+2])
    if c != (0, 0, 0):
        nonblack += 1
    top[c] += 1
print("screendump %dx%d nonblack=%d/%d (%.1f%%)" % (
    w, h, nonblack, w*h, 100.0*nonblack/(w*h)))
print("top colours:", top.most_common(4))
PY
else
  echo "no screendump produced"
fi
