#!/bin/sh
# hpcheck — post-hotplug assertions, run from the RES> prompt after the
# host did `device_add virtio-blk-pci` (QMP) + `echo 1 > /sys/bus/pci/rescan`.
# Prints HP:HOT-OK or HP:HOT-FAIL <what>.
fail=""

[ -b /dev/vdb ]                || fail="$fail vdb-node"
[ -f /sys/class/block/vdb/dev ] || fail="$fail sysfs-vdb"
[ -f /sys/class/block/vdb/size ] || fail="$fail sysfs-vdb-size"
grep -q '^254:16$' /sys/class/block/vdb/dev 2>/dev/null || fail="$fail vdb-devno"
grep -q 'devpath=/class/block/vdb' /run/hpevent.log 2>/dev/null || fail="$fail uevent-log"
grep -q 'DEVNAME=vdb' /run/hpevent.log 2>/dev/null || fail="$fail uevent-devname"
grep -q 'SEQNUM=' /run/hpevent.log 2>/dev/null || fail="$fail uevent-seqnum"

if [ -z "$fail" ]; then
    echo "HP:HOT-OK vdb=$(stat -c %t:%T /dev/vdb) size=$(cat /sys/class/block/vdb/size)"
else
    echo "HP:HOT-FAIL$fail"
fi

echo "--- /run/hpevent.log (uevent daemon) ---"
cat /run/hpevent.log
echo "--- ls -l /dev/vd* /dev/input ---"
ls -l /dev/vd* /dev/input 2>&1
