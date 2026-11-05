#!/bin/sh
cd /test
echo LMBENCH START
echo "LMBENCH RUN lat_syscall null"
timeout -t 120 /test/lat_syscall null 2>&1 | tail -20
echo "LMBENCH RUN lat_syscall read"
timeout -t 120 /test/lat_syscall read 2>&1 | tail -20
echo "LMBENCH RUN lat_syscall write"
timeout -t 120 /test/lat_syscall write 2>&1 | tail -20
echo "LMBENCH RUN lat_syscall stat"
timeout -t 120 /test/lat_syscall stat 2>&1 | tail -20
echo "LMBENCH RUN lat_syscall fstat"
timeout -t 120 /test/lat_syscall fstat 2>&1 | tail -20
echo "LMBENCH RUN lat_syscall open"
timeout -t 120 /test/lat_syscall open 2>&1 | tail -20
echo "LMBENCH RUN lat_pagefault 1M"
timeout -t 120 /test/lat_pagefault 1M 2>&1 | tail -20
echo "LMBENCH RUN lat_mmap 1M"
timeout -t 120 /test/lat_mmap 1M 2>&1 | tail -20
echo "LMBENCH RUN lat_pipe "
timeout -t 120 /test/lat_pipe  2>&1 | tail -20
echo "LMBENCH RUN lat_unix "
timeout -t 120 /test/lat_unix  2>&1 | tail -20
echo "LMBENCH RUN lat_fifo "
timeout -t 120 /test/lat_fifo  2>&1 | tail -20
echo "LMBENCH RUN lat_ctx 2 4 8"
timeout -t 120 /test/lat_ctx 2 4 8 2>&1 | tail -20
echo "LMBENCH RUN lat_proc fork"
timeout -t 120 /test/lat_proc fork 2>&1 | tail -20
echo "LMBENCH RUN lat_proc exec"
timeout -t 120 /test/lat_proc exec 2>&1 | tail -20
echo "LMBENCH RUN lat_sem "
timeout -t 120 /test/lat_sem  2>&1 | tail -20
echo "LMBENCH RUN lat_fs 1k"
timeout -t 120 /test/lat_fs 1k 2>&1 | tail -20
echo "LMBENCH RUN lat_mem_rd 512"
timeout -t 120 /test/lat_mem_rd 512 2>&1 | tail -20
echo "LMBENCH RUN lat_mem_rd 1m"
timeout -t 120 /test/lat_mem_rd 1m 2>&1 | tail -20
echo "LMBENCH RUN bw_mem 1m"
timeout -t 120 /test/bw_mem 1m 2>&1 | tail -20
echo "LMBENCH RUN bw_mem 4m"
timeout -t 120 /test/bw_mem 4m 2>&1 | tail -20
echo "LMBENCH RUN bw_pipe "
timeout -t 120 /test/bw_pipe  2>&1 | tail -20
echo "LMBENCH RUN bw_unix "
timeout -t 120 /test/bw_unix  2>&1 | tail -20
echo "LMBENCH RUN bw_file_rd 1m"
timeout -t 120 /test/bw_file_rd 1m 2>&1 | tail -20
echo "LMBENCH RUN bw_mmap_rd 1m"
timeout -t 120 /test/bw_mmap_rd 1m 2>&1 | tail -20
echo "LMBENCH RUN lat_tcp "
timeout -t 120 /test/lat_tcp  2>&1 | tail -20
echo "LMBENCH RUN lat_udp "
timeout -t 120 /test/lat_udp  2>&1 | tail -20
echo "LMBENCH RUN lat_connect "
timeout -t 120 /test/lat_connect  2>&1 | tail -20
echo "LMBENCH RUN bw_tcp "
timeout -t 120 /test/bw_tcp  2>&1 | tail -20
echo "LMBENCH RUN bw_udp "
timeout -t 120 /test/bw_udp  2>&1 | tail -20
echo "LMBENCH RUN lat_usleep "
timeout -t 120 /test/lat_usleep  2>&1 | tail -20
echo "LMBENCH DONE rc=0"
poweroff -f || reboot -f
