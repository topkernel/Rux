#!/bin/bash
set -u
# Repo root: walk up to the Cargo.toml+kernel marker (the one-level
# guess landed in scripts/ after the scripts/test relocation).
_d="$(cd "$(dirname "$0")" && pwd)"
while [ "$_d" != "/" ]; do
  if [ -f "$_d/Cargo.toml" ] && [ -d "$_d/kernel" ]; then break; fi
  _d="$(dirname "$_d")"
done
cd "$_d"
LOG=/tmp/plain-console.log
FIFO=/tmp/plain.fifo
rm -f "$FIFO" "$LOG"; mkfifo "$FIFO"
timeout 200 qemu-system-x86_64 -M q35 -accel kvm -cpu max -m 2G -smp 8 \
  -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file=work/gnome-x86.img,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
  -device virtio-gpu-pci,xres=1280,yres=800 -vga none \
  -kernel target/x86_64-unknown-none/debug/bzImage \
  -append "root=/dev/vda rw init=/sbin/runit console=ttyS0" \
  < "$FIFO" > "$LOG" 2>&1 &
Q=$!
exec 3>"$FIFO"
sleep 170
exec 3>&-; wait $Q 2>/dev/null; rm -f "$FIFO"
true
