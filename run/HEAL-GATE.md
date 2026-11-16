# HEAL-GATE — PREEMPT-HEAL adjudicated on acquire-observed evidence

Lane: heal-gate (122nd session), worktree /home/william/rux-agents/heal-gate,
branch `heal-gate`. Base: main @ d0fb0005a (the preemptible-TTAS fix in).
Commit: 70ee24fbc — `Gate PREEMPT-HEAL on acquire-observed evidence`.

Mission (runq-strand's named follow-up): the heal "can't distinguish a
leaked count from a slow legitimate waiter — the waits were on the
stalled binder OWNER-LATCH chain (latch_age 2820ms); their counts were
zeroed mid-acquire by the tick-side PREEMPT-HEAL storm at 20.5s". Make
the heal itself evidence-gated: a slow waiter actively spinning must
never be healed.

## 1. The design

Two halves, one contract.

### 1.1 Acquire-side evidence (kernel/src/sync/spinlock.rs)

The contended TTAS loop in `RawSpinlock::lock` already samples
need_resched once per `SPIN_PREEMPT_PERIOD` = 65536 spin iterations
(the 121st's preempt exit). The HEAL-GATE stamp rides the SAME 64K
boundary: every boundary iteration publishes, on the spinner's OWN
CPU's slot,

```
ACQUIRE_EVIDENCE[cpu] = { task: cpu_state(cpu).current, jiffies: now }
```

- `AcquireEvidence` = 16 bytes (AtomicUsize task + AtomicU64 jiffies),
  one slot per CPU, static BSS.
- Writer and reader BOTH use the scheduler's published
  `cpu_state(cpu).current` word — the exact pointer the tick-side heal
  adjudicates against. No tp-register-vs-slot skew is possible by
  construction (the x86_64 arch keeps a separate per-CPU current_task
  slot; reading different sources on the two sides would have silently
  disabled the gate).
- Concurrency: the only writer is the CPU's current spinner; the only
  reader is the tick IRQ on that SAME CPU. The tick interrupts the
  loop, so the pair never runs concurrently; Relaxed per-word
  coherence suffices (same shape as TICK_ENTRY_PC). A coupled spinner
  (preempt_count >= 1) cannot migrate or be preempted, so the task
  identity is stable for the whole contended wait.
- Cost: one per-CPU slot read, one jiffies read, two relaxed local
  stores per 64K spins (~0.07-2ms KVM/native) — nothing on the lock's
  own cacheline (TTAS keeps its no-RMW-while-held property), nothing
  on the fast path.

### 1.2 The gate (kernel/src/sched/sched.rs, scheduler_tick)

Both heal legs — `PREEMPT-HEAL` (base bits, the rs2 storm leg) and
`PREEMPT-HEAL-FIELD` (extra hardirq levels) — are now else-chained
behind one refusal:

```
acq_live = acquire_evidence_fresh(cpu_id, cur, now)
         = ev.task == cur && now - ev.jiffies <= ACQUIRE_EVIDENCE_FRESH_JIF
```

- `ACQUIRE_EVIDENCE_FRESH_JIF = 10` (100ms at 100Hz): 50-1300x margin
  over the stamp refresh period, 20x UNDER the heal's own 200-tick
  (2s) bar — a leaked count is delayed by at most this window past its
  last genuine contended spin.
- Fresh evidence ⇒ REFUSE: rate-gated `PREEMPT-HEAL-REFUSED cpu= pid=
  pc= ev_age= face=B|H` row (20j policy, same as PREEMPT-LEAK), NO
  window reset, NO run-counter reset — the count stays armed and the
  next tick re-adjudicates once the evidence goes stale.
- Stale evidence (>= 10j since this CPU's last contended-loop stamp by
  THIS task) ⇒ the heal proceeds exactly as before. The refusal is
  pure added information for the heal's existing adjudication; no
  other behavior changed.
- PARKED-HEAL stays ungated: a task current on NO CPU cannot be
  executing an acquire; no acquire-side evidence can exist for it.

Contract: **a waiter actively spinning in a raw acquire is never
healed.** A count whose owner left the contended loop (acquired, or
never spun) is adjudicated by the unchanged 200-observation bar.

## 2. Fast-path proof (disassembly, release bzImage)

Baseline = d0fb0005a release ELF (`run/syms/rux-baseline.elf`), fix =
70ee24fbc release ELF (`run/syms/rux-fix.elf`). Normalized diffs in
`run/disasm-lock-{baseline,fix}.txt`; x86_64 `lock()` at
0xffffffff80282580 in both.

- The uncontended fast path — prologue, ra capture, `xor r14d`
  (spins=0), `lock cmpxchg [rdi],ecx`, `jne` to the contended arm,
  epilogue, `ret` (0xffffffff80282580..0xffffffff802825b0) — is
  **byte-identical**. The only differences before the contended arm
  are branch DISPLACEMENTS of two jumps whose targets moved (+0x30
  text bytes inserted after the divergence point).
- The stamp lives strictly inside the once-per-64K boundary arm:
  after `test r14w,r14w; jne <pause-path>` (the mask test shared with
  the 121st preempt exit), the fix adds exactly:
  `gs:` cpu_id load → bounds cmp → `cpu_state(cpu).current` load →
  null test → `shl rax,4` (16-byte slot) → store task → load JIFFIES
  → store jiffies (0xffffffff802825e9..0xffffffff8028261b), then falls
  into the unchanged `spin_may_preempt` sequence.
- The normal spin iteration (load → test → inc → mask-test → jne →
  deadlock-check cmp → `pause` at +0x50 → load) executes none of it.
  The famous monopolist RIP `lock+0x52` (0xffffffff802825d2, the TTAS
  load) is unchanged in position and encoding.
- Cross-arch: the riscv64 build references ACQUIRE_EVIDENCE from the
  same two sites (lock's contended arm + scheduler_tick reader).
- Note for grep-based checks: the release compiler materializes the
  one-use print literals as mov-immediates, so `strings bzImage`
  finds no "PREEMPT-HEAL-REFUSED" — presence was verified by
  disassembly (writer stores + reader compare + age `cmp rcx,0xb` +
  base mask `test r15d,0xfff0ffff` in scheduler_tick).

## 3. Gates

- x86_64: `PLATFORM=x86_64 make build` rc=0 (dev);
  `make build-x86-release` rc=0 (twice — before/after the
  cpu_state-current symmetry fix); config-clear rebuild rc=0
  (run/gate-x86-cleanconfig.log).
- riscv64 build rc=0 (run/gate-riscv-build.log); aarch64 build rc=0
  (run/gate-aarch64-build.log) — both config-clear.
- riscv64 `make test`: **ALL TESTS PASSED**, 1330 PASS lines
  (run/gate-riscv-maketest.log; rootfs built first,
  run/gate-rootfs.log).

## 4. rs2-class verification (OH wedge-repro base, KVM q35 SMP8 8G)

Rig: `run/hg-boot.sh` + `run/lb-boot-qmp.py` — the runq-strand rs-boot
face (NOSNAP userdata, dfx=txnhunt,fuxstuck,periodic,parkdump,
schedtrace, LL4), except each boot uses a PRIVATE full copy of the
runq-strand `userdata-rsbase.img` (16G, ext4 has no reflink; copy
deleted after each boot) so the shared base is never mutated.

Boots:
- hg1 = 70ee24fbc (gate), 540s.
- hgc1 = d0fb0005a control (pre-gate release, `run/bzImage-control`
  rebuilt from the stash-built baseline ELF) — same base state, same
  rig: separates gate effect from boot-luck on this (heavier,
  post-rs5-accumulated) base.
- hg2w = 70ee24fbc + debug=wwcensus — adds the PREEMPT-LEAK reporter
  (rip=/pctick= rows) so the healed carriers' wait sites can be named.

### 4.1 hg1 results (the gate kernel)

| row | hg1 (gate) |
|---|---|
| panics | 0 |
| PREEMPT-HEAL-REFUSED rows | 1377 (ev_age=1j on sampled rows — live spinners, refused through the whole stall window) |
| PREEMPT-HEAL rows | 31 (all was=0x10001/0x10003, all with STALE evidence — see 4.3) |
| PREEMPT-HEAL-FIELD / PREEMPT-UNDERFLOW / WAKE-HEAL / PARKED-HEAL / PARKED-LEAK / TICK-GHOST / DEADLOCK | 0 each |
| MONOPOLIST rows | 54 — ALL with coupled counts (pc=0x1 x30, 0x3 x19, 0x101/0x103 x4, 0x10001 x1); ZERO pc=0x0 (the rs2 heal-product monopolist class is gone) |
| RUNQ-STRAND rows | 197 (wedge face present — driven by the image-side stall: 807 FUX-STUCK-class lines vs rs5's 45; FUX stuck_total=94 standing at window end) |
| window | alive to 539s (TXN-HANG-BWR census at end; tick cadence maintained) |

The monopolists in hg1 are coupled spinners at `lock+0x52` on
multi-second holder chains (act=6-22s) — kernel-legitimate spinlock
behavior under the (image-side, out-of-scope) param-chain stall. The
31 heals that still fired all landed on 0x1000x softirq-carrier counts
whose raw-spin evidence had gone stale >= 10j — by the contract those
targets were NOT actively spinning in a raw acquire at adjudication
time (e.g. pid 519: refused at 83.518 while spinning, healed at 83.747
only after its stamp went ~23j stale — it had left the contended loop
while keeping softirq+3 levels held in a different wait shape).

### 4.2 Control (hgc1) and wwcensus (hg2w) boots

| row | hgc1 (control d0fb0005a) | hg2w (gate + wwcensus) |
|---|---|---|
| panics / STRAND / MONOPOLIST | 0 / 0 / 0 | 0 / 0 / 0 |
| PREEMPT-HEAL / -FIELD / -REFUSED | 0 / 0 / — | 0 / 0 / 0 |
| PREEMPT-UNDERFLOW / WAKE-HEAL | 0 / 0 | 0 / 0 |
| FUX-STUCK face | mild (148 lines, stuck_total=244@177s, resolved) | mild (stuck_total=226@113s) |
| PREEMPT-LEAK rows (wwcensus) | — | 542 (see 4.4) |

Both drew clean boots — the wedge is stochastic on this base (the
rs-lane saw the same: rs1/rs3 clean, rs2/rs4 wedge), so no same-face
A/B of strand counts was possible in three draws. hg1 drew the
wedge-class face, which is where every protection claim below comes
from. Notably hgc1's image-side stall was HEAVIER by stuck_total
(244 vs hg1's 94) yet stayed clean — the wedge needs the specific
frozen-producer chain arrangement, not just stall volume.

### 4.3 The battery table (wedge-class boot, hg1)

| family | hg1 (gate, wedge face) | rs2 (pre-fix, wedge) | rs5 (121st fix, mild face) |
|---|---|---|---|
| PREEMPT-HEAL rows | 31 (all stale-evidence carriers) | ~10 storm at 20.5s (live waiters mid-acquire) | 0 |
| heals on actively-spinning waiters | 0 (1377 refusals, ALL ev_age=1j, 20 distinct pids) | 6 CPUs zeroed mid-acquire | — |
| monopolists pc=0x0 (heal-product) | **0** (all 54 rows coupled: 0x1 x30, 0x3 x19, 0x101/0x103 x4, 0x10001 x1) | 3 permanent (the rs2 conviction) | 0 |
| PREEMPT-UNDERFLOW | 0 | 0 (waiters never stopped spinning) | 0 |
| WAKE-HEAL / PARKED-HEAL / PARKED-LEAK / TICK-GHOST / FIELD / DEADLOCK / panics | 0 each | — | 0 each |
| RUNQ-STRAND rows / 540s | 197 | 401 | 0 |

Deviation reported honestly: the mission hoped "stranded-rows still
0" (the rs5 outcome). hg1's wedge face still strands (197 rows) — but
through the runq-strand conviction's arm (ii) DIRECTLY, not the
heal-product arm (i): every monopolist is a COUPLED spinner
(pc=0x1/0x3) at lock+0x52 waiting out the multi-second holder chains
of the frozen image-side binder stall (TXN-HANG-BWR standing at
539s), and spinlocks never sleep by design. rs5 showed 0 strands
because its face was mild (45 FUX lines, chains resolved at 28-46s);
hg1's face has the chains standing at window end. The heal-product
cascade that rs2's strand class actually depended on — healed
mid-spin waiter spinning at pc=0 forever — is structurally gone
(zero pc=0 monopolist rows, zero underflows). The trade the gate
makes is explicit: a coupled slow spinner keeps its CPU until the
holder releases (bounded by the image-side stall), instead of being
"fixed" by zeroing a live count (phantom manufacture + the rs2
monopolist era). Protection, not cosmetics.

### 4.4 Residual class, named honestly

The 64K-boundary stamp can only vouch for waiters INSIDE
RawSpinlock::lock's contended loop. Covered and proven (1377
refusals, all ev_age=1j — the refresh runs like clockwork and the
10j margin was never even needed). What remains healable is the
0x1000x SOFTIRQ/HARDIRQ-carrier class: tasks carrying softirq + 1-3
guard levels (or a single stranded hardirq level) while waiting or
working somewhere OTHER than a contended raw acquire. hg2w's
PREEMPT-LEAK rows (542; 433 idle-task residue, 109 on real tasks)
name their sites by rip:

- userspace addresses (0x3fbf... — 40+ rows): counts that returned
  to user mode with the task (residue words; drains before any bar
  matures on clean boots);
- kernel memset/memcpy/uart-puts_batch/from_utf8: multi-hundred-ms
  large-memory/console work executed under guards (long sections,
  not waits);
- lock+0x52 itself (3 rows): the covered class, refused when it
  matures.

On the wedge face, 31 of these carriers matured (2s+ continuous
presence with stale raw-spin evidence) and were healed — by the
contract they were NOT actively spinning in a raw acquire at
adjudication time (e.g. pid 519: refused at 83.518 while spinning
ev_age=1j, healed at 83.747 only after leaving the contended loop
with softirq+3 levels still held elsewhere). Covering THAT class
needs either fast-path stamping (forbidden: fast path must stay
unchanged) or a new instrument at the carriers' wait sites — the
natural follow-up candidate, in the same way this lane was the
runq-strand lane's follow-up.

## 5. Spin-path domain check

The stamp executes once per 65536 contended iterations, after the
mask test that already existed for the 121st preempt exit; the
per-iteration spin path (TTAS load, inc, mask test, deadlock cmp,
pause) is instruction-identical to d0fb0005a; the uncontended path is
byte-identical (section 2). GRQ hold times and wake→pick deltas have
no changed code path; the mission's spin-path domain check is the
disassembly above.
