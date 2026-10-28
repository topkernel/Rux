#!/bin/bash
# mkohlite.sh — build the OH-lite boot image set (no OH tree needed).
#
# Produces in work/ohlite/images/:
#   ramdisk.img   gzip cpio newc — dynamic glibc /init (init_early replica)
#   updater.img   ext4 filler (keeps system=vdb vendor=vdc like run-oh.sh)
#   system.img    ext4 → vdb → /usr: /bin/init second stage + services
#   vendor.img    ext4 → vdc → /vendor
#
# All binaries are compiled on the host and paired with the HOST's libs
# (ldd-resolved) so SONAME versions always match.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$ROOT/test/ohlite"
OUT="$ROOT/work/ohlite"
IMAGES="$OUT/images"

rm -rf "$OUT/rd" "$OUT/sys" "$OUT/vendor" "$OUT/updater" "$IMAGES"
mkdir -p "$OUT/rd" "$OUT/sys" "$OUT/vendor" "$OUT/updater" "$IMAGES"

CC=${CC:-gcc}
echo "[1/5] compiling (host gcc, dynamic glibc)"
"$CC" -O2 -Wall -o "$OUT/init_early"  "$HERE/init_early.c"
"$CC" -O2 -Wall -o "$OUT/init_second" "$HERE/init_second.c"
"$CC" -O2 -Wall -o "$OUT/svc_oh"      "$HERE/svc_oh.c"

# copy_libs <binary> <staging-root> — copy ldd-resolved host libs (the
# .so paths ldd prints are absolute; this also captures ld-linux.so)
copy_libs() {
    local bin=$1 stage=$2 lib dst
    for lib in $(ldd "$bin" | grep -o '/[^ ]*\.so[^ ]*'); do
        dst="$stage$(dirname "$lib")"
        mkdir -p "$dst"
        cp -L "$lib" "$dst/"
    done
}

echo "[2/5] staging ramdisk (initramfs with dynamic init + big-file stress)"
RD="$OUT/rd"
mkdir -p "$RD/bin" "$RD/lib" "$RD/etc" "$RD/usr" "$RD/vendor" "$RD/mnt"
install -m 0755 "$OUT/init_early" "$RD/init"
# large dynamic binary + symlink: the m3 frontier (toybox 538KB ramfs read)
cp -L /bin/bash "$RD/bin/bash"; chmod 0755 "$RD/bin/bash"
ln -s bash "$RD/bin/sh"
cp -L /bin/true "$RD/bin/true"; chmod 0755 "$RD/bin/true"
echo "oh-lite ramdisk" > "$RD/etc/motd"
# hardlink pair — cpio hardlink coverage (data-carrying entry LAST)
cp -L /bin/sync "$RD/bin/sync1"; chmod 0755 "$RD/bin/sync1"
ln "$RD/bin/sync1" "$RD/bin/sync2"
copy_libs "$RD/init" "$RD"
copy_libs "$RD/bin/bash" "$RD"
copy_libs "$RD/bin/sync1" "$RD"
# /lib64/ld-linux-x86-64.so.2 convenience symlink if lib64 absent
if [ ! -e "$RD/lib64/ld-linux-x86-64.so.2" ] && [ -e "$RD/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2" ]; then
    mkdir -p "$RD/lib64"
    ln -s ../lib/x86_64-linux-gnu/ld-linux-x86-64.so.2 "$RD/lib64/ld-linux-x86-64.so.2"
fi

echo "[3/5] staging system image (second-stage init + services)"
SYS="$OUT/sys"
mkdir -p "$SYS/bin" "$SYS/lib" "$SYS/etc" "$SYS/proc" "$SYS/sys" "$SYS/dev" \
         "$SYS/vendor" "$SYS/data" "$SYS/mnt" "$SYS/root" "$SYS/tmp" "$SYS/sbin"
chmod 1777 "$SYS/tmp"
install -m 0755 "$OUT/init_second" "$SYS/bin/init"
install -m 0755 "$OUT/svc_oh" "$SYS/bin/svc_oh"
ln -s svc_oh "$SYS/bin/svc_binder"
ln -s svc_oh "$SYS/bin/svc_ashmem"
ln -s svc_oh "$SYS/bin/svc_tokenid"
cp -L /bin/bash "$SYS/bin/bash"; chmod 0755 "$SYS/bin/bash"
ln -s bash "$SYS/bin/sh"
cat > "$SYS/etc/ohlite.conf" <<'EOF'
# OH-lite second-stage marker (parsed shape like OH init .cfg)
[path]
bin=/bin
EOF
mkdir -p "$SYS/etc/init"
cat > "$SYS/etc/init/ohlite.cfg" <<'EOF'
services = samgr-lite ashmem-lite tokenid-lite
EOF
copy_libs "$SYS/bin/init" "$SYS"
copy_libs "$SYS/bin/bash" "$SYS"
copy_libs "$SYS/bin/svc_oh" "$SYS"
if [ ! -e "$SYS/lib64/ld-linux-x86-64.so.2" ] && [ -e "$SYS/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2" ]; then
    mkdir -p "$SYS/lib64"
    ln -s ../lib/x86_64-linux-gnu/ld-linux-x86-64.so.2 "$SYS/lib64/ld-linux-x86-64.so.2"
fi

echo "[4/5] staging vendor + updater fillers"
echo "vendor image ready" > "$OUT/vendor/vendor_ready"
mkdir -p "$OUT/vendor/etc/init"
echo "services = none" > "$OUT/vendor/etc/init/demo.cfg"
echo "updater filler (keeps system at vdb)" > "$OUT/updater/updater_ready"

echo "[5/5] packing images"
python3 "$HERE/mkcpio.py" "$RD" "$IMAGES/ramdisk.img"
# Rux ext4 requirements (the repo-wide image convention): 4K blocks,
# no metadata_csum. e2fsprogs 1.47 additionally defaults
# metadata_csum_seed (incompat 0x2000) and orphan_file — the kernel
# refuses both at mount with ENODEV ("unsupported incompat features
# 0x2000"), which stalls the second-stage boot at "no /usr".
OFLAG='-O ^metadata_csum,^metadata_csum_seed,^orphan_file'
mke2fs -q -t ext4 -b 4096 $OFLAG -L ohlite-system -d "$SYS" \
    "$IMAGES/system.img" 32768
mke2fs -q -t ext4 -b 4096 $OFLAG -L ohlite-vendor -d "$OUT/vendor" \
    "$IMAGES/vendor.img" 4096
mke2fs -q -t ext4 -b 4096 $OFLAG -L ohlite-updater -d "$OUT/updater" \
    "$IMAGES/updater.img" 2048

ls -la "$IMAGES"
echo "OH-lite images ready: $IMAGES"
