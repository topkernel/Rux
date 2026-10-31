# Perf findings — sched/futex lane (perf-sched), 2026-10-31

Worktree: /home/william/rux-agents/perf-sched, branch perf-sched.
Mission: the charter's biggest gap — futex wake-to-run tick-quantized
(10-30ms steps, avg 108ms under OH load; reference kernel single-digit
µs) = 200-1000x. Method: identical pbench probe binary booted on the
reference bzImage and on rebuilt Rux kernels, same machine class
(q35, -cpu max, SMP2, 2G; TCG thread=multi for the relative record,
KVM as the primary rig per policy).

## Commits (this lane)

1. `5a86bc60` mm: mprotect coverage walk must seed with the VMA
   CONTAINING addr (correctness regression fix — see F1).
2. `4d9f26b4` test(pbench): P2f futex wake->run battery.
3. `6ab42522` sched(x86_64): arm the IRQ-exit kernel-preempt hook by
   default (kpreempt=0 opts out) — the headline fix (F2).

## F1. mprotect interior-range ENOMEM — regression from 1f2f18cb (FIXED)

The perf-mm lane's bounded coverage walk (commit 1f2f18cb, today)
uses `iter_after(addr-1)`, which yields only VMAs whose START >= addr:
the VMA *containing* addr is skipped, so every mprotect beginning
inside a VMA (not at its start) sees a phantom gap and returns ENOMEM.

That is musl pthread_create's stack construction verbatim (mmap
PROT_NONE whole, mprotect(base+guard, stack, RW)). Probe evidence
(run/ps-probe-boot.sh, x86_64 strong, 1f2f18cb kernel): interior
mprotects at sizes 0x1000..0x40000 all failed errno 12; whole-VMA
mprotect worked. Consequence: every threaded musl static binary broke
(pthread_create -> EAGAIN, residual errno ENOMEM). The b91-b92 comments
in sys_mprotect even describe this exact pattern as known-critical.

Fix: seed the walk with `vma_mgr.find(addr)` (O(log n) containing-VMA
lookup); None leaves cursor at addr so real gaps still fail ENOMEM.
No-alloc/no-sort property kept. Regression test added to
mem_mmap (test 8b); NOTE the in-kernel test context has no user mm so
the row SKIPs there — the operative verification is the boot probe
(all sizes pass post-fix) and the p2f battery running threads.

## F2. Futex wake-to-run is syscall-length-quantized behind kernel-mode
incumbents — kernel-preempt hook default-ON collapses it (FIXED, x86_64)

Mechanism (why the OH census saw 10-30ms steps / 108ms avg):
- USER-mode incumbents: the wake IPI sets need_resched; trap_exit asm
  (.Lreturn_user loop, x86_64 trap.S) preempts at IRQ return. µs. Fine.
- KERNEL-mode incumbents: the resched IPI only set a flag; the IRQ-exit
  kernel-preempt hook existed but was disarmed (sc69-73 leak
  amplification verdict), and cond_resched() call sites are nearly
  nonexistent (only char_dev UART + arch files). The woken task waits
  for the incumbent's next syscall return — i.e. wake latency is
  quantized by whatever syscall the incumbent is in. On the OH image
  that's our slow syscall classes (fsync 12-14ms, mkdrmdir 14-16ms,
  socket ops 36-40µs at 15-18x reference) stacked through 50-hop IPC
  chains — the "tick-quantized" signature and the 26x boot wall's
  engine, as the in-tree kpreempt commit message already argued.

New pbench P2f battery (thread ping-pong, musl private futexes, waiter
pinned CPU1, waker CPU0; hog pinned CPU1 loops pread(4MB) of a cached
file = one contiguous kernel stretch per syscall, hogcal row times it):

  TCG q35 smp2/2G (relative record)   hogcal med:
    reference kernel                     4783 us
    ours (hook off)                      5501 us
  khog wake->run med (w2r):
    reference kernel       85.8 us  (87% of wakes <100us — preempts mid-syscall)
    ours, hook off       5607.9 us  (97% of wakes 1-10ms)  = 65x reference
    ours, hook on          23.2 us                             = 241x collapse
  KVM (primary rig)                   hogcal med:
    reference kernel                       72 us
    ours (hook off)                       160 us
  khog w2r med:
    reference kernel        8.2 us
    ours, hook off        163.0 us  (b1lt1ms 19778/20000)  = 20x
    ours, hook on           9.5 us                          = parity
    ours, default-on        9.3 us  (no cmdline flag)
    ours, kpreempt=0      161.9 us  (opt-out verified)

Controls: quiet w2r and user-hog w2r never regressed (KVM: 7.6/9.0us
hook off vs 7.5/9.1us on — the user-mode preempt path was already fine;
TCG quiet: 24.7 -> 21.0us).

