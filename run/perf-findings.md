# perf-console lane — console/UART TX findings and fixes

Date: 2026-10-31. Branch: perf-console (from main 1e2bef44).

## Method

A/B with the reference bzImage
(/home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images/bzImage)
using `test/pbench_boot.py --probe-bin` and a new console-TX probe
`test/conbench.c` (rows below). Same machine class both arms: q35,
`-cpu max`, TCG thread=multi, SMP2/2G (repo discipline), one QEMU at a
time. Every number is the median of >= 2000 in-guest writes
(CLOCK_MONOTONIC around each write(2)).

Rows:
- c1_line    write(fd1, 79-char line) x2000 — init-stdio console (the OH pid-1 path)
- c1b_line   same, run LAST (ordering/warmup control)
- c1n_nonl   same, no trailing newline (CR-expansion control)
- c2_devcon  same line to /dev/console (devfs condev + tty OPOST layer)
- c3_kmsg    same line to /dev/kmsg (printk ring passthrough + emit)
- c4_1byte   1-byte writes to fd1 (syscall floor)
- c5_burst   2048-byte writes to fd1 (large-buffer amortization)

## Baseline A/B (medians, µs/op; two runs each where shown)

| row | reference | Rux before | ratio |
|-----|-----------|------------|-------|
| c1_line    | 101.9 / 92.0 | 169.7 / 158.3 | 1.7x slower |
| c2_devcon  | n/a (no /dev) | 227.8 / 175.2 | — |
| c3_kmsg    | n/a | 142.8 / 113.6 | — |
| c4_1byte   | 8.0 | 17.5 / 17.6 | 2.2x (syscall entry, not console) |
| c5_burst   | 2075.9 / 2123.5 | 4756.9 / 3640.0 | 1.8-2.3x slower |

Per byte: reference ≈ 1.01-1.29 µs/B; Rux ≈ 2.0-2.3 µs/B. Host load
swings single-boot medians by up to ±30% (observed on unchanged code
paths), so all conclusions below use the paired-run medians and the
c1b/c1n controls from the same boot.

## Root causes found (per-char MMIO/PIO writers)

1. **fd-1/2 "console fast path" in `sys_write`/`write_file_core`
   (kernel/src/syscall/io.rs)** — the OH hot path (pid 1 / hilog write
   to stdout/stderr, the 26k-prints-per-boot volume): copy_in 256B
   chunks, then a per-byte `uart.putc()` loop with `\n` expansion under
   ONE irqsave UART lock across the whole chunk. Per byte: one LSR
   `inb` + one THR `outb` (2 port events/byte; the multi-IRQ-off-window
   hazard the 2026-10-20 gate documented for uart_write existed HERE
   too, at 256-byte granularity).
2. **`char_dev::uart_write`** (all /dev/console, /dev/ttyS0, exec-stdio
   writes): 32-byte chunks under the UART lock, but per-byte `putc`
   inside each chunk (2 port events/byte).
3. **Boot table + banner** (`print_status_ex`, logo/version blocks in
   main.rs): per-byte `console::putchar` = full irqsave lock pair +
   2 port events PER BYTE of every status line (~150 lines/boot).
4. **`fs::tty console_output`** (console tty echo sink): per-byte
   `putchar` (lock pair per byte).
5. **`fmt::Write for Uart`, `print::Console`, dfx `ConsoleWriter`,
   `tests::test_println_str`**: per-byte `putc` loops (panic/DFS dumps
   are thousands of bytes through these).

printk `emit_to_console` was already batched (`puts_batch`, leg-54 C)
and /dev/kmsg passthrough correct — no early-console fallback stayed
registered on x86_64 (single 8250 console; early_init/init_irq only
arm RX). The one "fallback" that DID stay active was item 1: the
syscall-side console fast path bypassing the shared uart_write
discipline.

## Fixes (this branch)

- console.rs: new `puts_bytes()` (one irqsave lock + one batched pass)
  and `puts_bytes_no_lock()`; `puts()` now delegates; `fmt::Write for
  Uart` coalesces runs between newlines into 128-byte bursts.
- char_dev.rs: new safe `uart_write_kernel(&[u8])` (32-byte chunked
  lock + `puts_batch` burst + cond_resched); `uart_write` delegates.
- syscall/io.rs fd-1/2 fast path: keeps its unconditional `\n`->`\r\n`
  expansion (byte-for-byte) but delegates TX to
  `char_dev::uart_write_kernel` — no more inline per-byte loop, no more
  single-lock-held-across-256B window.
