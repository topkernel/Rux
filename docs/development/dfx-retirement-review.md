# DFX Retirement Review (micro-core-design §7 — the census-math input)

Date: 2026-11-22. Reviewer: the fixverify reverification lane.
EXECUTED 2026-11-23 by the dfx-retire lane (branch dfx-retire, on top
of the s1-closure merge 02bfede71): Phase 1 below landed as two commits
(r83hunt, treehunt — see "Execution record"); the dfx→debug rename of
§8 is DEFERRED with its precondition re-stated (it edits the six
service crates' `crate::dfx::` path tokens, which the retirement lane
does not own).

Method: git archaeology (intro commit → convictions → fixes) crossed with
face evidence from the four most recent full LTP scans on record
(prefix@344ef0efb 957, post-RTS-DEFER@a09e7ab08 984, P4-rescan@10adeaf69
955 — all in `run/` of the fixverify worktree — plus the r27 census).

Prior adjudication on record: fa94cb7a6 (the ur-L4c dfx sweep) verified
all five hunt modules ALIVE and registered them as the F1-F4/F10 floor.
That verdict predates the two-bug corruption fixes (fc5f23d47 +
2e9755c67, merged a557edd1b) and the RTS-DEFER chain — the class
closure question is re-opened here with post-fix scan evidence.

## Verdict table

| Instrument | Lines | Call sites | Target class | Class state on main | Verdict |
|---|---|---|---|---|---|
| r83hunt | 684 | 8 | R74/R83 task-header foreign-data smear | CLOSED (fix c1539f6ed + the corruption-fix family) | **RETIRE** |
| treehunt | 231 | 1 | E8-REPAR unlinked parent snapshots | CLOSED (fix 7d4481320 family) | **RETIRE** |
| scribble | 1657 | 49 | Task-field scribbles in fork/exec storms | ALIVE (residue race, see §1) | **KEEP** (flag `rux:scribble`) |
| wildwatch | 479 | 5 | Wild-write producer attribution (b77/wt1/b81 engines) | PARTLY ALIVE (b81 engine live, §4) | **KEEP** (flag `rux:wildwatch`) |
| leakorigin | 1201 | 26 | Preempt/hardirq-offset leak acquire sites | MOSTLY CLOSED, one live face (§2) | **KEEP** (flag `rux:leakhunt`) |
| forensics_edges | 121 | shared | Raw stale-handle edges for the hunt family | 4 live consumers incl. standing instruments | **KEEP** |

Net: −915 lines now (r83hunt 684 + treehunt 231, plus their call-site
stubs); a further −3,337 (scribble+leakorigin+wildwatch) unlocks when
the residue race family closes (criteria per section below).

## 1. scribble — KEEP (the class has live residue)

Target: byte-level scribbles of scheduling-critical Task fields
(`thread.{fs_base, gs_base, sp, callee.ret_addr}`, parked pt_regs
control words) during fork/exec storms. Introduced 64cb44ef9 ("canary
detectors caught live corruption"), hardened through R49/R63/R64
(b80fe87e7), wild-GS entry class closed eabd21d4d.

Archaeology of the producers it hunted: the signal double-put
(fc5f23d47) and placement-new lost-anchor (2e9755c67) — both convicted
and fixed, choke-pointed at c8285ab6d; the sys_wait4 NULL-ring
(RTS-DEFER, 8b34d01d6) and residue round 1 (4a624c354, cross-task
dead_threads contamination).

Why not retire: the CLASS is not closed on current main. Post-fix scan
evidence, all from this review's campaign:
- connect01 panics ~1 boot in 3 on the fixed debug kernel: task-tree
  link corruption (kp-ring `sb.nx` into kernel-stack memory), execution
  jumping into `.sbss` past the GRQ statics (rip 0xffffffff81601550).
- The P4 rescan's three boot-poisoning clusters (fork-failed band
  504..599, WEDGE block 1360..1399, EXECFAIL block 1639..1699+) —
  boot-cumulative fork/exec failure after a kill/keyctl-region trigger,
  every trigger passing in isolation.
- 4 `PERCPU-SCRIBBLE-HEALED` faces across the a09e7ab08 and P4 scans
  (sched.rs:4045 — the always-on heal for the b66 resume-family
  scribbling per-CPU idle/current slots; the heal repairs, only the
  hunter can attribute).

