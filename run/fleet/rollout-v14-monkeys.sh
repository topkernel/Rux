#!/bin/bash
# rollout-v14-monkeys.sh — session 8: roll the x86 monkey pair from v13
# onto v14 (v13 + the x86 single-page file-fault negative-read_at fix).
#
# Scheduling: v13 carries the KILLDUMP stack-freed fix whose verdict is
# "monkey rounds reaching HEAP-GROW >500MB survive panic-free". So this
# roll WAITS until both monkeys have at least one classified v13 round
# (round >= 43 for both) that either (a) reached HEAP-GROW >500MB
# panic-free (decisive negative -> move on to v14) or (b) KERNPANIC'd
# (fix failed -> v14's extra fix does not matter, swap anyway for the
# fresh pstore). Hard deadline 90 min so the session is not held hostage
# by small-heap rounds; at the deadline the swap happens regardless.
set -u
cd "$(dirname "$0")/../.." || exit 1
LOG=run/fleet/logs/rollout-v14-monkeys.log
V14X=$PWD/run/fleet/images/fleet-x86-v14-bzImage

next_round() {
    grep -E "round [0-9]+: R[0-9]+" "run/fleet/logs/$1-rounds.log" 2>/dev/null \
        | tail -1 | sed -n 's/.*round \([0-9]*\): R[0-9]*.*/\1/p'
}
wait_floor() {
    for i in $(seq 1 60); do
        [ "$(free -g | awk 'NR==2{print $7}')" -ge 12 ] && return 0
        sleep 30
    done
    return 1
}

echo "[$(date '+%F %T')] v14 monkey roll start (waiting on v13 verdict round), image=$V14X" >> "$LOG"
[ -f "$V14X" ] || { echo "v14 x86 image missing" >> "$LOG"; exit 1; }

# ---- wait for the v13 verdict condition ----
deadline=$(( $(date +%s) + 5400 ))
while :; do
    va=$(grep -E "round (4[3-9]|[5-9][0-9]): R[0-9]+" run/fleet/logs/monkey-a-rounds.log 2>/dev/null)
    vb=$(grep -E "round (4[3-9]|[5-9][0-9]): R[0-9]+" run/fleet/logs/monkey-b-rounds.log 2>/dev/null)
    big_a=$(echo "$va" | grep -cE "HEAP-GROW=[5-9][0-9]{5,}KiB")
    big_b=$(echo "$vb" | grep -cE "HEAP-GROW=[5-9][0-9]{5,}KiB")
    panics=$(echo "$va
$vb" | grep -c "KERNPANIC")
    if [ "$panics" -gt 0 ]; then
        echo "[$(date '+%H:%M:%S')] v13 verdict round PANICKED — fix failed; rolling v14 anyway" >> "$LOG"
        break
    fi
    if [ "$big_a" -ge 1 ] && [ "$big_b" -ge 1 ]; then
        echo "[$(date '+%H:%M:%S')] v13 verdict: both monkeys crossed HEAP-GROW>500MB panic-free" >> "$LOG"
        break
    fi
    if [ "$(date +%s)" -ge "$deadline" ]; then
        echo "[$(date '+%H:%M:%S')] 90-min deadline reached (big_a=$big_a big_b=$big_b); swapping anyway" >> "$LOG"
        break
    fi
    sleep 60
done

wait_floor || { echo "[$(date +%H:%M:%S)] floor not clear; aborting" >> "$LOG"; exit 1; }
XS=run/fleet/logs/verify-v14x-serial.log
rm -f "$XS"
OH=/home/william/rux-agents/oh-display/run/oh-img
setsid nice -n 10 timeout --signal=TERM --kill-after=10 300s \
    qemu-system-x86_64 -M q35 -accel kvm -cpu max -m 4G -smp 2 \
    -display none -vga none -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    -serial file:"$XS" -monitor none \
    -kernel "$V14X" -initrd "$OH/ramdisk-x64.img" \
    -append "ip=dhcp loglevel=4 console=ttyS0,115200 init=init root=/dev/ram0 rw ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio ohos.boot.sn=01234567890 ohos.required_mount.system=/dev/vdb@/usr@ext4@ro,barrier=1@wait,required ohos.required_mount.vendor=/dev/vdc@/vendor@ext4@ro,barrier=1@wait,required dfx=memwatch,periodic roundtag=verify-v14x" \
    -drive file=$OH/updater.img,if=none,id=ohd0,format=raw,snapshot=on,file.locking=off -device virtio-blk-pci,drive=ohd0 \
    -drive file=run/fleet/images/fleet-monkey-system.img,if=none,id=ohd1,format=raw,snapshot=on,file.locking=off -device virtio-blk-pci,drive=ohd1 \
    -drive file=$OH/vendor.img,if=none,id=ohd2,format=raw,snapshot=on,file.locking=off -device virtio-blk-pci,drive=ohd2 \
    -drive file=$OH/sys_prod.img,if=none,id=ohd3,format=raw,snapshot=on,file.locking=off -device virtio-blk-pci,drive=ohd3 \
    -drive file=$OH/chip_prod.img,if=none,id=ohd4,format=raw,snapshot=on,file.locking=off -device virtio-blk-pci,drive=ohd4 \
    -drive file=$OH/userdata-tok.img,if=none,id=ohd5,format=raw,snapshot=on,file.locking=off -device virtio-blk-pci,drive=ohd5 \
    -nic user,model=virtio-net-pci -device virtio-keyboard-pci -device virtio-tablet-pci \
    >/dev/null 2>&1 < /dev/null &
echo "[$(date +%H:%M:%S)] v14 x86 verify boot launched" >> "$LOG"
sleep 310
XB=$(grep -c "MONKEY-BEAT" "$XS" 2>/dev/null || true)
XP=$(grep -cE "KERNPANIC|PANIC: trap" "$XS" 2>/dev/null || true)
echo "[$(date +%H:%M:%S)] v14 x86 verify: beats=$XB panics=$XP" >> "$LOG"
if [ "$XB" -lt 5 ] || [ "$XP" -gt 0 ]; then
    echo "[$(date +%H:%M:%S)] v14 x86 GATE FAIL — monkeys stay on v13-x86" >> "$LOG"
    exit 1
fi
echo "[$(date +%H:%M:%S)] GATE PASS — rolling v14-x86 to monkey-a + monkey-b" >> "$LOG"

cp -f "$V14X" run/fleet/images/fleet-x86-bzImage
echo "[$(date +%H:%M:%S)] default x86 image := v14" >> "$LOG"

relaunch_on_v14x() { # member
    local m=$1
    touch run/fleet/$m.stop
    for i in $(seq 1 200); do
        pgrep -f "fleet-member.sh $m[ ]" >/dev/null || break
        sleep 10
    done
    if pgrep -f "fleet-member.sh $m[ ]" >/dev/null; then
        echo "[$(date +%H:%M:%S)] $m did not exit in time; skipping" >> "$LOG"
        rm -f run/fleet/$m.stop
        return 1
    fi
    rm -f run/fleet/$m.stop
    wait_floor || { echo "[$(date +%H:%M:%S)] floor not clear for $m; skipping" >> "$LOG"; return 1; }
    ./run/fleet/stress-fleet.sh start $m >> "$LOG" 2>&1
    echo "[$(date +%H:%M:%S)] $m relaunched on v14-x86" >> "$LOG"
}

relaunch_on_v14x monkey-a
relaunch_on_v14x monkey-b
echo "[$(date +%H:%M:%S)] v14-x86 monkey roll complete — dirty-page fix live on the OH storm surface" >> "$LOG"
