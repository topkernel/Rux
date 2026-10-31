#!/usr/bin/env bash
# aa64-tail task-4 flake hunt: boot-loop the current kernel against the
# shared image and classify each boot. The 1-of-9 flake signature (from
# aa64-fork/run/fix2.log): boot reaches init spawn, then PF#n ... K
# KernelPanic with a trap-frame access ~0xa0 past the linear-map end
# (sp=0xffffc0007fffff80, far=0xffffc00080000020, S_X20=0xa0 offset in
# restore_and_eret). One QEMU at a time (serial loop); kill only ours
# (timeout wrapper owns each).
set -u
RUN=/home/william/rux-agents/aa64-tail/run
KERNEL="${1:-/home/william/rux-agents/aa64-tail/target/aarch64-unknown-none/debug/rux}"
N="${2:-12}"
SECS="${3:-40}"

mkdir -p "$RUN/flakeloop"
clean=0; flake=0; other=0
for i in $(seq 1 "$N"); do
    OUT="$RUN/flakeloop/boot-$i.log"
    timeout -k 5 "$SECS" qemu-system-aarch64 \
        -M virt,gic-version=3 -cpu cortex-a72 -accel tcg,thread=multi \
        -m 1G -smp 2 -nographic -no-reboot \
        -kernel "$KERNEL" \
        -drive file=$RUN/a64-rootfs.img,if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
        -device virtio-blk-pci,drive=rootfs > "$OUT" 2>&1
    if grep -aq "KernelPanic" "$OUT"; then
        flake=$((flake+1))
        echo "boot $i: FLAKE (KernelPanic) -> $OUT"
        grep -aE "PF#|far=|sp=0x|pc=0x" "$OUT" | head -4
    elif grep -aq "PARKWALK-ARMED" "$OUT" || grep -aq "first RX irq" "$OUT"; then
        clean=$((clean+1))
        echo "boot $i: clean (shell alive)"
    elif grep -aqE "Oops|panic|end trace" "$OUT"; then
        other=$((other+1))
        echo "boot $i: OTHER FAILURE -> $OUT"
        grep -aE "panic|Oops|end trace|PF#" "$OUT" | head -4
    else
        other=$((other+1))
        echo "boot $i: NO SHELL MARKER (check $OUT)"
        tail -3 "$OUT"
    fi
done
echo "=== flake loop: $N boots, clean=$clean flake=$flake other=$other ==="
