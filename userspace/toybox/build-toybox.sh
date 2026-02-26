#!/bin/bash
# Rux OS - Toybox 构建脚本
#
# 使用 musl libc 交叉编译 toybox，生成静态链接的 RISC-V 64 位二进制文件

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TOYBOX_DIR="${SCRIPT_DIR}/toybox"
TOYBOX_VERSION="0.8.13"
MUSL_DIR="${PROJECT_ROOT}/toolchain/riscv64-rux-linux-musl"

echo "========================================"
echo "Rux OS - Toybox Build Script (musl libc)"
echo "========================================"
echo "TOYBOX_VERSION: ${TOYBOX_VERSION}"
echo "TOYBOX_DIR: ${TOYBOX_DIR}"
echo "PROJECT_ROOT: ${PROJECT_ROOT}"
echo "MUSL_DIR: ${MUSL_DIR}"
echo ""

# 检查交叉编译工具链
if ! command -v riscv64-linux-gnu-gcc &> /dev/null; then
    echo "Error: riscv64-linux-gnu-gcc not found"
    echo "Please install RISC-V cross-compiler toolchain"
    exit 1
fi

# 检查 musl 目录
if [ ! -d "$MUSL_DIR/include" ]; then
    echo "Error: musl include directory not found at $MUSL_DIR/include"
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

# 设置交叉编译环境变量 - 使用 musl libc
# 包含 musl 头文件和系统的 linux/asm 头文件
export CC=riscv64-linux-gnu-gcc
export CFLAGS="-static -nostdinc -isystem ${MUSL_DIR}/include -isystem /usr/riscv64-linux-gnu/include -isystem /usr/include"
export LDFLAGS="-static -nostdlib -L${MUSL_DIR}/lib ${MUSL_DIR}/lib/crt1.o ${MUSL_DIR}/lib/crti.o -lgcc ${MUSL_DIR}/lib/crtn.o -lc -lgcc"

echo ""
echo "Configuring toybox..."
make distclean 2>/dev/null || true
make defconfig

# 禁用需要 crypt 库的命令（su, login, mkpasswd）
echo "Disabling commands that require crypt library..."
# 使用标准 Linux 内核配置格式，toybox 的 kconfig 能正确处理
sed -i 's/CONFIG_SU=y/# CONFIG_SU is not set/' .config
sed -i 's/CONFIG_LOGIN=y/# CONFIG_LOGIN is not set/' .config
sed -i 's/CONFIG_MKPASSWD=y/# CONFIG_MKPASSWD is not set/' .config

# 启用 shell 命令
echo "Enabling sh (toysh) command..."
sed -i 's/# CONFIG_SH is not set/CONFIG_SH=y/' .config

# 重新生成配置
./generated/unstripped/kconfig -s .config 2>/dev/null || true

# 修复 toybox kconfig 的 bug：CFG_XXX=n 格式不会生成 USE_XXX 宏
# 需要手动将 "=n" 改为 "= 0" 并添加 USE_XXX(...) 宏
fix_config_h() {
    local cmd=$1
    if grep -q "#define CFG_${cmd} n" generated/config.h 2>/dev/null; then
        sed -i "s/#define CFG_${cmd} n/#define CFG_${cmd} 0\n#define USE_${cmd}(...)\n#define SKIP_${cmd}(...) __VA_ARGS__/" generated/config.h
    fi
}

fix_config_h "SU"
fix_config_h "LOGIN"
fix_config_h "MKPASSWD"

# 如果 SH 仍然被禁用，强制启用
if grep -q "#define CFG_SH 0" generated/config.h 2>/dev/null; then
    echo "Force enabling SH in config.h..."
    sed -i 's/#define CFG_SH 0/#define CFG_SH 1/' generated/config.h
    sed -i 's/#define USE_SH(...)/#define USE_SH(...) __VA_ARGS__/' generated/config.h
    sed -i 's/#define SKIP_SH(...)/#define SKIP_SH(...)/' generated/config.h
fi

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
