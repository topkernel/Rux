#!/bin/sh
# start-probe-gnome.sh — replicate start-gnome.sh EXACTLY (same env, same
# dbus bring-up, same X0 wait) but run the gsd probes before launching the
# real gnome-session, so the discovery path can be observed on serial.
export HOME=/root
export SHELL=/bin/bash
export DISPLAY=:0
export XAUTHORITY=/root/.Xauthority
export LANG=C.UTF-8
export XDG_SESSION_TYPE=x11
export XDG_SESSION_CLASS=user
export XDG_RUNTIME_DIR=/run/user/0
export XDG_DATA_DIRS=/usr/local/share:/usr/share
export XDG_CONFIG_DIRS=/etc/xdg
export GDK_BACKEND=x11
export CLUTTER_BACKEND=x11
export LIBGL_ALWAYS_SOFTWARE=1
export NO_AT_BRIDGE=1

mkdir /run/user 2>/dev/null
mkdir /run/user/0 2>/dev/null
mkdir /run/dbus 2>/dev/null
mkdir /tmp/.X11-unix 2>/dev/null
chmod 700 /run/user/0 2>/dev/null
chmod 1777 /tmp /tmp/.X11-unix 2>/dev/null

# ---- wait for Xorg :0 to create its socket (bounded) ----
i=0
while [ ! -S /tmp/.X11-unix/X0 ]; do
    sleep 5
    i=$((i+1))
    if [ $i -ge 200 ]; then
        echo "start-probe: X0 socket never appeared"
        break
    fi
done
echo "PROBE-GNOME: X0 wait done (loops=$i)"

# system bus best effort like the original
if [ ! -S /run/dbus/system_bus_socket ]; then
    /usr/bin/dbus-daemon --system --fork --nopidfile >/root/dbus-system.log 2>&1
fi
# session bus like the original
if [ ! -S /run/dbus/session_bus_socket ]; then
    /usr/bin/dbus-daemon --session --fork --nopidfile \
        --address=unix:path=/run/dbus/session_bus_socket \
        >/root/dbus-session.log 2>&1
fi
export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/dbus/session_bus_socket

echo "PROBE-ENV: XDG_CONFIG_DIRS=$XDG_CONFIG_DIRS HOME=$HOME"
/root/gsdprobe
/root/glibprobe

echo "PROBE-GNOME: version check"
/usr/bin/gnome-session --version
echo "PROBE-GNOME: subchild write test"
( sleep 5; echo "PROBE-SUBCHILD-OK" ) &
echo "PROBE-GNOME: launching real gnome-session (log -> /tmp/gs.log tmpfs)"
/usr/bin/gnome-session --session=gnome >/tmp/gs.log 2>&1 &
GSPID=$!
sleep 150
echo "PROBE-GNOME: /tmp/gs.log follows"
cat /tmp/gs.log
echo "PROBE-GNOME: DONE"
