#!/bin/bash
# 创建包含 shell 和 toybox 的 ext4 rootfs 镜像

set -e

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# 配置
IMAGE_FILE="$PROJECT_ROOT/test/rootfs.img"
IMAGE_SIZE="64M"
MOUNT_POINT="$PROJECT_ROOT/test/rootfs_mnt"

# Shell 和工具的路径
SHELL_BINARY="$PROJECT_ROOT/userspace/shell/shell"
DESKTOP_BINARY="$PROJECT_ROOT/userspace/target/riscv64gc-unknown-none-elf/release/desktop"
TOYBOX_BINARY="$PROJECT_ROOT/userspace/toybox/toybox/toybox"

echo "========================================"
echo "Building ext4 rootfs image"
echo "========================================"

# 清理旧文件
echo "Cleaning up old files..."
rm -f "$IMAGE_FILE"
# 如果挂载点存在且已挂载，先卸载
if mountpoint -q "$MOUNT_POINT" 2>/dev/null; then
    sudo umount -l "$MOUNT_POINT" 2>/dev/null || true
fi
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

# 安装 shell (musl libc)
if [ -f "$SHELL_BINARY" ]; then
    echo "Installing shell (musl libc) to /bin/shell..."
    sudo cp "$SHELL_BINARY" "$MOUNT_POINT/bin/shell"
    sudo chmod +x "$MOUNT_POINT/bin/shell"
    # 创建 /bin/sh 符号链接指向 shell
    sudo ln -sf shell "$MOUNT_POINT/bin/sh"
else
    echo "Error: shell not found at $SHELL_BINARY"
    echo "  Run 'make shell' to build it first"
    exit 1
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
    (
        cd "$MOUNT_POINT/bin"
        for cmd in $TOYBOX_COMMANDS; do
            if [ ! -e "$cmd" ]; then
                sudo ln -sf toybox "$cmd"
            fi
        done
    )
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
[ -f "$SHELL_BINARY" ] && echo "Shell:       $(stat -c%s "$SHELL_BINARY" 2>/dev/null || stat -f%z "$SHELL_BINARY") bytes"
[ -f "$DESKTOP_BINARY" ] && echo "Desktop:       $(stat -c%s "$DESKTOP_BINARY" 2>/dev/null || stat -f%z "$DESKTOP_BINARY") bytes"
[ -f "$TOYBOX_BINARY" ] && echo "Toybox:        $(stat -c%s "$TOYBOX_BINARY" 2>/dev/null || stat -f%z "$TOYBOX_BINARY") bytes"
echo ""
echo "Total image size: $(stat -c%s "$IMAGE_FILE" 2>/dev/null || stat -f%z "$IMAGE_FILE") bytes"
ls -lh "$IMAGE_FILE"

# 卸载镜像
echo ""
echo "Unmounting image..."
cd "$PROJECT_ROOT"
sudo umount "$MOUNT_POINT"
rmdir "$MOUNT_POINT"

echo ""
echo "Rootfs image created successfully: $IMAGE_FILE"
echo ""
echo "Available shells:"
echo "  /bin/shell     - musl libc shell (default)"
echo "  /bin/sh        - symlink to shell"
echo ""
echo "Toybox commands (via symlinks):"
echo "  ls, cat, echo, mkdir, rm, cp, mv, ln, chmod, chown, pwd,"
echo "  true, false, test, date, sleep, head, tail, wc, sort, uniq,"
echo "  grep, sed, awk, tr, cut, basename, dirname, realpath, touch,"
echo "  du, df, free, uname, hostname, id, whoami, env, printenv, yes, tee"
echo ""
echo "Usage:"
echo "  make run        - Run with shell"
