#!/bin/bash
#
# Rux OS - musl libc build script
#
# Usage:
#   ./build-musl.sh         - Download and build musl libc
#   ./build-musl.sh clean   - Clean build artifacts
#
# Dependencies:
#   - riscv64-linux-gnu-gcc (RISC-V cross-compile toolchain)
#   - wget, tar, make
#
# Output:
#   toolchain/riscv64-rux-linux-musl/
#     ├── include/   - C header files
#     └── lib/       - Static libraries (libc.a, crt1.o, etc.)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

MUSL_VERSION="1.2.5"
MUSL_DIR="${SCRIPT_DIR}/musl-${MUSL_VERSION}"
INSTALL_DIR="${SCRIPT_DIR}/riscv64-rux-linux-musl"

# Color output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { echo -e "${GREEN}[INFO]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

# Check dependencies
check_dependencies() {
    info "Checking dependencies..."

    if ! command -v riscv64-linux-gnu-gcc &> /dev/null; then
        error "riscv64-linux-gnu-gcc not found, please install RISC-V cross-compile toolchain"
    fi

    if ! command -v wget &> /dev/null && ! command -v curl &> /dev/null; then
        error "wget or curl is required to download musl"
    fi

    info "Dependency check passed"
}

# Download musl
download_musl() {
    if [ -d "$MUSL_DIR" ]; then
        info "musl source already exists, skipping extraction"
        return
    fi

    local TAR_FILE="${SCRIPT_DIR}/musl-${MUSL_VERSION}.tar.gz"

    if [ ! -f "$TAR_FILE" ]; then
        error "musl tarball not found: $TAR_FILE"
    fi

    info "Extracting musl from local tarball..."
    tar xzf "$TAR_FILE" -C "$SCRIPT_DIR"

    info "musl extraction complete"
}

# Build musl
build_musl() {
    info "Building musl libc..."

    cd "$MUSL_DIR"

    # Configure
    info "Configuring musl..."
    ./configure \
        --target=riscv64-linux-musl \
        --prefix="${INSTALL_DIR}" \
        --disable-gcc-wrapper \
        CROSS_COMPILE=riscv64-linux-gnu-

    # Compile
    info "Compiling musl..."
    make -j$(nproc)

    # Install
    info "Installing musl..."
    make install

    info "musl build complete!"
    info "Install directory: ${INSTALL_DIR}"
}

# Create Rux specific header files
create_rux_headers() {
    info "Creating Rux specific header files..."

    local INCLUDE_DIR="${INSTALL_DIR}/include"

    # Create rux/syscall.h with Linux compatible syscall numbers
    mkdir -p "${INCLUDE_DIR}/rux"

    cat > "${INCLUDE_DIR}/rux/syscall.h" << 'EOF'
#ifndef _RUX_SYSCALL_H
#define _RUX_SYSCALL_H

// RISC-V Linux syscall numbers
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

    info "Rux header files created"
}

# Clean
clean_musl() {
    info "Cleaning musl build artifacts..."

    rm -rf "$MUSL_DIR"

    info "Clean complete"
}

# Show usage
show_usage() {
    echo ""
    echo "=========================================="
    echo " musl libc build complete!"
    echo "=========================================="
    echo ""
    echo "Install directory: ${INSTALL_DIR}"
    echo ""
    echo "Usage:"
    echo "  # Compile C program"
    echo "  riscv64-linux-gnu-gcc -static -nostdlib \\"
    echo "    -I${INSTALL_DIR}/include \\"
    echo "    -L${INSTALL_DIR}/lib \\"
    echo "    -o program program.c \\"
    echo "    ${INSTALL_DIR}/lib/crt1.o \\"
    echo "    ${INSTALL_DIR}/lib/libc.a \\"
    echo "    -lgcc"
    echo ""
    echo "Or use musl-gcc wrapper (if available):"
    echo "  ${INSTALL_DIR}/bin/musl-gcc -static -o program program.c"
    echo ""
}

# Main function
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
            error "Unknown command: $COMMAND\nUsage: $0 [build|clean]"
            ;;
    esac
}

main "$@"
