#!/bin/bash
# Create ext4 rootfs image containing mrsh, toybox and the test programs.
#
# Population is done via `mkfs.ext4 -d <staging>` (e2fsprogs >= 1.43), so the
# script needs no root privileges and no loop mounts. Device nodes are not
# created: the kernel mounts devfs over /dev early in boot, so image-level
# nodes were never reachable anyway.

set -euo pipefail

# Get project root directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# Configuration
IMAGE_FILE="$PROJECT_ROOT/test/rootfs.img"
IMAGE_SIZE="1G"
STAGING=$(mktemp -d /tmp/rux-rootfs.XXXXXX)

# Always remove the staging tree on exit (review TEST-M8: no trap used to
# leave rootfs_mnt/ and temp files behind on failure).
trap 'rm -rf "$STAGING"' EXIT

# Tool paths
MRSH_BINARY="$PROJECT_ROOT/userspace/mrsh/mrsh/mrsh"
USERSPACE_TARGET="$PROJECT_ROOT/userspace/target/riscv64gc-unknown-linux-musl/release"
TOYBOX_BINARY="$PROJECT_ROOT/userspace/toybox/toybox/toybox"

# GUI applications
DESKTOP_BINARY="$USERSPACE_TARGET/desktop"
CALCULATOR_BINARY="$USERSPACE_TARGET/calculator"
CLOCK_BINARY="$USERSPACE_TARGET/clock"
VSHELL_BINARY="$USERSPACE_TARGET/vshell"

echo "========================================"
echo "Building ext4 rootfs image"
echo "========================================"

# Create directory structure
echo "Preparing staging tree: $STAGING"
mkdir -p "$STAGING/bin" "$STAGING/app" "$STAGING/test" "$STAGING/dev" \
         "$STAGING/etc" "$STAGING/lib" "$STAGING/proc" "$STAGING/tmp" \
         "$STAGING/var/log" "$STAGING/sbin"

# Install dynamic linker (ld-musl)
MUSL_LIB_DIR="$PROJECT_ROOT/toolchain/riscv64-rux-linux-musl/lib"
if [ -f "$MUSL_LIB_DIR/libc.so" ]; then
    echo "Installing dynamic linker to /lib/ld-musl-riscv64.so.1..."
    cp "$MUSL_LIB_DIR/libc.so" "$STAGING/lib/ld-musl-riscv64.so.1"
    chmod +x "$STAGING/lib/ld-musl-riscv64.so.1"
else
    echo "Warning: musl libc.so not found at $MUSL_LIB_DIR/libc.so"
fi

# Create /etc/mrshrc (sourced by mrsh interactive shells via ENV)
cat <<'MRSHRC' > "$STAGING/etc/mrshrc"
alias ls='ls --color=auto'
alias ll='ls -l --color=auto'
alias help='toybox --help'
MRSHRC

# Copy GUI applications to /app/ directory
for app in desktop calculator clock vshell; do
    eval "binary=\$$(echo $app | tr '[:lower:]' '[:upper:]')_BINARY"
    if [ -f "$binary" ]; then
        echo "Installing $app to /app/$app..."
        cp "$binary" "$STAGING/app/$app"
        chmod +x "$STAGING/app/$app"
    else
        echo "Warning: $app binary not found at $binary (skipping)"
    fi
done

# Copy test programs to /test/ directory
FORK_TEST_BINARY="$USERSPACE_TARGET/smoke_test"
if [ ! -f "$FORK_TEST_BINARY" ]; then
    FORK_TEST_BINARY="$PROJECT_ROOT/userspace/target/riscv64gc-unknown-linux-musl/debug/smoke_test"
fi
if [ -f "$FORK_TEST_BINARY" ]; then
    echo "Installing smoke_test to /test/smoke_test..."
    cp "$FORK_TEST_BINARY" "$STAGING/test/smoke_test"
    chmod +x "$STAGING/test/smoke_test"
fi

# Install dynamic linking test program
DYNAMIC_LINK_TEST="$PROJECT_ROOT/userspace/tests/smoke_test/dynamic_link_test"
if [ -f "$DYNAMIC_LINK_TEST" ]; then
    echo "Installing dynamic_link_test to /test/dynamic_link_test..."
    cp "$DYNAMIC_LINK_TEST" "$STAGING/test/dynamic_link_test"
    chmod +x "$STAGING/test/dynamic_link_test"
