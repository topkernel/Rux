#!/bin/bash
# oh-boot + periodic display screendumps via the QEMU monitor socket
cd /home/william/rux-agents/oh-launcher || exit 1
TAG="${1:-shot}"
SECS="${2:-420}"
MON="run/oh-$TAG-mon.sock"
SERIAL="run/oh-$TAG-serial.log"
for f in "$MON" "$SERIAL" run/oh-$TAG-shot-*.ppm; do rm -f "$f"; done
USERDATA=run/oh-img/userdata.img SECS="$SECS" bash run/oh-boot.sh "$TAG" &
BOOTPID=$!
( for t in 120 200 280 360; do
    sleep $(( t==120 ? 120 : 80 ))
    n=$(ls run/oh-$TAG-shot-*.ppm 2>/dev/null | wc -l)
    python3 - "$MON" "run/oh-$TAG-shot-$t.ppm" <<'PY'
import socket,sys
s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
try:
    s.connect(sys.argv[1]); s.settimeout(3)
    s.sendall(f"screendump {sys.argv[2]}\n".encode()); s.recv(4096)
except Exception as e: print("mon:",e)
PY
  done ) &
SHOTPID=$!
wait $SHOTPID 2>/dev/null
sleep 5; kill $BOOTPID 2>/dev/null
ls -la run/oh-$TAG-shot-*.ppm 2>/dev/null | awk '{print $5,$9}'
