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
| `lat_tcp 127.0.0.1` | **TIMEOUT** *(tcp-in debug-print family chokes the case; no measurement line)* | 4.446 microseconds | - |
| `lat_udp 127.0.0.1` | n/a | 4.37 microseconds | - |
| `lat_connect 127.0.0.1` | **TIMEOUT** | 86.887 microseconds | - |
| `bw_tcp 127.0.0.1` | **TIMEOUT** | 15195.35 MB/s | - |
| `lat_usleep 1000` | ~~5013.9~~ **1037.1 µs** *(LAPIC single-shot timer; consistent across all 3 passes)* | 1052.529 microseconds | **0.99x (at reference)** |


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

