# Perf review: console

Focus: CONSOLE/PRINTK + BOOT-TABLE — the boot-banner and print_status path writes per-char through the early putc; printk routing (aarch64 just got its PL011 arm); kmsg passthrough for OH (/dev/kmsg writes from pid 1). The OH boot pays 26k serial prints = 25% CPU. Hunt: per-char MMIO vs FIFO batch writes, console lock contention, the THRE pacing window (documented: multi-second IRQ-off windows in chunked writes), early-console fallbacks staying active after the real driver registers. Fix direction: batched TX (PIO burst / interrupt-driven TX ring like the reference 8250 FIFO discipline).

# Perf-review lane rules (binding)
- Mission: find where Rux is SLOW vs the reference kernel and close the gap. Target: 10x-class improvements on the lagging metrics. The user mandate: every interface metric where we lag the reference must be examined.
- GROUND TRUTH: the reference kernel bzImage is at /home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images/bzImage. test/pbench_boot.py --bzimage <path> boots ANY bzImage with the pbench P1-P7 battery — run the SAME battery on the reference kernel and on your rebuilt Rux kernel to get true A/B columns. This is the primary tool: measure reference, measure ours, fix, re-measure.
- Known instrumented gaps (from syscost/OH campaign, x86_64 debug): futex wake→run tick-quantized 10-108ms (reference: µs); openat avg 252µs; p1 null syscall ~4.5µs; p2 ctxsw 15.4µs (debug); p6_append ~640µs; write_fsync ~12ms; mkdrmdir ~20ms; unlink ~1ms; pipe roundtrip ~19µs; UART print 0.36-0.93ms/line; OH boot to launcher.ready 254-541s vs reference 14.4s (~26x whole-system).
- Every fix ships with NUMBERS: pbench median before/after (same boot config), or syscost per-op average before/after. No numbers = not merged.
- Correctness gate: `make test` green (riscv 1198) + x3 builds rc=0. If you touch aarch64 paths, `make test PLATFORM=aarch64` green too.
- Small commits, English messages, NEVER the word "Linux" in commits/docs — write "the reference kernel".
- Record findings/fixes/numbers in run/perf-findings.md; push origin/perf-<scope>.
- One QEMU at a time (check ps; many lanes run); kill only your own; pkill FORBIDDEN.
- Worktree: THIS one. Never write to /home/william/workspace/Rux.
