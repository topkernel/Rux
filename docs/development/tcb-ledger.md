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
- **Scope rule (2026-11-20)**: the ledger counts the files git tracks,
  not whatever the working directory happens to contain. A stale
  generated file left by an older build
  (`kernel/src/debug/ksyms_generated.rs`, 27,625 lines, referenced by no
  module) used to be picked up by the scan and inflated the line
  denominator — which silently diluted every percentage in the table
  (whole-kernel unsafe share read 8.9% instead of 9.5% for the same
  code). `scripts/tcb_ledger.py` and `scripts/unsafe_census.py` now
  filter by `git ls-files kernel/src`, so a bare checkout and a built
  tree agree by construction.

## Baseline 2026-11-19 (unsafe-slim round 3, lane A: primitives + mm/drivers conversion)

| Subsystem | .rs files | .rs lines | unsafe blocks | unsafe lines | unsafe % | unsafe fn/impl | fn/impl lines | dual unsafe lines | dual % |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| fs | 85 | 76849 | 808 | 7644 | 9.9% | 263 | 6576 | 14056 | 18.3% |
| process | 13 | 18609 | 314 | 4271 | 23.0% | 47 | 3311 | 7389 | 39.7% |
| sched | 12 | 20580 | 358 | 4610 | 22.4% | 33 | 2660 | 6915 | 33.6% |
| ipc | 9 | 21920 | 353 | 5331 | 24.3% | 41 | 1301 | 6572 | 30.0% |
| arch/x86_64 | 20 | 20055 | 334 | 2772 | 13.8% | 82 | 4026 | 6144 | 30.6% |
| arch/riscv64 | 21 | 15088 | 226 | 1960 | 13.0% | 69 | 3434 | 5303 | 35.1% |
| syscall | 12 | 41227 | 627 | 3556 | 8.6% | 8 | 310 | 3865 | 9.4% |
| (top-level files) | 14 | 15181 | 150 | 1368 | 9.0% | 34 | 2043 | 3298 | 21.7% |
| arch/aarch64 | 20 | 9636 | 185 | 1458 | 15.1% | 58 | 1676 | 2493 | 25.9% |
| drivers | 40 | 26280 | 296 | 1189 | 4.5% | 64 | 1174 | 2330 | 8.9% |
| dfx | 40 | 19097 | 214 | 1434 | 7.5% | 11 | 586 | 1979 | 10.4% |
| sync | 13 | 21022 | 391 | 860 | 4.1% | 59 | 1141 | 1968 | 9.4% |
| mm | 31 | 20126 | 182 | 805 | 4.0% | 59 | 1171 | 1928 | 9.6% |
| net | 18 | 20560 | 243 | 1571 | 7.6% | 17 | 216 | 1766 | 8.6% |
| interrupt | 9 | 3339 | 52 | 239 | 7.2% | 0 | 0 | 239 | 7.2% |
| io_uring | 1 | 1173 | 43 | 131 | 11.2% | 0 | 0 | 131 | 11.2% |
| perf | 1 | 2109 | 46 | 124 | 5.9% | 0 | 0 | 124 | 5.9% |
| safety | 5 | 817 | 28 | 34 | 4.2% | 4 | 8 | 42 | 5.1% |
| security | 4 | 584 | 7 | 41 | 7.0% | 0 | 0 | 41 | 7.0% |
| arch (shared) | 2 | 389 | 16 | 30 | 7.7% | 0 | 0 | 30 | 7.7% |
| trace | 2 | 4640 | 4 | 15 | 0.3% | 4 | 4 | 19 | 0.4% |
| module | 1 | 486 | 4 | 12 | 2.5% | 0 | 0 | 12 | 2.5% |
| timer | 3 | 1814 | 2 | 8 | 0.4% | 0 | 0 | 8 | 0.4% |
| **Whole kernel** | **376** | **361581** | **4883** | **39463** | **10.9%** | **853** | **29637** | **66652** | **18.4%** |

