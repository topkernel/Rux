# Hot-path print audit (print-audit lane, 2026-11-01)

Scope: after e2717687 (EXITGRP on the fork-exit path cost ~650 µs/hit and
a 20x lmbench lat_proc,fork), sweep every hot path for the same class —
console prints that fire per-op / per-event through the polled UART.

## Serial cost baseline

`kernel/src/console.rs` — TX is **polled and byte-at-a-time**: riscv64
`Uart::putc` (console.rs:274) does one MMIO store per byte; x86_64
`putc` (console.rs:360-379) does one LSR poll + one THR store per byte
(one VM exit pair per byte under KVM); only RX has an IRQ
(`init_irq`, console.rs:612). `puts_batch` (console.rs:402) batches the
register traffic on x86_64 but the UART itself is still synchronous.
Measured cost class: **~0.4–0.7 ms per ~40–60-char pr_err/warn/info
line** (default console loglevel is 6, so pr_info and everything more
severe print; pr_debug is both above the loglevel and compiled out in
release).

## Method

Census of every `pr_err!/pr_warn!/pr_info!/pr_notice!/println!/print!`
in the hot-path files (futex 68 sites, binder 116, syscall/process 49,
syscall/file 27, syscall/memory 21, mm/page_alloc 24, mm/pglist 19,
signal 17, sched/sched 11, arch/riscv64/trap 23, arch/x86_64/trap 29).
Classification: legit (boot one-shot / panic-oops-trap / fatal-error
per-death / dfx-switch-gated / budgeted+deduped) vs DEFECT (per-op or
per-event, ungated or unbounded, on paths real workloads hit).

## Defects found and fixed (13 sites, 6 files)

| # | File:line | Print | Frequency class (before) | Fix |
|---|---|---|---|---|
| 1 | signal.rs:2868 | TKSIG (`if true`, budget 4000) | EVERY tkill/tgkill — pthread_kill, raise(), every thread test | restore the 9/15 filter (SIGKILL/SIGTERM only), same revert as the KSIG twin in e2717687 |
| 2 | syscall/memory.rs:1517 | MPROTECT-RW | every PROT_WRITE mprotect — per thread create, malloc growth, JIT flip, unbounded | `vmap_budget_line` (dfx=vmap gate + shared 600/boot budget) |
| 3 | syscall/memory.rs:1397 | BIGMUNMAP (>=512K) | every thread exit (8 MiB stacks), unbounded | `vmap_budget_line` |
| 4 | syscall/memory.rs:1625 | MPROTECT-CFI | per RELRO finalize / dlopen loop | `vmap_budget_line` |
| 5 | sync/futex.rs:1629 | FXDV block (incl. per-op comm name-compare while unlatched) | EVERY futex syscall on EVERY boot (compare), +400 lines once hdf_devmgr latched | gate whole block behind dfx=futextrace |
| 6 | sync/futex.rs:1715 | FXDV2 (param-arena window) | every futex op in the 64K arena window; comment itself says it "churns healthily all boot", 500-line budget burned on healthy traffic | covered by the same futextrace gate (same block) |
| 7 | sync/futex.rs:2364 | stkfutex (2 lines/op) | any futex on page 0x7fffffffc000 — "3 pages below the stack top of EVERY x86_64 process" (its own comment); budget 400 ops = 800 lines | gate behind dfx=futextrace |
| 8 | sync/futex.rs:2482 | FTWAIT (+ring dumps) | every timed futex wait from foundation/sa_main (OH), budget 240 | gate behind dfx=futextrace |
| 9 | arch/riscv64/trap.rs:1379 | segfault one-liner + 2 VMA-forensics lines | EVERY user pagefault reaching Segfault/PermissionDenied/BusError — INCLUDING faults a user SIGSEGV/SIGBUS handler survives (GC guard pages, JIT, LTP mmap05/13) | prints moved to the fatal branch only (death keeps full attribution; survivors quiet; kernel-mode unchanged) |
| 10 | arch/riscv64/trap.rs:1487/1506 | PermissionDenied / BusError lines | same as above | same fatal-only treatment |
| 11 | arch/riscv64/trap.rs:989 | SIGILL-FORENSIC full dump (VMA table + PTE + 64-row hexdump ≈ 5 KB) | per SIGILL death — a respawn loop over a broken binary paid tens of ms per crash, unbounded | boot budget 2 (first two deaths keep full attribution; the per-death one-liner stays) |
| 12 | arch/riscv64/trap.rs:1221 | SSBRK (comment said "REMOVE before merge") | per untraced single-step ebreak trap | demote to pr_debug (compiled out in release) |
| 13 | ipc/binder.rs:1823/1833/2624 | binder open ×2 lines + binder mmap ×2 lines | per binder-using process start (nearly every spawn on OH boots) — a fixed ~2 ms serial tax on the spawn path | info lines → pr_debug; sc157/sc160 probes → dfx=syscalltrace |

