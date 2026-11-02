#!/bin/bash
# wedge-stack probe: serial as SOCKET (input for the DUMP! magic) + logfile.
# Usage: EXTRA_APPEND="dfx=taskdump" run/oh-boot-dump.sh <tag> <secs>
cd /home/william/rux-agents/oh-launcher || exit 1
TAG="${1:-dump1}"
SECS="${2:-240}"
MON="run/oh-$TAG-mon.sock"
SERIALSOCK="run/oh-$TAG-serial.sock"
SERIAL="run/oh-$TAG-serial.log"
for f in "$MON" "$SERIAL" "$SERIALSOCK" run/oh-$TAG-shot-*.ppm; do rm -f "$f"; done
IMGDIR=run/oh-img
APPEND="ip=dhcp loglevel=7 console=ttyS0,115200"
APPEND="$APPEND init=init root=/dev/ram0 rw"
APPEND="$APPEND ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio"
APPEND="$APPEND ohos.boot.sn=01234567890"
APPEND="$APPEND ohos.required_mount.system=/dev/block/vdb@/usr@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ohos.required_mount.vendor=/dev/block/vdc@/vendor@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ${EXTRA_APPEND:-}"
qemu_drive() { local idx=$1 file=$2; [ -f "$file" ] || return 0
  printf ' -drive file=%s,if=none,id=ohd%d,format=raw,file.locking=off,snapshot=on -device virtio-blk-pci,drive=ohd%d' "$file" "$idx" "$idx"; }
DISKS="$(qemu_drive 0 $IMGDIR/updater.img)$(qemu_drive 1 $IMGDIR/system.img)$(qemu_drive 2 $IMGDIR/vendor.img)$(qemu_drive 3 $IMGDIR/sys_prod.img)$(qemu_drive 4 $IMGDIR/chip_prod.img)$(qemu_drive 5 run/oh-userdata-rw.img)"

timeout --signal=TERM --kill-after=10 "${SECS}s" qemu-system-x86_64 \
    -M q35 -accel kvm -cpu max -m 4G -smp 2 -snapshot \
    -display none -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    -chardev socket,id=ser0,path="$SERIALSOCK",server=on,wait=off,logfile="$SERIAL" \
    -serial chardev:ser0 \
    -monitor unix:"$MON",server=on,wait=off \
    -kernel target/x86_64-unknown-none/debug/bzImage -initrd $IMGDIR/ramdisk.img \
    -append "$APPEND" $DISKS \
    -nic user,model=virtio-net-pci \
    -device virtio-keyboard-pci -device virtio-tablet-pci &
QPID=$!

# Serial-injector: connect to the serial socket, watch the logfile, and
# when output has been quiet for ~20s (the wedge), send the DUMP! magic.
sleep 3
python3 - "$SERIALSOCK" "$SERIAL" <<'PY' &
import socket,sys,time
sockpath,logpath=sys.argv[1],sys.argv[2]
s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
for _ in range(40):
    try:
        s.connect(sockpath); break
    except Exception:
        time.sleep(1)
else:
    sys.exit(1)
last=0; stable=0
while True:
    time.sleep(5)
    try:
        n=len(open(logpath,'rb').read())
    except Exception:
        continue
    if n==last:
        stable+=1
    else:
        stable=0
    last=n
    if stable>=4 and n>10000:      # booted some, then ~20s silent
        s.sendall(b'DUMP!')
        print('sent DUMP! once', flush=True)
        time.sleep(10)
        s.sendall(b'DUMP!')
        print('sent DUMP! twice', flush=True)
        time.sleep(25)
        break
PY
INJPID=$!
wait $QPID 2>/dev/null
kill $INJPID 2>/dev/null
echo "probe done (log: $SERIAL)"
