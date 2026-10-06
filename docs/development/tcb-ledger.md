# TCB Ledger — kernel `unsafe` accounting

This is a **living document**. Regenerate after any subsystem-wide change:

```sh
python3 scripts/tcb_ledger.py --out docs/development/tcb-ledger.md
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
- `unsafe fn` / `unsafe impl` bodies are **not** counted (only explicit
  `unsafe` blocks); their bodies are ordinary safe-by-default code from
  this metric's viewpoint, which undercounts raw-pointer `unsafe fn`s —
  acceptable for a monotone budget metric.

## Current baseline

| Subsystem | .rs files | .rs lines | unsafe blocks | unsafe lines | unsafe % |
|---|---:|---:|---:|---:|---:|
| fs | 66 | 42313 | 509 | 5352 | 12.6% |
| syscall | 11 | 24992 | 570 | 3955 | 15.8% |
| process | 12 | 11398 | 191 | 2807 | 24.6% |
| drivers | 30 | 13546 | 295 | 2131 | 15.7% |
| net | 18 | 16504 | 215 | 1418 | 8.6% |
| arch/riscv64 | 20 | 10778 | 151 | 1262 | 11.7% |
| sched | 9 | 6403 | 108 | 1129 | 17.6% |
| (top-level files) | 12 | 7596 | 69 | 863 | 11.4% |
| mm | 25 | 11887 | 112 | 813 | 6.8% |
| dfx | 11 | 2603 | 43 | 369 | 14.2% |
| interrupt | 8 | 1802 | 29 | 200 | 11.1% |
| ipc | 6 | 4402 | 51 | 185 | 4.2% |
| arch/x86_64 | 20 | 2498 | 41 | 150 | 6.0% |
| io_uring | 1 | 1167 | 49 | 137 | 11.7% |
| sync | 8 | 3596 | 51 | 103 | 2.9% |
| security | 4 | 559 | 6 | 40 | 7.2% |
| module | 1 | 476 | 4 | 12 | 2.5% |
| arch (shared) | 1 | 54 | 0 | 0 | 0.0% |
| **Whole kernel** | **263** | **162574** | **2494** | **20926** | **12.9%** |

Reading the baseline:

- Whole kernel: **12.9%** unsafe lines — under the ~14% reference point.
- Largest absolute contributors: `fs` (5,352), `syscall` (3,955),
  `process` (2,807), `drivers` (2,131) — together ~68% of all unsafe
  lines; the fs/ext4 + uaccess paths are where the budget is spent.
- Highest density: `process` 24.6% and `sched` 17.6% (task lifecycle,
  context switch, runqueue surgery) — the areas where raw `Task`
  pointers are most entrenched, and the natural first targets of the
  F2 newtype/guard migration.
- `sync` at 2.9% shows the intended shape: one small unsafe core per
  primitive, safe API above it (§1.5 of the best-practices guide).
