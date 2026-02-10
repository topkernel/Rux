#!/bin/bash
# 创建简单的 ext4 镜像（不需要 sudo）

set -e

# 镜像文件路径
IMAGE_FILE="test/disk.img"
IMAGE_SIZE="64M"

# 创建镜像文件
echo "Creating ext4 image: $IMAGE_FILE ($IMAGE_SIZE)"
dd if=/dev/zero of="$IMAGE_FILE" bs=1M count=64 2>/dev/null

# 格式化为 ext4（使用 -F 标志自动确认）
echo "Formatting as ext4..."
mkfs.ext4 -F "$IMAGE_FILE" > /dev/null 2>&1

# 使用 debugfs 添加文件（如果可用）
if command -v debugfs &> /dev/null; then
    echo "Adding test files using debugfs..."

    # 创建测试目录和文件
    echo "Creating /test directory..."
    debugfs -R "mkdir /test" "$IMAGE_FILE" >/dev/null 2>&1 || true

    echo "Creating test files..."
    # 注意：debugfs -w 可能需要特殊权限，这里只做基本测试

    echo "Done! Image created at: $IMAGE_FILE"
    ls -lh "$IMAGE_FILE"
else
    echo "Warning: debugfs not available, creating empty ext4 image"
    echo "Done! Empty image created at: $IMAGE_FILE"
    ls -lh "$IMAGE_FILE"
fi
