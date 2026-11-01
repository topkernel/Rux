# lmbench baseline — Rux x86_64 vs the reference kernel

**Status:** the optimization baseline for Rux. Every performance change
from here on is judged against this table, one case at a time.

- **Rux column:** the Rux x86_64 kernel under QEMU **KVM**
  (`-accel kvm -cpu host`), booting the shared x86_64 Ubuntu rootfs —
  the same image the LTP x86 leg uses — with the lmbench binaries
  injected under /test.
- **Reference column:** the **same static binaries** executed natively
  on the host (the reference kernel), on the same CPU, in the same
  session. Same code, same machine, one variable: the kernel.
- **Rig:** 9950X host, QEMU 10.2.1, `-smp 2 -m 2G`, Rux built with the
  strong profile (LTO, codegen-units=1, stripped).
- **Method:** one benchmark per boot (the serial console interleaves
  concurrent kernel prints with a long sweep, so a single-case boot is
  the only clean collection); the case travels on the kernel cmdline
  (`lmbench=<bench>,<args>` — the cmdline parser does not preserve
  quotes), each case has a 35 s in-guest timeout (60 s outer boot
  cap), and the runner is PID 1 (`init=/test/lmbench_run`), the same
  shape as the LTP lane's `test/lmpscan`-style runner. The runner
  also creates the shared 16 MiB scratch file
  (`/tmp/lmbench.file`) for the file-backed cases and forks the
  `"<bench> -s"` server locally for the client/server cases
  (lat_tcp / lat_udp / lat_connect / bw_tcp), mirroring exactly what
  the reference-column driver does on the host.

Two upstream (lmbench 3.0-a9) cases do not measure on EITHER column
and are dropped from the case list rather than reported as gaps:

- **lat_fifo** — its source forks its own writer (no external reader
  needed), but the benchmark never completes even natively: the fifo
  ping-pong spams `(w) read/write on pipe: No such file or
  directory` and has to be killed at the window edge. Verified with
  both the musl static build and a glibc build — broken upstream, not
  a rig artifact.
- **bw_udp** — its default message size equals the transfer size
  (10 MB datagrams, which UDP cannot send); with a sane `-m 32768`
  the client still stalls waiting for datagrams past 120 s natively.
  No reference number is obtainable, so no ratio can exist.

## Reproduce

```bash
userspace/lmbench/extract-lmbench.sh     # unpack the tarball (sources are not in git)
userspace/lmbench/build-lmbench.sh       # 24 static musl benchmarks
userspace/lmbench/build-rootfs.sh work/lmbench-x86.img   # inject into a copy of the Ubuntu rootfs
userspace/lmbench/run-rux.sh             # the Rux column  -> run/lmbench-rux.txt
userspace/lmbench/run-linux.sh           # the reference column -> run/lmbench-linux.txt
python3 userspace/lmbench/make-report.py run/lmbench-rux.txt run/lmbench-linux.txt
```

## Results

<!--RESULTS-->

