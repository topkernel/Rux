#!/bin/bash
# mkrootfs-x86_64-ltp.sh — x86_64 LTP-scan rootfs image for the r21+ scans.
#
# The x86_64 mirror of test/mkrootfs.sh's LTP-scan subset: toybox userland,
# the x86_64 LTP suite at /test/linux-ltp, the chunk-sweep runner at
# /test/ltpscan, the nobody/bin/daemon account database, and the same
# on-disk conventions (mkfs -O ^metadata_csum,^flex_bg, 1G fs + 256M swap
# tail). Build via the x86_64 musl SDK (toolchain/x86_64-rux-linux-musl);
# no soft-fp overlay (riscv64-only discipline).
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_ROOT"

MUSL_CC="$PROJECT_ROOT/toolchain/x86_64-rux-linux-musl/bin/musl-gcc"
LTP_OUT="$PROJECT_ROOT/userspace/linux-ltp/output-x86_64"
TOYBOX_DIR="$(ls -d "$PROJECT_ROOT"/userspace/toybox/toybox-0.* 2>/dev/null | head -1)"
IMAGE="$PROJECT_ROOT/test/rootfs-x86_64-ltp.img"

if [ ! -x "$MUSL_CC" ]; then
    echo "mkrootfs-x86_64-ltp: musl-gcc missing (run: make probes)" >&2
    exit 1
fi
if [ ! -d "$LTP_OUT/testcases/bin" ]; then
    echo "mkrootfs-x86_64-ltp: LTP output missing at $LTP_OUT (run: userspace/linux-ltp/build-x86_64.sh)" >&2
    exit 1
fi

# ---- toybox x86_64 (musl static) ----
TOYBOX_BIN="$PROJECT_ROOT/userspace/toybox/toybox-x86_64"
if [ ! -x "$TOYBOX_BIN" ] || [ "${FORCE_TOYBOX:-0}" = "1" ]; then
    if [ ! -d "$TOYBOX_DIR" ]; then
        (cd "$PROJECT_ROOT/userspace/toybox" && tar xzf toybox-*.tar.gz)
        TOYBOX_DIR="$(ls -d "$PROJECT_ROOT"/userspace/toybox/toybox-0.* 2>/dev/null | head -1)"
    fi
    echo "Building toybox (x86_64 musl)..."
    cd "$TOYBOX_DIR"
    make distclean >/dev/null 2>&1 || true
    CC="$MUSL_CC" CFLAGS="-static -O2 -U_FORTIFY_SOURCE" LDFLAGS="-static" \
        make defconfig >/dev/null 2>&1
    # Same config fixups as userspace/toybox/build-toybox.sh: disable the
    # crypt-library commands, force the shell on.
    sed -i 's/CONFIG_SU=y/# CONFIG_SU is not set/' .config
    sed -i 's/CONFIG_LOGIN=y/# CONFIG_LOGIN is not set/' .config
    sed -i 's/CONFIG_MKPASSWD=y/# CONFIG_MKPASSWD is not set/' .config
    sed -i 's/# CONFIG_SH is not set/CONFIG_SH=y/' .config
    ./generated/unstripped/kconfig -s .config >/dev/null 2>&1 || true
    fix_config_h() {
        if grep -q "#define CFG_${1} n" generated/config.h 2>/dev/null; then
            sed -i "s/#define CFG_${1} n/#define CFG_${1} 0\n#define USE_${1}(...)\n#define SKIP_${1}(...) __VA_ARGS__/" generated/config.h
        fi
    }
    fix_config_h SU
    fix_config_h LOGIN
    fix_config_h MKPASSWD
    if grep -q "#define CFG_SH 0" generated/config.h 2>/dev/null; then
        sed -i 's/#define CFG_SH 0/#define CFG_SH 1/' generated/config.h
        sed -i 's/#define USE_SH(...)/#define USE_SH(...) __VA_ARGS__/' generated/config.h
    fi
    CC="$MUSL_CC" CFLAGS="-static -O2 -U_FORTIFY_SOURCE -ffunction-sections -fdata-sections" \
        LDFLAGS="-static -Wl,--gc-sections" make -j"$(nproc)" >/dev/null
    cp toybox "$TOYBOX_BIN"
    cd "$PROJECT_ROOT"
