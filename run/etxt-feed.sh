#!/bin/bash
# etxt-feed.sh — feed the ETXTBSY lifecycle probe with per-line delays
# (kernel console prints interleave with undelayed stdin, corrupting cmds).
{
  sleep 12
  echo "cp /test/linux-ltp/testcases/bin/creat07_child /tmp/bb"
  sleep 2
  echo "/tmp/bb &"
  sleep 3
  echo "echo x > /tmp/bb && echo WRITE_RUNNING=BAD || echo WRITE_RUNNING=ETXTBSY_GOOD"
  sleep 2
  echo "kill -9 \$(pidof bb)"
  sleep 3
  echo "echo y > /tmp/bb && echo WRITE_AFTER_EXIT=OK || echo WRITE_AFTER_EXIT=FAIL"
  sleep 2
  echo "ETXT_LIFECYCLE_DONE"
  sleep 2
  echo "poweroff -f"
  sleep 5
} 
