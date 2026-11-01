#!/bin/bash
# inject <name>: place binary and chunk.txt into the image via debugfs command file
set -eu
cd /home/william/rux-agents/review-compat
NAME="$1"
IMG=run/rootfs-ltp-compat.img
printf '%s\n' "$NAME" > /tmp/chunk-$NAME.txt
cat > /tmp/inject-$NAME.dbfs <<EOC
cd /test/linux-ltp
rm chunk.txt
write /tmp/chunk-$NAME.txt chunk.txt
cd testcases/bin
rm $NAME
write run/probe/$NAME $NAME
sif $NAME mode 0100755
EOC
debugfs -w -f /tmp/inject-$NAME.dbfs "$IMG" >/dev/null 2>&1
echo injected $NAME
