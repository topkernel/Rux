# Unsafe-code meter: origin/main (post-collection)

Branch measured: `origin/main`, tip
`bf51b258c82d5a7f49b58a1fbb1f157a3bd9c359` — the current mainline after the
unsafe-collection campaign ("crate flip" follow-ups included).

Source materialized with `git archive origin/main kernel/src | tar -x` into
`run/unsafe-src/main/`. Measured with the syn-based tool in this crate.
The scan completed with **zero parse errors** on all 402 files.

Methodology: identical to the archive report (same tool, same exclusions,
same denominator rule) — see report-archive.md.

## Totals — origin/main (bf51b258c)

| series | count | lines | % of 375,315 |
|---|---|---|---|
| block-series (`unsafe { }`) | 1,764 blocks | 23,016 | 6.13% |
| fn/impl-series (`unsafe fn`/`trait`/`impl`/`extern`) | 166 items | 277 | 0.07% |
| **total unsafe (no double counting)** | — | **23,293** | **6.21%** |
| macro-internal (approximate, may overlap) | 3 `unsafe` tokens | 3 | — |

Sanity metrics: average 13.05 physical lines per unsafe block; average 1.7
lines per unsafe fn/impl item (mostly tiny `unsafe extern` blocks and trait
decls).

Macro-internal detail: `safety/unsafe.rs` 3 tokens. As on the archive
branch, macro bodies carry essentially no unsafe — nothing significant is
hidden from the parser.

## Concentration

`kernel/src/arch/unsafe.rs` alone holds **20,901 of 23,293 unsafe lines
(89.7%)**. It is the arch unsafe-consolidation file: every unsafe site of
riscv64/x86_64/aarch64 arch code moved verbatim into one module, with the
former `unsafe fn` contracts rewritten as safe fns whose whole body sits in
one "blanket" `unsafe { … }` block (the in-file comment says exactly that:
"blanket: the former unsafe-fn contract, now explicit").

Excluding that single file, mainline is
**2,392 unsafe lines / 347,957 = 0.69%**, spread across the per-subsystem
consolidation files (`dfx/unsafe.rs`, `sched/unsafe.rs`, `drivers/unsafe.rs`,
…). Every sibling file outside the unsafe.rs modules is
`#![forbid(unsafe_code)]` per the README.

## Top-20 files by unsafe lines

```
UNSAFE BLK    FNIMPL LINES  TOT      FILE
20901  1116   5      20901  27358    kernel/src/arch/unsafe.rs
514    62     1      514    1202     kernel/src/dfx/unsafe.rs
465    95     11     465    1474     kernel/src/sched/unsafe.rs
222    59     20     222    812      kernel/src/drivers/unsafe.rs
218    66     4      218    1221     kernel/src/process/unsafe.rs
195    89     46     195    1073     kernel/src/fs/unsafe.rs
193    36     3      193    734      kernel/src/unsafe.rs
112    54     20     112    812      kernel/src/mm/unsafe.rs
94     10     0      94     225      kernel/src/interrupt/unsafe.rs
91     28     29     91     502      kernel/src/sync/unsafe.rs
58     38     10     58     533      kernel/src/ipc/unsafe.rs
51     26     0      51     302      kernel/src/perf/unsafe.rs
37     12     0      37     202      kernel/src/io_uring/unsafe.rs
35     24     6      35     264      kernel/src/safety/unsafe.rs
33     20     6      33     347      kernel/src/net/unsafe.rs
33     5      0      33     101      kernel/src/security/unsafe.rs
30     19     1      30     535      kernel/src/syscall/unsafe.rs
6      2      4      6      41       kernel/src/trace/unsafe.rs
5      3      0      5      66       kernel/src/timer/unsafe.rs
0      0      0      0      35       kernel/src/arch/aarch64/boot.rs
```

(The tail of the top-20 is a run of zero-unsafe files — every arch file
outside `arch/unsafe.rs` is compiler-enforced zero-unsafe.)

Per-file block/fnimpl breakdown for the full file list is reproducible with
`unsafe-meter scan run/unsafe-src/main --json`.

## The blanket-block shape, hand-verified

Two segments of `kernel/src/arch/unsafe.rs` were verified line-by-line with
`unsafe-meter lines`:

- `get_dtb_pointer` (line 93): safe fn whose body is one blanket
  `unsafe { … }` spanning lines 93-98. Tool attributes lines 93-98 to the
  block series; the inner `unsafe { dtb_pointer }` on line 97 is counted as
  a block but its line attributes to the outermost blanket (no double
  counting). The `///` doc lines (88-92) are correctly outside the region.
- `riscv_switch_abort` (line 147): blanket block lines 147-175 attributed to
  the block series; `#[no_mangle]` on line 146 correctly outside (region
  starts at the fn header line where the blanket opens); the following
  `context_switch_body` blanket starts at line 180 with its inner
  `unsafe { riscv_switch_abort(...) }` on line 194 also block-attributed via
  the outermost blanket.

This is real restructured code, not a measurement artifact: the campaign
converted `unsafe fn` signatures into explicit blanket blocks. It moves
lines from the fn/impl-series into the block-series and wraps whole bodies
(hence avg 13 lines/block vs the archive's 3.86).

## Validation performed on this branch's tree

- The two `arch/unsafe.rs` segments above (blanket-block attribution,
  attribute lines excluded, inner blocks count-only).
- Fixture self-test (`unsafe-meter selftest` / `cargo test`): known-content
  file with nested blocks, unsafe fn containing an unsafe block, unsafe
  impl, unsafe extern, macro_rules template unsafe — all counts and exact
  line sets assert green before both scans were trusted.
