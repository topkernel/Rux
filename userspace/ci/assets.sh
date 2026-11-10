#!/bin/bash
# assets.sh — CI asset store: dedicated dir OUTSIDE git (default
# /home/william/rux-assets; override RUX_ASSETS). Rules (user 2026-11-10):
# scripts READ from here; only seeding/updating writes, and only when an
# asset is MISSING (never touch what exists — freshness by existence,
# builds skipped if outputs already present).
#
# Images are NOT downloadable — seeded from the standing local image
# store on first use, clear error if neither exists. Sources/builds
# (lmbench, geekbench) download or compile into the store on demand.
set -uo pipefail
A="${RUX_ASSETS:-/home/william/rux-assets}"
IMG_SRC=/home/william/rux-images            # standing store (read-only seed)
mkdir -p "$A"/{images,src,bin,logs}

asset_image() {  # asset_image <name> <seed-path...>  → echoes asset path
    local n="$1" p="$A/images/$n"; shift
    if [ ! -f "$p" ]; then
        for s in "$@"; do [ -f "$s" ] && { cp --reflink=auto "$s" "$p"; break; }; done
    fi
    [ -f "$p" ] || { echo "ASSET-MISSING $n (seeded-from candidates absent): $*" >&2; return 1; }
    echo "$p"
}

asset_lmbench() {  # build once into $A/bin; skip if present
    local bin="$A/bin/lmbench_run" ver=3.0-a9
    if [ ! -x "$bin" ]; then
        local d="$A/src/lmbench-$ver"
        [ -d "$d" ] || { mkdir -p "$d"
            curl -fsSL "https://downloads.sourceforge.net/lmbench/lmbench-$ver.tgz" \
                | tar xz -C "$A/src" || { echo "lmbench download FAILED" >&2; return 1; }; }
        cc -O2 -o "$bin" "$d/src/lmbench_run.c" 2>/dev/null \
            || cc -O2 -o "$bin" "$(dirname "$0")/drivers/lmbench_run.c" 2>/dev/null \
            || return 1
    fi
    echo "$bin"
}

asset_geekbench() {  # geekbench 6 tarball (free-to-run binary), x86_64
    local d="$A/src/geekbench6" b="$d/dist/Geekbench"
    if [ ! -d "$d" ]; then
        local t; t=$(curl -fsSL "https://cdn.geekbench.com/Geekbench-6.tar.gz" -o "$A/src/gb6.tgz" && echo ok)
        [ "$t" = ok ] && mkdir -p "$d" && tar xzf "$A/src/gb6.tgz" -C "$d" --strip-components=1
    fi
    [ -x "$b" ] && echo "$d" || { echo "ASSET-MISSING geekbench6 (download failed)" >&2; return 1; }
}

# ---- boot regressions: scripted serial, marker+panic verdict ----
boot_ubuntu() {  # image: ubuntu-gui.img (not downloadable; seeded or error)
    local img krn out="$A/logs/ubuntu-$(date +%m%d-%H%M).log"
    img=$(asset_image ubuntu-gui.img "$IMG_SRC/ubuntu/ubuntu-gui.img" /home/william/workspace/Rux/work/ubuntu-gui.img) || return 1
    krn=target/x86_64-unknown-none/debug/bzImage; [ -f "$krn" ] || return 1
    timeout 420 qemu-system-x86_64 -M q35 -accel kvm -cpu host -m 4G -smp 4 \
      -display none -serial stdio -monitor none -no-reboot -snapshot \
      -drive file="$img",if=none,id=d,format=raw,file.locking=off \
      -device virtio-blk-pci,drive=d -kernel "$krn" \
      -append "root=/dev/vda rw console=ttyS0" > "$out" 2>&1 || true
    grep -aq KERNPANIC "$out" && { echo "UBUNTU-REGRESS KERNPANIC"; return 2; }
    grep -aqE "login:|gnome-shell|Ubuntu" "$out" && echo "UBUNTU-BOOT OK $(basename $out)" \
        || { echo "UBUNTU-BOOT NO-MARKER"; return 1; }
}

boot_oh() {
    local img ud out="$A/logs/oh-$(date +%m%d-%H%M).log"
    img=$(asset_image oh-system.img "$IMG_SRC/oh/system.img") || return 1
    ud=$(asset_image oh-userdata.img "$IMG_SRC/oh/userdata.img") || return 1
    local krn=target/x86_64-unknown-none/release/bzImage
    [ -f "$krn" ] || krn=target/x86_64-unknown-none/debug/bzImage; [ -f "$krn" ] || return 1
    timeout 600 qemu-system-x86_64 -M q35 -accel kvm -cpu host -m 2G -smp 2 \
      -display none -serial stdio -monitor none -no-reboot -snapshot \
      -drive file="$img",if=none,id=s,format=raw,file.locking=off \
      -drive file="$ud",if=none,id=u,format=raw,file.locking=off \
      -device virtio-blk-pci,drive=s -device virtio-blk-pci,drive=u \
      -kernel "$krn" -append "console=ttyS0" > "$out" 2>&1 || true
    local n; n=$(grep -ac "insert SA:" "$out")
    grep -aq KERNPANIC "$out" && { echo "OH-REGRESS KERNPANIC"; return 2; }
    [ "$n" -ge 20 ] && echo "OH-BOOT OK sa=$n" || { echo "OH-BOOT LOW-SA sa=$n"; return 1; }
}

# ---- benches: run + parse into one CSV row each ----
bench_lmbench() {
    local bin img; bin=$(asset_lmbench) || return 1
    img=$(asset_image lmbench-x86.img "$IMG_SRC/testimages/lmbench-x86.img") || return 1
    # TODO-next: inject $bin via debugfs + init= harness (same qrtests rig),
    # parse lat_* rows → $A/logs/lmbench-<date>.csv. Wired to rig next session.
    echo "LMBENCH-PENDING-RIG bin=$bin img=$img"
}
bench_geekbench() {
    local d; d=$(asset_geekbench) || return 1
    # TODO-next: boot ltp image, inject dist/, run ./Geekbench --sys-info,
    # parse section scores → $A/logs/geekbench-<date>.csv.
    echo "GEEKBENCH-PENDING-RIG dir=$d"
}
"$@"