Full-battery regression (KVM, back-to-back kp=0 vs kp=1, same probe):
every row within ±5% (p1_null 0.080 identical, p2_ctxsw 0.180->0.175,
p5/p6 rows at 1.00-1.05, p6_unlink 0.91). The hook's disabled-path cost
is not measurable. p7_sleep1ms med 984->1089us (+11%) on one pair of
boots — flagged for the merge gate to re-verify with med-of-runs; p7 is
the timer-overshoot row, plausibly noise.

Boot beacon delta (same boots): PREEMPT-HEAL events DOWN with the hook
on (49 -> 2), zero WAKE-STRANDED, no new panic classes — consistent
with the kpreempt commit's argument that WAKE-HEAL + the 2s tick heal
now bound the old sc70 monopoly amplification.

Residual risk, recorded: a task carrying a LEAKED preempt_count that
stays in kernel loops is still unpreemptable while count != 0 (same as
before the flip); the leak producer hunt (sc73 fingerprints: ICACHE,
RECORD_LOCK_TABLE, heap spinlocks — the VFS double-run engine) remains
open and is now more user-visible. kpreempt=0 is the escape hatch.

Gates: riscv64 `make test` ALL TESTS PASSED (1198); x86_64 debug +
release-opt(strong) + aarch64 builds rc=0.

## F3. riscv64: secondary harts never enabled SSIE — every IPI masked,
##       every cross-hart wake tick-quantized (FIXED — the headline)

After the x86_64 work, porting the battery to riscv64 (primary
platform, GNOME target) reproduced the charter's signature on a QUIET
two-CPU boot:

  riscv64 strong, TCG virt SMP2, pbench p2f (waiter pinned hart 1):
    quiet futex wake->run med 9118us — 49% of wakes clustered at the
      10ms tick (p99 10009us); rt row shows exactly one tick-stall per
      roundtrip
    user-hog incumbent w2r med 9952us (96% at the tick)
    kernel-hog (pread4M, hogcal 1669us) w2r med 1447us (syscall-length)
    PROOF OF MECHANISM: pin BOTH threads to hart 0 (no cross-hart IPI
      needed) — quiet w2r med 24.2us, khog 25.2us, uhog 18.7us. The
      wake path was never slow; the cross-hart IPI never took effect.

ROOT CAUSE: `ipi::init()` sets sie.SSIE once, on the boot hart only
(main.rs). sie is per-hart, boot.S zeroes it on every hart, and
`secondary_cpu_entry` re-enabled only STIE + SEIE. Every secondary hart
ran with software interrupts masked forever: resched/call-function/
TLB-flush IPIs sat in sip.SSIP pending-but-disabled; the wake's
need_resched was never set by the IPI; the target ran the waiter only
when its 10ms timer tick's own scheduler work preempted the incumbent.
(TLB-flush IPIs to secondaries were silently never executing either —
fire-and-forget, a standing TLB-staleness hazard.)

FIX: one `csrsi sie,2` per secondary bring-up (ipi::
per_hart_enable_ssoftware from secondary_cpu_entry). Commit 94c8dcec.

  after (same rig, hook off):  quiet 35.0us (was 9118 = 255x)
                               uhog  33.2us (was 9952 = 300x)
                               khog 1438us (unchanged — see F2' below)
  stability: make test ALL TESTS PASSED; full P1-P7 battery on riscv64
  boots rc=0 zero panics; reproduced across two boots (35.7/35.0).

## F3b. riscv64 kernel-preempt twin: implemented, default OFF (FILED)

The riscv64 twin of the x86_64 kernel-preempt hook is in the tree
(commit 9f5ebc50, armed by kpreempt=1, default off). Armed, its first
battery hit the pick-machinery corruption the .Lcheck_resched_kernel
asm comments warn about (R46/R49/R56 phantom-heal cascade into an
idle-task user-frame KERNPANIC ~30s into the quiet regime). The x86_64
twin runs the same battery clean (its guards were tuned through
sc65-sc73). Stabilizing the riscv twin requires the R53/R84/R85
trap-frame work, not scheduler changes. Prize once stable: the khog
(syscall-quantized) wake, med 1438us today vs x86_64's 23us TCG /
9.5us KVM with the hook.

## F3c. riscv64 probe ecosystem notes (for future lanes)

- Ubuntu glibc-static is unusable on this kernel: unconditional RVV
  code + sstatus.VS never enabled -> every vector insn SIGILLs.
  toolchain/build-musl.sh builds a scalar riscv64 musl that works
  (pbench links it via -nostdlib).
- rdcycle traps in U-mode under QEMU's direct-S-boot (mcounteren
  state); rdtime is the legal U-mode counter (vDSO uses it).
- pbench_boot_rv.py carries both workarounds; the cmdline contract for
  kpreempt is uniform across arches now (=1 arms, =0 disarms, absent =
  arch default: ON for x86_64, OFF for riscv64).


