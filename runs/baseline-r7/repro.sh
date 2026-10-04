#!/bin/sh
echo REPRO-SCRIPT-START
r=0
while [ $r -lt 6 ]; do
  echo ROUND-$r-BEGIN
  head -c 268435456 /dev/zero > /tmp/big
  ( dd if=/tmp/big of=/dev/null bs=1048576 skip=0 count=8 ) & ( dd if=/tmp/big of=/dev/null bs=1048576 skip=31 count=8 ) & ( dd if=/tmp/big of=/dev/null bs=1048576 skip=62 count=8 ) & ( dd if=/tmp/big of=/dev/null bs=1048576 skip=93 count=8 ) & ( dd if=/tmp/big of=/dev/null bs=1048576 skip=124 count=8 ) & ( dd if=/tmp/big of=/dev/null bs=1048576 skip=155 count=8 ) & ( dd if=/tmp/big of=/dev/null bs=1048576 skip=186 count=8 ) & ( dd if=/tmp/big of=/dev/null bs=1048576 skip=217 count=8 ) & sleep 0
  j=0; while [ $j -lt 40 ]; do ( head -c 5242880 /tmp/big > /dev/null ) & j=$((j+1)); done
  j=0; while [ $j -lt 20 ]; do ( cat /tmp/big > /dev/null ) & j=$((j+1)); done
  wait
  rm /tmp/big
  echo ROUND-$r-END
  r=$((r+1))
done
echo REPRO-SCRIPT-ALL-DONE
