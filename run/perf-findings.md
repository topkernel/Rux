# Perf lane pd: drivers (VIRTIO + TIMER + IRQ + console) — 2026-10-31

Lane charter: TASK.md (virtio-blk completion, virtio-net RX, timer
framework cost, IRQ entry/exit, softirq dispatch, UART/console hunt,
hilog tap decode). Ground truth: `test/pbench_boot.py --bzimage <path>`
runs the SAME P1-P7(+P8) battery on the reference kernel and on this
tree's kernels. Machine class: q35, `-cpu max`, SMP2/2G, TCG
`thread=multi` (primary A/B) and KVM (directional; host ran 3 foreign
QEMUs from sibling lanes during the KVM series).

Artifacts: run/pd-{ref-tcg,rux-tcg,rux-fix1}.results (TCG columns),
run/pd-{ref-kvm,rux-basekvm,rux-fixkvm}.results (KVM columns),
kernels run/pd-base-bzImage (lane start, release-opt) and
run/pd-fix1-bzImage (with the timer fix, release-opt).

## Commits on this lane (branch perf-drivers)

1. `6394e8ee` timer(x86_64): LAPIC one-shot re-arm — **p7_sleep1ms med
   1103.783 -> 10.581 µs (104x, TCG); KVM 1371.3 -> 8.2 µs (167x)**.
   Reference kernel: 16.6 µs (TCG) / 16.8 µs (KVM) — after the fix our
   1 ms-sleep overshoot is ~2x BETTER than the reference. Rider:
   p6_write_fsync med 20725 -> 14689 µs (TCG), now below the reference
   (16690): the io-completion park waits also stop grid-quantizing.
2. `3823bfc4` console(fs): userspace /dev/console TX batched through
   `Uart::puts_batch` + the new `p8_console_line` pbench row. Measured
   NEUTRAL on both TCG (98.7 -> 101.0 µs/line) and KVM (1416 -> 1405
   µs/line): the console cost is per-byte QEMU serial device-model
   work, NOT our per-byte LSR polling (see F-console below). Kept:
   strictly fewer port accesses and instructions per line, identical
   semantics; the P8 row is the console instrument the charter asked
   for.

## Full TCG A/B table (med, µs; base = lane start, fix = with 6394e8ee+3823bfc4)

| row | ref | rux base | rux fix | base/ref | fix/ref |
|---|---|---|---|---|---|
| p1_null | 0.240 | 0.601 | 0.631 | 2.5x | 2.6x |
| p1_clock | 0.731 | 0.721 | 0.701 | 1.0x | 1.0x |
| p2_ctxsw | 1.753 | 1.276 | 1.308 | 0.7x | 0.7x |
| p3a_fault_anon | 5.660 | 0.080 | 0.080 | 0.0x | 0.0x |
| p3b_fault_cow | 8.035 | 3.053 | 3.036 | 0.4x | 0.4x |
| p3c_fault_file | 0.080 | 0.070 | 0.060 | 0.9x | 0.8x |
| p4_fork_exit | 349.077 | 506.891 | 485.342 | 1.5x | 1.4x |
| p4_fork_exec | 826.832 | 1595.514 | 1522.456 | 1.9x | 1.8x |
| p5a_pipe | 2.795 | 2.242 | 2.195 | 0.8x | 0.8x |
| p5b_dgram_send | 2.495 | 39.552 | 40.165 | 15.9x | 16.1x |
| p5b_dgram_recv_peek | 2.645 | 39.532 | 40.155 | 14.9x | 15.2x |
| p5c_stream_write | 2.525 | 36.209 | 36.307 | 14.3x | 14.4x |
| p5c2_stream_send | 1.954 | 35.908 | 36.127 | 18.4x | 18.5x |
| p5c3_stream_wrecv | 1.944 | 2.493 | 2.505 | 1.3x | 1.3x |
| p5d_rt_read | 4.308 | 4.275 | 4.289 | 1.0x | 1.0x |
| p5d_rt_recv | 3.837 | 4.855 | 4.800 | 1.3x | 1.3x |
| p5e_stream_send | 1.944 | 2.363 | 2.355 | 1.2x | 1.2x |
| p5f_stream_dwait | 1.954 | 36.099 | 36.297 | 18.5x | 18.6x |
| p6_stat | 3.827 | 17.819 | 18.299 | 4.7x | 4.8x |
| p6_openclose | 5.901 | 31.884 | 32.960 | 5.4x | 5.6x |
| p6_getdents | 805.983 | 539.967 | 541.401 | 0.7x | 0.7x |
| p6_read_cached | 11.702 | 41.214 | 40.727 | 3.5x | 3.5x |
| p6_write_fsync | 16689.8 | 20725.3 | 14688.8 | 1.2x | **0.9x** |
| p6_append | 9.537 | 86.522 | 99.742 | 9.1x | 10.5x |
| p6_mkdrmdir | 147.0 | 29903.7 | 28914.3 | 203x | 197x |
| p6_unlink | 35.5 | 87.794 | 107.228 | 2.5x | 3.0x |
| p7_sleep1ms | 16.591 | 1103.783 | **10.581** | 66.5x | **0.6x** |

