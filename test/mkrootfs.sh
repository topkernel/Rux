#!/bin/bash
# 创建包含多个 shell 的 ext4 rootfs 镜像

set -e

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# 配置
IMAGE_FILE="$PROJECT_ROOT/test/rootfs.img"
IMAGE_SIZE="64M"
MOUNT_POINT="$PROJECT_ROOT/test/rootfs_mnt"

# 三个 shell 的路径
SHELL_DEFAULT="$PROJECT_ROOT/userspace/target/riscv64gc-unknown-none-elf/release/shell"
SHELL_C="$PROJECT_ROOT/userspace/cshell/shell"
SHELL_RUST="$PROJECT_ROOT/userspace/rust-shell/target/riscv64gc-unknown-linux-musl/release/shell"
DESKTOP_BINARY="$PROJECT_ROOT/userspace/target/riscv64gc-unknown-none-elf/release/desktop"
TOYBOX_BINARY="$PROJECT_ROOT/userspace/toybox/toybox/toybox"

echo "========================================"
echo "Building ext4 rootfs image"
echo "========================================"

# 清理旧文件
echo "Cleaning up old files..."
rm -f "$IMAGE_FILE"
rm -rf "$MOUNT_POINT"
mkdir -p "$MOUNT_POINT"

# 创建镜像文件
echo "Creating image file: $IMAGE_FILE ($IMAGE_SIZE)"
dd if=/dev/zero of="$IMAGE_FILE" bs=1M count=64 2>/dev/null

# 格式化为 ext4
echo "Formatting as ext4..."
mkfs.ext4 -F "$IMAGE_FILE" > /dev/null 2>&1

# 挂载镜像
echo "Mounting image to $MOUNT_POINT..."
sudo mount -o loop "$IMAGE_FILE" "$MOUNT_POINT"

# 创建目录结构
echo "Creating directory structure..."
sudo mkdir -p "$MOUNT_POINT/bin"
sudo mkdir -p "$MOUNT_POINT/dev"
sudo mkdir -p "$MOUNT_POINT/etc"
sudo mkdir -p "$MOUNT_POINT/lib"

# 安装默认 shell (no_std Rust)
if [ -f "$SHELL_DEFAULT" ]; then
    echo "Installing default shell (no_std Rust) to /bin/shell..."
    sudo cp "$SHELL_DEFAULT" "$MOUNT_POINT/bin/shell"
    sudo chmod +x "$MOUNT_POINT/bin/shell"
    # 默认 /bin/sh 指向默认 shell
    sudo cp "$SHELL_DEFAULT" "$MOUNT_POINT/bin/sh"
    sudo chmod +x "$MOUNT_POINT/bin/sh"
else
    echo "Warning: Default shell not found at $SHELL_DEFAULT"
fi

# 安装 C shell (musl libc)
if [ -f "$SHELL_C" ]; then
    echo "Installing C shell (musl libc) to /bin/cshell..."
    sudo cp "$SHELL_C" "$MOUNT_POINT/bin/cshell"
    sudo chmod +x "$MOUNT_POINT/bin/cshell"
else
    echo "Warning: C shell not found at $SHELL_C"
fi

# 安装 Rust std shell
if [ -f "$SHELL_RUST" ]; then
    echo "Installing Rust std shell to /bin/rust-shell..."
    sudo cp "$SHELL_RUST" "$MOUNT_POINT/bin/rust-shell"
    sudo chmod +x "$MOUNT_POINT/bin/rust-shell"
else
    echo "Warning: Rust std shell not found at $SHELL_RUST"
fi

# 复制 desktop 到镜像（如果存在）
if [ -f "$DESKTOP_BINARY" ]; then
    echo "Installing desktop to /bin/desktop..."
    sudo cp "$DESKTOP_BINARY" "$MOUNT_POINT/bin/desktop"
    sudo chmod +x "$MOUNT_POINT/bin/desktop"
else
    echo "Warning: Desktop binary not found at $DESKTOP_BINARY (skipping)"
fi

