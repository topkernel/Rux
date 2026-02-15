#!/bin/bash
# Rux OS - Toybox 构建脚本
#
# 使用交叉编译工具链编译 toybox，生成静态链接的 RISC-V 64 位二进制文件

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TOYBOX_DIR="${SCRIPT_DIR}/toybox"
TOYBOX_VERSION="0.8.13"

echo "========================================"
echo "Rux OS - Toybox Build Script"
echo "========================================"
echo "TOYBOX_VERSION: ${TOYBOX_VERSION}"
echo "TOYBOX_DIR: ${TOYBOX_DIR}"
echo "PROJECT_ROOT: ${PROJECT_ROOT}"
echo ""

# 检查交叉编译工具链
if ! command -v riscv64-linux-gnu-gcc &> /dev/null; then
    echo "Error: riscv64-linux-gnu-gcc not found"
    echo "Please install RISC-V cross-compiler toolchain"
    exit 1
fi

echo "Cross-compiler: $(which riscv64-linux-gnu-gcc)"
echo "GCC version: $(riscv64-linux-gnu-gcc --version | head -1)"
echo ""

# 下载 toybox 源码
if [ ! -d "$TOYBOX_DIR" ]; then
    echo "Downloading toybox ${TOYBOX_VERSION}..."
    cd "$SCRIPT_DIR"

    # 尝试使用 tarball 下载（比 git clone 更稳定）
    TARBALL="toybox-${TOYBOX_VERSION}.tar.gz"
    if [ ! -f "$TARBALL" ]; then
        wget -c "https://landley.net/toybox/downloads/${TARBALL}" -O "$TARBALL"
    fi

    tar -xzf "$TARBALL"
    mv "toybox-${TOYBOX_VERSION}" toybox
    echo "Toybox source downloaded"
else
    echo "Toybox source already exists at $TOYBOX_DIR"
fi

# 构建 toybox
cd "$TOYBOX_DIR"

# 设置交叉编译环境变量
export CC=riscv64-linux-gnu-gcc
export CFLAGS="-static"
export LDFLAGS="-static"

echo ""
echo "Configuring toybox..."
make distclean 2>/dev/null || true
make defconfig

# 禁用需要 crypt 库的命令（su, login, mkpasswd）
echo "Disabling commands that require crypt library..."
sed -i 's/CONFIG_SU=y/CONFIG_SU=n/' .config
sed -i 's/CONFIG_LOGIN=y/CONFIG_LOGIN=n/' .config
sed -i 's/CONFIG_MKPASSWD=y/CONFIG_MKPASSWD=n/' .config

# 重新生成配置
yes "" | make oldconfig > /dev/null 2>&1

echo ""
echo "Building toybox (this may take a few minutes)..."
make -j$(nproc)

# 验证构建结果
if [ -f "$TOYBOX_DIR/toybox" ]; then
    echo ""
    echo "========================================"
    echo "Toybox built successfully!"
    echo "========================================"
    ls -la "$TOYBOX_DIR/toybox"
    file "$TOYBOX_DIR/toybox"
    echo ""
    echo "Binary size: $(du -h "$TOYBOX_DIR/toybox" | cut -f1)"
    echo "Output: $TOYBOX_DIR/toybox"
    echo ""
    echo "Note: su, login, mkpasswd commands are disabled (require crypt library)"
else
    echo "Error: toybox build failed"
    exit 1
fi