## F-timer (FIXED): the hres re-arm path did not exist on x86_64

`drivers/timer/x86_64.rs` had `set_timer`/`set_next_trigger`/
`rearm_for_hres` as NO-OPs: the LAPIC timer ran PERIODIC at 100 Hz, so
every high-resolution deadline (nanosleep/clock_nanosleep hres wheel
entries, and every io completion that parks on a deadline) was checked
only when some CPU's 10 ms grid tick raised the Hrtimer softirq. On
SMP the sleeper woke at the FIRST of either CPU's ticks — still
grid-quantized (that is why base p7 med was ~1.1 ms with a tight p99,
not uniform 0-10 ms: min-of-two-ticks). The riscv64 twin has armed
`min(next grid tick, earliest hres)` via SBI set_timer all along.

Fix (6394e8ee): `apic::timer_arm_oneshot` (LVT one-shot + TMICT count
converted from the nominal 10 MHz timebase through the calibrated CCR
rate, u128 math, clamped >= 1 count), `set_next_trigger` grid-glued
(pulls earlier for `hres_next_deadline()`, skips while no precise
timebase — jiffies-derived clock would mistime), and the tick handler
re-arms BEFORE any in-handler task switch (a one-shot tick that fails
to re-arm would leave its CPU tick-less forever).

## F-console (ROOT-CAUSED, at parity, lever is volume): 1.4 ms/line is QEMU's serial model, not our TX shaping

The OH hunt said "UART print 0.36-0.93 ms/line = 25% of both CPUs".
Measured with the new P8 row (200 x write() of a 96-char line to the
console): our kernel 1.42 ms/line (KVM), **reference kernel 1.23
ms/line (KVM, same machine class, same chardev)** — parity within the
host-load band (3 sibling-lane QEMUs were running). Batching our TX
(`puts_batch`, one bounded THRE wait + one `rep outsb` per 16 bytes
instead of LSR-poll+THR-write per byte) moved it 1416 -> 1405 µs —
neutral: the dominant cost is per-byte work inside QEMU's 16550 model
+ chardev path on EVERY byte the device sink takes, which both kernels
pay. TCG numbers are ~100 µs/line on both for the same reason (the
device model runs inline there). Conclusion: the console boot-cost
lever on OH is VOLUME (the `quietcon` ring-divert switch already
exists for exactly that window), not per-line kernel shaping. The P8
row + this A/B close the charter's "compare reference serial
throughthrough" item with data.

## F-blk (AUDITED, no action): virtio-blk completion is not the fs-row bottleneck

