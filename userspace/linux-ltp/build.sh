#!/bin/bash
# Rux OS - LTP Test Suite Build Script
#
# Download and build official LTP (Linux Test Project) tests
# for kernel compatibility testing using musl libc

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTPUT_DIR="${SCRIPT_DIR}/output"
MUSL_DIR="${PROJECT_ROOT}/toolchain/riscv64-rux-linux-musl"
LTP_VERSION="20240524"
LTP_SRC_DIR="${SCRIPT_DIR}/ltp-full-${LTP_VERSION}"

# Color output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { echo -e "${GREEN}[INFO]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

echo "========================================"
echo "Rux OS - LTP Test Suite Build Script"
echo "========================================"
echo "LTP Version: ${LTP_VERSION}"
echo "Source Dir:  ${LTP_SRC_DIR}"
echo "Output Dir:  ${OUTPUT_DIR}"
echo "Musl Dir:    ${MUSL_DIR}"
echo ""

# Check cross-compiler toolchain
if ! command -v riscv64-linux-gnu-gcc &> /dev/null; then
    error "riscv64-linux-gnu-gcc not found. Install: apt install gcc-riscv64-linux-gnu"
fi

# Check musl directory
if [ ! -d "$MUSL_DIR/include" ]; then
    error "musl include directory not found. Run: cd toolchain && ./build-musl.sh"
fi

# Download LTP source
download_ltp() {
    if [ -d "$LTP_SRC_DIR" ]; then
        info "LTP source already exists, skipping extraction"
        return
    fi

    local TAR_FILE="${SCRIPT_DIR}/ltp-${LTP_VERSION}.tar.xz"

    if [ ! -f "$TAR_FILE" ]; then
        error "LTP tarball not found: $TAR_FILE"
    fi

    info "Extracting LTP from local tarball..."
    tar xf "$TAR_FILE" -C "$SCRIPT_DIR"

    info "LTP source ready: $LTP_SRC_DIR"
}