Retirement criterion: two consecutive full 1869 scans with zero
boot-poisoning clusters AND zero PERCPU-SCRIBBLE-HEALED faces AND the
connect01 race at 0/10 repro boots — then retire with the closing fix
commit as proof.

## 2. leakorigin — KEEP (primary producers closed, one live face)

Target: the acquire site of a leaked PREEMPT_OFFSET / stranded
HARDIRQ_OFFSET (PARKED-LEAK pc=1/2, WAKE-STRANDED incumbents).
Introduced e17fb73bc (v1), 584c11059 (v3 parkwalk adjudication), the
s14 canary round 766d60a93.

Producers closed on main: cc009d107 (deferred-exit notify never runs
with IRQs disabled — the GRQ-wedge holder's wake chain), c4976fef1
(the DEAD->RUNNING nanosleep-park rescue flip; the SCHEDULE-ATOMIC
producer's shape closed structurally by bbd37e464's reference-parity
refusal — schedule() BUGs in interrupt context instead of leaking),
plus c885e423a (the instrument's own print path on release builds).

Why not retire: one live face — `PARKED-WILDWRITE pid=0x150 pc=0x1`
in the P4 rescan's wedged chunk-1300 boot (pc=1 is exactly the leaked
preempt-disable bit signature), and the P4 scan's WEDGE block shows
boots still stalling in that family's territory. Single face, but in
the boot the census lost 40 rows on.

Retirement criterion: the pc=1 parked face absent from every boot of
two consecutive full scans, with at least one scan run WITH
`rux:leakhunt` armed and silent.

## 3. r83hunt — RETIRE (engine convicted and fixed)

Target: the R74-SLOTLESS-TP / PREEMPT-UNDERFLOW family — thread_info
headers overwritten by foreign heap-payload-looking data
(decimal-printed ksp halves, preempt_count==-1, garbage written once
and frozen). Introduced 9c2981486 (R83 task-header audit), extended
9e8445683 (R83b ext4 waiter self-watch).

Closure archaeology: c1539f6ed — "R83 stash user sp only after SPP=0 —
stop the thread_info smear engine" — the trap.S producer was convicted
and fixed outright. The family's other face (Task freed while still
referenced → header recycling) is the corruption family closed by
fc5f23d47 + 2e9755c67 + the INV-PROC-PLACE-2 choke point (c8285ab6d).

Face evidence: zero R74-SLOTLESS-TP / PREEMPT-UNDERFLOW / frozen-garbage
ksp faces in any of the four recent scans (the module is flag-gated
`rux:r83scan`, but the CLASS faces it was built to pre-empt are
always-visible when they occur, and are gone).

Action: delete `kernel/src/dfx/r83hunt.rs` (684 lines), its 8 call-site
stubs, and its `forensics_edges` consumer registration; keep the fix
references above in the safety ledger.

## 4. wildwatch — KEEP (the b81 engine is live)

Target: attribution tripwire for the residual wild-write engines —
b77 (hpet::BASE single-byte smash), wt1 (BASE+BASE_LOCKED wide smash),
b81 (PER_CPU slots). Introduced 42eb2006b. Read-side immunity for the
timekeeping victims landed separately (9d940990a, the vdso-clock
wrap-guard family).

Why not retire: the b81 engine specifically is STILL WRITING. The 4
PERCPU-SCRIBBLE-HEALED faces in the recent scans are the b81 signature
(per-CPU idle/current slots scribbled then healed); the heal cannot
name the writer — wildwatch's RO-page guard cluster around the
PER_CPU statics is exactly the attribution tool for it, at zero cost
when the flag is off. The hpet engines (b77/wt1) show no faces in any
recent scan and their read-side victims are immune; their clusters can
be trimmed in the same pass if the maintainers prefer.

Retirement criterion: PERCPU-SCRIBBLE-HEALED absent across two
consecutive full scans (the b81 engine dead), plus one armed
`rux:wildwatch` boot with zero guard hits.

## 5. treehunt — RETIRE (family closed by the exit-path fixes)

Target: the E8-REPAR "unlinked snapshot" family — thread_group_join
copying the leader's parent into every member as an unlinked snapshot
that goes stale on reparent, release_task then walking freed parents.
Introduced 7105434c9 (with txnhunt).

