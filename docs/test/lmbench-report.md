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

## Open-tail anatomy (2026-11-17, fsw pathcost, 20.7M opens, nohz_full=1)

fsw=1 pathcost counters on the open/close storm (PC1 PRE bracket =
probe copy + root fetch; PC1 TAIL = File::new + Arc + fd install +
return; path_lookup decomposed separately):

- per-open: PRE 239 ns + TAIL 950 ns; path_lookup walk pre 93 ns +
  **loop 598 ns** + tail 22 ns + comp 4 ns — the per-component
  dcache walk (lookup_child_walk: memo version check + seqlock pair +
  name compare) is the largest single block (83% of the walk).
- fdreap/RCU finding: under the open/close storm the RCU grace
  cannot complete for 50+ ms (RCU-GP-BOUND, no quiescent reports from
  user-busy CPUs — no context tracking), the deferred queue grows, and
  the fdreap serial report needed its budget cut to 1/8192 (was
  flooding the sampled boot to 0 samples). The reference reports RCU
  QS from context tracking at user entry/exit — that mechanism is the
  structural fix (also a NOHZ dependency already noted in nohz.rs).
- File allocation already runs through the existing slab allocator
  (ArcInner ≤ 4096 class) — no filp-cache win there; the File::new
  field construction and the Arc drop's 5 registry lookups remain
  candidates, gated on a working sampler.
- Sampler tooling note: the perf-sample boot for open was starved by
  the same fdreap serial flood (0 samples); scripts saved as
  open_sample.sh/open_fsw.sh (absolute-path safe, warmer included).

## Standing round 3 (2026-11-17, kernel main=931623176 — PT_REGS gating)