# Configure LTP for cross-compilation with musl
configure_ltp() {
    info "Configuring LTP for cross-compilation with musl..."

    cd "$LTP_SRC_DIR"

    # Ensure kernel headers are available in musl include directory
    if [ ! -d "${MUSL_DIR}/include/linux" ]; then
        info "Copying kernel headers to musl include directory..."
        cp -r /usr/riscv64-linux-gnu/include/linux "${MUSL_DIR}/include/"
        cp -r /usr/riscv64-linux-gnu/include/asm "${MUSL_DIR}/include/"
        cp -r /usr/riscv64-linux-gnu/include/asm-generic "${MUSL_DIR}/include/"
    fi

    # Export cross-compile environment
    export CC=riscv64-linux-gnu-gcc
    export LD=riscv64-linux-gnu-ld
    export AR=riscv64-linux-gnu-ar
    export RANLIB=riscv64-linux-gnu-ranlib
    export STRIP=riscv64-linux-gnu-strip
    export CROSS_COMPILE=riscv64-linux-gnu-

    # Use -nostdinc to exclude glibc headers, then add musl and GCC headers
    export ADD_CFLAGS="-nostdinc -isystem /usr/lib/gcc-cross/riscv64-linux-gnu/13/include -isystem ${MUSL_DIR}/include"
    export LDFLAGS="-static -L${MUSL_DIR}/lib"

    # Configure with musl headers, disable features requiring external libs
    ./configure \
        --prefix="${OUTPUT_DIR}" \
        --host=riscv64-linux-gnu \
        --without-numa \
        --without-aio \
        --without-selinux \
        --without-crypto \
        --without-tirpc \
        --without-keyutils \
        --without-libcap \
        --without-libacl \
        --without-open-posix-testsuite \
        --without-realtime-testsuite

    # Post-configure patches for musl compatibility
    info "Applying musl compatibility patches..."

    # 1. Fix CPPFLAGS in config.mk to use musl headers exclusively
    # configure generates empty CPPFLAGS, we need to set it properly
    # Use -nostdinc to exclude glibc, then add GCC and musl headers
    # Auto-detect GCC version
    GCC_VERSION=$(ls -d /usr/lib/gcc-cross/riscv64-linux-gnu/*/include 2>/dev/null | head -1)
    if [ -z "$GCC_VERSION" ]; then
        error "Cannot find GCC cross-compiler include directory"
    fi
    sed -i "s|^CPPFLAGS.*|CPPFLAGS\t\t:= -nostdinc -isystem ${GCC_VERSION} -isystem ${MUSL_DIR}/include|" \
        include/mk/config.mk

    # 2. Disable HAVE_SYS_PIDFD_H (musl doesn't have sys/pidfd.h)
    sed -i 's/#define HAVE_SYS_PIDFD_H 1/\/\* #undef HAVE_SYS_PIDFD_H \*\//' \
        include/config.h

    info "Configuration complete"
}

# Build LTP tests with musl
build_ltp() {
    info "Building LTP tests with musl..."

    cd "$LTP_SRC_DIR"

    # Build library first with LTPLIB defined
    info "Building LTP library..."
    make -C lib libltp.a -j$(nproc) \
        CC=riscv64-linux-gnu-gcc \
        ADD_CFLAGS="-static -O2 -nostdinc -isystem /usr/lib/gcc-cross/riscv64-linux-gnu/13/include -isystem ${MUSL_DIR}/include" \
        LDFLAGS="-static -L${MUSL_DIR}/lib"

    # Build all test subdirectories in parallel for maximum coverage
    info "Building test cases in parallel..."
    find testcases -mindepth 3 -name "Makefile" -exec dirname {} \; | while read dir; do
        make -C "$dir" -j1 \
            CC=riscv64-linux-gnu-gcc \
            ADD_CFLAGS="-static -O2 -nostdinc -isystem /usr/lib/gcc-cross/riscv64-linux-gnu/13/include -isystem ${MUSL_DIR}/include" \
            LDFLAGS="-static -L${MUSL_DIR}/lib -L${LTP_SRC_DIR}/lib" 2>/dev/null &
        # Limit parallel jobs to avoid overload
        if [ $(jobs -r | wc -l) -ge $(nproc) ]; then
            wait -n
        fi
    done
    wait

    # Count built binaries (ELF executables only)
    local COUNT=$(find testcases -type f -executable -exec file {} \; 2>/dev/null | grep -c "ELF.*executable")
    info "Built $COUNT test binaries"

    info "Build complete"
}

# Install LTP tests
install_ltp() {
    info "Installing LTP tests..."

    cd "$LTP_SRC_DIR"

    # Create output directories
    mkdir -p "${OUTPUT_DIR}/testcases/bin"
    mkdir -p "${OUTPUT_DIR}/runtest"
    mkdir -p "${OUTPUT_DIR}/scenario_groups"

    # Copy all built test binaries (ELF executables, not scripts)
    info "Copying test binaries..."
    # First copy all ELF executables (identified by file command)
    find testcases -type f -executable 2>/dev/null | while read f; do
        if file "$f" | grep -q "ELF.*executable"; then
            cp -v "$f" "${OUTPUT_DIR}/testcases/bin/"
        fi
    done

    # Copy runtest scenario files
    cp -r runtest/* "${OUTPUT_DIR}/runtest/" 2>/dev/null || true
    cp -r scenario_groups/* "${OUTPUT_DIR}/scenario_groups/" 2>/dev/null || true

    # Strip all binaries to reduce size
    info "Stripping binaries..."
    find "${OUTPUT_DIR}/testcases/bin" -type f -exec riscv64-linux-gnu-strip {} \; 2>/dev/null || true

    # Create test runner scripts
    create_runner_scripts

    # Count final binaries
    local FINAL_COUNT=$(find "${OUTPUT_DIR}/testcases/bin" -type f | wc -l)
    info "Installed $FINAL_COUNT test binaries"

    info "Installation complete"
}

# Create test runner scripts
create_runner_scripts() {
    info "Creating test runner scripts..."

    # Main LTP runner
    cat > "${OUTPUT_DIR}/run_ltp.sh" << 'EOF'
#!/bin/sh
# LTP Test Runner for Rux OS

LTP_DIR=/test/linux-ltp
PASSED=0
FAILED=0
TOTAL=0

echo "========================================"
echo "LTP Kernel Compatibility Tests"
echo "========================================"
echo ""

run_test() {
    test=$1
    name=$(basename "$test")

    # Skip shell scripts
    case "$test" in
        *.sh) return ;;
    esac

    TOTAL=$((TOTAL + 1))
    echo -n "Testing $name... "

    if [ -x "$test" ]; then
        if timeout 10 "$test" > /dev/null 2>&1; then
            echo "PASS"
            PASSED=$((PASSED + 1))
        else
            echo "FAIL"
            FAILED=$((FAILED + 1))
        fi
    fi
}

# Run all test binaries
for test in "$LTP_DIR/testcases/bin"/*; do
    if [ -f "$test" ] && [ -x "$test" ]; then
        run_test "$test"
    fi
done

echo ""
echo "========================================"
echo "Results: $TOTAL tests"
echo "Passed: $PASSED"
echo "Failed: $FAILED"
echo "========================================"

[ $FAILED -eq 0 ]
EOF
    chmod +x "${OUTPUT_DIR}/run_ltp.sh"

    # Quick test runner - essential tests only
    cat > "${OUTPUT_DIR}/run_quick.sh" << 'EOF'
#!/bin/sh
# Quick LTP Test Runner - essential syscall tests

TEST_DIR=/test/linux-ltp/testcases/bin
PASSED=0
FAILED=0

echo "========================================"
echo "LTP Quick Test Suite"
echo "========================================"
echo ""

# Essential syscall tests
for test in \
    getpid01 getpid02 \
    fork01 \
    write01 write02 \
    read01 read02 \
    open01 open02 \
    close01 close02 \
    pipe01 pipe02 \
    mkdir01 mkdir02 \
    rmdir01 rmdir02 \
    unlink01 unlink02 \
    stat01 stat02 \
    lseek01 lseek02 \
    dup01 dup02 \
    dup201 dup202 \
    mmap01 mmap02 \
    brk01 brk02 \
    nanosleep01 nanosleep02 \
    wait01 wait02 \
    exit01 exit02 \
    getuid01 \
    geteuid01 geteuid02 \
    getgid01 \
    getegid01 getegid02 \
    getppid01 getppid02
do
    if [ -x "$TEST_DIR/$test" ]; then
        echo -n "Testing $test... "
        if timeout 5 "$TEST_DIR/$test" > /dev/null 2>&1; then
            echo "PASS"
            PASSED=$((PASSED + 1))
        else
            echo "FAIL"
            FAILED=$((FAILED + 1))
        fi
    fi
done

echo ""
echo "========================================"
echo "Results: $PASSED passed, $FAILED failed"
echo "========================================"

[ $FAILED -eq 0 ]
EOF
    chmod +x "${OUTPUT_DIR}/run_quick.sh"

    # Syscall category runner
    cat > "${OUTPUT_DIR}/run_syscalls.sh" << 'EOF'
#!/bin/sh
# Run all syscall tests from LTP

TEST_DIR=/test/linux-ltp/testcases/bin
PASSED=0
FAILED=0
TOTAL=0

echo "========================================"
echo "LTP Syscall Tests"
echo "========================================"
echo ""

# Run all syscall tests
for test in "$TEST_DIR"/*; do
    if [ -f "$test" ] && [ -x "$test" ]; then
        name=$(basename "$test")
        # Skip non-syscall tests (scripts, etc)
        case "$name" in
            *.sh) continue ;;
            run_*) continue ;;
        esac

        TOTAL=$((TOTAL + 1))
        echo -n "Testing $name... "
        if timeout 10 "$test" > /dev/null 2>&1; then
            echo "PASS"
            PASSED=$((PASSED + 1))
        else
            echo "FAIL"
            FAILED=$((FAILED + 1))
        fi
    fi
done

echo ""
echo "========================================"
echo "Total: $TOTAL tests"
echo "Passed: $PASSED"
echo "Failed: $FAILED"
echo "========================================"

[ $FAILED -eq 0 ]
EOF
    chmod +x "${OUTPUT_DIR}/run_syscalls.sh"
}

# Clean build artifacts
clean() {
    info "Cleaning build artifacts..."
    rm -rf "${OUTPUT_DIR}"
    rm -rf "${LTP_SRC_DIR}"
    rm -f "${SCRIPT_DIR}/ltp-${LTP_VERSION}.tar.xz"
    info "Clean complete"
}

# Show usage
show_usage() {
    echo ""
    echo "========================================"
    echo " Build Complete!"
    echo "========================================"
    echo ""
    echo "Output directory: ${OUTPUT_DIR}"
    echo ""

    if [ -d "${OUTPUT_DIR}/testcases/bin" ]; then
        TEST_COUNT=$(find "${OUTPUT_DIR}/testcases/bin" -type f | wc -l)
        echo "Test binaries: $TEST_COUNT"
    fi

    echo ""
    du -sh "${OUTPUT_DIR}" 2>/dev/null || true
    echo ""
    echo "Runner scripts:"
    echo "  run_ltp.sh      - Run all LTP tests"
    echo "  run_quick.sh    - Run essential tests only"
    echo "  run_syscalls.sh - Run syscall tests"
    echo ""
    echo "To add to rootfs:"
    echo "  cd /home/william/Rux && make rootfs"
    echo ""
    echo "Tests will be installed at /test/linux-ltp/"
}

# Main function
main() {
    local COMMAND="${1:-build}"

    case "$COMMAND" in
        clean)
            clean
            ;;
        download)
            download_ltp
            ;;
        configure)
            download_ltp
            configure_ltp
            ;;
        build|"")
            download_ltp
            configure_ltp
            build_ltp
            install_ltp
            show_usage
            ;;
        install)
            install_ltp
            show_usage
            ;;
        *)
            error "Unknown command: $COMMAND
Usage: $0 [download|configure|build|install|clean]"
            ;;
    esac
}

main "$@"
