#!/bin/bash
# fleet-member.sh — stress-fleet generic member driver (standing lane,
# s1). One member = one kind of stress surface, run in rounds with
# rotation, wedge watching, and per-round classification. Resumable:
# the master (stress-fleet.sh) or a future session relaunches it with
# the same <member> name and it continues the round numbering.
#
# Usage:   fleet-member.sh <member> <kind> [rounds] [first-round]
# Kinds:
#   ltp       riscv64 TCG smp2/2G, LTP sweep runner, random sweepstart
#   memstorm  riscv64 TCG smp2/1G, init=/test/memstorm, dfx=memwatch,watchdog
#   fsfuzz    riscv64 TCG smp2/1G, init=/test/fsfuzz, dfx=watchdog,taskdump
#   monkey    x86_64 KVM q35 4G smp2, OH image + stress-monkey churn
# Stop:     touch run/fleet/<member>.stop
#
# Per-round outputs in run/fleet/logs/:
#   <member>-r<N>-serial.log  current/past round serial
#   <member>-rounds.log       one classification line per round (append)
# Wedge rule: serial growth stalls >= WEDGE_SILENCE secs -> kill QEMU,
#             mark WEDGE with tail preserved as <member>-r<N>-wedge.log.
set -u

FLEET="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$FLEET/../.." && pwd)"
IMG="$FLEET/images"
LOGS="$FLEET/logs"
MEMBER="${1:?member name}"
KIND="${2:?kind: ltp|memstorm|fsfuzz|monkey}"
RUNS="${3:-10000}"
i="${4:-1}"
LAST=$((i + RUNS - 1))
STOP="$FLEET/$MEMBER.stop"
SECS="${SECS:-1800}"
WEDGE_SILENCE="${WEDGE_SILENCE:-300}"
NICE="${NICE:-10}"

rm -f "$STOP"
mkdir -p "$LOGS"

CMD=() # boot_cmd builds the full argv here
rv_drive() { # file id — appends shared-snapshot -drive/-device pairs to CMD
    CMD+=(-drive "file=$1,if=none,id=$2,format=raw,snapshot=on,file.locking=off"
          -device "virtio-blk-pci,disable-legacy=on,drive=$2")
}
rv_kdrive() { # private kernel-plain drive pair (no snapshot flag games)
    CMD+=(-drive "file=$1,if=none,id=$2,format=raw,file.locking=off,snapshot=on"
          -device "virtio-blk-pci,drive=$2")
}
rv_common() { # serial — the shared riscv64 front-matter
    # CPU string: zcb REQUIRED — the toolchain-built user probes
    # (musl-gcc: fsfuzz/memstorm/sweep and variants) contain Zcb
    # instructions (c.mul etc). Round-2 correction of the round-1
    # "zcb withdrawal": the SIGILLs were (a) user-mode: c.mul decoding
    # illegal without zcb — deterministic per binary, not corruption;
    # (b) kernel-mode: read_counter's write-flavored csrrw time (now
    # fixed in the kernel — read-only CSR write attempt). zcb was never
    # the fault; it was withdrawn for ~1h on a misread and is restored.
    CMD+=(nice -n "$NICE" timeout --signal=TERM --kill-after=10 "${SECS}s"
          qemu-system-riscv64 -M virt -accel tcg,thread=multi
          -cpu "rv64,zbb=true,zba=true,zbs=true,zcb=true,v=true"
          -m "${MEM:-2G}" -smp 2 -display none -monitor none
          -serial "file:$1")
}

