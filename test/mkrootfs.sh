#!/bin/bash
# Create ext4 rootfs image containing shell and toybox

set -e

# Get project root directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# Configuration
IMAGE_FILE="$PROJECT_ROOT/test/rootfs.img"
IMAGE_SIZE="1G"
MOUNT_POINT="$PROJECT_ROOT/test/rootfs_mnt"

# Shell and tool paths
SHELL_BINARY="$PROJECT_ROOT/userspace/shell/shell"
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

# Clean up old files
echo "Cleaning up old files..."
rm -f "$IMAGE_FILE"
# If mount point exists and is mounted, unmount first
if mountpoint -q "$MOUNT_POINT" 2>/dev/null; then
    sudo umount -l "$MOUNT_POINT" 2>/dev/null || true
fi
rm -rf "$MOUNT_POINT"
mkdir -p "$MOUNT_POINT"

# Create image file
echo "Creating image file: $IMAGE_FILE ($IMAGE_SIZE)"
dd if=/dev/zero of="$IMAGE_FILE" bs=1M count=1024 2>/dev/null

# Format as ext4
echo "Formatting as ext4..."
mkfs.ext4 -F "$IMAGE_FILE" > /dev/null 2>&1

# Mount image
echo "Mounting image to $MOUNT_POINT..."
sudo mount -o loop "$IMAGE_FILE" "$MOUNT_POINT"

# Create directory structure
echo "Creating directory structure..."
sudo mkdir -p "$MOUNT_POINT/bin"
sudo mkdir -p "$MOUNT_POINT/app"
sudo mkdir -p "$MOUNT_POINT/test"
sudo mkdir -p "$MOUNT_POINT/dev"
sudo mkdir -p "$MOUNT_POINT/etc"
sudo mkdir -p "$MOUNT_POINT/lib"

# Install dynamic linker (ld-musl)
MUSL_LIB_DIR="$PROJECT_ROOT/toolchain/riscv64-rux-linux-musl/lib"
if [ -f "$MUSL_LIB_DIR/libc.so" ]; then
    echo "Installing dynamic linker to /lib/ld-musl-riscv64.so.1..."
    sudo cp "$MUSL_LIB_DIR/libc.so" "$MOUNT_POINT/lib/ld-musl-riscv64.so.1"
    sudo chmod +x "$MOUNT_POINT/lib/ld-musl-riscv64.so.1"
else
    echo "Warning: musl libc.so not found at $MUSL_LIB_DIR/libc.so"
fi
sudo mkdir -p "$MOUNT_POINT/proc"
sudo mkdir -p "$MOUNT_POINT/tmp"
sudo mkdir -p "$MOUNT_POINT/var"
sudo mkdir -p "$MOUNT_POINT/var/log"

# Install shell (musl libc)
if [ -f "$SHELL_BINARY" ]; then
    echo "Installing shell (musl libc) to /bin/shell..."
    sudo cp "$SHELL_BINARY" "$MOUNT_POINT/bin/shell"
    sudo chmod +x "$MOUNT_POINT/bin/shell"
    # Create /bin/sh symlink pointing to shell
    sudo ln -sf shell "$MOUNT_POINT/bin/sh"
else
    echo "Error: shell not found at $SHELL_BINARY"
    echo "  Run 'make shell' to build it first"
    exit 1
fi

# Copy GUI applications to /app/ directory
for app in desktop calculator clock vshell; do
    eval "binary=\$$(echo $app | tr '[:lower:]' '[:upper:]')_BINARY"
    if [ -f "$binary" ]; then
        echo "Installing $app to /app/$app..."
        sudo cp "$binary" "$MOUNT_POINT/app/$app"
        sudo chmod +x "$MOUNT_POINT/app/$app"
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
    sudo cp "$FORK_TEST_BINARY" "$MOUNT_POINT/test/smoke_test"
    sudo chmod +x "$MOUNT_POINT/test/smoke_test"
fi

