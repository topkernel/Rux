#!/usr/bin/env bash
# aa64-tail task-5 smoke: inject the cross-built LTP subset into a COPY
# of the shared a64-rootfs and run it from the serial-fed PID-1 shell.
# Copy + debugfs inject (aarch64-port/run/aa64-sigtest.sh pattern);
# boot discipline: -smp 2 -m 1G, snapshot+locking-off on the COPY.
set -u
RUN=/home/william/rux-agents/aa64-tail/run
STAGE="$RUN/ltp-aa64/disk-root/ltp"
IMG="$RUN/a64-ltpsmoke.img"
KERNEL="${1:-/home/william/rux-agents/aa64-tail/target/aarch64-unknown-none/debug/rux}"
OUT="${2:-$RUN/ltpsmoke-serial.log}"

# The runner's test list (must match run.sh in aa64-ltp-build.sh).
TESTS="getpid01 getpid02 fork01 write01 write05 read01 read02 open01 open02 \
close01 close02 pipe01 pipe02 mkdir01 pipe02 rmdir01 unlink01 unlink05 \
stat01 stat02 lseek01 lseek02 dup01 dup02 dup203 mmap01 brk01 \
nanosleep01 nanosleep02 wait01 wait02 exit01 getuid01 geteuid01 \
getgid01 getegid01 getppid01"

if [ ! -f "$IMG" ] || [ "${REINJECT:-0}" = "1" ]; then
    echo "[ltp-smoke] building $IMG (copy + inject)"
    cp /home/william/rux-images/aarch64/a64-rootfs.img "$IMG"
    CMDS="$RUN/ltp-aa64/debugfs.cmds"
    {
        echo "mkdir /ltp"
        for t in $TESTS; do
            [ -f "$STAGE/$t" ] || continue
            echo "write $STAGE/$t /ltp/$t"
            echo "sif /ltp/$t mode 0100755"
        done
        echo "write $STAGE/run.sh /ltp/run.sh"
        echo "sif /ltp/run.sh mode 0100755"
    } > "$CMDS"
    debugfs -w -f "$CMDS" "$IMG" > "$RUN/ltp-aa64/inject.log" 2>&1
    echo "[ltp-smoke] injected: $(grep -c 'Allocated' "$RUN/ltp-aa64/inject.log" 2>/dev/null || echo '?') files"
fi

( sleep 28
  printf '/ltp/run.sh\n'
  sleep 90
) | timeout -k 5 150 qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu cortex-a72 -accel tcg,thread=multi \
    -m 1G -smp 2 -nographic -no-reboot \
    -kernel "$KERNEL" \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
    -device virtio-blk-pci,drive=rootfs > "$OUT" 2>&1
rc=$?
echo "qemu rc=$rc (124 = timeout, expected)"
echo "--- LTP smoke lines ---"
grep -aE "LTPSMK" "$OUT" | head -45
echo "--- serial tail ---"
tail -5 "$OUT"
