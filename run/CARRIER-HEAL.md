# CARRIER-HEAL — softirq-window carrier evidence for PREEMPT-HEAL

Lane: carrier-heal (123rd session), worktree /home/william/rux-agents/carrier-heal,
branch `carrier-heal`. Base: main @ a103863f7 (heal-gate is in).
Kernel commit: 9d747c6cb — `Gate PREEMPT-HEAL on softirq-window
carrier evidence (CARRIER-HEAL)` (`kernel/src/interrupt/softirq.rs`,
`kernel/src/sched/sched.rs`, `kernel/src/arch/riscv64/trap.rs`).

Mission (heal-gate's named follow-up, HEAL-GATE.md §4.4): HEAL-GATE's
acquire-side stamp can only vouch for waiters INSIDE RawSpinlock::lock's
contended TTAS loop. Its residual healable class on the hg1 wedge face
was 31 heals on 0x1000x carrier counts whose acquire evidence had gone
STALE — the holder left the contended loop and carried its levels
through work somewhere else (hg2w's LEAK rips: kernel memset/memcpy/
uart-puts_batch/from_utf8, mm free/unmap paths, userspace residue
words). The fast-path constraint forbids stamping guard acquire/release
— so the evidence must come from the carrier's OWN boundaries.

## 0. What the 31 actually were (re-derived from hg1/hg2w evidence)

Before designing, the class was re-read from the heal-gate artifacts
(`run/oh-hg1-serial.log`, `run/oh-hg2w-serial.log`, both committed by
the heal-gate lane; symbolized against heal-gate's `run/syms/rux-fix.elf`):

- All 31 hg1 heals printed `was=0x10001/0x10003`. The heal runs inside
  the timer IRQ bracket, so the printed count carries the tick's own
  in-flight HARDIRQ_OFFSET: the incumbents' OWN counts were 1 or 3 —
  pure base PREEMPT_DISABLE levels, no SOFTIRQ bit, no own hardirq bit.
- The wedge-face carriers' tick-time rips (hg2w census + the hg1
  MONOPOLIST rows): `RawSpinlock::lock+0x52` (covered by HEAL-GATE),
  `Zone::free_pages`, `MmStruct::unmap_pages`, `lru_del_page`,
  `icache_add`, `memset`, `memcpy`, `Uart::puts_batch`, `from_utf8`,
  and userspace addresses (0x3fbf.../0x2aaa...) — task-context guarded
  work (the fd-reaper's mm teardown) or residue words returned to user
  mode. NOT, at adjudication time, holders inside an open
  `__do_softirq` window (that would read 0x101xx at tick).
- hg1 had ZERO PREEMPT-UNDERFLOW after those 31 heals: none of the
  healed counts' guard releases ever arrived, i.e. none was provably
  cut mid-section.

Consequence, stated honestly: a softirq-boundary instrument alone does
NOT zero those 31 — they were base-count carriers outside any softirq
window. What it DOES do is close the one structurally identifiable
false-heal family left in the heal's reach — holders inside a softirq
window — which is not hypothetical: the make-test sgid_mode VW-WEDGE
conviction (a heal landing mid-window makes the executor preemptible,
the handler never finishes, 29 completions lost, both CPUs idle
forever) is exactly this class, and HEAL-GATE's acquire gate cannot see
it (a softirq handler is not spinning in a raw acquire). This lane
builds that gate, and adds fire-time naming evidence so every residual
heal (the task-context carrier class) is NAMED per-row instead of
anonymous — the mission's "(or each residual named)" branch.

## 1. The design

Three pieces, one contract. None touches any fast path.

### 1.1 Carrier-boundary evidence (kernel/src/interrupt/softirq.rs)

`__do_softirq`'s own boundaries publish the HEAL-GATE evidence shape on
a per-CPU 16-byte slot:

```
CARRIER_EVIDENCE[cpu] = { task: cpu_state(cpu).current, jiffies: now }
```

- Written at window ENTRY (after the SOFTIRQ_OFFSET booking, so the
  slot never vouches for a window whose count does not yet carry the
  bit), at EVERY VECTOR DISPATCH (the heartbeat — one store pair per
  dispatch, the affordable cadence the mission names; keeps the stamp
  fresh while handlers keep running), and at window EXIT (after the
  SOFTIRQ_OFFSET sub: `task` cleared to 0, `jiffies` records when the
  window closed). Cost: two plain relaxed stores per boundary; nothing
  on any lock, no RMW, no atomic op.
- `CarrierEvidence` = 16 bytes (AtomicUsize + AtomicU64), one slot per
  CPU, static BSS — the exact ACQUIRE_EVIDENCE pattern (MAX_CPUS×16 =
  128 bytes of BSS).
- Task identity comes from the scheduler's published
  `cpu_state(cpu).current` — the same word the tick-side heal
  adjudicates against (HEAL-GATE's symmetry lesson: writer and reader
  name the same source, so no tp-register-vs-slot skew is possible).
- Concurrency (all Relaxed): the only writers are the CPU's own
  `__do_softirq` boundaries; the only reader is the tick IRQ on that
  SAME CPU. The tick interrupts the window, so writer and reader never
  run concurrently; per-word coherence suffices (same shape as
  ACQUIRE_EVIDENCE / TICK_ENTRY_PC).

### 1.2 The gate (kernel/src/sched/sched.rs, scheduler_tick)

Both heal legs (PREEMPT-HEAL base / PREEMPT-HEAL-FIELD) are now
else-chained behind one refusal that ORs three signals:

```
acq_live = acquire_evidence_fresh(cpu, cur, now)          [HEAL-GATE]
car_win  = SOFTIRQ_IN_PROGRESS[cpu] != 0                  [exact]
car_live = carrier_evidence_fresh(cpu, cur, now)          [new]
```

- `car_win` is the exact window-open truth. SOFTIRQ_IN_PROGRESS is
  written only by this CPU's own `__do_softirq` bracket, and the heal
  runs in the tick IRQ on the same CPU — the pair can never execute
  concurrently (interrupt nesting), so a set flag at adjudication time
  proves the tick interrupted a live window whose executor IS `cur`.
  This leg covers windows of ANY length, including a single handler
  running longer than any freshness bound (the known gap of
  stamp-based evidence alone).
- `car_live` is the {task, jiffies} slot with
  `CARRIER_EVIDENCE_FRESH_JIF = 10` (100ms at 100Hz — HEAL-GATE's
  margin doctrine: refresh cadence is per-dispatch, and the bound sits
  20x UNDER the heal's own 200-tick bar). It covers the closed-window
  sliver and gives the refusal an auditable age.
- Fresh evidence ⇒ REFUSE both legs: rate-gated (20j, PREEMPT-LEAK
  policy) `PREEMPT-HEAL-REFUSED cpu= pid= pc= ev_age= car_age= face=
  via=A|C` row, NO window reset, NO run-counter reset — the count
  stays armed and the next tick re-adjudicates once the window closes
  and the stamp goes stale. A leaked/stranded count is delayed by at
  most the freshness window past the last genuine boundary; a live
  window is never healed mid-flight.
- PARKED-HEAL stays ungated (a task current on NO CPU executes no
  window; no carrier evidence can exist for it).

Contract: **a holder inside (or within 10j of) a softirq window is
never healed.** Everything else is adjudicated by the unchanged
200-observation bar.

### 1.3 Residual naming at fire time (both arches)

Every PREEMPT-HEAL row now carries the tick-entry snapshots that were
already being refreshed per tick but never printed on this family:

```
pctick=  incumbent's OWN count at tick ENTRY (before the observer's
         HARDIRQ_OFFSET) — splits the printed was= into task-owned
         bits vs observer offset
rip=     the interrupted instruction (symbolize with addr2line; a
         0x3fbf.../0x2aaa... address = user-returned residue)
cs=      the interrupted code segment: x86_64 prints the RAW CS
         selector (8 = kernel ring 0, 0x33 = user ring 3; the ring
         bits cs&3 discriminate); riscv64 prints a synthesized ring
         number (3 = user, 0 = kernel)
```

The riscv64 timer path additionally stamps TICK_ENTRY_CS now (the
x86_64 twin has stored the raw selector since the runq-strand lane):
on riscv64 `3` when sstatus says the tick hit user mode, else `0`.
One relaxed store per tick.

## 2. Fast-path proof (disassembly, clean-config release ELFs)

Baseline = a103863f7 release ELF (`run/syms/rux-baseline.elf`), fix =
this branch release ELF (`run/syms/rux-fix.elf`), both built from
cleared `.config` (default config) — sizes 11674568 vs 11674648 bytes
(+80: the three stamp sites + adjudication code; the earlier
dev-config release was 15.6MB and is NOT comparable).

- `RawSpinlock::lock` (0xffffffff8027fc30, size 0x212 in BOTH): the
  whole function diff is **6 lines, all relocation displacements** —
  one RIP-relative JIFFIES load target (BSS moved +0x80 = exactly the
  128-byte CARRIER_EVIDENCE array) and two call targets (schedule,
  panic_bounds_check). The uncontended fast path (prologue, ra capture,
  `xor r14d`, `lock cmpxchg [rdi],ecx`, `jne` to the contended arm) is
  byte-identical; the normal spin iteration (TTAS load, inc, mask test,
  deadlock cmp, pause) is byte-identical; spinlock.rs was not touched
  at all.
- `__do_softirq` (fix at 0xffffffff80597d40, size 0xdc5 vs baseline
  0xd15, +0xb0): the three stamps appear exactly as designed and
  nowhere else — entry arm at 0xffffffff80597f32..f5c and dispatch arm
  at 0xffffffff80598388..83ba (each: `gs:` cpu_id load → bounds cmp →
  cpu_state(cpu).current load → `test/je` null check → `shl $0x4`
  (16-byte slot) → store task → JIFFIES load → store jiffies = 2
  stores), exit arm at 0xffffffff805986dd..8702 (`task=0` store +
  JIFFIES store) placed after the SOFTIRQ_OFFSET sub, before the
  SOFTIRQ_WIN_T0 clear. The baseline contains zero references to the
  slot address. The pending-snapshot loop, budget checks and handler
  dispatch calls are otherwise unchanged.
- `scheduler_tick` (0xffffffff804a5600, +0x310 bytes): the adjudication
  reads ACQUIRE_EVIDENCE (slot → cmp cur → `cmp $0xb; setb` = age ≤ 10
  → acq_live), then `SOFTIRQ_IN_PROGRESS[cpu]` (test; if zero, the
  CARRIER_EVIDENCE slot → cmp cur → `cmp $0xb; setb` = car_live), then
  the refusal branch `or` of the two live bits ahead of the unchanged
  `test $0xfff0ffff` base-mask adjudication (0xffffffff804a80a6..80be).
  Tick-side only; executes once per tick per incumbent with a matured
  window.

## 3. Gates

- x86_64: `PLATFORM=x86_64 make build` rc=0; clean-config
  `make build-x86-release` rc=0 (`run/ch-gate-x86-cleanconfig.log`).
- riscv64 build rc=0, config-clear (`run/ch-gate-riscv-build.log`);
  aarch64 build rc=0, config-clear (`run/ch-gate-aarch64-build.log`).
- riscv64 `make test`: **ALL TESTS PASSED — Passed: 1329, Failed: 0**
  (`run/ch-gate-riscv-maketest.log`, rootfs rc=0,
  `run/ch-gate-rootfs.log`). The boot kernel for the rig below is the
  dev-config release (16171008 bytes — byte-size-identical to
  heal-gate's boot kernel, same config lineage, saved as
  `run/bzImage-carrierheal`).

## 4. hg-class verification (OH wedge-repro base, KVM q35 SMP8 8G)

Rig: `run/ch-boot.sh` + `run/lb-boot-qmp.py` — heal-gate's hg-boot rig
(private 16G full copy of the runq-strand userdata-rsbase.img per boot,
deleted after; shared images via -snapshot + file.locking=off), with
`dfx=txnhunt,fuxstuck,periodic,parkdump,schedtrace,wwcensus` (the hg2w
face — wwcensus adds the PREEMPT-LEAK census; heal adjudication is
identical with or without it).

Three draws (ch1, ch2, ch3 — serials `run/oh-ch{1,2,3}-serial.log`):

- **ch1 — image-side deliberate-reboot face.** OH init's reboot plugin
  path fired at 49.3s (fstab/misc failures → init killed all services
  → `PANIC: sysrq 'c' — deliberate crash`, kernel/src/fs/procfs/
  sysrq.rs:34); the machine sat in the panic halt to window end. Before
  the crash: 0 heals, 0 refusals, 0 strands, 0 monopolists — the gate
  never adjudicated anything, so this lane's behavioral delta on this
  draw was exactly two relaxed stores per softirq boundary. The
  post-halt census is itself one clean demonstration: init sat in
  `panic_policy::apply` holding a PURE 0x10000 count for 49111 jiffies
  (491s, `held_nonzero_for` on 1549 census rows) and NO heal ever fired
  — a softirq-only count (base 0, no hardirq field) is outside both
  heal legs' reach by design; the carrier gate protects the
  base+window shapes, it does not need to protect this one.
- **ch2 — image-side memory-pressure face.** Kernel-heap exhaustion at
  112.9s (`ALLOCTHROW size=69632`, heap size=999424K free=168K) — the
  known fake-OOM class (roadmap Current Goals). 0 heals / 0 refusals /
  0 strands before it. Same non-involvement argument as ch1.
- **ch3 — the WEDGE face, full 540s window, the evidence draw.**

### 4.1 ch3 battery (wedge face) vs hg1 (heal-gate's wedge face)

| family | ch3 (carrier-heal) | hg1 (heal-gate) |
|---|---|---|
| panics | 0 | 0 |
| PREEMPT-HEAL rows | **40, every one named** (below) | 31 (anonymous was= only) |
| heals inside a live softirq window | 0 (via=C refusals 0; no window-coupled candidate ever adjudicated) | — |
| PREEMPT-HEAL-REFUSED rows | 2069 clean + 1 serial-interleaved (all via=A, ALL ev_age=1j, ALL car_age=1j, 29 distinct pids) | 1377 (all ev_age=1j, 20 pids) |
| PREEMPT-HEAL-FIELD / -UNDERFLOW / WAKE-HEAL / PARKED-HEAL / PARKED-LEAK / TICK-GHOST / DEADLOCK | 0 each | 0 each |
| MONOPOLIST rows | 84, ALL coupled (pc=0x3 x66, 0x1 x13, 0x103 x3, 0x101 x2), ALL rip=lock+0x52 — ZERO pc=0x0 | 54, ALL coupled — ZERO pc=0x0 |
| RUNQ-STRAND rows / 540s | 325 | 197 |
| FUX-STUCK lines | 822 | 807 |
| TXN-HANG-BWR rows | 929 | 1468 |
| FUX stuck_total at window end | 115 standing | 94 standing |
| alive to | 540.2s | 539.6s |

Wedge anatomy UNCHANGED, heavier draw: same arms as hg1 — the
image-side binder stall chains (TXN-HANG-BWR standing at window end,
FUX stuck_total standing), strand rows through the runq-strand
conviction's coupled-spinner arm (ii), monopolists all coupled spinners
at lock+0x52 waiting out multi-second holder chains (act up to 36.6s).
The heal-product monopolist class (pc=0) stays structurally gone.

### 4.2 The 40 residuals, each named (the mission's second branch)

Every heal row now carries pctick/rip/cs; against the boot kernel ELF
(`target/x86_64-unknown-none/release/rux`, dev-config release):

- pctick: 19 rows `1`, 21 rows `3` — the incumbents' OWN counts were
  pure base PREEMPT_DISABLE levels at tick entry. Empirical
  confirmation of §0's re-derivation: the 0x10000 in the printed
  was=0x10001/0x10003 is the observer tick's in-flight HARDIRQ_OFFSET,
  NOT a live softirq-window bit. The healed class never carried an
  open-window coupling — which is also why via=C never fired: on this
  face there was nothing for the carrier leg to refuse.
- rip: 39 rows `lock+0x52` (the TTAS load), 1 row `lock+0x127` — all
  kernel carriers, none a userspace residue address this draw.
- cs: 40 rows `8` (kernel CS selector) — zero heals landed on a
  user-returned count.
- 27 distinct pids. Worked example (pid 0x281, cpu4): refused at
  116.796 / 121.196 / 121.796 ... at ev_age=1j while actively
  spinning (via=A), census-tracked at rip=lock+0x52 pctick=3
  throughout, healed at 126.138 after its acquire stamp staled —
  hg1's pid-519 shape, now with every observation named.

The carrier instrument is demonstrably live in the same rows: every
refusal prints car_age=1j — the per-boundary stamps keep the CPU's
slot fresh on every tick that processes softirqs. The gate leg exists
in the adjudication (disassembly §2) and fires on window-coupled
candidates; this face produced none, matching the §0 prediction that
hg1's 31 were task-context carriers outside any window.

### 4.3 Heal-adjacent families and census

PREEMPT-HEAL-FIELD, PREEMPT-UNDERFLOW, WAKE-HEAL, PARKED-HEAL,
PARKED-LEAK, TICK-GHOST, DEADLOCK: 0 each across all three draws —
the heal-adjacent families stay closed. wwcensus PREEMPT-LEAK rows
(informational): ch3 2346 (top: pid 1 x283, pid 0x14 x257, idle x248);
ch1 1737 (1549 of them the post-halt pid=1 window of §ch1); ch2 463
before the OOM. The ch3 census site classes (symbolized) are the same
families hg2w named: cpu_idle_loop residue, memset/memcpy/from_utf8
large-section work, lock+0x52 waits.

## 5. Residual classes, named honestly

- The task-context base-count carriers (the hg1 31-shape: fd-reaper mm
  teardown under guards, console/memset sections, user-returned residue
  words) remain adjudicated by the unchanged 200-tick bar. Covering
  them needs evidence at the guards' own boundaries — the fast path —
  or a different instrument class (per-task section counters), out of
  this lane's fast-path constraint. Their heals are now NAMED per-row
  (pctick/rip/cs) instead of anonymous: ch3's 40 are all kernel
  carriers at lock+0x52 with own-counts 1/3 (§4.2).
- Under host-vCPU starvation (this rig runs 8 vCPUs on a busy KVM
  host), a spinner's 64K-boundary stamp can go stale while the
  incumbent is still POSITIONED in the contended loop — the vCPU
  advances guest jiffies in tick bursts between host deschedules, so
  no boundary is crossed for >10j and the next adjudication heals a
  count whose holder never left the loop. ch3's 39/40 heals carry
  exactly that signature (rip=lock+0x52 at tick entry with a stale
  stamp); the serial alone cannot separate it from "left the loop and
  re-entered within the first 64K spins". The refusal leg protects
  everything it can observe (2069 refusals, all ev_age=1j); the
  stale-stamp ambiguity between "left the loop" and "vCPU starved" is
  inherent to stamp-based evidence on an overcommitted host and is
  named here rather than hidden.
- A single softirq handler running longer than 10j with no further
  dispatch in between loses slot-freshness mid-handler; that gap is
  covered by the exact window-open flag (`car_win`) for as long as the
  window is open on the adjudicating CPU. A PURE softirq-only count
  (base 0, no hardirq field — ch1's post-halt init window, 491s) is
  outside both heal legs' reach entirely; no gate needed.
- NMI-context carriers (NMI_OFFSET) remain ungated (no NMI source is
  configured on these boards; named for completeness).
