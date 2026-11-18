# TCB Ledger — kernel `unsafe` accounting

This is a **living document**. Regenerate the table after any subsystem-wide change (paste it under
"Current baseline"; `--out` writes the bare table and is meant for temp files):

```sh
python3 scripts/tcb_ledger.py
```

## What this number means

The kernel's memory-safety TCB (trusted computing base) is the portion of
the code where memory safety depends on human review instead of the Rust
compiler: every line inside an `unsafe { ... }` block. Everything else is
memory-safe by construction (compiler-enforced). The ledger measures,
per subsystem, how much of the tree is in the review-bound part.

This is the accounting adopted in
`docs/development/rust-kernel-best-practices.md` §3.2. The reference
point in the field: a comparable Linux-ABI-compatible Rust kernel
(Asterinas, USENIX ATC'25) reports a memory-safety TCB of ~14% of its
codebase under the same kind of measurement. Rux tracks the same axis.

**The rule: the whole-kernel percentage may only go down, or an increase
must be justified per release.** Arch bring-up raises it temporarily (the
x86_64 tree does; that is expected and must shrink as the port
stabilizes). Unsafe added by any fix requires a comment in the commit
body saying why safe code was not possible.

## Method

`scripts/tcb_ledger.py` walks `kernel/src/**/*.rs` (excluding
`kernel/src/tests/`), attributes each `unsafe { ... }` block's span by
brace matching, and counts the covered lines. Approximations are
documented in the script header; the important ones:

- "unsafe lines" = lines from the `unsafe {` line through its matching
  `}` (inclusive), unioned across blocks (a line in two blocks counts
  once); comments and string/char literal contents are stripped first so
  braces inside literals cannot skew the span.
- **Dual metric** (unsafe-slim campaign, 2026-11-19): `unsafe fn` /
  `unsafe impl` / `unsafe trait` / `unsafe extern` bodies are counted as
  a second series (item count + body span lines, scanner shared with
  `scripts/unsafe_census.py`), plus a union column "dual unsafe lines".
  The block-only series undercounted raw-pointer `unsafe fn`s; the dual
  column is the honest total.  The block-only series is kept for trend
  continuity.
- Generated sources (`kernel/src/config.rs`,
  `kernel/src/dfx/ksyms_generated.rs`) are excluded so a bare checkout
  and a built tree produce the same table.

## Current baseline
| Subsystem | .rs files | .rs lines | unsafe blocks | unsafe lines | unsafe % | unsafe fn/impl | fn/impl lines | dual unsafe lines | dual % |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| fs | 85 | 76500 | 915 | 8823 | 11.5% | 284 | 6942 | 15557 | 20.3% |
| process | 12 | 18099 | 312 | 4259 | 23.5% | 47 | 3171 | 7242 | 40.0% |
| sched | 11 | 19703 | 337 | 4872 | 24.7% | 34 | 2639 | 7165 | 36.4% |
| ipc | 9 | 21847 | 366 | 5330 | 24.4% | 41 | 1287 | 6557 | 30.0% |
| arch/x86_64 | 20 | 19833 | 337 | 2767 | 14.0% | 82 | 3948 | 6042 | 30.5% |
| arch/riscv64 | 21 | 15061 | 228 | 1961 | 13.0% | 69 | 3434 | 5304 | 35.2% |
| syscall | 12 | 41454 | 881 | 4495 | 10.8% | 12 | 377 | 4871 | 11.8% |
| drivers | 40 | 26541 | 483 | 3194 | 12.0% | 66 | 1483 | 4463 | 16.8% |
| (top-level files) | 14 | 14928 | 149 | 1359 | 9.1% | 33 | 1928 | 3234 | 21.7% |
| mm | 31 | 20132 | 204 | 1376 | 6.8% | 59 | 1171 | 2499 | 12.4% |
| arch/aarch64 | 20 | 9636 | 186 | 1458 | 15.1% | 58 | 1676 | 2493 | 25.9% |
| dfx | 40 | 18934 | 216 | 1432 | 7.6% | 11 | 542 | 1932 | 10.2% |
| sync | 13 | 20747 | 400 | 869 | 4.2% | 59 | 1141 | 1977 | 9.5% |
| net | 18 | 20460 | 239 | 1563 | 7.6% | 16 | 144 | 1694 | 8.3% |
| interrupt | 9 | 3255 | 52 | 239 | 7.3% | 0 | 0 | 239 | 7.3% |
| io_uring | 1 | 1173 | 49 | 137 | 11.7% | 0 | 0 | 137 | 11.7% |
| perf | 1 | 2109 | 46 | 124 | 5.9% | 0 | 0 | 124 | 5.9% |
| security | 4 | 584 | 7 | 41 | 7.0% | 0 | 0 | 41 | 7.0% |
| trace | 2 | 4531 | 3 | 14 | 0.3% | 4 | 4 | 18 | 0.4% |
| module | 1 | 486 | 4 | 12 | 2.5% | 0 | 0 | 12 | 2.5% |
| timer | 3 | 1814 | 2 | 8 | 0.4% | 0 | 0 | 8 | 0.4% |
| arch (shared) | 2 | 145 | 4 | 4 | 2.8% | 0 | 0 | 4 | 2.8% |
| **Whole kernel** | **369** | **357972** | **5420** | **44337** | **12.4%** | **875** | **30045** | **71712** | **20.0%** |

Reading the baseline:

- Whole kernel: **12.9%** unsafe lines — under the ~14% reference point.
- Largest absolute contributors: `fs` (5,352), `syscall` (3,955),
  `process` (2,807), `drivers` (2,131) — together ~68% of all unsafe
  lines; the fs/ext4 + uaccess paths are where the budget is spent.
- Highest density: `process` 24.6% and `sched` 17.6% (task lifecycle,
  context switch, runqueue surgery) — the areas where raw `Task`
  pointers are most entrenched, and the natural first targets of the
  F2 newtype/guard migration.
- `sync` at 2.8% shows the intended shape: one small unsafe core per
  primitive, safe API above it (§1.5 of the best-practices guide).