Lane A delta vs its fork point: drivers 2985 -> 1189 unsafe block-lines (-60%, unsafe %% 11.2 -> 4.5), mm 1385 -> 805 (-42%%, 6.8 -> 4.0); territory total 4370 -> 1994 (-54.4%%). Whole-kernel unsafe %% 11.6 -> 10.9.

## Current baseline (2026-11-20, unsafe-slim round 3c: binder conversion + the fs wrappers)

| Subsystem | .rs files | .rs lines | unsafe blocks | unsafe lines | unsafe % | unsafe fn/impl | fn/impl lines | dual unsafe lines | dual % |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| fs | 85 | 76886 | 715 | 3978 | 5.2% | 257 | 6715 | 10570 | 13.7% |
| sched | 12 | 20576 | 358 | 4610 | 22.4% | 33 | 2660 | 6915 | 33.6% |
| arch/x86_64 | 20 | 20055 | 334 | 2772 | 13.8% | 82 | 4026 | 6144 | 30.6% |
| arch/riscv64 | 21 | 15095 | 226 | 1960 | 13.0% | 69 | 3434 | 5303 | 35.1% |
| process | 13 | 18723 | 321 | 1777 | 9.5% | 23 | 2328 | 4002 | 21.4% |
| (top-level files) | 14 | 15241 | 150 | 1353 | 8.9% | 34 | 2043 | 3283 | 21.5% |
| arch/aarch64 | 20 | 9636 | 185 | 1458 | 15.1% | 58 | 1676 | 2493 | 25.9% |
| drivers | 40 | 26280 | 296 | 1189 | 4.5% | 64 | 1174 | 2330 | 8.9% |
| syscall | 12 | 41041 | 473 | 1962 | 4.8% | 5 | 103 | 2064 | 5.0% |
| dfx | 40 | 19097 | 214 | 1434 | 7.5% | 11 | 586 | 1979 | 10.4% |
| sync | 13 | 21022 | 391 | 860 | 4.1% | 59 | 1141 | 1968 | 9.4% |
| mm | 31 | 20126 | 182 | 805 | 4.0% | 59 | 1171 | 1928 | 9.6% |
| net | 18 | 20637 | 165 | 670 | 3.2% | 8 | 18 | 688 | 3.3% |
| ipc | 9 | 22109 | 119 | 307 | 1.4% | 10 | 10 | 317 | 1.4% |
| interrupt | 9 | 3339 | 52 | 239 | 7.2% | 0 | 0 | 239 | 7.2% |
| io_uring | 1 | 1173 | 43 | 131 | 11.2% | 0 | 0 | 131 | 11.2% |
| perf | 1 | 2109 | 46 | 124 | 5.9% | 0 | 0 | 124 | 5.9% |
| safety | 5 | 817 | 28 | 34 | 4.2% | 4 | 8 | 42 | 5.1% |
| security | 4 | 584 | 7 | 41 | 7.0% | 0 | 0 | 41 | 7.0% |
| arch (shared) | 2 | 389 | 16 | 30 | 7.7% | 0 | 0 | 30 | 7.7% |
| trace | 2 | 4640 | 4 | 15 | 0.3% | 4 | 4 | 19 | 0.4% |
| module | 1 | 486 | 4 | 12 | 2.5% | 0 | 0 | 12 | 2.5% |
| timer | 3 | 1814 | 2 | 8 | 0.4% | 0 | 0 | 8 | 0.4% |
| **Whole kernel** | **376** | **361875** | **4331** | **25769** | **7.1%** | **780** | **27097** | **50630** | **14.0%** |

Round 3c delta vs round 3b: whole-kernel block bodies 34,464 -> 25,769
lines (9.5% -> 7.1%), fn/impl bodies 28,249 -> 27,097, dual series
60,376 -> 50,630 (16.7% -> 14.0%). The binder's block-view census cells
were rewritten as relaxed atomics, one audited TxnBlock window replaced
~20 scattered volatile derefs (INV-BINDER-BLOCKVIEW), and eleven ext4
VFS wrappers now ride one audited (inode, fs, ext4-inode) resolution
(INV-EXT4-FILERES). The scope rule from this morning still applies: the
ledger counts git-tracked sources only.