perf-2x round 2: SYSCALL-vector entries skip the CURRENT_PT_REGS
load+store pair (fork/exec derive the frame from task.pt_regs(); the
slot's remaining readers are diagnostics served by the IRQ stores).

- write: **0.0811 µs** (med-of-3, reps 0.0800-0.0814) vs 0.082 round-1
  / 0.086-0.089 standing = −4% vs round 1, −7% cumulative.
- null 0.0435 / read 0.101 — both at their best-ever values, PT_REGS
  gating within noise for them (relaxed stores to an uncontended
  per-CPU line).
- Campaign scoreboard vs the 2× targets (Linux/2): null 0.0435 (target
  0.0195) · read 0.101 (0.030) · write 0.0811 (0.030) · stat 0.34
  (0.120) · open 0.84 (0.197) · fork 37.8 (17) · exec 40.6 (18.8).
- Open-tail anatomy (code-read, sampler starved by the fdreap serial
  flood): per-open = probe copy (fast path exists) → path_lookup walk
  → Arc::new(File) heap alloc → inode/dentry Arc bumps →
  install_lowest_fd entry lock. Next levers: File allocation slab
  cache (the reference's filp kmem_cache), getname-style name cache,
  and the r14 path-memo audit (memo not visible in file_open's path).
- Gates: triple-arch green, riscv suite green.

## Standing round 2 (2026-11-16 evening, kernel main=34b339e01 — fdget fast path + getppid O(1))

perf-2x round 1 landed (merge 34b339e01): the fdget no-refcount fast
path (unshared fdtable borrows the File from the lock-free slot — no
irq bracket, no rcu section, no refcount RMW; CLONE_FILES tables fall
back to fget) on read/write, and getppid follows the parent Task
pointer (task_tgid_vnr shape) instead of a pid-hash lookup per call.

- null: **0.045 µs** median (x5, 0.043-0.059) vs 0.051 standing = −12%.
- write: **0.082 µs** (x3, dead-stable 0.0819-0.0821) vs 0.086-0.089 = −6%.
- read: best 0.099 (band 0.099-0.132, n=3) vs 0.103-0.108 — in band.
- stat ~0.34, open ~0.84 — in standing bands.
- **Calibration stability: opt 14/15 boots converge vs base 0/15 in
  the same window** — the getppid hash lookup's lock/cacheline variance
  was itself a calibration-jitter source; the lmbench strand family
  (35 s budget) disappears for these rows on the opt kernel.
- pbench p2_ctxsw (1-byte pipe ping-pong, aff=1): **0.090-0.105 µs**
  (two independent boots) from the 1.24-2.15 class — the borrow path
  hits both sides of every roundtrip; −90%+. Pending: same-rig
  reference p2 for an honest ratio.
- Gates: Tier-1 census exact (8/32), GRQ hold 581 ns (release class,
  improvement — band banked on debug), volume 81M (family band edge);
  Tier-2 rows improvement-class vs the stale anchor; riscv suite green;
  triple-arch builds green.

2× campaign targets (Linux/2): null 0.0195 / read 0.030 / write 0.030 /
stat 0.120 / open 0.197 / fork 17 / exec 18.8 µs. Next levers: entry
floor (frame build + exit checks), open tail (memo hit + fd install),
fork/exec (fdtable/mm copy).

## Standing (2026-11-16, kernel main=017bf556b — NOHZ merged; guest-vs-guest correction; rig discipline)

Round facts, then the corrected column.

**Rig discipline (applies to every future battery):**
- Host CCD pinning: `taskset -c 0-7,16-23` (9950X CCD0). Cross-CCD
  vCPU placement is the single biggest strand amplifier — unpinned
  sequential boots strand calibration stochastically (~40%/boot),
  CCD0-pinned concurrent boots pass 4/4.
- `performance` cpufreq governor during measurement (powersave ramps
  mid-calibration and breaks the 0.25% proportionality check).
- A nice-19 warmer thread on one CCD0 logical CPU keeps the package out
  of deep idle between sequential boots (battery TIMEOUT rows dropped
  9 -> 3); remaining TIMEOUT rows get the retry rule.
- The native reference column (run-linux.sh) is a HARDWARE CEILING,
  never a ratio denominator for the wake/switch family. Ratios are
  guest-vs-guest only. (A 2026-11-16 table briefly published
  ctx/pipe/sem as 2-3.6x "slower" against the native column — wrong
  denominator, retired the same day.)

**Fresh same-session reference column:** the OHOS 6.6.101 bzImage
booted in the SAME guest rig (q35/KVM/-cpu host/smp2/2G, CCD0-pinned,
performance governor; wrapper init mounts /proc /sys /tmp — the Rux
runner relies on Rux auto-mounting /proc). 2 reps, medians. Note the
fresh column reads slower than the frozen 2026-11-04 reference window
(ctx 6.71 -> 12.9): the reference kernel's switch family is highly
condition-sensitive; ratios are only honest within one session.

| Case | Rux (nohz_full=1) | Reference (same-rig guest) | Ratio |
|---|---|---|---|
| lat_ctx -s 64 2 | **2.86-3.25 µs** | 12.84-12.97 µs | **ahead 4.0-4.5x** |
| lat_ctx -s 64 8 | **3.33 µs** | 12.06-12.08 µs | **ahead 3.6x** |
| lat_pipe | **5.54-5.64 µs** | 25.06-26.27 µs | **ahead 4.6x** |
| lat_sem | **1.52-1.57 µs** | 4.47-4.55 µs | **ahead 2.9x** |
| lat_proc fork | **37.8-40.3 µs** | 33.9 µs | 1.11-1.19x |
| lat_proc exec | 40.6 µs | 37.6 µs | 1.08x |
| lat_syscall fstat | **0.084-0.094 µs** | 0.119-0.120 µs | **ahead 1.3x** |
| lat_syscall null | 0.051 µs | 0.0345-0.0430 µs | 1.2-1.5x |
| lat_syscall write | 0.086-0.089 µs | 0.0597-0.0600 µs | 1.4-1.5x |
| lat_syscall read | 0.103-0.108 µs | 0.0601-0.0624 µs | 1.7-1.8x |
| lat_syscall stat | 0.322-0.446 µs (±30% boot var) | 0.239 µs | 1.4-1.9x |
| lat_syscall open | 0.756-1.00 µs (±30% boot var) | 0.394-0.397 µs | 1.9-2.5x |

Memory/bandwidth rows (from the same Rux battery, native ceiling as
context only): mem_rd 0.95-0.97x, bcopy 1.14x ahead, bzero 1.23x ahead,
bw_pipe 3.70x ahead, bw_unix 2.10x ahead, bw_file_rd >timer (see
correction below), unix latency 0.70x ahead, usleep 0.98x.

**NOHZ round (main 9f01767df, details in
docs/architecture/rust-native-register.md J):** interleaved strand A/B
on a hostile background — main tip 0/10, churn fix (entity_tick guard +
ENQ print removal) 5/12, nohz_full=1 11/12 PASS; p1_null 0.050 vs
0.060 µs (-17%); Tier-1 census bands hold (TERM 8, REFUSED 31-32).

**Parser corrections (make-report.py):** `bw_file_rd` "inf" throughput
(faster than the timer granularity) was mis-read as the size column —
the old table's "33662x slower" row was actually the fastest row;
retired. Division-by-zero guard on 0-throughput rows. Net rows
(lat_tcp 0.099 µs) fabricated from tcp-in debug noise — the net-server
handshake family (lat_tcp/udp/connect, bw_tcp, lat_fs) remains open
with no honest number on either column.

**Open:** plain-syscall band null/read/write/stat/open 1.2-2.5x is the
remaining true gap (SMAP AC brackets + entry floor, unchanged since
11-04 audit); net-server family; lat_fs.

## Standing (2026-11-15, kernel main=2649af4e3, lane gb-opt)

Fresh full battery on the current main (both kernels same-session
native reference). **Caveat that shapes this table: the sched
claim-strand bug (see docs/test/geekbench-report.md 11-15 section)
strands the forked benchmark child on ~60% of `-smp 2` boots; every
TIMEOUT/n-a Rux cell below is a strand casualty (per-case isolated
retries up to 4x landed most rows), not a load window. Rows the strand
never released stay n-a. The opt column (gb-opt branch: munmap
whole-VMA fast path + fault-entry walk consolidation) is the
merge-candidate read of the same battery.

| Case | Rux main | Rux opt (gb-opt) | Reference (native) | main ratio |
|---|---|---|---|---|
| `lat_syscall null` | 0.051 µs | 0.050 µs | 0.040 µs | 1.26x |
| `lat_syscall read` | TIMEOUT | 0.104 µs | 0.061 µs | - |
| `lat_syscall write` | 0.090 µs | 0.089 µs | 0.058 µs | 1.57x |
| `lat_syscall stat` | 0.333 µs | 0.328 µs | 0.219 µs | 1.52x |
| `lat_syscall fstat` | 0.085 µs | 0.081 µs | 0.103 µs | **0.82x (faster)** |
| `lat_syscall open` | 0.781 µs | 0.772 µs | 0.445 µs | 1.76x |
| `lat_pagefault /tmp/lmbench.file` | n/a | n/a | 0.066 µs | - |
| `lat_mmap 1m /tmp/lmbench.file` | TIMEOUT | n/a | 3.549 µs | - |
| `lat_pipe` | 5.276 µs | 5.124 µs | 2.639 µs | 2.00x |
| `lat_unix` | 2.968 µs | 2.573 µs | 2.748 µs | 1.08x (**opt: 0.94x faster**) |
| `lat_ctx -s 64 2` | 2.49 µs/ctx | 2.57 µs/ctx | 1.09 µs/ctx | 2.28x |
| `lat_ctx -s 64 8` | 2.33 µs/ctx | 2.56 µs/ctx | 1.11 µs/ctx | 2.10x |
| `lat_proc fork` | TIMEOUT | 36.582 µs | 54.551 µs | (**opt: 0.67x faster**) |
| `lat_proc exec` | 36.596 µs | 37.138 µs | 56.782 µs | **0.64x (faster)** |
| `lat_sem` | 1.132 µs | 1.086 µs | 0.428 µs | 2.64x |
| `lat_fs -s 1k` | TIMEOUT | n/a | 476823 files/s | - |
| `lat_mem_rd 1m` | 2.291 ns | 2.244 ns | 2.232 ns | 1.03x |
| `lat_mem_rd 256m` | 6.693 ns | 6.630 ns | 6.461 ns | 1.04x |
| `bw_mem 1m bcopy` | 84176 MB/s | 82994 MB/s | 79788 MB/s | **1.05x (faster)** |
| `bw_mem 4m bzero` | 67576 MB/s | 66844 MB/s | 62269 MB/s | **1.09x (faster)** |
| `bw_pipe` | TIMEOUT | 31927 MB/s | 10596 MB/s | (**opt: 3.01x faster**) |
| `bw_unix` | 48676 MB/s | 45906 MB/s | 28733 MB/s | **1.69x (faster)** |
| `bw_file_rd 1m io_only` | 1.05 MB/s (bad-value artifact) | n/a | 35345 MB/s | - |
| `bw_mmap_rd 1m mmap_only` | n/a | n/a | 80461 MB/s | - |
| `lat_tcp 127.0.0.1` | n/a | n/a | 4.185 µs | - |
| `lat_udp 127.0.0.1` | TIMEOUT | n/a | 3.873 µs | - |
| `lat_connect 127.0.0.1` | n/a | n/a | 66.518 µs | - |
| `bw_tcp 127.0.0.1` | n/a | n/a | 16002 MB/s | - |
| `lat_usleep 1000` | 1044.8 µs | (landed, see raw) | 1051.9 µs | **0.99x (at reference)** |

**Reads vs the 11-04 standing:** the syscall band is stable (null 1.26x
vs 1.31x, write 1.57x vs 1.62x, stat 1.52x vs 1.68x — the 11-04 window
read at load 6, this one at load 3-5; fstat/open unchanged). The
headline movement is the IPC pair under the same rig's native
reference: lat_unix 1.08x (opt 0.94x — faster than the reference for
the first time in this table's history), lat_proc fork 36.6 µs vs
54.6 µs native (0.67x), lat_proc exec 0.64x, bw_pipe 3.01x over native.
The reference column moved with the host session (native readings are
per-session), so cross-session ratio drift on the µs-scale rows is
expected; same-file same-session main-vs-opt deltas are the clean
signal: unix/fork/pipe improved, everything else flat.

**mm-probe attribution behind the opt branch** (see
docs/test/geekbench-report.md): munmap 64 MB 61x → 5.3x (whole-VMA
fast path), random-anon fault 1.9x → 1.7x (fault-entry walk
consolidation), mprotect 10x (hoist tried, REVERTED — wedged the
machine 4/4 boots inside the timer tick; documented at the site).

Reproduce: `userspace/lmbench/run-rux.sh <bzImage> work/lmbench-x86.img
run/lmbench-rux-<col>.txt 75` + `userspace/lmbench/run-linux.sh` +
`make-report.py`; the per-case strand-retry pass is
`run/retry-par3.sh` (gb-opt). Raw columns: gb-opt
`run/lmbench-rux-{main,opt}.txt`, `run/lmbench-linux.txt`.

## Superseded head table (2026-11-04 refresh, kept for the annotations)

| Case | Rux (x86_64+KVM) | Reference kernel | Ratio |
|---|---|---|---|
| `lat_syscall null` | ~~0.095~~ **0.055 µs** | 0.044 microseconds | 1.25x |
| `lat_syscall read` | ~~0.234~~ **0.110 µs** *(11-03/04: per-call alloc purge + dispatch consolidation + RCU fd lookup)* | 0.069 microseconds | 1.59x |
| `lat_syscall write` | ~~0.198~~ **0.094 µs** *(same set)* | 0.064 microseconds | 1.47x |
| `lat_syscall stat` | ~~4.077~~ 1.067 → 0.651 → **0.382 µs** *(path-walk fast paths, then the 11-03 rounds)* | 0.213 microseconds | 1.79x |
| `lat_syscall fstat` | ~~0.17~~ **0.086 µs** *(RCU fd lookup — ahead of native reference)* | 0.111 microseconds | **0.77x (faster)** |
| `lat_syscall open` | ~~5.685~~ 1.773 → 1.222 → **0.70 µs** *(epoll-purge off the close path + RCU fd lookup)* | 0.395 microseconds | 1.77x |
| `lat_pagefault /tmp/lmbench.file` | 0.449 microseconds *(11-01 value; not re-landed in the 11-04 load window)* | 0.07 microseconds | 6.45x |
| `lat_mmap 1m /tmp/lmbench.file` | 1.049 us *(11-01 value; not re-landed in the 11-04 load window)* | 4.41 us | **0.24x (faster)** |
| `lat_pipe` | **4.00 µs** *(rig-divergent row: native ref 2.745 vs KVM ref 13.296 — the KVM pair is authoritative, see fair-rig section)* | 2.745 microseconds | KVM pair: **ahead 3.3x** |
| `lat_unix` | ~~2.688 (quiet window)~~ **4.56 µs** *(load-6 window)* | 3.011 microseconds | 1.52x |
| `lat_ctx -s 64 2` | **2.13 us/ctxsw** *(rig-divergent: KVM pair authoritative)* | 0.91 us/ctxsw | KVM pair: **ahead 3.2x** |
| `lat_ctx -s 64 8` | 1.05 (quiet) / **1.94 us/ctxsw** (load-6) | 1.3 us/ctxsw | 1.49x (load window) |
| `lat_proc fork` | ~~672.9~~ 47.6 (quiet) / **59.9 µs** (load-6, 180 s window) | 33.133 microseconds | KVM pair: 1.11x |
| `lat_proc exec` | ~~673.6~~ 55.9 → **66.2 µs** (load-6) | 57.54 microseconds | KVM pair: **ahead 1.3x** |
| `lat_sem` | **0.800 µs** *(first landing — was TIMEOUT on 11-01)* | 0.444 microseconds | 1.80x |
| `lat_fs -s 1k` | n/a *(not re-landed in the load window)* | 438368 files/s | - |
| `lat_mem_rd -s 1048576 -e 1048576 1` | 2.321 ns/load | 2.437 ns/load | **0.95x (faster)** |
| `lat_mem_rd -s 268435456 -e 268435456 256` | 6.967 ns/load | 6.753 ns/load | 1.03x |
| `bw_mem 1m bcopy` | 77770.49 MB/s | 72596.57 MB/s | **1.07x (faster)** |
| `bw_mem 4m bzero` | 66837.16 MB/s | 60682.25 MB/s | **1.10x (faster)** |
| `bw_pipe` | **28030.46 MB/s** | 10442.95 MB/s | **2.68x (faster)** |
| `bw_unix` | ~~2637.87~~ **42885.03 MB/s** *(unix staging elimination; landed 1/3 passes in the load window — quiet-window confirm pending)* | 22782.66 MB/s | **1.88x (faster)** |
| `bw_file_rd 1m io_only /tmp/lmbench.file` | "inf" *(timer floor — per-iteration cost below clock granularity; ≥32 GB/s class)* | 33600.68 MB/s | at/above reference |
| `bw_mmap_rd 1m mmap_only /tmp/lmbench.file` | 70917.84 MB/s *(11-01 value; not re-landed)* | 74316 MB/s | 1.05x slower |
| `lat_tcp 127.0.0.1` | **7.75–8.03 µs** *(11-04 lane fix: AF_PACKET locks irqsave'd + TCP-table exhaustion fixed; 3/3 release boots)* | 4.446 microseconds | 1.78x |
| `lat_udp 127.0.0.1` | **6.90–7.07 µs (release-opt, 9/11 boots)** *(r14: the 0/8 strand was NOT sched — it was the UDP-v4 checksum zero-escape drop in udp_rcv; fixed)* | 4.37 microseconds | 1.60x |
| `lat_connect 127.0.0.1` | **14.3–16.1 ms measured, 3/4 boots** *(was TIMEOUT: TCP-table exhaustion; eviction + 512 slots landed)*; remaining cost = scheduler stale-claim refusing the accept server's wakes | 86.887 microseconds | 165x (sched-blocked) |
| `bw_tcp 127.0.0.1` | **TIMEOUT** *(scheduler stale-claim refuses the benchmark worker's wakes)* | 15195.35 MB/s | - |
| `lat_usleep 1000` | ~~5013.9~~ **1037.1 µs** *(LAPIC single-shot timer)* | 1052.529 microseconds | **0.99x (at reference)** |


*Refreshed 2026-11-04 (kernel main=8d81c5da — RCU fd lookup + retire
reaper), 9950X host, loadavg 4.4–6.2 window, median of 3 standing
passes + serial retries at longer windows (fork 180 s) for starved
rows. Rows marked "not re-landed" keep their 2026-11-01 value (the
load window starved their auto-calibration). The original 2026-11-01
sweep ran under loadavg 10–19; strikethrough values show the lineage.*


## Standing addendum (2026-11-03, kernel 33413d92 lineage — idle poll window 120us)

The battery2 cctx anatomy (3.8M switches in a 4s unix roundtrip window)
attributed the remaining roundtrip cost to the wake+IPI chain (enqueue
244ns + dispatch 22ns + IPI mean 995ns). The 30us idle poll window
missed handoffs landing just past it; widening to 120us removed the
bimodal tail on lat_unix (base boots showed 16.7/6.05us bimodal; the
120us window boots read 6.2-6.3us under load 7).

| Case | Rux (120us window) | Reference (native) |
|---|---|---|
| lat_unix | 6.24-6.30 us | 3.01 us |
| null/read/write | 0.059-0.077 / 0.133-0.153 / 0.109-0.136 µs | 0.044 / 0.069 / 0.064 µs |

Semop ping-pong under load stayed 2M-round clean through the soak.



## Negative result (2026-11-04): adaptive idle-poll window — rejected with evidence

The reference kernel's guest halt-polling discipline (adaptive
`guest_halt_poll_ns` instead of a fixed window) was implemented in
three variants and measured against the static 120 us window. None
beat the static window on the primary perf platform (KVM lmbench);
the change was reverted. Recorded here because the mechanisms are
reusable knowledge:

| variant | KVM lmbench (vs base, same-window interleave) | TCG pbench |
|---|---|---|
| v1 collapse-on-halt, ceiling 500 us | ctx +18%, pipe +8-12% | - |
| v2 grow-on-fast-halt (ns clock), ceiling 500 us | ctx +13%, exec -14%, fork -6% | p4_fork_exit **+56%** |
| v2 + ceiling 120 us | (not run) | p4_fork_exit +47% |
| v2 + 5 ms decay hysteresis | (not run) | p4_fork_exit +55% |
| X1 jiffy-clock verdict, ceiling 120 us | ctx +6%, pipe +8%, exec +43% (single pair), fork +4%, null flat | p4 flat, p2 -11%, p5a -13% |

Mechanisms established by bisection (constant-window build isolated
the code path from the window values):

1. **Grow-only-on-catch is chicken-and-egg**: a window below every
   wake gap can never grow (the wake always lands after the window),
   so the 10-120 us gap class paid an IPI on every handoff (v1).
2. **Measuring the halt with `monotonic_ns_since_boot` costs +50% on
   translation rigs**: the call walks the HPET device model and its
   lock twice per halt, serializing both vCPUs' timer traffic during
   fork storms. Jiffy-granular verdicts (two atomic loads) fixed the
   TCG p4 regression entirely — the idle/halt path must only use
   cheap clocks.
3. **Why adaptivity does not pay on this rig**: the host's KVM
   `halt_poll_ns=200000` already keeps the vCPU THREAD from sleeping
   through sub-200 us wakes — the guest-side window only saves the
   guest IPI (waker-side VM-exit), and the static 120 us already
   covers the ping-pong gap class. The adaptive variants only added
   regrow transients at benchmark phase boundaries.

Tool: `run/ab-haltpoll.sh` (interleaved same-window A/B boots — the
interleaving is what separates real deltas from load drift; see the
ctx rows: base read 1.85-1.88 at load 1.3 vs 1.96-2.17 at load 5).

## Attribution experiment (2026-11-04): the sync-realign ctx story corrected; the poll window is the whole show

Three-arm interleaved A/B on the same rig and load window
(run/ab-halt-attribution.sh, results in run/halt-attribution-results.txt;
EXP counters printed at init exit: resched_cpu calls vs actual IPI sends):

| arm | ctx (med of landed) | pipe | IPI send rate |
|---|---|---|---|
| pre-realign (ab397dfd, no poll window) | 4.67 us (4.69/4.65) | 7.84 | 99.9% |
| current, window 120 us | **2.40 us** (2.47/2.29/2.44) | **4.63** | 86-96% |
| current, window 0 (forced halt) | 4.91 us (4.96/4.85) | 8.18 | 99.7% |

Findings, each measured:

1. **The yielding-spinlock removal was NOT the handoff cost.** The
   pre-realign kernel reads statistically identical to current-with-
   window-0 (4.67 vs 4.91 ctx, 7.84 vs 8.18 pipe). The historical
   "0.73 -> 1.85 caused by sync-realign" attribution was an era
   confound: the pre-realign kernel does not reproduce 0.73 on today's
   rig — it pays the same full halt tax as any window-less kernel.
2. **The halt boundary is the dominant per-handoff cost and the poll
   window is worth 2.5 us/ctx and 3.6 us/pipe** (same code, only the
   window differs). This is the single largest scheduler-adjacent
   number on the rig and it is already banked in main.
3. The window converts 99.9% send-rate to 86-96% (the remainder
   includes preemption IPIs to RUNNING targets, which the flag cannot
   skip by design).

Corollary for the optimization queue: locks are not the handoff lever;
the remaining kernel-side levers are the resched-IPI volume to halted
CPUs (coverage), the GRQ hold time on the wake/pick path, and the
syscall entry floor (null 1.31x). See the session close report.

## GRQ critical-section anatomy (2026-11-04, instrumented build)

Per-boot rdtsc sampling of every 64th GRQ irqsave section (spin vs
work split, >10us tails attributed by caller line; counters printed at
init exit), interleaved A/B for each hypothesis:

| hypothesis | test | verdict |
|---|---|---|
| lock contention | avg spin 241-328 cycles across all loads | **excluded** — the section cost is the work, not waiting |
| in-lock resched-IPI (VM exit) | defer-send A/B | **excluded at handoff loads** — pipe +15% WORSE (the IPI lands ~200ns later per handoff); in-lock send is strictly better when uncontended |
| BTreeMap node alloc under lock | 99.96% of 2.64M inserts/boot hit an EMPTY map (guaranteed root alloc+free per insert, inside the GRQ irqsave section); swap to a preallocated sorted Vec (`run/timeline-vec-swap.patch`) | **excluded as a latency cost** — avg_work and tails unchanged (the small allocator serves this size class in ~100ns); kept as a robustness lead, NOT merged |

Remaining facts: the __schedule pick section (sched.rs lock at the
pick site) averages 10-18K cycles on ctx-class loads (3-4.5us) with
14ms max tails, and is NOT explained by spin, IPI, or allocation —
the next attribution step is sub-section rdtsc pairs (accounting /
pick-walk / requeue). The 14ms tails concentrate (99.9%) in the
__schedule section line.

Soak-instability note for the stress lane: 150s riscv TCG soaks panic
on BOTH the base kernel (PREEMPT-UNDERFLOW, c3c4c3c4 scribble pattern —
the known scribble engine family) and the timeline-swap build
(different signature, freed-task pfault) — the 90s standard soak is
clean on both. The 150s window is a sharper probe than the standard
soak and should be adopted by the stress lane with signature
comparison before any scheduler-structure merge.

## Slow-metric inventory + open/close decomposition (2026-11-04, syscost)

Still-behind rows (fair KVM where available): open/close 1.95x (0.70
vs 0.36 us), stat 1.68x, read 1.67x, write 1.62x, unix 1.52x, sem
1.80x (first landing), fork 1.11x, null 1.31x, pagefault 6.45x (11-01
stale value — not re-landed since; re-measure before believing).

open/close anatomy (syscost rest-sites, 3.44M iterations): the
allocator is ~106 ns/iteration — CpuLocal::with cli/sti+cpu_id at 27
ns per access x2 (54 ns) + magazine refill/flush amortization ~32 ns
(241+265 ns per section at depth 16) + magazine push/pop; close-side
machinery (fdtable removal + retire park) ~88 ns measured cheap;
syscall entries 2x55 ns. The lock-free path walk is invisible to
rest-sites (no cli window) — bounded by subtraction at ~200-250 ns.

MAG_DEPTH 16->32 was measured at -18 ns on open/close (consistent,
interleaved) but REVERTED: riscv soaks hit intermittent
allocator-corruption panics 3/4 runs (vs 0/1 base same window) — the
magazine transient paths (len>MAG_DEPTH handling, two-CPU r8final
entry) are tuned for 16. Deepening needs a magazine redesign first;
the comment at the const carries the finding.

Allocation-domain queue (from this data): (a) %gs-based per-cpu
access to replace CpuLocal::with's cli/sti+cpu_id (54 ns/iter on
open/close, ~2 ns with a kernel-GS per-cpu base) — the bigger lever;
(b) magazine depth after the transient-path redesign.

## Context-switch first-principles audit (2026-11-04, cctx decomposition)

One lat_ctx handoff (1.39-1.42 us at load ~4.5, main kernel, cctx=1
boots, n=2.4-2.6M switches per boot), decomposed against the
component floors:

| segment | measured | first-principles floor | verdict |
|---|---|---|---|
| __schedule: pre+acct+deq+pub | 125 ns | bookkeeping | near floor |
| GRQ lock acquire | 102 ns | ~20 ns uncontended | global-queue design price |
| pick walk | 96 ns | ~50 ns | lean (retires the "slow pick section" lead: the µs-class GRQ section AVERAGES were tail-inflated) |
| mm switch | 202-295 ns | ~40 ns (CR3 write) | PCID + same-mm CR3-write skip already present; remainder = asid resolve + guard read_cr3 probe (~30-60 ns trimmable) |
| fpu save/restore | 34 ns | | xsaveopt skip working |
| MSR/FS-GS base | 33+21 ns | | FSGSBASE path |
| GPR asm | 21-23 ns | | near floor |
| **kernel machinery total** | **~620 ns** | | **below the reference's native whole-switch 910 ns** |
| remainder ~780 ns | | | wake/sleep rendezvous + halt/IPI residue (optimized to plateau) + benchmark's own user-side work |

Conclusion: the switch machinery itself has no fat left within the
current design; every component carries its known fast path (same-mm
CR3 skip, PCID, FSGSBASE, xsaveopt). The two remaining structural
levers, in size order: (1) runqueue sharding — attacks the 102 ns lock
AND wake-path queuing; that is the D-1 divergence decision, a campaign
with its own gate budget, not a patch; (2) a per-CPU prev-root tag
cache replacing the read_cr3 guard probe (~30-60 ns/switch, marginal).

## Standing (2026-11-04 00:45, kernel 8d81c5da — fair KVM-vs-KVM, RCU fd lookup)

Both kernels under the same q35+KVM rig (reference column frozen from
its own window). The 8d81c5da round landed the RCU fd lookup + retire
reaper (get_file: banks → one Acquire load + refcount inc) and the
read/write dispatch consolidation. fstat — the purest fdtable-lookup
row — moved from 1.19x ahead to **1.37x ahead** measured at a HIGHER
host load (6 vs 4), the clearest single signal of the round.

| Case | Rux | Reference (KVM) | Ratio | vs 11-03 19:15 |
|---|---|---|---|---|
| lat_ctx -s64 2 | **2.13 µs** | 6.71 µs | **ahead 3.15x** | 3.4x (load window) |
| lat_pipe | **4.00 µs** | 13.30 µs | **ahead 3.33x** | 2.6x → improved |
| lat_proc exec | **66.2 µs** | 87.43 µs | **ahead 1.32x** | 1.39x (load window) |
| lat_syscall fstat | **0.086 µs** | 0.118 µs | **ahead 1.37x** | 1.19x → improved |
| lat_proc fork | 59.9 µs | 53.99 µs | 1.11x | 1.17x → improved |
| lat_syscall null | **0.055 µs** | 0.042 µs | 1.31x | 1.37x → improved |
| lat_syscall open | 0.70 µs | 0.360 µs | 1.95x | 2.28x → improved |
| lat_syscall write | **0.094 µs** | 0.058 µs | 1.62x | flat |
| lat_syscall read | **0.110 µs** | 0.066 µs | 1.67x | flat |
| lat_syscall stat | **0.382 µs** | 0.227 µs | 1.68x | flat |

5 rows ahead (ctx/pipe/exec/fstat + unix-bandwidth family), the rest in
the 1.11–1.95x band — the read/write/stat band is unchanged at this
load window (the earlier isolated boots at load 3.7 read 0.106 µs; the
standing window reads 0.110). Remaining read/write gap lives in the
fill/copy tail (SMAP AC brackets) and the syscall-entry floor (null
1.31x), not fd lookup anymore.

## Standing (2026-11-03 19:15, kernel e264a64b — fair KVM-vs-KVM, LASTSYS gated)

Both kernels under the same q35+KVM rig. The LASTSYS gating (this
kernel) cut the syscall floor across the board.

| Case | Rux | Reference (KVM) | Ratio |
|---|---|---|---|
| lat_ctx -s64 2 | **1.98 µs** | 6.71 µs | **ahead 3.4x** |
| lat_pipe | **5.11 µs** | 13.30 µs | **ahead 2.6x** |
| lat_proc exec | **62.9 µs** | 87.43 µs | **ahead 1.39x** |
| lat_syscall fstat | **0.099 µs** | 0.118 µs | **ahead 1.19x** |
| lat_proc fork | 63.3 µs | 53.99 µs | 1.17x |
| lat_syscall null | **0.057 µs** | 0.042 µs | 1.37x |
| lat_syscall write | **0.094 µs** | 0.058 µs | 1.63x |
| lat_syscall stat | **0.377 µs** | 0.227 µs | 1.66x |
| lat_syscall read | **0.111 µs** | 0.066 µs | 1.68x |
| lat_syscall open | 0.820 µs | 0.360 µs | 2.28x |

4 rows ahead (ctx/pipe/exec/fstat), the rest in the 1.17-2.28x band.
The remaining read/write/stat/open gaps live in the VFS dispatch layer
(fdtable → file → ops-ptr), not the syscall entry (null is at 1.37x).

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
- **Validity rule (perf lanes):** a measurement taken while host
  loadavg(1min) >= 4 is void — re-run it. The original sweep ran under
  loadavg 10-19, which alone invalidates its absolute values (the
  bw_file_rd row's 50x was additionally the sparse-file artifact of
  Corrections §1). Treat any row whose conclusion depends on >10x
  ratios with suspicion until re-measured under load < 4.

## Optimization queue

Ranked by the measured ratios above (the queue is the working list for
the per-case optimization work):

1. **Functional gaps (TIMEOUT / n-a rows) — each is a missing or
   wrong semantic, not a slow path.** Fixing one turns a dead row
   into a number:
   - **lat_sem — RESOLVED** (main's R40 semop retry + the sched
     fixes): 0.800 µs standing (perf-lmbench lane), re-confirmed
     0.730 µs (debug) / 0.847 µs (release) on the 11-04
     lmbench-fix tree. The EAGAIN blocking_idx suspect is fixed in
     `kernel/src/ipc/sysv_sem.rs` (the `None => continue 'outer`
     arm is on main).
   - **lat_tcp — RESOLVED 11-04 (lmbench-fix lane), 7.75–8.03 µs,
     3/3 release boots.** Two stacked causes: (a) the AF_PACKET
     registry/per-socket locks were softirq-contended — a timer IRQ
     landing on a CPU whose task context held PACKET_SOCKETS made
     the RX fanout spin on its own holder (one-CPU self-deadlock,
     system-wide at smp2); irqsave'd (predecessor commit 7f9c5e7b).
     (b) TCP socket-table exhaustion: the SYN spawn path had no
     reclamation and the 64-slot table cannot hold the workload's
     60+ concurrent ESTABLISHED connections plus its TIME_WAIT /
     orphaned CLOSE_WAIT corpses — the SYN was dropped silently and
     every connect waited for the 1 s SYN retransmit (eviction +
     512-slot table, commit c489347c).
   - **lat_connect — TABLE EXHAUSTION FIXED (14.3–16.1 ms, 3/4
     boots; was TIMEOUT).** Same table fixes; the residual 165x cost
     and the 1-in-4 wedge are the scheduler stale-claim family (see
     the queue's sched entry). With a healthy accept server the
     handshake completes in the synchronous loopback drain.
   - **lat_udp — RESOLVED (r14, 11-04 stress-port lane): the 0/8
     release strand was a NET defect, not sched.** After the
     source-address fix (d0452aaf) the remaining strand reproduced
     6/6 on properly-invoked release boots; the r13 tracepoints
     (net:lo_xmit checksum snapshot + net:pkt_drop) convicted the
     exact hop: the ping child's datagram left the sender with the
     RFC 768 zero-escape checksum (computed 0x0000 transmitted as
     0xFFFF — both TX builders do this), traversed loopback
     byte-identical, and was SILENTLY DROPPED by udp_rcv's plain
     `computed != stated` verification, which lacks the zero-escape
     parity (udp_rcv6 and TCP verify by summation and were always
     correct). Content varies per datagram, so each roundtrip rolls
     a 1-in-65536 die; at release speed the boot hits it within
     seconds (debug timing mostly hides it — the old 94-96 µs
     5/5 debug passes never rolled the collision). Fix:
     udp_csum_accepts() (equivalence to the reference summation
     test proven per input pair + swept in the suite). Post-fix:
     6.90–7.07 µs, 9/11 boots clean, 0 pkt_drop, 0 SIGALRM rescues;
     the 2 misses are host-contention budget hits with the ring
     showing live ping-pong at the kill. The `WAKE-HEAL-SKIP
     pid=<server> why=c` prints riding the strand were adjudicated
     non-causal in r12 (woken=1 measured) and are now moot for this
     case. NOTE for bw_tcp/lat_connect below: those rows keep the
     sched attribution until re-tested on the r14 tree — the lat_udp
     lesson is that a wake-refused print on a stranded task does
     not by itself convict the scheduler.
   - **bw_tcp — connect/data path works; the case is wedged by the
     scheduler stale-claim** refusing the benchmark worker's wakes
     (39 `WAKE-HEAL-SKIP why=c` to the kill, one `WAKE-HEAL was=1`
     firing but not un-stranding). No net-side defect found on the
     11-04 tree.
   - **SCHEDULER STALE-CLAIM (the one remaining producer, blocks
     bw_tcp + the lat_connect tail; lat_udp was misattributed here
     — its strand was the r14 checksum zero-escape fix).** A task
     parked in a socket wait is left {state=RUNNING, linked=0,
     on_rq=0, on_cpu=1, pk=1, claim=<cpu>} — a leaked preempt
     count on a sleeping task with the pick ownership never
     released. Every wake of it is refused by the curr_on scan
     (`WAKE-HEAL-SKIP why=c`), `STALE-CLAIM-RESET` fires on the
     OTHER victims but never heals these, the 1 Hz phantom relink
     (177eec0b) does not match (they are claim-named, not slotless),
     and the r24 idle-slot heal only runs when the claiming CPU
     idles. Faster (release) kernels hit the window within seconds;
     debug timing mostly hides it. Fresh evidence this session:
     (this session's lat_udp 0/8 was re-attributed to the r14
     checksum fix — see the lat_udp bullet;) bw_tcp wedges with
     the worker refused, lat_connect 1-in-4
     wedge + 15 ms tail, all with the census shape above in the
     serial. This is sched-lane territory (kill10/R63–R85 lineage).
   - **lat_fs** — no functional error, but the mk/rm calibration
     does not finish even in a 300 s single-case window: it creates
     thousands of 1k files, and at the measured 14–19x per-call file
     cost that is minutes. The kernel also prints
     `readlink fail(-2)` traces for its `/tmp/lat_fs_*` tempdir
     pattern while it runs. Re-measure once file ops are faster.
2. **bw_file_rd — RESOLVED as an artifact + 1.49x real gap closed to
   1.02x** (perf-filerd lane, see Corrections §1). The "50x" was hole
   reads of a sparse scratch file amplified by a 512-lookups-per-read
   hole walk; with a real-data file the base tree was already at
   21-23 GB/s vs the 33.6 GB/s reference. The lane's fixes (stac/clac
   uaccess brackets, merged page-run copyout, read-ahead stops at
   holes) bring the row to ~32.9 GB/s. Remaining follow-up: hole
   verdicts are still re-resolved per page on repeat reads of sparse
   ranges (~10 GB/s hole reads vs 33.6 GB/s reference) — cache them
   in the inode the way the reference kernel's extent cache does.
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

## Corrections

### 1. The `bw_file_rd` "50.21x" row was an artifact of a sparse scratch file (2026-11-01, perf-filerd lane)

The original row (669.19 MB/s, "50.21x slower") measured **hole reads,
not cached-file reads**. Two stacked causes:

- **Rig bug:** `make_scratch` created the shared 16 MiB
  `/tmp/lmbench.file` with `ftruncate`, leaving a fully-sparse file.
  `bw_file_rd` therefore read 1 MiB of holes per iteration. The case's
  own header states its intent — "the file is in memory" — which a
  hole file never exercises. The same bug shapes `bw_mmap_rd`,
  `lat_pagefault` and `lat_mmap` (zero-page faults instead of real
  file pages); those rows need re-measurement with the fixed rig too.
- **Kernel hole-path cost:** Rux's read-ahead fill walked its whole
  window (up to 512 pages) resolving an extent verdict per page and
  skipping holes — pages that can never become cache entries — so a
  hole read paid ~512 extent lookups per `read(2)`: a guest-timed
  611-620 MB/s on both the base and the fixed kernel. The reference
  kernel serves holes from memory (33.6 GB/s same case).
- The sweep also ran under host loadavg 10-19, void by the lane's own
  load rule. The **same baseline kernel binary** measures 21.4 GB/s
  (sparse file) on a quieter host; with a real-data scratch file the
  base kernel measures 21.3-23.0 GB/s. The real cached-read gap on the
  baseline tree was **1.49x**, not 50x.

**Fixes (branch `feature/perf-filerd`, all measured on the same rig,
KVM, strong profile, interleaved boots under host load 5.9-7.5 —
informal until the med-of-runs runner catches a loadavg<4 window):**

| Kernel | bw_file_rd 1m io_only | vs reference |
|---|---|---|
| base (09d2969c) | 21,304-23,030 MB/s | 1.49x |
| + stac/clac uaccess brackets | 30,225 MB/s | 1.11x |
| + merged page-run copyout | (neutral alone; order-0 batch fills are not physically contiguous) | - |
| + read-ahead stops at holes | **32,481-33,458 MB/s (median 32,889)** | **1.02x** |

The `dfx=filerd` phase probe (this branch, `/proc/filerd`) attributed
the warm read(2) kernel time before the fixes: 1943 ns per 64 KiB
read, 81% of it the copyout, in 16 per-page `copy_to_user` calls whose
pushfq/orq/popfq AC brackets cost ~55 ns each; STAC/CLAC brackets cut
the copy phase 1578 -> 735 ns. Hole reads went 611 -> 10,006 MB/s
(guest-timed 40 MiB window) when the fill stops at the first hole; the
remaining ~3x hole-path gap (per-page extent re-resolution on repeat
reads of sparse ranges) is follow-up work — the reference caches its
extent verdicts in the inode.

### 2. Rows still carrying the sparse-file artifact

`bw_mmap_rd`, `lat_pagefault` and `lat_mmap` in the table above were
measured against the hole file (mmap/fault of the shared zero page).
Their reference columns were produced by the same runner natively, so
both columns measured the same degenerate workload; re-measure all
file-backed rows after this branch merges.

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

## Fair-rig comparison — both kernels under the same KVM rig (2026-11-03)

The headline table above compares Rux-under-KVM against the reference
kernel running **natively** — a column that pays no virtualization
tax. This section adds the missing column: the **same reference kernel
binary** the native column ran (the host's 7.0.0-38-generic distro
kernel), booted under the **same QEMU KVM rig as the Rux column**, on
the same lmbench binaries and the same Ubuntu-rootfs shape, one case
per boot, serial to file, loadavg-gated (rows captured at 1-min host
load 5.3-6.8; the >=7 void bar held — the one row that ended at 7.07
was re-measured and agreed within 0.4%).

Rig (identical to `run-rux.sh` except two deltas, both on the kernel
cmdline side): `qemu-system-x86_64 -M q35 -accel kvm -cpu host -m 2G
-smp 2`, virtio-blk rootfs (`snapshot=on` + `file.locking=off`),
user-mode NIC, `-serial file:`, one case per boot. Deltas: `-kernel`
points at the distro kernel (no initrd — virtio/ext4 are builtin; it
boots the same ext4 image in ~4 s), and `init=/test/init_ref` — a
static musl wrapper (`init_ref.c`) that mounts proc/sys/devtmpfs (the
reference kernel does not pre-mount /proc, and the rootfs carries no
static /dev/console) and then execs the exact same `lmbench_run`
PID-1 runner the Rux column uses, so case selection, `-N 3 -W 1`,
scratch-file creation and the 35 s in-guest budget are identical.
`lat_ctx` needed its documented longer window (`budget=120`, 150 s
outer); `lat_proc fork` needed `budget=60` after one 35 s timeout
under rising load (the re-run completed cleanly and is the row below).

**Parity caveats.** (1) The reference columns ship the static
`/tmp/hello` binary for `lat_proc exec` (the native run had it); the
**Rux column's image has no `/tmp/hello`, so its exec row (55.9 us)
measured a failing execve** (ENOENT + immediate exit) — cheaper than
a real exec. Treat the Rux exec row as an under-measurement, not a
win. (2) The Rux column rows were measured 2026-11-01 under host load
10-19 (footnote above); this column ran at 5.3-6.8, and the native
column at 7.6-8.8. Load noise inflates the Rux numbers somewhat; the
addendum's quieter re-measurements (null 0.059-0.077 us) suggest by
how much.

| Case | Rux (KVM) | reference (KVM) | reference (native) | Virt tax (KVM/native) | Fair ratio (Rux/refKVM) |
|---|---|---|---|---|---|
| `lat_syscall null` | 0.095 us | 0.0416 us | 0.0441 us | 0.94x | 2.28x slower |
| `lat_syscall read` | 0.234 us | 0.0661 us | 0.0687 us | 0.96x | 3.54x slower |
| `lat_syscall write` | 0.198 us | 0.0577 us | 0.0639 us | 0.90x | 3.43x slower |
| `lat_syscall stat` | 0.651 us | 0.2266 us | 0.2172 us | 1.04x | 2.87x slower |
| `lat_syscall fstat` | 0.170 us | 0.1181 us | 0.1113 us | 1.06x | 1.44x slower |
| `lat_syscall open` | 1.222 us | 0.3596 us | 0.4096 us | 0.88x | 3.40x slower |
| `lat_proc fork` | 47.6 us | 53.99 us | 33.13 us | **1.63x** | **0.88x faster** |
| `lat_proc exec` | 55.9 us (failed exec, see caveat) | 87.43 us | 57.54 us | **1.52x** | 0.64x (not comparable) |
| `lat_pipe` | 2.006 us | 13.30 us | 2.74 us | **4.85x** | **0.15x faster (6.6x)** |
| `lat_ctx -s 64 2` | 0.73 us/ctx | 6.71 us/ctx | 0.91 us/ctx | **7.37x** | **0.11x faster (9.2x)** |

**What the tax column says.** Under KVM, guest syscalls do not
vmexit (the guest runs its own syscall entry natively), so the six
`lat_syscall` rows carry **no material virtualization tax** (0.88x to
1.06x — sub-native readings are load-window noise, both sides
single-boot). The tax concentrates where a kernel talks across vCPUs
or rewrites address spaces: wakeup IPIs in `lat_pipe` (4.85x) and
`lat_ctx` (7.37x), and the page-table/TLB work in `lat_proc`
(1.52-1.63x).

**Where Rux actually stands, same rig.** The old two-column table
flattered the reference on exactly the rows with the big tax:

- `lat_pipe` and `lat_ctx`: Rux's "0.73x/0.80x faster than native"
  was an understatement — against the reference **in the same rig**,
  Rux is **6.6x and 9.2x faster** (2.0 vs 13.3 us pipe; 0.73 vs
  6.71 us/ctx). The reference's wakeup path pays the IPI-vmexit tax
  under KVM; Rux's apparently does not (its pipe ping-pong completes
  without cross-vCPU reschedule storms). The scheduler/wake lanes'
  conclusion holds with margin.
- `lat_proc fork`: 1.44x slower natively becomes **0.88x — faster
  than the reference on the same rig**.
- `lat_proc exec`: 0.64x, but not comparable (Rux caveat above) —
  re-measure on Rux with `/tmp/hello` in the image before claiming it.
- The six `lat_syscall` rows: the tax is ~1x, so those columns were
  already fair — the 1.44-3.54x gaps there are **real kernel cost**
  (syscall entry and the path-walk work behind stat/open), not rig
  bias, and they remain the top optimization targets.

Reproduce:

```bash
userspace/lmbench/build-rootfs-ref.sh run/refkvm/lmbench-ref.img
userspace/lmbench/run-refkvm.sh        # -> run/lmbench-refkvm.txt
```

Raw column: `run/lmbench-refkvm.txt`; per-case host load and re-run
history: `run/refkvm/run-audit.txt` (loadavg-gated, LOAD_MAX=7).