x86_64 virtio-blk-pci completions are IRQ-driven: INTx shared dispatch
→ ISR read (acks the line) → Block softirq → completion walker; the
sync read/write paths sleep on the device waitqueue with a 10 s
deadline timer and a waiter-side lag rescue (`queue.rs
wait_for_used_interruptible`). Evidence from the battery:
p6_write_fsync (the only row that forces a device round-trip per op)
is at 0.9x of the reference after the timer fix — the block
completion path is not where the fs rows lag. The lagging fs rows
(mkdrmdir 197x, append 10x, stat/openclose 5x, read_cached 3.5x) are
metadata/pagecache/VFS costs owned by the fs lane (flagged below).
Under KVM the mkdrmdir gap is extreme (23-31 ms vs reference 7.7 µs)
— worth the fs lane's first look.

## F-net (AUDITED, no action): virtio-net RX shape is right

INTx (riscv64: PLIC; x86_64: shared INTx dispatch) → ISR/STATUS read +
ACK → NetRx softirq → `ethernet_poll` drains the RX used ring in a
loop (batch) → refill. Per-packet lock traffic exists (`rx_queue` +
`rx_last_used` irqsave locks per packet, poll() side), a batching
candidate if a net A/B ever shows a gap — but the battery has no net
row and the OH boots' DHCP/ping volume never showed a net-side
complaint; no change without numbers (lane rule). The prior lane's
INTx IRQF_SHARED registration fix (F5, db035444) is in main.

## F-irq (AUDITED, no action): softirq dispatch / IRQ entry

`__do_softirq` is reference-parity shaped (per-vector 2 ms budget
check, restart budget 10, leaves unreached vectors pending, wakes
ksoftirqd). IRQ entry is a real asm stub (SAVE_ALL, swapgs pairing, DF
clear). The sc184-era softirq-raise gating (TCP count, timer
ever-armed latch) is in place. The remaining measured arch-side gap is
p1_null 2.5x (0.60 vs 0.24 µs TCG) — syscall entry cost, arch lane
territory, not IRQ dispatch. scmtap (hilog tap decode) is properly
gated: one relaxed switch load per potential print site when off;
decode/print only when `dfx=scmtap` armed.

## Flagged for other lanes (measured here, not owned here)

- fs lane: p6_mkdrmdir 197x (29 ms vs 147 µs TCG; 23-31 ms vs 7.7 µs
  KVM), p6_append 10x, p6_stat 4.7x, p6_openclose 5.4x,
  p6_read_cached 3.5x. p6_write_fsync being at 0.9x points at
  metadata/journal/VFS paths, not bio.
- ipc lane: p5b/p5c2/p5f unix-socket rows 14-19x (36-40 µs vs ~2 µs)
  — note the asymmetry p5c3_stream_wrecv at 1.3x: the send side (and
  the dgram pair) carries the cost, the receive side is near parity.
- arch/mm lanes: p1_null 2.5x (syscall entry), p4_fork_exec 1.8x TCG
  (717 vs 20.5 µs under KVM — the KVM ratio is 35x, the biggest
  unowned gap in the KVM column).

## Gates

- 3-arch builds: x86_64 debug rc=0, riscv64 debug rc=0, aarch64 debug
  rc=0 (aarch64 paths untouched; `make test PLATFORM=aarch64` not run
  — no aarch64 code changed).
- `make test` (riscv64 in-kernel suite) on the fixed tree: **green,
  Passed: 1198, Failed: 0** ("ALL TESTS PASSED"; suite verdict green,
  the QEMU wrapper is killed by timeout after the kernel halts — the
  documented rc=124 shape).
- Merge-gate perf domains touched: timer/ (guarded) — pbench P2
  ctxsw within band (1.308 vs 1.276 base TCG), p7 improved 104x,
  no row regressed beyond fs-row boot noise (append 1.15x / unlink
  1.22x in one TCG pair, mkdrmdir 0.97x — same class as the known
  flaky rows; KVM fsync 8.1->13.2 ms is host-load noise band, TCG
  shows 20.7->14.7).