fi

# Build + install the loopback network E2E test (freestanding, raw
# syscalls — no libc, so it only needs the system RISC-V cross-gcc, not
# the musl SDK). Compiled here so the image always carries a fresh build.
NETTEST_SRC="$PROJECT_ROOT/test/nettest.c"
if [ -f "$NETTEST_SRC" ] && command -v riscv64-linux-gnu-gcc &> /dev/null; then
    echo "Installing nettest to /test/nettest..."
    if riscv64-linux-gnu-gcc -nostdlib -nostartfiles -static -O2 -fno-builtin \
         -o "$STAGING/test/nettest" "$NETTEST_SRC"; then
        chmod +x "$STAGING/test/nettest"
    else
        echo "Warning: nettest failed to compile (skipping)"
        rm -f "$STAGING/test/nettest"
    fi
else
    echo "Warning: riscv64-linux-gnu-gcc not found, skipping nettest"
fi

# Build + install the slirp virtio-net E2E test (same freestanding recipe
# as nettest; exercises the REAL virtio-net MMIO device against QEMU user
# networking — see docs/test/testing.md for the QEMU invocation).
SLIRP_TEST_SRC="$PROJECT_ROOT/test/slirp_test.c"
if [ -f "$SLIRP_TEST_SRC" ] && command -v riscv64-linux-gnu-gcc &> /dev/null; then
    echo "Installing slirp_test to /test/slirp_test..."
    if riscv64-linux-gnu-gcc -nostdlib -nostartfiles -static -O2 -fno-builtin \
         -o "$STAGING/test/slirp_test" "$SLIRP_TEST_SRC"; then
        chmod +x "$STAGING/test/slirp_test"
    else
        echo "Warning: slirp_test failed to compile (skipping)"
        rm -f "$STAGING/test/slirp_test"
    fi
fi

# Copy linux-ltp test suite
LINUX_LTP_DIR="$PROJECT_ROOT/userspace/linux-ltp/output"
if [ -d "$LINUX_LTP_DIR/testcases" ]; then
    echo "Installing LTP tests to /test/linux-ltp/..."
    mkdir -p "$STAGING/test/linux-ltp"
    cp -r "$LINUX_LTP_DIR/"* "$STAGING/test/linux-ltp/"
    chmod -R +x "$STAGING/test/linux-ltp/testcases/bin/"* 2>/dev/null || true
    TEST_COUNT=$(find "$STAGING/test/linux-ltp/testcases/bin" -type f 2>/dev/null | wc -l)
    echo "  Installed $TEST_COUNT test binaries"
fi

# Install mrsh as /bin/sh (POSIX-compliant shell)
if [ -f "$MRSH_BINARY" ]; then
    echo "Installing mrsh to /bin/sh..."
    cp "$MRSH_BINARY" "$STAGING/bin/mrsh"
    chmod +x "$STAGING/bin/mrsh"
    # Force mrsh as /bin/sh (overwriting toybox's sh symlink)
    ln -sf mrsh "$STAGING/bin/sh"
    echo "  mrsh installed as /bin/sh (POSIX shell)"
else
    echo "Warning: mrsh binary not found at $MRSH_BINARY (skipping)"
fi