Closure archaeology: 7d4481320 / 6f5e397ad — "reparent never selects a
PF_EXITING destination; member snapshots refresh; release-time heal" —
the suspect mechanism was convicted and fixed in the reference shape.
Hardened by 7605171c4 (typed tree/thread-ring walkers) and fe70d819f
(zombies claimed in do_waitid/do_wait_nonblock before release). The
defensive range check that first stopped the fault: 740ca427.

Face evidence: zero E8-REPAR / stale-ancestor faces in any recent scan.

Action: delete `kernel/src/dfx/treehunt.rs` (231 lines) and its single
call-site stub.

## 6. forensics_edges — KEEP (shared plumbing with standing consumers)

`forensics_edges.rs` (121 lines, the L5 consolidation 8fd1b97d2) is the
shared raw stale-handle read layer (adopt_task / t_pid / t_comm /
t_preempt_count / raw_qword / raw_u32 / location) for the whole hunt
family. Consumers on main: interrupt/preempt.rs, dfx/syscost.rs,
dfx/softlockup.rs, dfx/leakorigin.rs, dfx/r83hunt.rs. Retiring r83hunt
removes one consumer; syscost/softlockup/preempt are standing
instruments — the file stays regardless of any hunt retirements, and
survives with the KEEP trio. The "stale-handle trio" census reduction
therefore comes entirely from §3 and §5 (915 lines), not from
forensics_edges.

## 7. Census math

| Action | Lines removed |
|---|---|
| Retire r83hunt (684) + call-site stubs | ~690 |
| Retire treehunt (231) + call-site stub | ~233 |
| **Phase 1 total** | **~920** |
| scribble (1657) + leakorigin (1201) + wildwatch (479), when their criteria land | ~3,337 more |

Phase 1 lands inside the −600..1,000 band the plan called for; the
KEEP trio's retirement is evidence-gated on the live race family, not
on preference.

## 7a. Execution record (2026-11-23, dfx-retire lane)

Phase 1 landed as two commits on branch `dfx-retire` (rebased on
02bfede71, the s1-closure merge):

- **r83hunt retirement**: −784/+89 across 10 files (net −695). The +89
  is deliberate, not overhead padding: the six R85 ti_kernel_sp
  containment GATES in riscv64 trap.S stay as defensive depth (they
  repair a wild sp to the task's own stack top before PtRegs is
  pushed), so their report target `r84_ksp_trap` moved from the retired
  module into `arch/riscv64/trap.rs` in compact form — same symbol,
  same discipline (raw UART, one report per site per boot). Also
  removed: the freed-header paint loop AND its trailing duplicate
  poison re-write in `free_task_slot` (the R47/R12-4 state/pid poison
  the stale-wake guards actually read is untouched); the R83b waiter
  self-watch snapshot block in SleepMutex's slowpath; the
  gate-only `rux:r83scan` trace event row.
- **treehunt retirement**: −278/+28 across 8 files (net −250). Also
  removed: the `parent_children_head_ptr` diagnostic accessor (the
  probe was its only consumer; the `parent_children_head` field is
  live list machinery); the `rux:treehunt` trace event row;
  `scripts/test/subreap_boot.py` no longer arms the retired token
  (kept as the churn regression rig, TREE-* regex stays as a
  regression tripwire).

Net: **−915 dfx/ lines** (git-tracked: 19,123 → 18,208, exactly the
two modules; the wider −945/−1,062 diffstat numbers include the
call-site stub removals in sched/sync/timer/trace and the +89/+28
replacement comments and trap.rs hook); dfx/ tree 39 → 37 files; trace
event table 109 → 107 rows; dfx= switch table unchanged at 6 permanent
switches (monotonicity preserved — nothing added).

Re-verification during execution (beyond the review's face-evidence
method): a fresh all-tree reference census INCLUDING the path-included
service crates (`kernel/rux-*/src`, which a `kernel/src/`-only grep
misses) confirmed every other dfx module has live call sites — scmtap
in particular (20 emit sites across rux-fs/fifo and
rux-syscall/network) is ALIVE as an EVENT_SCMTAP echo consumer, not
dead; an earlier `kernel/src/`-only census had misread it as orphaned.

