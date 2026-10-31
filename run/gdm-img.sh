#!/bin/bash
# gdm-img.sh — build the gdm login bring-up image (work/gnome-gdm-r1.img).
#
# Goal (session 17): the real gdm login screen — gdm → greeter → PAM →
# logind seat → accountsservice — on the existing kernel+image stack.
# The r13 image boots GNOME via runit's direct chain (no display
# manager, no PAM, no logind). This script snapshots the r13 staging
# tree, layers the gdm chain on top, and packs a SEPARATE image so the
# r13 asset/verification flow is untouched:
#   - gdm3 + closure (apt download, dpkg-deb -x — same as build-img.sh)
#     including the systemd package (provides systemd-logind, run
#     STANDALONE by runit's new "gdm" mode — no systemd PID1)
#   - users: gdm (uid 120, greeter) + rux (uid 1000, password "rux")
#     so the greeter has an account to list and PAM has a real
#     /etc/shadow hash to verify
#   - PAM fixup: pam_loginuid lines (if any) downgraded to optional —
#     /proc/self/loginuid does not exist on Rux and a "required" line
#     would deny every login before the chain is even mapped
#   - runit recompiled with the "gdm" mode (kernel cmdline
#     runit-mode=gdm selects it per boot; -snapshot boots stay possible
#     in session/gs modes on the same image)
# Boot (first-boot blocker mapping — current kernel, no VT layer yet):
#   python3 test/ubuntu-gnome/fire.py --tag gdmr1 --img work/gnome-gdm-r1.img \
#     --seconds 960 --dump-every 30 --dfx watchdog,inputtrace \
#     --extra-append "runit-mode=gdm"
set -euo pipefail
WT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$WT/work"
ROOT="$WORK/gnome-root-gdm"
IMG="$WORK/gnome-gdm-r1.img"
BASE_ROOT="$WORK/gnome-root-riscv64"   # r13-era staging tree (pristine)

command -v dpkg-deb >/dev/null || { echo "missing dpkg-deb" >&2; exit 1; }
[ -d "$BASE_ROOT" ] || { echo "missing staging tree $BASE_ROOT" >&2; exit 1; }

# Same musl-CC recipe as build-img.sh for runit.
MUSL="$WT/toolchain/riscv64-rux-linux-musl"
if [ -f "$MUSL/lib/crt1.o" ]; then
    CC() {
        riscv64-linux-gnu-gcc -static -march=rv64gc_zicsr -O2 -nostdinc \
            -isystem /usr/lib/gcc-cross/riscv64-linux-gnu/13/include \
            -isystem "$MUSL/include" -nostdlib "$MUSL/lib/crt1.o" \
            -L"$MUSL/lib" -o "$1" "$2" -lc -lgcc
    }
else
    CC() { riscv64-linux-gnu-gcc -static -O2 -o "$1" "$2"; }
fi

echo "== snapshotting staging tree"
rm -rf "$ROOT"
cp -a --reflink=auto "$BASE_ROOT" "$ROOT"

echo "== fetching gdm chain (apt download + dpkg-deb -x — no chroot)"
APT_SRC="$(mktemp)"
APT_LIST="$WORK/gnome-apt-lists"
APT_CACHE="$WORK/gnome-apt-cache"
mkdir -p "$APT_LIST/partial" "$APT_CACHE"
cat > "$APT_SRC" <<'EOS'
deb [trusted=yes] http://mirrors.aliyun.com/ubuntu-ports/ jammy main restricted universe multiverse
deb [trusted=yes] http://mirrors.aliyun.com/ubuntu-ports/ jammy-updates main restricted universe multiverse
EOS
# NOTE on resolution: do NOT recurse over gdm3 itself — its Depends
# line has the wide alternative chain "ubuntu-session | gnome-session
# | x-session-manager | x-window-manager | x-terminal-emulator", and
# apt-cache depends --recurse walks EVERY provider of every virtual
# (observed: enlightenment-data 382M, fluid-soundfont 124M, clisp,
# guile — 800+ debs of unrelated desktop extras). Instead: recurse on
# the concrete runtime list (gdm3's own missing libs, named
# explicitly) and download the gdm3 deb WITHOUT recursion. The final
# DT_NEEDED check below catches anything the hand list missed.
PKGS="systemd udev libpam-modules libpam-runtime libpam-systemd policykit-1 dconf-cli \
libgdm1 gir1.2-gdm-1.0 libaudit1 libcanberra-gtk3-0 libcanberra0 \
libgudev-1.0-0 libkeyutils1 libselinux1 libxdmcp6"
AOPTS=(-o Dir::Etc::sourcelist="$APT_SRC" -o Dir::Etc::sourceparts=-
       -o Dir::State::lists="$APT_LIST" -o Dir::State::status=/dev/null
       -o Dir::Cache="$APT_CACHE"
       -o APT::Architecture=riscv64 -o Architectures=riscv64
       -o Acquire::AllowInsecureRepositories=true -o APT::Get::AllowUnauthenticated=true)
