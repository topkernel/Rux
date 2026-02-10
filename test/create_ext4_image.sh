#!/bin/bash
# 创建测试用的 ext4 镜像

set -e

# 镜像文件路径
IMAGE_FILE="test/disk.img"
IMAGE_SIZE="64M"

# 创建镜像文件
echo "Creating ext4 image: $IMAGE_FILE ($IMAGE_SIZE)"
dd if=/dev/zero of="$IMAGE_FILE" bs=1M count=64 2>/dev/null

# 格式化为 ext4
echo "Formatting as ext4..."
mkfs.ext4 -F "$IMAGE_FILE"

# 创建挂载点
MOUNT_POINT="/tmp/rux_ext4_test"
mkdir -p "$MOUNT_POINT"

# 挂载镜像
echo "Mounting image..."
sudo mount -o loop "$IMAGE_FILE" "$MOUNT_POINT"

# 创建测试文件
echo "Creating test files..."
sudo mkdir -p "$MOUNT_POINT/test"
echo "Hello from Rux OS ext4!" | sudo tee "$MOUNT_POINT/test/hello.txt" > /dev/null
echo "Testing file read" | sudo tee "$MOUNT_POINT/test/test.txt" > /dev/null

# 列出文件
echo "Files in image:"
sudo ls -la "$MOUNT_POINT/"
sudo ls -la "$MOUNT_POINT/test/"

# 卸载镜像
echo "Unmounting image..."
sudo umount "$MOUNT_POINT"

echo "Done! Image created at: $IMAGE_FILE"
ls -lh "$IMAGE_FILE"