| Case | Rux (x86_64+KVM) | Reference kernel | Ratio |
|---|---|---|---|
| `lat_syscall null` | 0.095 microseconds | 0.044 microseconds | 2.15x |
| `lat_syscall read` | 0.234 microseconds | 0.069 microseconds | 3.41x |
| `lat_syscall write` | 0.198 microseconds | 0.064 microseconds | 3.09x |
| `lat_syscall stat` | 4.077 microseconds | 0.217 microseconds | 18.77x |
| `lat_syscall fstat` | 0.17 microseconds | 0.111 microseconds | 1.53x |
| `lat_syscall open` | 5.685 microseconds | 0.41 microseconds | 13.88x |
| `lat_pagefault /tmp/lmbench.file` | 0.449 microseconds | 0.07 microseconds | 6.45x |
| `lat_mmap 1m /tmp/lmbench.file` | 1.049 us | 4.41 us | **0.24x (faster)** |
| `lat_pipe` | 2.006 microseconds | 2.745 microseconds | **0.73x (faster)** |
| `lat_unix` | 2.688 microseconds | 3.011 microseconds | **0.89x (faster)** |
| `lat_ctx -s 64 2` | 0.73 us/ctxsw | 0.91 us/ctxsw | **0.80x (faster)** |
| `lat_ctx -s 64 8` | 1.05 us/ctxsw | 1.3 us/ctxsw | **0.81x (faster)** |
| `lat_proc fork` | 672.936 microseconds | 33.133 microseconds | 20.31x |
| `lat_proc exec` | 673.619 microseconds | 57.54 microseconds | 11.71x |
| `lat_sem` | **TIMEOUT** | 0.444 microseconds | - |
| `lat_fs -s 1k` | n/a | 438368 files/s | - |
| `lat_mem_rd -s 1048576 -e 1048576 1` | 2.283 ns/load | 2.437 ns/load | **0.94x (faster)** |
| `lat_mem_rd -s 268435456 -e 268435456 256` | 6.794 ns/load | 6.753 ns/load | 1.01x |
| `bw_mem 1m bcopy` | 79818.42 MB/s | 72596.57 MB/s | **1.10x (faster)** |
| `bw_mem 4m bzero` | 65799.4 MB/s | 60682.25 MB/s | **1.08x (faster)** |
| `bw_pipe` | 21515.66 MB/s | 10442.95 MB/s | **2.06x (faster)** |
| `bw_unix` | 2637.87 MB/s | 22782.66 MB/s | 8.64x slower |
| `bw_file_rd 1m io_only /tmp/lmbench.file` | 669.19 MB/s | 33600.68 MB/s | 50.21x slower |
| `bw_mmap_rd 1m mmap_only /tmp/lmbench.file` | 70917.84 MB/s | 74316 MB/s | 1.05x slower |
| `lat_tcp 127.0.0.1` | **TIMEOUT** | 4.446 microseconds | - |
| `lat_udp 127.0.0.1` | n/a | 4.37 microseconds | - |
| `lat_connect 127.0.0.1` | **TIMEOUT** | 86.887 microseconds | - |
| `bw_tcp 127.0.0.1` | **TIMEOUT** | 15195.35 MB/s | - |
| `lat_usleep 1000` | 5013.928 microseconds | 1052.529 microseconds | 4.76x |