No panic/oops path, boot one-shot, or already-gated hunt was touched;
all message texts are preserved (gating is opt-in via the named switch).

## False positives deliberately left (representative list)

- **OOM-FORENSIC dump family (mm/page_alloc.rs:320-520)** — capped at
  4/boot; tied to the active fake-OOM hunt. Loud but budgeted by design.
- **KSIG / DEQUEUE / SIGKILL-SEND / SWI / KILLTRACE (signal.rs)** —
  SIGKILL/SIGTERM-filtered with budgets (80–4000); the e2717687 revert
  explicitly left this shape in place; per-op only under kill storms.
- **EXECTRACE / EXECCOST / appclone / forkgate / APPMAPSRC** —
  comm-filtered (appspawn/launcher family, OH images only) + budgeted,
  or ExecCost-switch-gated.
- **open ENOENT / so-open / dri / paramws tracers (syscall/file.rs)** —
  deduped lists and per-(path,dir) counters with hard budgets (32/96/
  420); mounted-path or switch filtered.
- **USEGV-FOR dump (arch/x86_64/trap.rs:1965+)** — capped at 6/boot
  (SEGVD_N) — the budget the riscv64 arm was missing before this audit.
- **SLOW-TXN / AMS46 / POOL-THR / W3 / TERM-DESCHED census prints** —
  already dfx-switch-gated (TxnHang/Ams46/PoolCensus/WwCensus/SchedTrace).
- **rwfz_op_log / RWFRZ-WATCH / park dumps (futex.rs)** — armed only
  through ParkDump/WwCensus-gated paths.
- sched/sched.rs, mm/pglist.rs (print_buddyinfo is an on-demand dump):
  nothing un-gated found beyond items above.

## Prevention

- `scripts/hotpath_print_check.py` + committed ratchet baseline
  `scripts/hotpath_print_baseline.txt`: fails when the count of
  un-annotated pr_err/pr_warn/pr_info/pr_notice/println/print sites in
  the hot-path files grows; new sites need a `// HOTPATH-PRINT: <why>`
  annotation. Wired into `scripts/review_coverage.py --check`.
- Rule documented in
  `docs/development/rust-kernel-best-practices.md` §2.4.

## Verification

- `make build` (riscv64) rc=0; `rm build/.config && make build-x86`
  rc=0; strong-profile x86_64 release build rc=0.
- `make test` (riscv64): **ALL TESTS PASSED** (1273 PASS lines, zero
  test failures, rc=0) — run/print-audit-test.log.
- A/B (x86_64 KVM, strong profile, one boot per run, 3 interleaved
  runs per kernel, BEFORE = this branch's base e7b3bf2c, AFTER = this
  branch; probe `run/printaudit/hotpath_probe.c` as PID 1, medians of
  per-100-op batches, run/printaudit/ab-probe-results.txt):

  | case (ns/op, median of 3 boots) | BEFORE | AFTER | delta |
  |---|---|---|---|
  | tgkill to self + handler (TKSIG per op) | 580,801 | 441 | **1317x** |
  | mprotect RW/NONE flip 1 MiB (MPROTECT-RW per op) | 565,109 | 1,451 | **389x** |
  | mmap+munmap 1 MiB anon (BIGMUNMAP per op) | 555,536 | 6,167 | **90x** |
  | futex WAKE uncontended (FXDV name-compare) | 119 | 218 | within noise: BEFORE's own boots spread 117–223 ns |

- Quiet-boot proof (same boots, whole-serial grep): BEFORE printed
  MPROTECT-RW x4000 + BIGMUNMAP x4000 + TKSIG x3998 (~12,000 lines,
  ~7 s of polled UART in one 90-second probe run — budgets saturated);
  AFTER printed **0** of every hunted pattern (MPROTECT-RW, BIGMUNMAP,
  TKSIG, stkfutex, FXDV, FTWAIT).

