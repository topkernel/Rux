#!/bin/bash
# sprint probe: dfx taskdump+watchdog boot with 4 screendumps
cd /home/william/rux-agents/oh-launcher || exit 1
TAG="${1:-dfx1}"
SECS="${2:-300}"
MON="run/oh-$TAG-mon.sock"
SERIAL="run/oh-$TAG-serial.log"
for f in "$MON" "$SERIAL" run/oh-$TAG-shot-*.ppm; do rm -f "$f"; done
USERDATA=run/oh-img/userdata.img SECS="$SECS" EXTRA_APPEND="dfx=taskdump,watchdog" bash run/oh-boot.sh "$TAG" &
BOOTPID=$!
( for t in 90 150 210 270; do
    sleep 60
    python3 - "$MON" "run/oh-$TAG-shot-$t.ppm" <<'PY'
import socket,sys
s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
try:
    s.connect(sys.argv[1]); s.settimeout(3)
    s.sendall(f"screendump {sys.argv[2]}\n".encode()); s.recv(4096)
except Exception as e: print("mon:",e)
PY
  done ) &
wait $BOOTPID 2>/dev/null
