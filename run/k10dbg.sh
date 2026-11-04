#!/bin/sh
export LTP_IPC_PATH=/tmp/ltp-ipc
mount -t proc proc /proc 2>/dev/null
echo "K10DBG-BEGIN"
/test/linux-ltp/testcases/bin/kill10 -d 8 -n 3 -g 1 > /tmp/k10.out 2>&1 &
KPID=$!
sleep 25
echo "K10DBG-PS-BEGIN"
ps 2>/dev/null || cat /proc/ps 2>/dev/null || echo no-ps
echo "K10DBG-PS-END"
kill -9 $KPID 2>/dev/null
echo "K10DBG-OUT-BEGIN"
cat /tmp/k10.out
echo "K10DBG-OUT-END"
echo "SCAN_DONE"
exec /bin/sh