DEBDIR="$WORK/gnome-debs-gdm"
rm -rf "$DEBDIR"; mkdir -p "$DEBDIR"
( cd "$DEBDIR" && apt-get "${AOPTS[@]}" download $(apt-cache "${AOPTS[@]}" \
        depends --recurse --no-recommends --no-suggests \
        --no-conflicts --no-breaks --no-replaces --no-enhances \
        $PKGS 2>/dev/null | grep "^\w" | sort -u) ) || echo "warn: some downloads failed"
echo "== gdm3 deb (no recursion — its closure is the explicit list + r13 tree)"
( cd "$DEBDIR" && apt-get "${AOPTS[@]}" download gdm3 ) \
    || echo "warn: gdm3 download failed"

echo "== extracting $(ls "$DEBDIR"/*.deb 2>/dev/null | wc -l) debs"
for d in "$DEBDIR"/*.deb; do
    dpkg-deb -x "$d" "$ROOT" 2>/dev/null || echo "warn: extract failed: $(basename "$d")"
done

echo "== DT_NEEDED check (gdm chain binaries vs tree)"
# Anything still missing shows up here as a soname with no file in the
# tree — fix by adding the package to PKGS above.
SL="$ROOT/usr/lib/riscv64-linux-gnu:$ROOT/lib/riscv64-linux-gnu"
MISSING=0
for b in "$ROOT/usr/sbin/gdm3" \
         "$ROOT/usr/libexec/gdm-session-worker" \
         "$ROOT/lib/systemd/systemd-logind" \
         "$ROOT/usr/lib/systemd/systemd-logind" \
         "$ROOT/usr/libexec/polkitd" "$ROOT/usr/lib/policykit-1/polkitd"; do
    [ -f "$b" ] || continue
    for so in $(riscv64-linux-gnu-readelf -d "$b" 2>/dev/null \
                | awk '/Shared library/ {gsub(/\[|\]/,"",$NF); print $NF}'); do
        # resolve against the three standard dirs of the tree
        found=""
        for dd in usr/lib/riscv64-linux-gnu lib/riscv64-linux-gnu \
                   usr/lib/systemd usr/lib/x86_64-linux-gnu; do
            [ -e "$ROOT/$dd/$so" ] && found=1 && break
        done
        if [ -z "$found" ]; then
            echo "   MISSING: $(basename "$b") needs $so"
            MISSING=1
        fi
    done
done
[ "$MISSING" = 0 ] && echo "   all DT_NEEDED present"

echo "== systemd1shim (gdmr54: org.freedesktop.systemd1 bus owner)"
# logind's CreateSession starts the session scope through
# org.freedesktop.systemd1; without the name owned, dbus activation
# fails ("Launch helper exited with unknown return code 1") and no
# logind session can exist for the greeter — gdm-x-session's
# RegisterDisplay then fails and the greeter exit-loops (gdmr53).
# The shim (test/ubuntu-gnome/systemd1shim.c) owns the name and
# completes every unit job instantly. Build: cross gcc against the
# tree's glib + the riscv64 glib headers extracted from the
# libglib2.0-dev deb into work/glibdev-riscv64 (kept between walks).
GLIBDEV="$WORK/glibdev-riscv64"
SHIM="$ROOT/usr/libexec/systemd1shim"
mkdir -p "$ROOT/usr/libexec"
if [ -d "$GLIBDEV/usr/include/glib-2.0" ]; then
    riscv64-linux-gnu-gcc -O2 -Wall -o "$SHIM" \
        "$WT/test/ubuntu-gnome/systemd1shim.c" \
        -I"$GLIBDEV/usr/include/glib-2.0" \
        -I"$GLIBDEV/usr/lib/riscv64-linux-gnu/glib-2.0/include" \
        "$ROOT/usr/lib/riscv64-linux-gnu/libgio-2.0.so" \
        "$ROOT/usr/lib/riscv64-linux-gnu/libgobject-2.0.so" \
        "$ROOT/usr/lib/riscv64-linux-gnu/libglib-2.0.so" \
        -Wl,-rpath-link,"$ROOT/usr/lib/riscv64-linux-gnu" \
        -Wl,-rpath-link,"$ROOT/lib/riscv64-linux-gnu" \
        -L/usr/riscv64-linux-gnu/lib \
        && cp "$SHIM" "$WORK/gnome-debs-gdm/systemd1shim.bin" 2>/dev/null \
        || echo "warn: systemd1shim build failed (needs glib headers)" >&2
    chmod 0755 "$SHIM" 2>/dev/null || true
elif [ -f "$WORK/gnome-debs-gdm/systemd1shim.bin" ]; then
    cp "$WORK/gnome-debs-gdm/systemd1shim.bin" "$SHIM"
    chmod 0755 "$SHIM"
else
    echo "warn: no glib headers, no banked shim — greeter needs systemd1" >&2
fi

echo "== users, groups, shadow, homes"
# gdm greeter user (uid 120 range like stock adduser) + login user.
# password for 'rux' is 'rux' (sha512-crypt, fixed salt — reproducible).
HASH="$(openssl passwd -6 -salt ruxgdm1 'rux')"
grep -q '^gdm:' "$ROOT/etc/passwd" 2>/dev/null || \
    echo 'gdm:x:120:120:G Display Manager:/var/lib/gdm:/bin/false' >> "$ROOT/etc/passwd"
grep -q '^rux:' "$ROOT/etc/passwd" 2>/dev/null || \
    echo 'rux:x:1000:1000:Rux Login User:/home/rux:/bin/bash' >> "$ROOT/etc/passwd"
grep -q '^gdm:' "$ROOT/etc/group" 2>/dev/null || echo 'gdm:x:120:' >> "$ROOT/etc/group"
grep -q '^rux:' "$ROOT/etc/group" 2>/dev/null || echo 'rux:x:1000:' >> "$ROOT/etc/group"
# video/audio membership: stock gdm adds these so the greeter may open
# devices even though Xorg runs as root (harmless if groups exist).
if grep -q '^video:' "$ROOT/etc/group" && ! grep -q '^video:.*gdm' "$ROOT/etc/group"; then
    sed -i 's/^video:\([0-9]*\):\(.*\)$/video:\1:\2,gdm/' "$ROOT/etc/group"
fi
if grep -q '^audio:' "$ROOT/etc/group" && ! grep -q '^audio:.*gdm' "$ROOT/etc/group"; then
    sed -i 's/^audio:\([0-9]*\):\(.*\)$/audio:\1:\2,gdm/' "$ROOT/etc/group"
fi
touch "$ROOT/etc/shadow"
grep -q '^gdm:' "$ROOT/etc/shadow" || echo 'gdm:!:19400:0:99999:7:::' >> "$ROOT/etc/shadow"
grep -q '^rux:' "$ROOT/etc/shadow" || echo "rux:$HASH:19400:0:99999:7:::" >> "$ROOT/etc/shadow"
install -d -m 0755 "$ROOT/var/lib/gdm" "$ROOT/var/lib/gdm/.config" "$ROOT/home/rux"

echo "== PAM fixup (loginuid + pam_systemd)"
# pam_loginuid writes /proc/self/loginuid — absent on Rux. A "required"
# line would deny every login before the rest of the chain is mapped.
for f in "$ROOT"/etc/pam.d/gdm-password "$ROOT"/etc/pam.d/gdm-autologin; do
    [ -f "$f" ] || continue
    if grep -q 'pam_loginuid' "$f"; then
        sed -i 's/^\(session\s*\)required\(.*pam_loginuid.*\)$/\1optional\2  # RUX: no loginuid/' "$f"
        echo "   trimmed: $(basename "$f")"
    fi
done
# pam_systemd (from libpam-systemd, extracted above): stock Ubuntu has
# it in common-session; dpkg-deb never runs pam-auth-update, so add the
# line ourselves. It makes the greeter/login PAM sessions call login1
# CreateSession — logind then owns sessions/c1 + users/120, which
# gdm-manager's RegisterDisplay lookup reads (see gdm-repack.sh).
grep -q 'pam_systemd' "$ROOT/etc/pam.d/common-session" || \
    sed -i '/^session[[:space:]]*required[[:space:]]*pam_unix.so/a session\toptional\tpam_systemd.so debug' \
        "$ROOT/etc/pam.d/common-session"
grep pam_systemd "$ROOT/etc/pam.d/common-session" | sed 's/^/   /'
echo "-- /etc/pam.d/gdm-password as booted:"
sed 's/^/   /' "$ROOT/etc/pam.d/gdm-password" 2>/dev/null || echo "   (missing)"

echo "== gdm3 custom.conf (X-only greeter, debug, proven fbdev X server)"
# WaylandEnable=false: this stack has no Wayland; gdm must use Xorg for
# the greeter or it dies trying the Wayland compositor path.
sed -i 's/^#WaylandEnable=false/WaylandEnable=false/' "$ROOT/etc/gdm3/custom.conf"
# [debug] Enable: gdm's own verbose logging to /tmp/gdm3.log.
sed -i 's/^#Enable=true/Enable=true/' "$ROOT/etc/gdm3/custom.conf"
# [server-X0]: gdm's stock server launcher lets autoconfig pick the
# driver — the first boot chose modesetting on /dev/dri/card0 (the
# Rux DRM node), an unproven path on this stack. Pin gdm's X server to
# the EXACT proven fbdev invocation the r13 desktop uses (same conf,
# same static evdev inputs). gdm appends :0 -auth ... itself.
if ! grep -q '^\[server-X0\]' "$ROOT/etc/gdm3/custom.conf"; then
cat >> "$ROOT/etc/gdm3/custom.conf" <<'EOG'

[server-X0]
name=Rux fbdev server
command=/usr/lib/xorg/Xorg -config /etc/X11/xorg-fbdev.conf -noreset -verbose 3
flexible=true
EOG
fi
echo "-- custom.conf as booted:"
sed 's/^/   /' "$ROOT/etc/gdm3/custom.conf"

echo "== /var/log/gdm3 (gdm3 LogDir — greeter stderr evidence)"
# gdm-session-worker writes the greeter's stdout/stderr to
# <LogDir>/:0-greeter.log; without the directory the open fails and the
# greeter's own failure messages ("Unable to ..." from gdm-x-session's
# EX_SOFTWARE exits) go nowhere at all (gdmr55: exit 70 with zero
# diagnostic surface).
mkdir -p "$ROOT/var/log/gdm3"
chmod 0755 "$ROOT/var/log/gdm3"

echo "== gdm-theme.gresource (update-alternatives gap)"
# gnome-shell-common's postinst installs gdm-theme.gresource via
# update-alternatives -> gnome-shell-theme.gresource (priority 10).
# dpkg-deb -x never runs postinst, so the link would be absent and
# gnome-shell's reloadThemeResource (main.js:497) ENOENTs -> shell
# exits(1) -> gdm-session re-starts it forever (the gdmr48 loop).
ln -sfn gnome-shell-theme.gresource \
    "$ROOT/usr/share/gnome-shell/gdm-theme.gresource"

echo "== /etc/X11/xorg.conf (gdmr51: the greeter Xorg driver choice)"
# gdm's [server-X0] command passes -config /etc/X11/xorg-fbdev.conf,
# but the greeter Xorg runs as uid 120 (gdm-x-session) and Xorg only
# honors an absolute -config path for root — for every other user it
# silently falls back to autoconfig (gdmr50 Xorg.0.log), which picks
# modesetting+glamor+gbm on /dev/dri/card0 and crashes in the gbm
# DRI-driver loader. The SYSTEM default xorg.conf is honored for all
# users: same proven fbdev+shadow config, now the default.
if [ -f "$ROOT/etc/X11/xorg-fbdev.conf" ]; then
    cp "$ROOT/etc/X11/xorg-fbdev.conf" "$ROOT/etc/X11/xorg.conf"
fi

echo "== gsettings schemas refresh"
command -v glib-compile-schemas >/dev/null && \
    glib-compile-schemas "$ROOT/usr/share/glib-2.0/schemas/" \
        --targetdir="$ROOT/usr/share/glib-2.0/schemas/" || true

echo "== runit (with gdm mode)"
CC "$ROOT/sbin/runit" "$WT/test/ubuntu-gnome/runit.c"

echo "== pack (fakeroot: root-owned tree, suid bits)"
HELPER=""
for c in "$ROOT/usr/lib/dbus-1.0/dbus-daemon-launch-helper" \
         "$ROOT/usr/lib/dbus-1/dbus-daemon-launch-helper"; do
    [ -f "$c" ] && HELPER="${c#"$ROOT"}"
done
[ -n "$HELPER" ] || { echo "dbus launch helper not found" >&2; exit 1; }
WORKER="/usr/libexec/gdm-session-worker"
[ -f "$ROOT$WORKER" ] || { echo "gdm-session-worker not found" >&2; exit 1; }
export ROOT IMG HELPER WORKER
fakeroot -- sh -c '
    chown -R 0:0 "$ROOT"
    chown 0:105 "$ROOT$HELPER";  chmod 4754 "$ROOT$HELPER"
    chown 0:0 "$ROOT$WORKER";    chmod 4755 "$ROOT$WORKER"
    chown -R 120:120 "$ROOT/var/lib/gdm"
    chown -R 1000:1000 "$ROOT/home/rux"
    mkfs.ext4 -b 4096 -O ^metadata_csum -d "$ROOT" "$IMG" 3800M >/dev/null
'

echo "== e2fsck gate"
FSCK_RC=0
e2fsck -fy "$IMG" >/dev/null 2>&1 || FSCK_RC=$?
[ "$FSCK_RC" -gt 1 ] && { echo "e2fsck rc=$FSCK_RC — NOT shippable" >&2; exit 1; }

echo "image ready: $IMG ($(du -h "$IMG" | cut -f1))"
echo "boot: python3 test/ubuntu-gnome/fire.py --tag gdmr1 --img $IMG --seconds 960 --dump-every 30 --dfx watchdog,inputtrace --extra-append 'runit-mode=gdm'"