# Install toybox (if exists)
if [ -f "$TOYBOX_BINARY" ]; then
    echo "Installing toybox to /bin/toybox..."
    cp "$TOYBOX_BINARY" "$STAGING/bin/toybox"
    chmod +x "$STAGING/bin/toybox"

    # Create symlinks for all toybox commands in /bin/
    echo "Creating toybox symlinks in /bin/..."
    TOYBOX_BIN_COMMANDS="[ acpi arch ascii base32 base64 basename bash blkdiscard blkid \
bunzip2 bzcat cal cat chattr chgrp chmod chown chrt chvt cksum clear cmp comm \
count cp cpio crc32 cut date dd deallocvt df dirname dmesg dnsdomainname dos2unix du \
echo egrep eject env expand factor fallocate false fgrep file find flock fmt fold \
free fstype fsync ftpget ftpput getconf getopt gpiodetect gpiofind gpioget gpioinfo \
gpioset grep groups gunzip hd head help hexedit host hostname httpd iconv id \
inotifyd install ionice iorenice iotop kill killall link linux32 ln logger \
logname losetup ls lsattr lspci lsusb makedevs mcookie md5sum memeater microcom \
mix mkdir mkfifo mknod mktemp mount mountpoint mv nbd-client nbd-server nc netcat \
netstat nice nl nohup nologin nproc nsenter od openvt paste patch pgrep pidof \
ping ping6 pivot_root pkill pmap poweroff printenv printf prlimit ps pwd pwdx \
pwgen readahead readelf readlink realpath reboot renice reset rev rfkill rm rmdir \
rmmod rtcwake sed seq setfattr setsid sh sha1sum sha224sum sha256sum sha384sum \
sha3sum sha512sum shred shuf sleep sntp sort split stat strings swapoff swapon \
switch_root sync sysctl tac tail tar taskset tee test time timeout top touch toysh \
true truncate ts tsort tty tunctl uclampset ulimit umount uname unicode uniq \
unix2dos unlink unshare uptime usleep uudecode uuencode uuidgen vmstat w watch \
wc wget which who whoami xargs xxd yes zcat"
    (
        cd "$STAGING/bin"
        for cmd in $TOYBOX_BIN_COMMANDS; do
            # Force create symlinks for shell commands (sh, bash, toysh)
            case "$cmd" in
                sh) ;; # sh is provided by mrsh, skip
                bash|toysh) ln -sf toybox "$cmd" ;;
                *) [ ! -e "$cmd" ] && ln -sf toybox "$cmd" ;;
            esac
        done
    )

    # Create symlinks for sbin commands in /sbin/
    echo "Creating toybox symlinks in /sbin/..."
    TOYBOX_SBIN_COMMANDS="blockdev chroot devmem freeramdisk fsfreeze halt hwclock \
i2cdetect i2cdump i2cget i2cset i2ctransfer ifconfig insmod killall5 \
losetup lsmod mkswap modinfo oneit partprobe poweroff reboot rfkill rmmod \
swapoff swapon sysctl vconfig watchdog"
    (
        cd "$STAGING/sbin"
        for cmd in $TOYBOX_SBIN_COMMANDS; do
            if [ ! -e "$cmd" ]; then
                ln -sf ../bin/toybox "$cmd"
            fi
        done
    )

    BIN_COUNT=$(echo $TOYBOX_BIN_COMMANDS | wc -w)
    SBIN_COUNT=$(echo $TOYBOX_SBIN_COMMANDS | wc -w)
    echo "Toybox symlinks created: $BIN_COUNT in /bin/, $SBIN_COUNT in /sbin/"
else
    echo "Warning: Toybox binary not found at $TOYBOX_BINARY (skipping)"
    echo "  Run 'make toybox' to build toybox first"
fi

# Build the image: mkfs.ext4 -d populates it from the staging tree in one
# step — no loop mount, no root privileges.
echo "Creating image $IMAGE_FILE ($IMAGE_SIZE) from staging tree..."
rm -f "$IMAGE_FILE"
mkfs.ext4 -q -F -O ^metadata_csum,^flex_bg -d "$STAGING" "$IMAGE_FILE" "$IMAGE_SIZE"

# Display image contents summary
echo ""
echo "========================================"
echo "Rootfs image created: $IMAGE_FILE"
echo "========================================"
debugfs -R "ls -l /test" "$IMAGE_FILE" 2>/dev/null | awk 'NF > 7 {print "  /test/"$8, "("$6" bytes)"}'
echo ""
echo "Total image size: $(stat -c%s "$IMAGE_FILE") bytes"
ls -lh "$IMAGE_FILE"

echo ""
echo "Directory structure:"
echo "  /bin/          - mrsh (as sh), toybox, basic commands"
echo "  /app/          - GUI applications"
echo "  /test/         - test programs"
echo ""
echo "Test programs (/test/):"
echo "  /test/smoke_test        - kernel smoke test"
echo "  /test/dynamic_link_test - dynamic linking test"
echo "  /test/nettest           - loopback network E2E test (UDP/TCP echo)"
echo "  /test/slirp_test        - virtio-net slirp E2E test (real NIC traffic)"
echo "  /test/linux-ltp/        - official LTP tests (if built)"
echo "    run: /test/linux-ltp/run_quick.sh"