# Install dynamic linking test program
DYNAMIC_HELLO="$PROJECT_ROOT/userspace/tests/dynamic_hello"
if [ -f "$DYNAMIC_HELLO" ]; then
    echo "Installing dynamic_hello to /test/dynamic_hello..."
    sudo cp "$DYNAMIC_HELLO" "$MOUNT_POINT/test/dynamic_hello"
    sudo chmod +x "$MOUNT_POINT/test/dynamic_hello"
fi

# Copy mini-ltp test suite
MINI_LTP_DIR="$PROJECT_ROOT/userspace/tests/mini-ltp/output"
if [ -d "$MINI_LTP_DIR/bin" ]; then
    echo "Installing mini-ltp tests to /test/mini-ltp/..."
    sudo mkdir -p "$MOUNT_POINT/test/mini-ltp/bin"
    sudo cp -r "$MINI_LTP_DIR/bin/"* "$MOUNT_POINT/test/mini-ltp/bin/"
    sudo cp "$MINI_LTP_DIR/run_tests.sh" "$MOUNT_POINT/test/mini-ltp/"
    sudo chmod +x "$MOUNT_POINT/test/mini-ltp/bin/"*
    sudo chmod +x "$MOUNT_POINT/test/mini-ltp/run_tests.sh"
    echo "  Installed $(ls "$MINI_LTP_DIR/bin" | wc -l) test binaries"
fi

# Copy linux-ltp test suite
LINUX_LTP_DIR="$PROJECT_ROOT/userspace/linux-ltp/output"
if [ -d "$LINUX_LTP_DIR/testcases" ]; then
    echo "Installing LTP tests to /test/linux-ltp/..."
    sudo mkdir -p "$MOUNT_POINT/test/linux-ltp"
    sudo cp -r "$LINUX_LTP_DIR/"* "$MOUNT_POINT/test/linux-ltp/"
    sudo chmod -R +x "$MOUNT_POINT/test/linux-ltp/testcases/bin/"* 2>/dev/null || true
    TEST_COUNT=$(find "$MOUNT_POINT/test/linux-ltp/testcases/bin" -type f 2>/dev/null | wc -l)
    echo "  Installed $TEST_COUNT test binaries"
fi