## F4. Shared futex keys are raw VAs — no page identity (FILED)

FutexKey::matches for FLAGS_SHARED futexes compares `uaddr` only.
Two processes mapping the same physical page at different VAs never
match (lost wakeup); unrelated mappings at the same VA falsely match
(cross-process wake). The reference kernel keys shared futexes on the
page (inode offset / anon page). Worked around in every in-tree user
by fork-inherited VA equality. Not fixed in this lane (correctness
surface, needs its own LTP coverage: FUTEX_SHARED across processes
with distinct mmaps of one shm object).

## F5. Side observations from the hunt (numbers, uninvestigated)

- pwrite(4MB) of a fresh file: med 2.02 s/op on our kernel (TCG,
  20-sample probe row p2f_hogcal pread era). The write path serializes
  on real I/O per pwrite (journal/writeback interplay) — 2MB/s class.
  Worth an fs-lane look; it makes any write-heavy incumbent a
  seconds-class wake blocker (though it sleeps, so it frees the CPU).
- getdents64 of a freshly-created 8000-entry dir: 18.1 us/full sweep
  on ours (hot), yet p6_getdents of the p6-built 1000-file tree is
  ~550 us/sweep (strong, KVM 127us) — an order-of-magnitude gap
  between hot-created and p6-shaped trees that the fs lane should
  decompose (dcache presence? page-cache vs dentry path?).
- p7_sleep1ms overshoot: ours ~984-1089us med (KVM) / 1475us (TCG) vs
  reference 16.7us — a ~60-88x timer-resolution gap (1ms sleeps return
  after ~1.5 ticks). Separate lane (timer/hres), but it is the other
  half of "woken task waits for the next tick".

## F6. Full-battery KVM A/B — the remaining-gap map (for other lanes)

Reference vs ours (ours = default-on kernel, kp=1 arm), same probe,
KVM, medians (boot pair psrefk2 / psfull1):

  row                    ref      ours    ours/ref
  p1_null              0.040     0.080     2.0x
  p1_clock             0.070     0.190     2.7x
  p2_ctxsw             0.085     0.175     2.1x
  p3a_fault_anon       4.669     0.010     we WIN (eager mapping)
  p3b_fault_cow        0.671     2.314     3.5x
  p3c_fault_file       0.090     0.010     we WIN
  p4_fork_exit        12.964   633.680    48.9x   <- mm lane (known)
  p4_fork_exec        26.139  1006.735    38.5x   <- mm lane
  p5a_pipe             0.170     0.341     2.0x
  p5b/p5c socket family 0.12-0.19  1.96-2.30  12-19x  <- ipc lane
  p5d_rt_read/recv     0.24-0.28  0.63-0.67  2.3-2.8x
  p6_stat              0.230     3.687    16.0x   <- fs lane
  p6_openclose         0.371     5.140    13.9x   <- fs lane
  p6_read_cached       0.531     5.691    10.7x   <- fs lane
  p6_append            0.411    62.416   151.9x   <- fs lane
  p6_mkdrmdir          7.745 14028.371  1811.3x   <- fs lane (the single
                                                      worst row anywhere)
  p6_unlink            1.924    11.151     5.8x
  p6_write_fsync    9329.493 14864.057     1.6x   (I/O-bound both)
  p7_sleep1ms         17.089  1089.354    63.8x   <- timer lane (1ms
                                                      sleeps return ~1
                                                      tick late)
  p2f_futex_* (quiet) 18.0      7.3      0.41x   <- this lane: we now
                                                    WIN the futex rows
  p2h_futex_uhog       6.2       9.1      1.45x

(The full-battery p2g rows have no hog — the hog file only exists in
p2fonly mode; use the dedicated psrefk1/psruxk1-k4 boots for khog
verdicts. Single-boot medians on a multi-lane host; treat ±25% as
noise per the lane protocol.)

Note for the sched lane's open charter items: p2_ctxsw at 2.1x
reference (0.175 vs 0.085us KVM) is a fixed ~90ns syscall-entry/
switch tax, not a scheduling defect; the GRQ hold-time dissection
(5.1-6.1us census band) remains merge-gate territory — nothing in this
lane's A/B points at the pick path as a wake-latency contributor once
kernel preemption is on.

## Artifact map

- run/ps-pbench — the probe binary used on BOTH kernels (md5-identical
  arms; pbench_boot.py --probe-bin).
- run/ps-our-{base,fix1,kpdef}-bzImage — our kernels: 159111af lineage
  base / +mprotect fix / +default-on.
- work/pbench-psref*.log, -psrux*.log, -psfull*.log — serial logs of
  every numbered boot above.
- run/ps-probe-boot.sh — one-off init=/test/probe boot driver.
- Full battery baselines: psref1/psref2 (ref TCG), psrux1 (ours strong
  TCG, pre-fix probe), psfull0/psfull1 (ours KVM kp=0/1).