# 安装 toybox（如果存在）
if [ -f "$TOYBOX_BINARY" ]; then
    echo "Installing toybox to /bin/toybox..."
    sudo cp "$TOYBOX_BINARY" "$MOUNT_POINT/bin/toybox"
    sudo chmod +x "$MOUNT_POINT/bin/toybox"

    # 创建常用命令符号链接
    echo "Creating toybox symlinks for common commands..."
    TOYBOX_COMMANDS="ls cat echo mkdir rm cp mv ln chmod chown pwd true false test date sleep head tail wc sort uniq grep sed awk tr cut basename dirname realpath touch du df free uname hostname id whoami env printenv yes tee"
    cd "$MOUNT_POINT/bin"
    for cmd in $TOYBOX_COMMANDS; do
        if [ ! -e "$cmd" ]; then
            sudo ln -sf toybox "$cmd"
        fi
    done
    echo "Toybox symlinks created for: $TOYBOX_COMMANDS"
else
    echo "Warning: Toybox binary not found at $TOYBOX_BINARY (skipping)"
    echo "  Run 'make toybox' to build toybox first"
fi

# 创建一些基本的设备节点（如果 mknod 可用）
if command -v mknod &> /dev/null; then
    echo "Creating device nodes..."
    sudo mknod "$MOUNT_POINT/dev/console" c 5 1 2>/dev/null || true
    sudo mknod "$MOUNT_POINT/dev/null" c 1 3 2>/dev/null || true
    sudo mknod "$MOUNT_POINT/dev/zero" c 1 5 2>/dev/null || true
fi

# 显示镜像内容
echo ""
echo "========================================"
echo "Rootfs contents:"
echo "========================================"
sudo find "$MOUNT_POINT" -type f -o -type d | sudo sort | sed 's|'$MOUNT_POINT'||'

# 获取文件大小
echo ""
echo "========================================"
echo "Image statistics:"
echo "========================================"
[ -f "$SHELL_DEFAULT" ] && echo "Default shell: $(stat -c%s "$SHELL_DEFAULT" 2>/dev/null || stat -f%z "$SHELL_DEFAULT") bytes"
[ -f "$SHELL_C" ] && echo "C shell:       $(stat -c%s "$SHELL_C" 2>/dev/null || stat -f%z "$SHELL_C") bytes"
[ -f "$SHELL_RUST" ] && echo "Rust std shell: $(stat -c%s "$SHELL_RUST" 2>/dev/null || stat -f%z "$SHELL_RUST") bytes"
[ -f "$DESKTOP_BINARY" ] && echo "Desktop:       $(stat -c%s "$DESKTOP_BINARY" 2>/dev/null || stat -f%z "$DESKTOP_BINARY") bytes"
[ -f "$TOYBOX_BINARY" ] && echo "Toybox:        $(stat -c%s "$TOYBOX_BINARY" 2>/dev/null || stat -f%z "$TOYBOX_BINARY") bytes"
echo ""
echo "Total image size: $(stat -c%s "$IMAGE_FILE" 2>/dev/null || stat -f%z "$IMAGE_FILE") bytes"
ls -lh "$IMAGE_FILE"

# 卸载镜像
echo ""
echo "Unmounting image..."
sudo umount "$MOUNT_POINT"
rmdir "$MOUNT_POINT"

echo ""
echo "✓ Rootfs image created successfully: $IMAGE_FILE"
echo ""
echo "Available shells:"
echo "  /bin/shell      - Default no_std Rust shell"
echo "  /bin/cshell     - C + musl libc shell"
echo "  /bin/rust-shell - Rust std shell"
echo ""
echo "Toybox commands (via symlinks):"
echo "  ls, cat, echo, mkdir, rm, cp, mv, ln, chmod, chown, pwd,"
echo "  true, false, test, date, sleep, head, tail, wc, sort, uniq,"
echo "  grep, sed, awk, tr, cut, basename, dirname, realpath, touch,"
echo "  du, df, free, uname, hostname, id, whoami, env, printenv, yes, tee"
echo ""
echo "Usage:"
echo "  make run           - Run with default shell"
echo "  make run-shell     - Run with default shell"
echo "  make run-cshell    - Run with C shell"
echo "  make run-rust-shell - Run with Rust std shell"