## 7b. End state after Phase 1 (the documented KEEP-LEGACY rump)

The §7 end state chose the rump over dissolution: `kernel/src/dfx/`
stays (rename deferred, below) holding —

- **6 permanent dfx= switches** (claiminject, oncpuinject,
  scribblepark, scribblepid1, scribblesweep, wildwatch): fault
  injectors, gdb-park actions, and the PTE-mutating detector —
  behavior changers with no tracepoint form (switches.rs documents
  each). Exit conditions: the injectors retire when their adjudication
  hunts (GNOME wedge / R85 tail-phase) close; wildwatch's is §4's
  criterion below.
- **The KEEP trio + plumbing** (scribble, leakorigin, wildwatch,
  forensics_edges) with the evidence-gated exit criteria in §§1/2/4.
- **The migrated echo-consumer instruments** (allocwatch, driftwatch,
  filerd, lostpark, monowatch, metawatch, slowtask, reaper, schb,
  scmtap, ...): gates are `rux:` trace events since dfx-migrate
  rounds 4/4b/5; the dfx= tokens are gone (no aliases, per the
  2026-11-17 user directive). They retire family-by-family when their
  hunts close, the same way r83hunt/treehunt just did.
- **Standing debug infrastructure, not hunt instruments at all**:
  taint/bug/backtrace/ksyms/softlockup/hung_task/oops/pstore/taskdump/
  syscost/fsw/cctx/unixrd/forkprof/mmprof/hilogcensus/spinwatch/
  mmforensics/ledger/pathstash/memwatch — perf-gate tooling (syscost
  is REQUIRED by the merge gate), LTP-facing debug surfaces, and
  pstore/oops which are boot-critical. Cmdline-gated (syscost=1 style)
  or always-available; no dfx= switch involved.

The rename (§8) is the only remaining mechanical step and is blocked
on lane ownership, not on evidence: `crate::dfx::` appears in
kernel/rux-fs, rux-syscall, rux-net, rux-mm, rux-drivers,
rux-sched-policy (as of the s1-closure base) — six crates the
dfx-retire lane does not own. It needs one coordinated mechanical
commit when the crate lanes are quiet; the recipe in §8 is unchanged
and remains valid.

## 8. MERGE PLAN — dfx/ → debug/ (DEFERRED: needs crate-lane coordination)

The 11-06 dfx→debug rename never reached the module directory:
`kernel/src/dfx/` still holds the module while it is declared
`pub mod dfx` (lib.rs). As of the Phase-1 execution (2026-11-23, on
the s1-closure base) that is 37 git-tracked files / 18,208 lines
(plus the build-generated, gitignored ksyms_generated.rs). S1 has
closed, but a NEW blocker replaces the sequencing one: the
`crate::dfx::` path token now lives inside the six path-included
service crates (rux-fs, rux-syscall, rux-net, rux-mm, rux-drivers,
rux-sched) — the rename is a one-commit mechanical rewrite across
trees the retirement lane does not own, so it waits for a quiet
window across the crate lanes. Steps, one mechanical commit:

1. ~~Execute Phase 1 retirements above (r83hunt, treehunt)~~ DONE
   2026-11-23 (§7a).
2. `git mv kernel/src/dfx/*.rs kernel/src/debug/` (the 37 surviving
   files, ksyms_generated.rs included; drop the old directory).
3. lib.rs: `pub mod dfx;` → `pub mod debug;`; same for any `#[path]`
   references and build.rs/Makefile paths that name `src/dfx`
   (ksyms.bin embed, any doc tooling).
4. Tree-wide mechanical path rewrite: `crate::dfx::` → `crate::debug::`
   (plus `dfx::` in the few `use dfx::` forms), INCLUDING the service
   crates' sources. No signature changes, no logic changes — the diff
   must be path-tokens only.
5. Gate: dual-arch build rc=0, riscv suite green, one quiet syscost
   boot with census in band (Tier 1) — a pure rename must be
   performance-flat, and the gate proves it.
6. Bookkeeping in the same change: docs/architecture/ unsafe-ledger
   row re-banks for the renamed file paths (same-region same-class
   ordinal match, per the ca1005987 precedent), and the design-divergence
   register if any row cites a dfx path.
