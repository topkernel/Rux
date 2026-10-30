#!/bin/bash
# cr-i128.sh — build a 128-byte-inode variant of the minimal rootfs by
# re-running test/mkrootfs.sh with a PATH shim that appends "-I 128" to
# mkfs.ext4 (s_inode_size=128 is a valid ext4 config; all fleet images
# use 256, so the read_inode/write_inode slot-clamp defects are latent
# there). The unfixed kernel should fail/EIO on every 32nd inode (last
# slot of each inode-table block); the fixed one boots normally.
#
# Usage: run/cr-i128.sh mkfs|boot <kernel> [tag]
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$ROOT/run/cr-i128-rootfs.img"
LOG="$ROOT/run/cr-i128-serial.log"
SHIM_DIR="$ROOT/run/cr-i128-shim"

case "${1:-}" in
mkfs)
    mkdir -p "$SHIM_DIR"
    cat > "$SHIM_DIR/mkfs.ext4" <<'EOF'
#!/bin/bash
exec /usr/sbin/mkfs.ext4 "$@" -I 128
EOF
    chmod +x "$SHIM_DIR/mkfs.ext4"
    # mkrootfs builds test/rootfs.img; run it under the shim, then move
    # the result to the private name and restore nothing (rootfs.img is
    # gitignored and rebuilt at will; we keep a backup copy).
    cp "$ROOT/test/rootfs.img" "$ROOT/run/cr-i128-rootfs-backup.img"
    PATH="$SHIM_DIR:$PATH" bash "$ROOT/test/mkrootfs.sh" || exit 1
    mv "$ROOT/test/rootfs.img" "$IMG"
    mv "$ROOT/run/cr-i128-rootfs-backup.img" "$ROOT/test/rootfs.img"
    dumpe2fs -h "$IMG" 2>/dev/null | grep -i "inode size"
    ;;
boot)
    K="${2:?kernel}"
    TAG="${3:-i128}"
    GUEST="$ROOT/run/cr-i128.guest"
    cat > "$GUEST" <<'EOF'
echo ===I128: start
ls -l /bin/chown
echo LS-RC:$?
echo hi > /tmp/i128-w
cat /tmp/i128-w
sync
echo I128-RC:$?
echo ===I128: done
EOF
    rm -f "$LOG" "$ROOT/run/cr-i128.fifo"
    mkfifo "$ROOT/run/cr-i128.fifo"
    setsid nice -n 10 qemu-system-riscv64 -M virt -accel tcg,thread=single \
        -cpu rv64,zbb=true,zba=true,zbs=true,zcb=true,v=true \
        -m 1G -smp 2 -display none -monitor none \
        -serial stdio \
        -drive "file=$IMG,if=none,id=rootfs,format=raw,snapshot=on,file.locking=off" \
        -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
        -kernel "$K" \
        -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
        < "$ROOT/run/cr-i128.fifo" > "$LOG" 2>&1 &
    QPID=$!
    exec 3> "$ROOT/run/cr-i128.fifo"
    sleep 45
    cat "$GUEST" >&3
    sleep 4
    exec 3>&-
    waited=0
    while kill -0 "$QPID" 2>/dev/null && [ "$waited" -lt 300 ]; do
        if grep -aq "===I128: done" "$LOG" 2>/dev/null; then
            sleep 3
            break
        fi
        sleep 2
        waited=$((waited + 2))
    done
    if kill -0 "$QPID" 2>/dev/null; then
        kill -TERM "$QPID" 2>/dev/null; sleep 2; kill -KILL "$QPID" 2>/dev/null
    fi
    wait "$QPID" 2>/dev/null
    echo "=== tag=$TAG verdict:"
    grep -a "I128" "$LOG" | head -8
    grep -aq "===I128: done" "$LOG" && echo "I128 BOOT-OK" || echo "I128 BOOT-FAIL"
    ;;
esac