## Prior baseline (pre-campaign reading, retained for trend continuity)
| Subsystem | .rs files | .rs lines | unsafe blocks | unsafe lines | unsafe % | unsafe fn/impl | fn/impl lines | dual unsafe lines | dual % |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| fs | 85 | 76500 | 913 | 8821 | 11.5% | 279 | 6914 | 15527 | 20.3% |
| process | 13 | 18244 | 310 | 4238 | 23.2% | 47 | 3171 | 7222 | 39.6% |
| sched | 11 | 19713 | 338 | 4780 | 24.2% | 32 | 2622 | 7052 | 35.8% |
| ipc | 9 | 21850 | 354 | 5321 | 24.4% | 41 | 1287 | 6548 | 30.0% |
| arch/x86_64 | 20 | 19833 | 334 | 2764 | 13.9% | 82 | 3948 | 6039 | 30.4% |
| arch/riscv64 | 21 | 15061 | 226 | 1960 | 13.0% | 69 | 3434 | 5303 | 35.2% |
| drivers | 40 | 26541 | 478 | 3189 | 12.0% | 61 | 1103 | 4241 | 16.0% |
| syscall | 12 | 41221 | 627 | 3558 | 8.6% | 8 | 310 | 3867 | 9.4% |
| (top-level files) | 14 | 14933 | 147 | 1357 | 9.1% | 33 | 2042 | 3286 | 22.0% |
| mm | 31 | 20132 | 204 | 1376 | 6.8% | 59 | 1171 | 2499 | 12.4% |
| arch/aarch64 | 20 | 9636 | 185 | 1458 | 15.1% | 58 | 1676 | 2493 | 25.9% |
| dfx | 40 | 18937 | 211 | 1428 | 7.5% | 11 | 586 | 1973 | 10.4% |
| sync | 13 | 20748 | 388 | 857 | 4.1% | 59 | 1141 | 1965 | 9.5% |
| net | 18 | 20460 | 239 | 1563 | 7.6% | 16 | 144 | 1694 | 8.3% |
| interrupt | 9 | 3255 | 52 | 239 | 7.3% | 0 | 0 | 239 | 7.3% |
| io_uring | 1 | 1173 | 43 | 131 | 11.2% | 0 | 0 | 131 | 11.2% |
| perf | 1 | 2109 | 46 | 124 | 5.9% | 0 | 0 | 124 | 5.9% |
| security | 4 | 584 | 7 | 41 | 7.0% | 0 | 0 | 41 | 7.0% |
| arch (shared) | 2 | 389 | 16 | 30 | 7.7% | 0 | 0 | 30 | 7.7% |
| trace | 2 | 4531 | 3 | 14 | 0.3% | 4 | 4 | 18 | 0.4% |
| module | 1 | 486 | 4 | 12 | 2.5% | 0 | 0 | 12 | 2.5% |
| timer | 3 | 1814 | 2 | 8 | 0.4% | 0 | 0 | 8 | 0.4% |
| **Whole kernel** | **370** | **358150** | **5127** | **43269** | **12.1%** | **859** | **29553** | **70312** | **19.6%** |

Reading the baseline:

- Whole kernel (2026-11-20 baseline above): **7.1%** of lines inside
  `unsafe` blocks, **14.0%** on the dual series (blocks plus `unsafe
  fn`/`impl` bodies) — the honest total. The token-metric headline
  (unsafe constructs outside the Framework layer) is 3,166 of 5,224.
- Largest absolute contributors: `sched` (4,610 unsafe block-lines),
  `fs` (3,978), `syscall` (1,962), `sync` (860) — the fs/ext4 +
  uaccess paths are where the budget is spent.
- Highest density: `sched` 22.4% (task lifecycle, context switch,
  runqueue surgery) — the area where raw `Task` pointers are most
  entrenched, and the natural first target of the F2 newtype/guard
  migration. `ipc` is at 1.4% after the binder conversion.
- `sync` at 4.1% shows the intended shape: one small unsafe core per
  primitive, safe API above it (§1.5 of the best-practices guide).
