#!/bin/bash
# rs-seq.sh <serial-log> <pid> [addr-hex-prefix]
# Extract one process's memory-surgery syscall sequence in serial order.
# Usage: ./run/rs-seq.sh run/rs-repro7-serial.log 693 0x7ff0006
set -u
LOG="${1:?log}"
PID="${2:?pid}"
PRE="${3:-}"
grep -o "\ \(PROTNONE-MAP\|MPROTECT-NONE\|MPROTECT-RW\|ANONFIXED-MAP\|BIGMUNMAP\|MPROTECT-FAIL\) pid ${PID} addr 0x[0-9a-f]* len 0x[0-9a-f]*\( prot 0x[0-9a-f]*\)\?" "$LOG" \
  | grep -v " $PRE" | head -0 # placeholder: prefix filter handled below
# (the grep above with -o already yields full matches; filter by prefix)
if [ -n "$PRE" ]; then
  grep -o "\ \(PROTNONE-MAP\|MPROTECT-NONE\|MPROTECT-RW\|ANONFIXED-MAP\|BIGMUNMAP\|MPROTECT-FAIL\) pid ${PID} addr 0x[0-9a-f]* len 0x[0-9a-f]*\( prot 0x[0-9a-f]*\)\?" "$LOG" | grep " addr ${PRE}" | cat -n
else
  grep -o "\ \(PROTNONE-MAP\|MPROTECT-NONE\|MPROTECT-RW\|ANONFIXED-MAP\|BIGMUNMAP\|MPROTECT-FAIL\) pid ${PID} addr 0x[0-9a-f]* len 0x[0-9a-f]*\( prot 0x[0-9a-f]*\)\?" "$LOG" | cat -n
fi