*Measured 2026-11-01, 9950X host. Rux column under host loadavg
10–19 (32-core box shared with other agents' QEMU VMs), reference
column under 7.6–8.8 — both run strictly one case at a time. The
`lat_ctx -s 64 8`, `lat_fs -s 1k`, `bw_pipe`, `lat_connect` reference
rows and the `lat_mem_rd 1m`, `bw_mmap_rd` Rux rows were re-measured
single-case with a longer window (90–150 s) when the standard window
proved shorter than the benchmark's auto-calibration; identical
kernel, image and arguments.*

## Reading the table

- **Ratio** = Rux value / reference value for latency cases; for the
  `bw_*` throughput rows the ratio is printed inverted
  (reference/Rux) so that "Nx" always reads "Rux is Nx slower" and
  bold always marks Rux winning.
- **TIMEOUT** = the benchmark did not finish inside the in-guest
  budget. On Rux this is a **functional gap** (the case needs a
  syscall or device semantic the kernel does not implement yet), not a
  slow number — those are the first optimization targets, ahead of any
  micro-optimization.
- **n/a** = the case produced no parseable measurement on that side
  (both sides use the same binary and the same arguments, so a
  missing line usually means the case failed outright — see the
  queue entries for the per-case reason).

## Optimization queue

Ranked by the measured ratios above (the queue is the working list for
the per-case optimization work):

1. **Functional gaps (TIMEOUT / n-a rows) — each is a missing or
   wrong semantic, not a slow path.** Fixing one turns a dead row
   into a number:
   - **lat_sem** — the writer's blocking 2-op `semop` (sem_flg=0)
     returns `EAGAIN`, which the reference kernel never does for a
     non-IPC_NOWAIT wait. Reproduced with serial evidence
     (`run/repro-lat_sem.log` in the lane worktree: `(w) error on
     semaphore: Resource temporarily unavailable`, plus
     `WAKE-HEAL-SKIP` traces). Suspect: `kernel/src/ipc/sysv_sem.rs`
     around the `blocking_idx` match — when
     `find_blocking_op()` returns `None` after
     `try_apply_semops()` reported NEED_BLOCK (a concurrent post
     raced the scan), the code returns `-EAGAIN` where it must
     `continue` the retry loop.
   - **lat_tcp / lat_connect / bw_tcp** — loopback TCP: the in-guest
     server reaches `tcp-listen`, the client's `connect(127.0.0.1)`
     never completes (hangs to the kill; no error printed).
   - **lat_udp** — loopback UDP: the client's handshake datagram gets
     no reply within its own deadline (`Recv timed out` from
     lat_udp.c), so the case exits instead of hanging.
   - **lat_fs** — no functional error, but the mk/rm calibration
     does not finish even in a 300 s single-case window: it creates
     thousands of 1k files, and at the measured 14–19x per-call file
     cost that is minutes. The kernel also prints
     `readlink fail(-2)` traces for its `/tmp/lat_fs_*` tempdir
     pattern while it runs. Re-measure once file ops are faster.
2. **bw_file_rd (50x slower)** — 669 MB/s vs 33.6 GB/s on a
   page-cache-resident 1 MiB read. The read path (not the block
   device — the file is cached) dominates. Biggest single payoff in
   the table.
3. **lat_proc fork (20.3x) / exec (11.7x)** — the process lifecycle
   path: page-table teardown/copy, mm drop, ELF load. Both land at
   ~673 us, i.e. the shared fork cost dominates exec.
4. **lat_syscall stat (18.8x) / open (13.9x)** — the path-walk and
   inode/dentry side of VFS, vs 1.5–3.4x for the no-path syscalls
   (null/read/write/fstat). The directory walk, not the syscall
   entry, carries the cost.
5. **bw_unix (8.6x slower)** — AF_UNIX stream bandwidth 2.6 GB/s vs
   22.8 GB/s; note lat_unix round-trip is already at 0.89x, so the
   gap is in the bulk-copy path (writev/truesize), not the wakeup.
6. **lat_pagefault (6.5x) / lat_usleep (4.8x)** — demand-fault path
   and timer sleep granularity (5014 us for a 1000 us sleep — check
   the timer slack/rounding, not the syscall entry).
7. **Healthy, leave alone**: context switch (0.80–0.81x), pipe
   round-trip (0.73x) and pipe bandwidth (2.06x), mmap
   install/uninstall (0.24x), memory read latency and copy
   bandwidth (parity to better) — the scheduler wake paths and the
   page/zeroing paths are at or past the reference already.

## Notes and caveats

- The reference column runs **natively**, not under KVM; the guest
  adds one virtualization layer to the Rux column. That layer is not
  free (the guest pays for it on every syscall and every fault), so
  the Rux numbers include a cost the reference does not. The ratios
  are therefore conservative in Rux's favour.
- The first sweep of each case is a **cold** boot: caches are empty,
  the page cache is cold, and the first fault of each kind pays the
  demand-fault path in full. The reference column runs warm after the
  first case. Read the numbers as "cold-ish guest vs warm host" and
  re-run a case twice before drawing conclusions.
- lmbench's `lat_syscall` stat/fstat/open cases stat
  `/usr/include/linux/types.h`; the Ubuntu rootfs ships that header
  under the multiarch subdirectory, so `build-rootfs.sh` places a
  placeholder at the plain path. Without it those three cases fail
  with "No such file or directory" and report nothing.
