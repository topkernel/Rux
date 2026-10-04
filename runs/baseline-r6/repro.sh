#!/bin/sh
echo REPRO-SCRIPT-START
r=0
while [ $r -lt 6 ]; do
  echo ROUND-$r-BEGIN
  head -c 268435456 /dev/zero > /tmp/big
  i=0; while [ $i -lt 8 ]; do ( dd if=/tmp/big of=/dev/null bs=1048576 skip=$((i*31+r*7)) count=8 ) & i=$((i+1)); done
  j=0; while [ $j -lt 40 ]; do ( head -c 5242880 /tmp/big > /dev/null ) & j=$((j+1)); done
  j=0; while [ $j -lt 20 ]; do ( cat /tmp/big > /dev/null ) & j=$((j+1)); done
  wait
  rm /tmp/big
  echo ROUND-$r-END
  r=$((r+1))
done
echo REPRO-SCRIPT-ALL-DONE