# Install toybox (if exists)
if [ -f "$TOYBOX_BINARY" ]; then
    echo "Installing toybox to /bin/toybox..."
    sudo cp "$TOYBOX_BINARY" "$MOUNT_POINT/bin/toybox"
    sudo chmod +x "$MOUNT_POINT/bin/toybox"

    # Create /sbin directory
    sudo mkdir -p "$MOUNT_POINT/sbin"

    # Create symlinks for all toybox commands in /bin/
    echo "Creating toybox symlinks in /bin/..."
    TOYBOX_BIN_COMMANDS="[ acpi arch ascii base32 base64 basename bash blkdiscard blkid \
bunzip2 bzcat cal cat chattr chgrp chmod chown chrt chvt cksum clear cmp comm \
count cp cpio crc32 cut date dd deallocvt df dirname dnsdomainname dos2unix du \
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
wget which who whoami xargs xxd yes zcat"
    (
        cd "$MOUNT_POINT/bin"
        for cmd in $TOYBOX_BIN_COMMANDS; do
            if [ ! -e "$cmd" ]; then
                sudo ln -sf toybox "$cmd"
            fi
        done
    )

    # Create symlinks for sbin commands in /sbin/
    echo "Creating toybox symlinks in /sbin/..."
    TOYBOX_SBIN_COMMANDS="blockdev chroot devmem freeramdisk fsfreeze halt hwclock \
i2cdetect i2cdump i2cget i2cset i2ctransfer ifconfig insmod killall5 \
losetup lsmod mkswap modinfo oneit partprobe poweroff reboot rfkill rmmod \
swapoff swapon sysctl vconfig watchdog"
    (
        cd "$MOUNT_POINT/sbin"
        for cmd in $TOYBOX_SBIN_COMMANDS; do
            if [ ! -e "$cmd" ]; then
                sudo ln -sf ../bin/toybox "$cmd"
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

# Create some basic device nodes (if mknod is available)
if command -v mknod &> /dev/null; then
    echo "Creating device nodes..."
    sudo mknod "$MOUNT_POINT/dev/console" c 5 1 2>/dev/null || true
    sudo mknod "$MOUNT_POINT/dev/null" c 1 3 2>/dev/null || true
    sudo mknod "$MOUNT_POINT/dev/zero" c 1 5 2>/dev/null || true
fi

# Display image contents
echo ""
echo "========================================"
echo "Rootfs contents:"
echo "========================================"
sudo find "$MOUNT_POINT" -type f -o -type d | sudo sort | sed 's|'$MOUNT_POINT'||'

# Get file sizes
echo ""
echo "========================================"
echo "Image statistics:"
echo "========================================"
[ -f "$SHELL_BINARY" ] && echo "Shell:      $(stat -c%s "$SHELL_BINARY" 2>/dev/null || stat -f%z "$SHELL_BINARY") bytes"
[ -f "$TOYBOX_BINARY" ] && echo "Toybox:     $(stat -c%s "$TOYBOX_BINARY" 2>/dev/null || stat -f%z "$TOYBOX_BINARY") bytes"
[ -f "$DESKTOP_BINARY" ] && echo "Desktop:    $(stat -c%s "$DESKTOP_BINARY" 2>/dev/null || stat -f%z "$DESKTOP_BINARY") bytes"
[ -f "$CALCULATOR_BINARY" ] && echo "Calculator: $(stat -c%s "$CALCULATOR_BINARY" 2>/dev/null || stat -f%z "$CALCULATOR_BINARY") bytes"
[ -f "$CLOCK_BINARY" ] && echo "Clock:      $(stat -c%s "$CLOCK_BINARY" 2>/dev/null || stat -f%z "$CLOCK_BINARY") bytes"
[ -f "$VSHELL_BINARY" ] && echo "VShell:     $(stat -c%s "$VSHELL_BINARY" 2>/dev/null || stat -f%z "$VSHELL_BINARY") bytes"
echo ""
echo "Total image size: $(stat -c%s "$IMAGE_FILE" 2>/dev/null || stat -f%z "$IMAGE_FILE") bytes"
ls -lh "$IMAGE_FILE"

# Unmount image
echo ""
echo "Unmounting image..."
cd "$PROJECT_ROOT"
sudo umount "$MOUNT_POINT"
rmdir "$MOUNT_POINT"

echo ""
echo "Rootfs image created successfully: $IMAGE_FILE"
echo ""
echo "Directory structure:"
echo "  /bin/          - shell, toybox, basic commands"
echo "  /app/          - GUI applications"
echo "  /test/         - test programs"
echo ""
echo "Available shells:"
echo "  /bin/shell     - musl libc shell (default)"
echo "  /bin/sh        - symlink to shell"
echo ""
echo "GUI applications (/app/):"
echo "  /app/desktop   - Desktop environment"
echo "  /app/calculator- Calculator"
echo "  /app/clock     - Clock"
echo "  /app/vshell    - Visual Shell"
echo ""
echo "Test programs (/test/):"
echo "  /test/smoke_test     - smoke test program"
echo "  /test/mini-ltp/      - mini-ltp kernel tests"
echo "    run: /test/mini-ltp/run_tests.sh"
echo "  /test/linux-ltp/     - official LTP tests (if built)"
echo "    run: /test/linux-ltp/run_quick.sh"
echo ""
echo "Toybox commands (via symlinks):"
echo "  /bin/  - user commands (ls, cat, grep, vi, etc.)"
echo "  /sbin/ - system commands (mount, ifconfig, halt, etc.)"
echo ""
echo "Usage:"
echo "  make run        - Run with shell"
