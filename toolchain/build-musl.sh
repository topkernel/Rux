#!/bin/bash
#
# Rux OS - musl libc 构建脚本
#
# 用法:
#   ./build-musl.sh         - 下载并构建 musl libc
#   ./build-musl.sh clean   - 清理构建产物
#
# 依赖:
#   - riscv64-linux-gnu-gcc (RISC-V 交叉编译工具链)
#   - wget, tar, make
#
# 输出:
#   toolchain/riscv64-rux-linux-musl/
#     ├── include/   - C 头文件
#     └── lib/       - 静态库 (libc.a, crt1.o, etc.)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

MUSL_VERSION="1.2.5"
MUSL_DIR="${SCRIPT_DIR}/musl-${MUSL_VERSION}"
INSTALL_DIR="${SCRIPT_DIR}/riscv64-rux-linux-musl"

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { echo -e "${GREEN}[INFO]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

# 检查依赖
check_dependencies() {
    info "检查依赖..."

    if ! command -v riscv64-linux-gnu-gcc &> /dev/null; then
        error "未找到 riscv64-linux-gnu-gcc，请安装 RISC-V 交叉编译工具链"
    fi

    if ! command -v wget &> /dev/null && ! command -v curl &> /dev/null; then
        error "需要 wget 或 curl 来下载 musl"
    fi

    info "依赖检查通过"
}

# 下载 musl
download_musl() {
    if [ -d "$MUSL_DIR" ]; then
        info "musl 源码已存在，跳过下载"
        return
    fi

    info "下载 musl ${MUSL_VERSION}..."

    local MUSL_URL="https://musl.libc.org/releases/musl-${MUSL_VERSION}.tar.gz"
    local TAR_FILE="${SCRIPT_DIR}/musl-${MUSL_VERSION}.tar.gz"

    if command -v wget &> /dev/null; then
        wget -O "$TAR_FILE" "$MUSL_URL"
    else
        curl -L -o "$TAR_FILE" "$MUSL_URL"
    fi

    info "解压 musl..."
    tar xzf "$TAR_FILE" -C "$SCRIPT_DIR"
    rm -f "$TAR_FILE"

    info "musl 下载完成"
}

# 构建 musl
build_musl() {
    info "构建 musl libc..."

    cd "$MUSL_DIR"

    # 配置
    info "配置 musl..."
    ./configure \
        --target=riscv64-linux-musl \
        --prefix="${INSTALL_DIR}" \
        --disable-gcc-wrapper \
        CROSS_COMPILE=riscv64-linux-gnu-

    # 编译
    info "编译 musl..."
    make -j$(nproc)

    # 安装
    info "安装 musl..."
    make install

    info "musl 构建完成！"
    info "安装目录: ${INSTALL_DIR}"
}

# 创建 Rux 特定头文件
create_rux_headers() {
    info "创建 Rux 特定头文件..."

    local INCLUDE_DIR="${INSTALL_DIR}/include"

    # 创建 rux/syscall.h 与 Linux 兼容的系统调用号
    mkdir -p "${INCLUDE_DIR}/rux"

    cat > "${INCLUDE_DIR}/rux/syscall.h" << 'EOF'
#ifndef _RUX_SYSCALL_H
#define _RUX_SYSCALL_H

// RISC-V Linux 系统调用号
#define __NR_set_tid_address    96
#define __NR_set_robust_list    99
#define __NR_gettimeofday      169
#define __NR_clock_gettime     113
#define __NR_uname             160
#define __NR_exit               93
#define __NR_read               63
#define __NR_write              64
#define __NR_openat             56
#define __NR_close              57
#define __NR_brk               214
#define __NR_mmap              222
#define __NR_munmap            215
#define __NR_fork              220
#define __NR_execve            221
#define __NR_wait4             260
#define __NR_getpid            172
#define __NR_getppid           110

#endif /* _RUX_SYSCALL_H */
EOF

    info "Rux 头文件创建完成"
}

# 清理
clean_musl() {
    info "清理 musl 构建产物..."

    rm -rf "$MUSL_DIR"
    rm -rf "${SCRIPT_DIR}/musl-${MUSL_VERSION}.tar.gz"

    info "清理完成"
}

# 显示使用说明
show_usage() {
    echo ""
    echo "=========================================="
    echo " musl libc 构建完成！"
    echo "=========================================="
    echo ""
    echo "安装目录: ${INSTALL_DIR}"
    echo ""
    echo "使用方法:"
    echo "  # 编译 C 程序"
    echo "  riscv64-linux-gnu-gcc -static -nostdlib \\"
    echo "    -I${INSTALL_DIR}/include \\"
    echo "    -L${INSTALL_DIR}/lib \\"
    echo "    -o program program.c \\"
    echo "    ${INSTALL_DIR}/lib/crt1.o \\"
    echo "    ${INSTALL_DIR}/lib/libc.a \\"
    echo "    -lgcc"
    echo ""
    echo "或者使用 musl-gcc wrapper (如果可用):"
    echo "  ${INSTALL_DIR}/bin/musl-gcc -static -o program program.c"
    echo ""
}

# 主函数
main() {
    local COMMAND="${1:-build}"

    case "$COMMAND" in
        clean)
            clean_musl
            ;;
        build|"")
            check_dependencies
            download_musl
            build_musl
            create_rux_headers
            show_usage
            ;;
        *)
            error "未知命令: $COMMAND\n用法: $0 [build|clean]"
            ;;
    esac
}

main "$@"
