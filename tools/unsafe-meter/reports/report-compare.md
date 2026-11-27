# Unsafe-code meter: archive vs mainline comparison

Same tool, same methodology, same exclusions (`kernel/src/tests/`), same
denominator rule, run identically on both trees. Zero parse errors on either
side.

## Side-by-side

| metric | archive/monolith-pre-crate-flip (f1900270e) | origin/main (bf51b258c) | delta |
|---|---|---|---|
| files scanned | 397 | 402 | +5 |
| physical lines (denominator) | 365,592 | 375,315 | +9,723 (+2.7%) |
| block-series blocks | 2,097 | 1,764 | -333 (-15.9%) |
| block-series lines | 8,085 (2.21%) | 23,016 (6.13%) | +14,931 (+185%) |
| fn/impl-series items | 278 | 166 | -112 (-40.3%) |
| fn/impl-series lines | 3,136 (0.86%) | 277 (0.07%) | -2,859 (-91.2%) |
| **total unsafe lines** | **11,221 (3.07%)** | **23,293 (6.21%)** | **+12,072 (+108%)** |
| macro-internal tokens (approx.) | 5 | 3 | -2 |
| avg lines/block | 3.86 | 13.05 | x3.4 |
| avg lines/fn-impl item | 11.3 | 1.7 | x0.15 |

Top-5 files each side:

| archive | unsafe lines | main | unsafe lines |
|---|---|---|---|
| fs/ext4/file.rs | 398 | **arch/unsafe.rs** | **20,901** |
| fs/file.rs | 358 | dfx/unsafe.rs | 514 |
| sched/edges.rs | 358 | sched/unsafe.rs | 465 |
| arch/x86_64/trap.rs | 324 | drivers/unsafe.rs | 222 |
| mm/buddy_allocator.rs | 269 | process/unsafe.rs | 218 |

## Delta interpretation

**1. The headline total is a concentration artifact, not a safety
regression.** Main's raw total (+108%) is dominated by one file:
`kernel/src/arch/unsafe.rs` holds 20,901 of 23,293 unsafe lines (89.7%). It
is the arch unsafe-consolidation file — every arch unsafe site moved
verbatim into one module (file grows to 27,358 lines; the +9,723 denominator
growth is largely consolidation modules, module docs and re-export shims).
Excluding that single file from both sides' style of accounting, main is
**2,392 unsafe lines / 347,957 = 0.69%** spread over the per-subsystem
`unsafe.rs` files, while everything outside them is `#![forbid(unsafe_code)]`.
The archive tree had no such concentration: 11,221 lines spread over fs,
sched, mm, dfx and arch inline code.

**2. The dual series show a contract-shape change, not added unsafe.**
Archive: 278 `unsafe fn`-family items carrying 3,136 lines (0.86%). Main:
166 items carrying 277 lines (0.07%, -91%). The campaign rewrote
`unsafe fn` signatures as safe fns whose body is one explicit blanket
`unsafe { … }` ("the former unsafe-fn contract, now explicit", per the
in-file comment). That moves the fn-contract lines into the block series
(+185% block lines) and explains the average block size jumping 3.86 → 13.05
lines: blanket blocks wrap whole function bodies, e.g. `riscv_switch_abort`
in `arch/unsafe.rs` lines 147-175. Both series must be read together; the
sum is the real surface, and the fn-series collapse is the campaign working
as designed (less signature-level unsafety, explicit block boundaries).

**3. Subsystem re-homing.** The archive top files are ordinary subsystem
files with inline unsafe (`fs/ext4/file.rs` 398, `fs/file.rs` 358,
`sched/edges.rs` 358) plus the `edges.rs`/`unsafe_edges.rs` audit-edge
modules. On main those are consolidated per subsystem
(`fs/unsafe.rs`, `sched/unsafe.rs`, `drivers/unsafe.rs`, …), so per-file
rankings are not directly comparable file-by-file — the comparable unit is
the total per subsystem, and the totals show the same code now concentrated
in guarded modules.

**4. Macro-internal is noise on both branches (5 vs 3 tokens).** Macro
bodies are not where unsafe hides in either tree. This also closes the
naive-scanner postmortem: the archive branch's earlier brace-scanning run
reported 2,277 "blocks" averaging 1.15 lines — impossible numbers caused by
brace imbalance from `asm!` template placeholders inside string literals
(`"fsd f0, 0*8({0})"`) and macro delimiter tokens, not by unsafe hidden in
macros. Parser-grade enumeration counts 2,097 real blocks averaging 3.86
lines on the same tree.

## Honest measurement notes

- **fn-series spans include attributes/doc comments** (the region is the
  whole item span per the methodology; syn's `ToTokens` emits attrs first).
  On the archive branch, where plain `unsafe fn`s are common, this adds
  their doc-comment lines to the fn/impl-series (avg 11.3 lines/item vs a
  body-only convention). The convention is identical on both branches, and
  on main it matters little (1.7 lines/item). Cross-references to
  body-based numbers (e.g. README's 2,539 fn/impl lines) should expect a
  systematic upward difference on fn-heavy trees for this reason.
- **Blanket-block attribution is by outermost region**: inner blocks inside
  a blanket count (block_count) but their lines attribute to the blanket.
  No line is counted twice; inner-only lines never inflate the block series.
- **Macro-internal lines may overlap** the two primary series; they are
  reported separately and excluded from the total. With 5/3 tokens total
  the overlap question is immaterial here.
- The denominator grew +2.7% between branches; percentages in this report
  always use the same-branch denominator.
- Both scans ran with the tool's hard failure mode (exit non-zero on any
  parse/read error) and exited clean — no file on either branch fell back
  to estimation.

## Validation record

- Fixture self-test (checked-in known-content file: nested blocks, unsafe fn
  with inner unsafe block, unsafe impl, unsafe trait, unsafe extern, plain
  extern NOT counted, macro_rules template unsafe): passes via `cargo test`
  and via the `--selftest` flag — exact counts and exact line sets.
- Hand-checks on real files (all three passed):
  1. archive `kernel/src/arch/riscv64/cpu.rs` — 184-line asm-helper file;
     hand count 12 blocks / 38 lines; tool identical, line-for-line.
  2. archive `kernel/src/arch/riscv64/thread.rs` — 24 `asm!` invocations,
     double-nested `unsafe { unsafe {`, braces inside asm strings (the
     naive-scanner killer); hand count 18 blocks / 4 unsafe fns; tool
     identical, fn-series runs end exactly at each fn's closing brace and
     exclude the blank separator lines.
  3. mainline `kernel/src/arch/unsafe.rs` segments — `get_dtb_pointer`
     blanket (93-98) and `riscv_switch_abort` blanket (147-175): per-line
     attribution verified including inner-block count-only handling and
     attribute/doc lines outside the region.
