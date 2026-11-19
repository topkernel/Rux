#!/bin/sh
# dbustest.sh - minimal D-Bus matrix on Rux (no GNOME). Streams to console.
exec > /dev/console 2>&1
export HOME=/root
P=/usr/bin/python3.10
W=/root/dbuswire.py
DBUS_SEND=/usr/bin/dbus-send

echo "DBUSTEST: begin"
/bin/mkdir /run/dbus 2>/dev/null

# ---------------- session bus ----------------
export XDG_DATA_DIRS=/root/xdg:/usr/local/share:/usr/share
if [ ! -S /run/dbus/session_bus_socket ]; then
    /usr/bin/dbus-daemon --session --fork --nopidfile \
        --address=unix:path=/run/dbus/session_bus_socket \
        >/root/dbus-session.log 2>&1
fi
export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/dbus/session_bus_socket
sleep 1
echo "== S0: pid map =="
echo "sh pid=$$ ppid=$PPID"
/bin/ps

echo "== A1: dbus-send to unknown non-activatable name (25s timeout) =="
S=$(/bin/date +%s)
$DBUS_SEND --session --print-reply --reply-timeout=25000 --dest=org.Not.Exist / org.Foo.Bar
RC=$?
E=$(/bin/date +%s)
echo "RSLT: A1 dbus-send rc=$RC elapsed=$((E-S))s"

echo "== A2: raw caller to unknown name =="
$P $W probe --dest org.Not.Exist --iface org.Foo --member Bar --timeout 25

echo "== A3: raw caller 100x ListNames (bus round-trip) =="
$P $W call --dest org.freedesktop.DBus --iface org.freedesktop.DBus \
    --member ListNames --count 100

echo "== B1v2: activation of org.rux.Test (sh wrapper -> python peer) =="
rm -f /root/peer.log /root/peer-owned /root/obs.log
/usr/bin/python3.10 /root/obs.py 16 >/root/obs.log 2>&1 &
OBS=$!
/bin/sleep 1
S=$(/bin/date +%s)
$P $W probe --dest org.rux.Test --iface org.rux.Test --member Echo --timeout 30
E=$(/bin/date +%s)
echo "RSLT: B1v2 wall=$((E-S))s"
if [ -f /root/peer-owned ]; then
    echo "RSLT: B1v2 peer-owned: $(/bin/cat /root/peer-owned)"
else
    echo "RSLT: B1v2 peer-owned MISSING"
fi
wait $OBS
echo "--- peer.log ---"
/bin/cat /root/peer.log 2>/dev/null || echo "(no peer.log)"
echo "--- obs.log ---"
/bin/cat /root/obs.log 2>/dev/null || echo "(no obs.log)"

echo "== B2: 100x Echo to the activated peer =="
$P $W call --dest org.rux.Test --iface org.rux.Test --member Echo --count 100

echo "== C1: same under 4 CPU burners =="
BP=""
for i in 1 2 3 4; do
    /bin/sh -c 'while :; do :; done' &
    BP="$BP $!"
done
sleep 1
$P $W call --dest org.rux.Test --iface org.rux.Test --member Echo --count 100
echo "== C2: bus round-trip under load =="
$P $W call --dest org.freedesktop.DBus --iface org.freedesktop.DBus \
    --member ListNames --count 100
echo "== C3: unknown name under load =="
$P $W probe --dest org.Not.Exist --iface org.Foo --member Bar --timeout 25
kill $BP 2>/dev/null

echo "== D1: dconf round-trip via gsettings (activates ca.desrt.dconf) =="
export GSETTINGS_BACKEND=dconf
S=$(/bin/date +%s)
/usr/bin/gsettings set org.gnome.desktop.interface clock-format '24h'
RC=$?
E=$(/bin/date +%s)
echo "RSLT: D1-set rc=$RC elapsed=$((E-S))s"
S=$(/bin/date +%s)
/usr/bin/gsettings get org.gnome.desktop.interface clock-format
RC=$?
E=$(/bin/date +%s)
echo "RSLT: D1-get rc=$RC elapsed=$((E-S))s"
S=$(/bin/date +%s)
/usr/bin/gsettings get org.gnome.desktop.interface clock-format
RC=$?
E=$(/bin/date +%s)
echo "RSLT: D1-get2 rc=$RC elapsed=$((E-S))s"
/bin/ps | /bin/grep dconf
echo "RSLT: D1 dconf-service ps above (if any)"

echo "== F1: nanosleep EFAULT repro (200 sleeps after fork) =="
$P - <<'PYEOF'
import os, time
errs = 0
for i in range(200):
    try:
        time.sleep(0.002)
    except OSError as e:
        errs += 1
        if errs <= 5:
            print("RSLT: F1 sleep err #%d at i=%d: %s" % (errs, i, e))
    pid = os.fork()
    if pid == 0:
        os._exit(0)
    os.waitpid(pid, 0)
print("RSLT: F1 done errs=%d/200" % errs)
PYEOF

echo "== G1: bare python sleep (no fork) =="
$P -c 'import time; print("G1 pre"); time.sleep(0.05); print("G1 post")'
echo "RSLT: G1 rc=$?"
echo "== G2: sleep immediately after fork+wait =="
$P - <<'PYEOF'
import os, time
pid = os.fork()
if pid == 0:
    os._exit(0)
os.waitpid(pid, 0)
print("G2 forked+reaped")
try:
    time.sleep(0.05)
    print("RSLT: G2 sleep ok")
except OSError as e:
    print("RSLT: G2 sleep err %s" % e)
PYEOF

echo "== E1: system bus + login1 activation (Xorg repro, Exec=/bin/false) =="
if [ ! -S /run/dbus/system_bus_socket ]; then
    /usr/bin/dbus-daemon --system --fork --nopidfile \
        --address=unix:path=/run/dbus/system_bus_socket \
        >/root/dbus-system.log 2>&1
fi
sleep 1
rm -f /root/obs-sys.log
/usr/bin/python3.10 /root/obs.py 30 >/root/obs-sys.log 2>&1 &
OBS2=$!
/bin/sleep 1
S=$(/bin/date +%s)
$DBUS_SEND --system --print-reply --reply-timeout=30000 \
    --dest=org.freedesktop.login1 /org/freedesktop/login1 \
    org.freedesktop.login1.Manager.ListSeats
RC=$?
E=$(/bin/date +%s)
echo "RSLT: E1 dbus-send rc=$RC elapsed=$((E-S))s"
wait $OBS2
echo "--- obs-sys.log ---"
/bin/cat /root/obs-sys.log 2>/dev/null | /bin/grep -v "comm=sh ppid" | head -60
echo "== E2: gdbus system login1 under load (GDBus caller, Xorg-like) =="
if [ ! -S /run/dbus/system_bus_socket ]; then
    /usr/bin/dbus-daemon --system --fork --nopidfile \
        --address=unix:path=/run/dbus/system_bus_socket \
        >/root/dbus-system.log 2>&1
fi
sleep 1
BP2=""
for i in 1 2; do
    /bin/sh -c 'while :; do :; done' &
    BP2="$BP2 $!"
done
S=$(/bin/date +%s)
/usr/bin/gdbus call --system --timeout 25000 \
    --dest org.freedesktop.login1 --object-path /org/freedesktop/login1 \
    --method org.freedesktop.login1.Manager.ListSeats
RC=$?
E=$(/bin/date +%s)
echo "RSLT: E2 gdbus rc=$RC elapsed=$((E-S))s"
kill $BP2 2>/dev/null
echo "DBUSTEST-DONE"
