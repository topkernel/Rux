#!/bin/bash
#
# Rux 用户程序构建脚本
#
# 用法:
#   ./build.sh           - 构建所有用户程序 (debug)
#   ./build.sh release   - 构建所有用户程序 (release)
#   ./build.sh clean     - 清理构建产物

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info() { echo -e "${GREEN}[INFO]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

# 备份根目录的 cargo config（如果存在）
CARGO_CONFIG="$ROOT_DIR/.cargo/config.toml"
BACKUP_CONFIG="$ROOT_DIR/.cargo/config.toml.backup"

disable_root_config() {
    if [ -f "$CARGO_CONFIG" ] && [ ! -f "$CARGO_CONFIG.disabled" ]; then
        info "备份根目录的 cargo 配置..."
        cp "$CARGO_CONFIG" "$BACKUP_CONFIG"
        mv "$CARGO_CONFIG" "$CARGO_CONFIG.disabled"
    fi
}

restore_root_config() {
    if [ -f "$BACKUP_CONFIG" ]; then
        info "恢复根目录的 cargo 配置..."
        mv "$CARGO_CONFIG.disabled" "$CARGO_CONFIG"
        rm -f "$BACKUP_CONFIG"
    fi
}

# 清理函数
cleanup() {
    restore_root_config
}

# 设置 trap 确保清理
trap cleanup EXIT

# 清理构建
clean_build() {
    info "清理用户程序构建产物..."
    cd "$SCRIPT_DIR"
    cargo clean
    info "清理完成"
}

# 构建用户程序
build_userspace() {
    local MODE="${1:-debug}"
    local RELEASE_FLAG=""

    if [ "$MODE" = "release" ]; then
        RELEASE_FLAG="--release"
        info "构建用户程序 (release 模式)..."
    else
        info "构建用户程序 (debug 模式)..."
    fi

    # 构建 shell (musl libc)
    info "编译 shell (musl libc)..."
    if [ -f "$SCRIPT_DIR/shell/Makefile" ]; then
        make -C "$SCRIPT_DIR/shell"
    else
        warn "shell/Makefile 不存在，跳过 shell 编译"
    fi

    # 禁用根目录的 cargo 配置
    disable_root_config

    # 进入用户程序目录
    cd "$SCRIPT_DIR"

    # 构建所有用户程序
    # - rux_gui: GUI 库
    # - desktop: 桌面环境
    info "编译 rux_gui 库..."
    cargo build $RELEASE_FLAG -p rux_gui

    info "编译 desktop..."
    cargo build $RELEASE_FLAG -p desktop

    # 显示输出文件
    echo ""
    info "构建完成！输出文件："

    local TARGET_DIR="target/riscv64gc-unknown-linux-musl/debug"
    if [ "$MODE" = "release" ]; then
        TARGET_DIR="target/riscv64gc-unknown-linux-musl/release"
    fi

    # 列出生成的可执行文件
    for bin in desktop; do
        local bin_path="$TARGET_DIR/$bin"
        if [ -f "$bin_path" ]; then
            local size=$(stat -c%s "$bin_path" 2>/dev/null || stat -f%z "$bin_path" 2>/dev/null)
            info "  $bin: $size bytes"
        fi
    done

    # 显示 shell 信息
    if [ -f "$SCRIPT_DIR/shell/shell" ]; then
        local shell_size=$(stat -c%s "$SCRIPT_DIR/shell/shell" 2>/dev/null || stat -f%z "$SCRIPT_DIR/shell/shell" 2>/dev/null)
        info "  shell: $shell_size bytes"
    fi

    echo ""
    info "用户程序构建成功！"
}

# 主函数
main() {
    local COMMAND="${1:-build}"

    case "$COMMAND" in
        clean)
            clean_build
            ;;
        release)
            build_userspace release
            ;;
        build|debug|"")
            build_userspace debug
            ;;
        *)
            error "未知命令: $COMMAND"
            echo "用法: $0 [build|release|clean]"
            exit 1
            ;;
    esac
}

# 运行主函数
main "$@"