fi

# ---- ltpscan x86_64 ----
LTPSCAN_BIN="$PROJECT_ROOT/run/ltpscan-x86_64"
"$MUSL_CC" -static -O2 -Wall -o "$LTPSCAN_BIN" "$PROJECT_ROOT/test/ltpscan/ltpscan.c"

# ---- staging tree ----
STAGING=$(mktemp -d /tmp/rux-ltp-rootfs.XXXXXX)
trap 'rm -rf "$STAGING"' EXIT

mkdir -p "$STAGING"/{bin,sbin,app,test,dev,etc,lib,proc,sys,tmp,var/log,root,home,run}
chmod 1777 "$STAGING/tmp"

cat > "$STAGING/etc/passwd" <<'PASSWD'
root:x:0:0:root:/root:/bin/sh
bin:x:1:1:bin:/bin:/sbin/nologin
daemon:x:2:2:daemon:/sbin:/sbin/nologin
nobody:x:65534:65534:nobody:/nonexistent:/sbin/nologin
PASSWD
cat > "$STAGING/etc/group" <<'GROUP'
root:x:0:
bin:x:1:
daemon:x:2:
tty:x:5:
disk:x:6:
nogroup:x:65534:
GROUP

cp "$TOYBOX_BIN" "$STAGING/bin/toybox"
chmod +x "$STAGING/bin/toybox"
ln -sf toybox "$STAGING/bin/sh"

TOYBOX_BIN_COMMANDS="base64 basename bash cat chgrp chmod chown cksum cp cut date dd df dirname \
dmesg du echo env expand false file find grep groups head hostname id install kill killall link ln \
logger logname ls md5sum mkdir mkfifo mknod mktemp mv nl nohup od paste pgrep pidof printenv printf \
ps pwd readlink realpath renice rm rmdir sed seq setsid sha1sum sha256sum sha512sum shred sleep sort \
split stat strings sync tac tail tar tee test time timeout touch true truncate tty uname uniq unlink \
uudecode uuencode wc which who whoami xargs yes"
(
    cd "$STAGING/bin"
    for cmd in $TOYBOX_BIN_COMMANDS; do
        [ "$cmd" = "sh" ] && continue
        ln -sf toybox "$cmd"
    done
)
TOYBOX_SBIN_COMMANDS="blockdev chroot freeramdisk halt hwclock losetup poweroff reboot swapoff swapon"
(
    cd "$STAGING/sbin"
    for cmd in $TOYBOX_SBIN_COMMANDS; do
        ln -sf ../bin/toybox "$cmd"
    done
)

echo "Installing LTP tests to /test/linux-ltp/..."
mkdir -p "$STAGING/test/linux-ltp"
cp -r "$LTP_OUT/"* "$STAGING/test/linux-ltp/"
chmod -R +x "$STAGING/test/linux-ltp/testcases/bin/"* 2>/dev/null || true
TEST_COUNT=$(find "$STAGING/test/linux-ltp/testcases/bin" -type f | wc -l)
echo "  Installed $TEST_COUNT test binaries"

cp "$LTPSCAN_BIN" "$STAGING/test/ltpscan"
chmod +x "$STAGING/test/ltpscan"

echo "Creating image $IMAGE (1G fs + 256M swap tail)..."
rm -f "$IMAGE"
mkfs.ext4 -q -F -O ^metadata_csum,^flex_bg -d "$STAGING" "$IMAGE" 1G
truncate -s 1280M "$IMAGE"

echo "Done: $IMAGE ($(ls -lh "$IMAGE" | awk '{print $5}')), $TEST_COUNT LTP binaries."
