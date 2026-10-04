#!/bin/sh
echo REPRO-SCRIPT-START
r=0
while [ $r -lt 10 ]; do
  echo ROUND-$r-BEGIN
  head -c 67108864 /dev/zero > /tmp/big
  dd if=/tmp/big of=/dev/null bs=1048576 skip=0 count=4 & dd if=/tmp/big of=/dev/null bs=1048576 skip=31 count=4 & dd if=/tmp/big of=/dev/null bs=1048576 skip=62 count=4 & dd if=/tmp/big of=/dev/null bs=1048576 skip=93 count=4 & dd if=/tmp/big of=/dev/null bs=1048576 skip=124 count=4 & dd if=/tmp/big of=/dev/null bs=1048576 skip=155 count=4 & dd if=/tmp/big of=/dev/null bs=1048576 skip=186 count=4 & dd if=/tmp/big of=/dev/null bs=1048576 skip=217 count=4 & sleep 0
  j=0; while [ $j -lt 8 ]; do head -c 5242880 /tmp/big > /dev/null & j=$((j+1)); done
  j=0; while [ $j -lt 4 ]; do cat /tmp/big > /dev/null & j=$((j+1)); done
  wait
  rm /tmp/big
  echo ROUND-$r-END
  r=$((r+1))
done
echo REPRO-SCRIPT-ALL-DONE
