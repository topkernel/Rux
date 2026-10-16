#!/bin/bash
# check.sh <tag> — one-shot milestone scan of an OH boot serial log.
L=/home/william/rux-agents/oh-display/run/oh-$1-serial.log
[ -f "$L" ] || { echo "no log"; exit 1; }
sz=$(stat -c %s "$L")
echo "== size $sz, tail state:"
tail -c 800 "$L" | strings | tail -4
echo "== milestones:"
for pat in 'is the context manager' 'starting:composer_host' 'starting:render_service' 'starting:bootanimation' 'so-open.*composer_driver' 'drm: open' 'binder-stall' 'unhandled ioctl' 'ServiceStart started info.*devhost' 'AddDevice\|AttachDeviceHost' 'Load.*uccess\|load driver'; do
  n=$(grep -ac "$pat" "$L"); echo "  [$n] $pat"
done
