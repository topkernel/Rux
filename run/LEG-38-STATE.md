# LEG-38 state (perf-audit lane, 2026-10-26)

Worktree: /home/william/rux-agents/perf-audit, branch feature/perf-audit.
Head at time of writing: 237ec423 (all leg work COMMITTED).

## Leg tasks -> status

1. **Census gate on current main-class (b4da2106) — ARMED, WAITING
   WINDOW.** Main b4da2106 merged clean (920c7b3e; COW-under-PTE fix
   2968ad13 = mm guarded domain, net batch = socket domain,
   small_alloc 7a6e1bf4 = mm domain, pidns/kcmp/clone-ABI). Census
   kernel: PURE main b4da2106 x86_64 debug in pa52-main
   (BUILD_RC=0). Decider run/pa60-census.sh (committed): waits for
   the pa57 chain (done 08:27), then two-stage gate (9/8 until
   ~08:43, then 13/10; 6 h deadline = 12:42), census boot pa60main
   780 s syscost=1, then perf-gate parse (run/pa60-gate-parse.txt)
   + slowtxn parse (run/pa60-slowtxn-parse.txt), then the adjacent
   pbench pair M,B,B,M in a re-gated quiet window (M =
   run/pa60-a-main-bzImage pure main strong; B =
   run/pa60-b-branch-bzImage branch HEAD strong — route-1 p6
   medians + p5 socket-family re-measure on the net-batch class).
   Host load held 18-30 with 15-16 foreign QEMUs all morning.

   The pa57 census (b332d08a, booted 06:10 at load 15-18) is
   BANKED: census rows green; two FAIL rows pending quiet
   adjudication on the b4da2106 census (tick cpu0 99.43 marginal;
   volume 85.8M vs 107.5M+-10% — rerun-once rule); slowtxn FAILED on
   one code-1 OUT 11770 ms (historical max 7730; load-elastic
   stretch candidate — quiet evidence moves the band, leg-36 code-7
   precedent). NO code 14 in pure main.

2. **pa55/pa57 A/B (wake/epoll levers 6fdada6b) — MEASURED,
   MERGE-READY (237ec423, ledger 8.10.9).** p2_ctxsw -40% with
   arm-aligned clusters across two host regimes; p5a_pipe /
   p4_fork_exec consistent small wins; p5b socket family is
   HOST-REGIME-BIMODAL (5-8 us vs 45-76 us clusters, not
   arm-correlated) — the post-p5 residual was host noise; the
   track-2 ladder moves to single-regime rows (p6 metadata /
   route-1). Code-14 targeted look closed (tr.code is a userspace
   RPC op number; the pa56branch row was a starved server thread at
   load 30, queue 0 — not a lost wake).

3. **DUAL-TRACK.** Track-1: §3 typed per-CPU landed as D-7 (d8d23064)
   — CpuLocal<T,N> + small_alloc magazine conversion + percpu-shape
   unit group; suite 1110/1110 (run/pa62-maketest3.log); D-7
   registered in design-divergences.md; GRQ typed lock key is the
   named follow-on. Track-2: route-1 mechanism CONFIRMED FIRING
   (mech pb=683.880 vs ctl pb=0.0 on the leg-37 pa59 pair); the
   honest fsw pair re-run (WITH the sd separator fix 3750e17c) +
   LTP creat05/fork09-class spot run (17 rows, image
   run/pa59-ltp-r1-rootfs.img) + p6 medians all ride the
   pa60/pa61 chains (run/pa61-route1.sh, committed, waits for
   pa60). Route-1 merge claim stays OPEN until those three land.

4. **STANDING.** sc2xx launcher.ready trigger NOT FIRED (sc258
   newest, zero events; pa39-watch pid 470703 standing, kept this
   leg per leg-36 convention — next leg decides keep/kill).
   <=2 lane QEMUs held all leg (deciders are load-gated and count
   every QEMU on the box). wip(snapshot) committed early (2a54e42c).

## This leg's commits (chronological)

- 3750e17c docs+dfx(leg-37 closeout): perf-report 8.10.7 + FSW1 sd
  separator (print-only).
- 920c7b3e merge main b4da2106.
- 2a54e42c wip(leg-38 snapshot) + pa57 census evidence banked.
- d8d23064 D-7 typed per-CPU storage (CpuLocal + magazine
  conversion + percpu-shape tests + design-divergences entry).
- 57a0dc7e wip: pa60/pa61 decider chains + D-7 banked kernels
  (then re-banked post-D-7 rebuild in the follow-up commit).
- 54b38536 ledger 8.10.8.
- 237ec423 ledger 8.10.9 (A/B measured) + pa57-pb artifacts.

## Merge set to report to the main session

READY: 6fdada6b (wake/epoll empty fast paths — measured this leg,
8.10.9), d8d23064+register (D-7 typed per-CPU — unit-verified; its
functional smoke rides the pa60/pa61 branch boots: pbench B arm,
fsw mech, LTP spot; report after those parse clean, or with the
caveat if the window never opens). HELD: 22452d60 (F4 route-1 —
fsw pair + LTP spot + p6 medians pending in the chains).
ALSO REPORT: the oh-img tracked-symlink trap on origin/main
(leg-37 note — still unresolved there; this worktree keeps the
fixed symlink UNCOMMITTED on purpose).

## Detached processes (handoff)

- pa60-census.sh (setsid, armed 06:42): census + pbench pair chain;
  deadline 12:42, then exits by itself.
- pa61-route1.sh (setsid, armed 06:45): LTP spot + fsw pair; starts
  when pa60's script exits; ~45 min runtime.
- pa39-watch.sh (pid 470703, leg-30 origin): standing.

If the census fires: parse with perf-gate.sh --skip-boot --log
run/pa-pa60main-serial.log --tag pa60main, then pa46-slowtxn.sh on
the same log (the decider does both automatically — the outputs are
run/pa60-gate-parse.txt + run/pa60-slowtxn-parse.txt). Adjudicate:
volume/tick vs the two pa57 FAIL rows; quiet code-1 band question;
small_alloc REST share row (7a6e1bf4 watch) on the b4da2106 class;
the pbench pair vs the pb1 anchor (p3b_cow row = COW-fix watch, p5
rows = net-batch watch).

## Build worktrees (removable)

- pa52-main: KEEP until the pa60 census boot runs (its kernel is
  the census target).
- Older removable (per leg-36 list): pa46-main, pa46-f4r2, pa50-main,
  pa55-a.