boot_cmd() { # prints the full qemu argv, %q-quoted for exact eval round-trip
    local serial="$1" round="$2"
    CMD=()
    case "$KIND" in
    ltp)
        local start=$(( (RANDOM * 32768 + RANDOM) % 1080 ))
        rv_common "$serial"
        rv_drive "$IMG/fleet-ltp-rootfs.img" rootfs
        CMD+=(-kernel "$IMG/fleet-rv64-rux"
              -append "root=/dev/vda rw init=/test/sweep console=ttyS0 sweepstart=$start dfx=watchdog,taskdump roundtag=$MEMBER-r$round")
        ;;
    memstorm)
        rv_common "$serial"
        rv_drive "$IMG/fleet-ltp-rootfs.img" rootfs
        CMD+=(-kernel "$IMG/fleet-rv64-rux"
              -append "root=/dev/vda rw init=/test/memstorm console=ttyS0 dfx=periodic,memwatch,watchdog roundtag=$MEMBER-r$round")
        ;;
    fsfuzz)
        rv_common "$serial"
        rv_drive "$IMG/fleet-ltp-rootfs.img" rootfs
        CMD+=(-kernel "$IMG/fleet-rv64-rux"
              -append "root=/dev/vda rw init=/test/fsfuzz console=ttyS0 dfx=watchdog,taskdump roundtag=$MEMBER-r$round")
        ;;
    monkey)
        local OH=/home/william/rux-agents/oh-display/run/oh-img
        local ACC=tcg,thread=multi
        [ "${KVM:-1}" = 1 ] && ACC=kvm
        local idx=0 f
        CMD+=(nice -n "$NICE" timeout --signal=TERM --kill-after=10 "${SECS}s"
              qemu-system-x86_64 -M q35 -accel "$ACC" -cpu max -m "${MEM:-4G}" -smp 2
              -display none -vga none -device "virtio-gpu-pci,id=ohgpu,xres=720,yres=1280"
              -serial "file:$serial" -monitor none
              -kernel "$IMG/fleet-x86-bzImage" -initrd "$OH/ramdisk-x64.img"
              -append "ip=dhcp loglevel=4 console=ttyS0,115200 init=init root=/dev/ram0 rw ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio ohos.boot.sn=01234567890 ohos.required_mount.system=/dev/vdb@/usr@ext4@ro,barrier=1@wait,required ohos.required_mount.vendor=/dev/vdc@/vendor@ext4@ro,barrier=1@wait,required ${EXTRA_APPEND:-} dfx=memwatch,periodic roundtag=$MEMBER-r$round")
        for f in updater.img; do
            rv_kdrive "$OH/$f" "ohd$idx"; idx=$((idx+1))
        done
        rv_kdrive "$IMG/fleet-monkey-system.img" "ohd$idx"; idx=$((idx+1))
        for f in vendor.img sys_prod.img chip_prod.img; do
            rv_kdrive "$OH/$f" "ohd$idx"; idx=$((idx+1))
        done
        rv_kdrive "$OH/userdata-tok.img" "ohd$idx"
        CMD+=(-nic user,model=virtio-net-pci -device virtio-keyboard-pci -device virtio-tablet-pci)
        ;;
    *) echo "unknown kind $KIND" >&2; return 1 ;;
    esac
    printf '%q ' "${CMD[@]}"
    echo
}

wedge_watch() { # serial-file qemu-pid -> returns when round ends; kills on wedge
    local serial="$1" qpid="$2" last=-1 same=0
    while kill -0 "$qpid" 2>/dev/null; do
        sleep 60
        kill -0 "$qpid" 2>/dev/null || break
        local sz
        sz=$(stat -c %s "$serial" 2>/dev/null || echo 0)
        if [ "$sz" = "$last" ]; then
            same=$((same + 60))
            if [ "$same" -ge "$WEDGE_SILENCE" ]; then
                echo "WEDGE no-growth ${same}s" > "$serial.wedge"
                tail -c 8192 "$serial" > "$serial.wedgetail"
                kill -TERM "$qpid" 2>/dev/null
                return 0
            fi
        else
            same=0; last=$sz
        fi
    done
    return 0
}

while [ "$i" -le "$LAST" ]; do
    [ -f "$STOP" ] && { echo "[$MEMBER] stop file, ending after $((i-1)) rounds"; break; }
    S="$LOGS/${MEMBER}-r${i}-serial.log"
    rm -f "$S" "$S.wedge" "$S.wedgetail"
    # keep only the last 3 round serials (disk discipline)
    for old in "$LOGS/${MEMBER}-r$((i-3))-serial.log" "$LOGS/${MEMBER}-r$((i-3))-serial.log.wedgetail"; do
        rm -f "$old" "$old".* 2>/dev/null
    done

    echo "[$MEMBER] $(date +%H:%M:%S) round $i ($KIND) starting"
    eval "$(boot_cmd "$S" "$i")" &
    BOOTPID=$!
    # qemu is a child of nice/timeout; find it after a moment
    sleep 5
    QPID=$(pgrep -P "$BOOTPID" -f qemu 2>/dev/null | head -1)
    if [ -n "$QPID" ]; then
        wedge_watch "$S" "$QPID" &
        WPID=$!
        wait $BOOTPID; RC=$?
        kill "$WPID" 2>/dev/null
    else
        wait $BOOTPID; RC=$?
    fi

    # classify the finished round
    WEDGE="no"
    [ -f "$S.wedge" ] && WEDGE="yes"
    OUT=$("$FLEET/fleet-classify-round.sh" "$MEMBER" "$i" "$S" "$RC" "$WEDGE")
    echo "[$MEMBER] round $i: $OUT" | tee -a "$LOGS/${MEMBER}-rounds.log"

    i=$((i+1))
    sleep 5
done
echo "[$MEMBER] done after $((i-1)) rounds" >> "$LOGS/${MEMBER}-rounds.log"
