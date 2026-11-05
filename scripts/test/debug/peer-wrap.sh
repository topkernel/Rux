#!/bin/sh
# peer-wrap.sh - activated-service wrapper that captures stderr/exit so
# activation failures are diagnosable (dbus swallows child stdio).
echo "wrap start $(/bin/date +%s) pid=$$ ppid=$PPID" >> /root/peer.log
/usr/bin/python3.10 /root/dbuswire.py serve --own org.rux.Test >> /root/peer.log 2>&1
echo "wrap py rc=$? $(/bin/date +%s)" >> /root/peer.log
