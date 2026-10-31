#!/usr/bin/env bash
# aa64-tail task-5 groundwork: cross-build a small LTP subset for
# aarch64 with the distro glibc toolchain (STATIC — the shared
# a64-rootfs is a toybox symlink farm with no aarch64 dynamic loader;
# TASK.md: "aarch64-linux-gnu toolchain; no zcb pin needed" — no musl
# SDK, no soft-fp overlay).
#
# Output: run/ltp-aa64/out/ test binaries + run/ltp-aa64/disk-root/
# staging tree (consumed by aa64-ltp-smoke.sh to inject into an image
# copy). Rebuildable scratch stays in the lane worktree.
set -u
LANE=/home/william/rux-agents/aa64-tail
TARBALL="$LANE/userspace/linux-ltp/ltp-20240524.tar.xz"
WORK="$LANE/run/ltp-aa64"
SRC="$WORK/ltp-full-20240524"
OUT="$WORK/out"
STAGE="$WORK/disk-root/ltp"

# Smoke subset: leaf dirs under testcases/kernel/syscalls (the classic
# quick-suite families — getpid/fork/write/read/open/close/pipe/mkdir/
# rmdir/unlink/stat/lseek/dup/dup2/mmap/brk/nanosleep/wait/exit/uid).
SUBSET="getpid fork write read open close pipe mkdir rmdir unlink stat lseek dup dup2 mmap brk nanosleep wait exit getuid geteuid getgid getegid getppid"

mkdir -p "$WORK"
if [ ! -d "$SRC" ]; then
    echo "[ltp-aa64] extracting $(basename "$TARBALL")"
    tar xf "$TARBALL" -C "$WORK" || { echo "extract failed"; exit 1; }
fi

cd "$SRC"
if [ ! -f include/mk/config.mk ] || [ ! -f "$WORK/.configured-aa64" ]; then
    echo "[ltp-aa64] configure (host=aarch64-linux-gnu, glibc, static)"
    make clean >/dev/null 2>&1 || true
    CC=aarch64-linux-gnu-gcc \
    CFLAGS="-O2 -U_FORTIFY_SOURCE" \
    LDFLAGS="-static" \
    ./configure \
        --prefix="$OUT" \
        --host=aarch64-linux-gnu \
        --without-numa --without-aio --without-selinux --without-crypto \
        --without-tirpc --without-keyutils --without-libcap --without-libacl \
        --without-open-posix-testsuite --without-realtime-testsuite \
        > "$WORK/configure.log" 2>&1 || { tail -20 "$WORK/configure.log"; exit 1; }
    touch include/stamp-h1 2>/dev/null || true
    touch "$WORK/.configured-aa64"
fi

echo "[ltp-aa64] building libltp"
make -C lib -j"$(nproc)" AUTOHEADER=true CC=aarch64-linux-gnu-gcc \
    LDFLAGS="-static" > "$WORK/lib-build.log" 2>&1 || true
make -C libs -j"$(nproc)" AUTOHEADER=true CC=aarch64-linux-gnu-gcc \
    LDFLAGS="-static" > "$WORK/libs-build.log" 2>&1 || true
ls lib/libltp.a >/dev/null 2>&1 || { echo "libltp.a missing"; tail -10 "$WORK/lib-build.log"; exit 1; }

LIB_PATHS="-L$SRC/lib -L$SRC/libs/libltp -L$SRC/libs/libltpipc"
for d in "$SRC"/libs/*/; do LIB_PATHS="$LIB_PATHS -L$d"; done

rm -rf "$STAGE"; mkdir -p "$STAGE"
built=0; failed=""
for t in $SUBSET; do
    d="$SRC/testcases/kernel/syscalls/$t"
    [ -d "$d" ] || continue
    make -C "$d" -j4 AUTOHEADER=true CC=aarch64-linux-gnu-gcc \
        LDFLAGS="-static $LIB_PATHS" > "$WORK/build-$t.log" 2>&1 || true
    n=0
    for b in "$d"/*; do
        if [ -f "$b" ] && [ -x "$b" ] && file "$b" | grep -q "ARM aarch64.*statically linked"; then
            cp "$b" "$STAGE/" && n=$((n+1))
        fi
    done
    built=$((built+n))
    [ "$n" -eq 0 ] && failed="$failed $t"
done
echo "[ltp-aa64] $built static binaries staged under $STAGE"
[ -n "$failed" ] && echo "[ltp-aa64] zero-binary dirs:$failed"

# Runner script the serial-fed shell executes.
cat > "$STAGE/run.sh" << 'EOF'
#!/bin/sh
# aarch64 LTP smoke subset (serial-fed)
P=0; F=0
for t in getpid01 getpid02 fork01 write01 write05 read01 read02 open01 open02 \
         close01 close02 pipe01 pipe02 mkdir01 pipe02 rmdir01 unlink01 unlink05 \
         stat01 stat02 lseek01 lseek02 dup01 dup02 dup203 mmap01 brk01 \
         nanosleep01 nanosleep02 wait01 wait02 exit01 getuid01 geteuid01 \
         getgid01 getegid01 getppid01; do
    if [ -x "/ltp/$t" ]; then
        if timeout 10 "/ltp/$t" >/dev/null 2>&1; then
            echo "LTPSMK PASS $t"; P=$((P+1))
        else
            echo "LTPSMK FAIL $t rc=$?"; F=$((F+1))
        fi
    fi
done
echo "LTPSMK SUMMARY passed=$P failed=$F"
EOF
chmod +x "$STAGE/run.sh"
du -sh "$STAGE"
echo "[ltp-aa64] done"