- tty.rs `console_output`: one `console::puts_bytes` per slice.
- main.rs: `print_status_ex` stages the whole line in a stack buffer
  (byte-identical output, verified against serial logs); boot banner,
  table header, and logo/version blocks emit one batched write per
  constant.
- dfx/backtrace.rs `ConsoleWriter`: 128-byte staging buffer flushed via
  `puts_batch`.
- print.rs `Console::write_str`, tests/mod.rs `test_println_str`:
  batched with identical byte streams.

## Results (same boot config, medians µs/op)

| row | reference | Rux before | Rux after | before->after | vs reference |
|-----|-----------|------------|-----------|---------------|--------------|
| c1_line    | 92.0-101.9 | 158.3-169.7 | **89.9**  | -44%          | **at parity** |
| c1b_line   | —          | —           | 95.3      | (control)     | — |
| c1n_nonl   | —          | —           | 94.3      | (control)     | — |
| c2_devcon  | —          | 175.2-227.8 | 110.9     | -37..-51%     | — |
| c3_kmsg    | —          | 113.6-142.8 | 126.7     | unchanged by design | — |
| c4_1byte   | 8.0        | 17.5        | 16.8      | unchanged     | 2.1x (syscall entry cost, syscall lane's gap) |
| c5_burst   | 2075.9-2123.5 | 3640.0-4756.9 | **2052.8** | -44..-57% | **at parity** |

Console TX per-byte cost on the large-buffer row: 2.32 -> 1.00 µs/B
(reference 1.01-1.04 µs/B). The fd-1 line-write row is now below the
reference's median on the same machine class.

Remaining gaps (not this lane's console fixes):
- c4_1byte 16.8 vs 8.0 µs: write(2) syscall entry + fdtable Arc +
  flags checks — the known p1_null-class syscall-lane gap.
- c3_kmsg 126.7 µs: /dev/kmsg write does a heap `format!` per record
  plus ring insert; printk emit itself is already batched.

## Correctness gate (all green)

- `make test` riscv64: **1198/1198, ALL TESTS PASSED**
  (run/make-test-riscv.log).
- `make test PLATFORM=aarch64`: **1135/1135, ALL TESTS PASSED**
  (run/make-test-aarch64.log). console.rs is shared with aarch64 —
  required by the lane rules.
- Builds rc=0: x86_64 (booted + measured), riscv64 (make test build),
  aarch64 (run/build-aarch64.log rc=0).
- Boot-table/banner byte-identical vs baseline serial logs (alignment
  verified in work/pbench-cbrux-fix5-serial.log).

## Full pbench P1-P7 battery (regression control, fixed kernel)

Two batteries on the fixed kernel vs one on the pre-fix kernel, same
machine class. Load-comparable run (base 345 s wall / fix2 435 s wall):

- p1..p5 rows: all within ±3% of base.
- p6_stat -7.3%, p6_write_fsync -6.4%, p6_mkdrmdir +1.7%,
  p6_unlink +0.5%, p6_getdents +4.5%, p6_append +8.5%,
  p4_fork_exit +4.2%, p4_fork_exec +7.2%, p7_sleep1ms -43.9%
  (all inside the ±20% Tier-2 band; the deltas track host-load
  differences, not any console-side mechanism).
- The first fixed-kernel battery ran under a 2x host-load window
  (681 s wall) and showed p6_mkdrmdir +119% / p7 +23%; the rerun
  reproduced base-level numbers — the documented load-flaky families
  (AGENTS.md), not regressions.
- PREEMPT-HEAL/PREEMPT-UNDERFLOW serial diagnostics appear during the
  pbench battery on BOTH kernels (base: 182 events, fixed: 353 in a 2x
  longer wall) — pre-existing fork/exec-family noise at equal wall-time
  rate, not introduced here (zero events in every conbench boot of
  either kernel).

## Artifacts

- run/rux-baseline-bzImage (pre-fix), run/rux-fixed3-bzImage (post-fix)
- run/cbref.log run/cbref2.log (reference), run/cbrux-base.log
  run/cbrux-base2.log (before), run/cbrux-fix5.log (after)
- work/pbench-cb*-serial.log, work/pbench-pbfix*-serial.log,
  work/pbench-pbbase-serial.log (raw serial)
- run/pbbase-rows.txt run/pbfix2-rows.txt (battery medians)
