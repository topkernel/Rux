# Unsafe-code meter: archive/monolith-pre-crate-flip

Branch measured: `archive/monolith-pre-crate-flip`, tip `f1900270e42072fb9bdae1146f8b9697a7df07d5`
(the pre-collection archive — unsafe still spread across `edges.rs` /
`unsafe_edges.rs` modules and inline in ordinary subsystem files).

Source materialized with `git archive f1900270e kernel/src | tar -x` into
`run/unsafe-src/archive/`. Measured with the syn-based tool in this crate
(parser-grade; see methodology below). The scan completed with **zero parse
errors** on all 397 files (the tool exits non-zero on any parse or read
error).

## Methodology (identical on both branches)

- Walk `kernel/src/**/*.rs`, excluding `kernel/src/tests/` (excluded from
  both the series and the %-denominator).
- **block-series**: every `unsafe { … }` (syn `ExprUnsafe`). Count = all
  blocks including nested ones. Lines = the block's span, attributed only
  when the block is OUTERMOST (no enclosing unsafe fn / impl / block).
- **fn/impl-series**: `unsafe fn` (items, impl methods, trait methods),
  `unsafe trait`, `unsafe impl`, `unsafe extern`. Count = all items. Lines =
  the whole item span (attributes/doc comments included, matching the
  "whole fn span" region convention), attributed only when OUTERMOST.
- Lines inside an inner unsafe region attribute to the outermost region —
  no double counting; inner regions still count.
- **macro-internal (approximate)**: syn sees macro invocations as opaque
  token streams; bodies are token-scanned for `unsafe` keywords. Identical
  treatment on both branches.
- Line mapping: proc-macro2 fallback spans (`span-locations`), byte offsets
  through a per-file line-offset table.
- Denominator: all physical lines of the scanned files.

## Totals — archive/monolith-pre-crate-flip (f1900270e)

| series | count | lines | % of 365,592 |
|---|---|---|---|
| block-series (`unsafe { }`) | 2,097 blocks | 8,085 | 2.21% |
| fn/impl-series (`unsafe fn`/`trait`/`impl`/`extern`) | 278 items | 3,136 | 0.86% |
| **total unsafe (no double counting)** | — | **11,221** | **3.07%** |
| macro-internal (approximate, may overlap) | 5 `unsafe` tokens | 5 | — |

Sanity metrics: average 3.86 physical lines per unsafe block; average 11.3
lines per unsafe fn/impl item. The two series plus the macro-internal tokens
sum consistently; no file failed to parse.

Macro-internal detail: `fs/ext4/file.rs` 1 token, `list.rs` 1 token,
`safety/call.rs` 3 tokens. Macro bodies are NOT a significant unsafe carrier
on this branch — the naive scanner's earlier failure here (2,277 "blocks"
averaging 1.15 lines) was brace imbalance from `asm!` template braces inside
string literals (`"fsd f0, 0*8({0})"` etc.), not real unsafe hidden in
macros.

## Top-20 files by unsafe lines

```
UNSAFE BLK    FNIMPL LINES  TOT      FILE
398    53     6      398    6301     kernel/src/fs/ext4/file.rs
358    41     10     358    2052     kernel/src/fs/file.rs
358    83     0      358    1100     kernel/src/sched/edges.rs
324    84     2      324    4604     kernel/src/arch/x86_64/trap.rs
269    9      14     269    1729     kernel/src/mm/buddy_allocator.rs
235    5      7      235    516      kernel/src/arch/riscv64/uaccess.rs
232    18     4      232    549      kernel/src/arch/riscv64/thread.rs
222    48     0      222    2147     kernel/src/arch/riscv64/trap.rs
222    14     1      222    12810    kernel/src/sched/sched.rs
219    55     4      219    3856     kernel/src/fs/bio.rs
218    35     5      218    3017     kernel/src/fs/ext4/namei.rs
214    67     4      214    1172     kernel/src/process/unsafe_edges.rs
199    14     5      199    1495     kernel/src/mm/vdso.rs
196    9      1      196    479      kernel/src/dfx/wildwatch.rs
194    20     5      194    541      kernel/src/fs/jbd2/recovery.rs
192    30     1      192    1657     kernel/src/dfx/scribble.rs
189    12     7      189    567      kernel/src/arch/x86_64/uaccess.rs
187    47     1      187    2444     kernel/src/arch/riscv64/mm/mmu_init.rs
186    8      0      186    413      kernel/src/arch/riscv64/vext.rs
182    17     0      182    1201     kernel/src/dfx/leakorigin.rs
```

Per-file block/fnimpl breakdown for the full file list is reproducible with
`unsafe-meter scan run/unsafe-src/archive --json`.

## Structural shape

- Unsafe is spread wide: the top-20 files span fs (ext4, bio, jbd2), sched,
  mm, dfx, and per-arch code, plus the `edges.rs` / `unsafe_edges.rs`
  audit-edge modules (`sched/edges.rs`, `process/unsafe_edges.rs`).
- Plain `unsafe fn` items still exist on this branch (278 items, e.g. the
  four vector-state methods in `arch/riscv64/thread.rs`).
- Blocks are small (avg 3.86 lines) — each `asm!`/raw-pointer edge wrapped
  individually.

## Validation performed on this branch's tree

- `kernel/src/arch/riscv64/cpu.rs` (184-line asm-helper file): hand count
  gave 12 outermost unsafe blocks, eleven 3-line + one 5-line = 38 lines, 0
  fn/impl items. Tool output: identical (38 BLOCK lines at exactly lines
  36-38, 46-48, 73-75, 86-88, 99-101, 110-112, 121-123, 131-133, 140-142,
  149-151, 159-161, 171-175; block_count 12).
- `kernel/src/arch/riscv64/thread.rs` (24 `asm!` invocations, including
  double-nested `unsafe { unsafe {` and braces inside asm template strings —
  the exact style that broke the naive scanner): hand count 18 unsafe
  blocks, 4 unsafe fns (`vstate_save_for_switch`, `restore_vstate`,
  `save_vstate`, `vstate_first_use`). Tool output: block_count 18,
  fnimpl_count 4, 84 fn/impl lines forming four runs ending exactly at each
  fn's closing brace, 148 block lines, 232 total — matches the source.
