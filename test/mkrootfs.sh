#!/bin/bash
# 创建包含 shell 的 ext4 rootfs 镜像

set -e

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# 配置
IMAGE_FILE="$PROJECT_ROOT/test/rootfs.img"
IMAGE_SIZE="64M"  # 增大镜像以容纳 desktop
SHELL_BINARY="$PROJECT_ROOT/userspace/target/riscv64gc-unknown-none-elf/release/shell"
DESKTOP_BINARY="$PROJECT_ROOT/userspace/target/riscv64gc-unknown-none-elf/release/desktop"
MOUNT_POINT="$PROJECT_ROOT/test/rootfs_mnt"

echo "========================================"
echo "Building ext4 rootfs image"
echo "========================================"

# 检查 shell 二进制是否存在
if [ ! -f "$SHELL_BINARY" ]; then
    echo "Error: Shell binary not found at $SHELL_BINARY"
    echo "Please run: ./userspace/build.sh"
    exit 1
fi

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

# 复制 shell 到镜像
echo "Installing shell to /bin/sh..."
sudo cp "$SHELL_BINARY" "$MOUNT_POINT/bin/sh"
sudo cp "$SHELL_BINARY" "$MOUNT_POINT/bin/shell"
sudo chmod +x "$MOUNT_POINT/bin/sh"
sudo chmod +x "$MOUNT_POINT/bin/shell"

# 复制 desktop 到镜像（如果存在）
if [ -f "$DESKTOP_BINARY" ]; then
    echo "Installing desktop to /bin/desktop..."
    sudo cp "$DESKTOP_BINARY" "$MOUNT_POINT/bin/desktop"
    sudo chmod +x "$MOUNT_POINT/bin/desktop"
else
    echo "Warning: Desktop binary not found at $DESKTOP_BINARY (skipping)"
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
SHELL_SIZE=$(stat -c%s "$SHELL_BINARY" 2>/dev/null || stat -f%z "$SHELL_BINARY")
DESKTOP_SIZE=""
if [ -f "$DESKTOP_BINARY" ]; then
    DESKTOP_SIZE=$(stat -c%s "$DESKTOP_BINARY" 2>/dev/null || stat -f%z "$DESKTOP_BINARY")
fi
IMAGE_SIZE=$(stat -c%s "$IMAGE_FILE" 2>/dev/null || stat -f%z "$IMAGE_FILE")

echo ""
echo "========================================"
echo "Image statistics:"
echo "========================================"
echo "Shell binary size: $SHELL_SIZE bytes"
if [ -n "$DESKTOP_SIZE" ]; then
    echo "Desktop binary size: $DESKTOP_SIZE bytes"
fi
echo "Total image size:  $IMAGE_SIZE bytes"
ls -lh "$IMAGE_FILE"

# 卸载镜像
echo ""
echo "Unmounting image..."
sudo umount "$MOUNT_POINT"
rmdir "$MOUNT_POINT"

echo ""
echo "✓ Rootfs image created successfully: $IMAGE_FILE"
echo ""
echo "To use this rootfs:"
echo "  QEMU: -drive file=test/rootfs.img,if=none,format=raw,id=rootfs -device virtio-blk-device,drive=rootfs"
echo "  Kernel cmdline: root=/dev/vda rw init=/bin/sh"
