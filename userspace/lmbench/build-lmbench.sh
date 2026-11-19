#!/bin/bash
# build-lmbench.sh — compile the lmbench core benchmarks as static musl
# binaries for the Rux guest (x86_64). One invocation builds every
# benchmark that compiles cleanly against the rpc stub; failures are
# tolerated and reported (some sources need sunrpc/WWW servers and are
# out of scope).
#
# Usage: build-lmbench.sh [outdir]   (default: ../bin)
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
TC="/home/william/rux-agents/code-review-perf/toolchain/x86_64-rux-linux-musl/bin/musl-gcc"
SRC="$HERE/src/lmbench-master/src"
SHIM="$HERE/musl-shim"
OUT="${1:-$HERE/bin}"
mkdir -p "$OUT"

LIBS="lib_timing.c lib_stats.c lib_sched.c getopt.c lib_unix.c lib_tcp.c lib_udp.c lib_mem.c"

# benchmark binary: list of primary sources
declare -A BENCHES=(
    [lat_syscall]="lat_syscall.c"
    [lat_ctx]="lat_ctx.c"
    [lat_proc]="lat_proc.c"
    [lat_pipe]="lat_pipe.c"
    [lat_fifo]="lat_fifo.c"
    [lat_unix]="lat_unix.c"
    [lat_connect]="lat_connect.c"
    [lat_tcp]="lat_tcp.c"
    [lat_udp]="lat_udp.c"
    [lat_sem]="lat_sem.c"
    [lat_pagefault]="lat_pagefault.c"
    [lat_mmap]="lat_mmap.c"
    [lat_fs]="lat_fs.c"
    [lat_fifo]="lat_fifo.c"
    [lat_mem_rd]="lat_mem_rd.c"
    [lat_dram_page]="lat_dram_page.c"
    [lat_usleep]="lat_usleep.c"
    [bw_mem]="bw_mem.c"
    [bw_pipe]="bw_pipe.c"
    [bw_unix]="bw_unix.c"
    [bw_tcp]="bw_tcp.c"
    [bw_udp]="bw_udp.c"
    [bw_file_rd]="bw_file_rd.c"
    [bw_mmap_rd]="bw_mmap_rd.c"
    [lmdd]="lmdd.c"
)

ok=0; fail=0
cd "$SRC"
for name in "${!BENCHES[@]}"; do
    srcs="${BENCHES[$name]}"
    extra=""
    [ "$name" = "lat_usleep" ] && extra="-DTRUE=1 -DFALSE=0"
    if $TC -static -O2 -I"$SHIM" $extra -o "$OUT/$name" $srcs $LIBS 2>"$OUT/$name.build.log"; then
        ok=$((ok+1))
    else
        fail=$((fail+1))
        rm -f "$OUT/$name"
        echo "FAIL: $name (see $OUT/$name.build.log)"
    fi
done
echo "built $ok, failed $fail -> $OUT"
