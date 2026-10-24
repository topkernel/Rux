# Rux systematic performance audit (OH boot): measurement, ranking, root causes

Date: 2026-10-23/24. Lane: `feature/perf-audit` (branch off main `0727d295`).
Question: is the 16-23 min OpenHarmony boot (vs 10.7 s for the same image on
the reference Linux kernel, `runs/oh-ref-r1`) caused by a few systemic
root causes or a long tail of individual slow points? If few: which, how
much, and in what order should they be fixed?

Method: measure the whole quiet-OH boot on current main with new
kernel-side instrumentation (`syscost=1`, committed on this branch) plus
host-side serial timestamping; compare phases against the reference
Linux timeline; rank cost centers; group into families; propose a fix
roadmap routed to the existing lanes.

Runs (this worktree, `run/`):

| run | kernel | boot args | wall | notes |
|---|---|---|---|---|
| `pa-pa1` | main@0727d295 + syscost | syscost=1 | 1560 s (killed; alive) | 66,910 serial lines; full diagnostics on |
| `pa-pa2` | + softirq-vec counters | syscost=1 | ~360 s (killed) | dump lines interleaved by WW prints; superseded by pa3 |
| `pa-pa3` | + flood attribution | syscost=1 sprobe=0 | 780 s (killed) | flood/process attribution, softirq vecs |
| `pa-pa4` | + kmsg read counters | syscost=1 sprobe=0 | ~180 s (killed) | proves the read flood never touches /dev/kmsg |
| `pa-pa5/6/7/8` | + EOF/EAGAIN read censuses | syscost=1 sprobe=0 | 60-120 s each (killed) | fd-path census hunts (see F2) |
| `pa-pa9` | + positive-read census (pa10 adds fail census) | syscost=1 sprobe=0 | 330 s | ATTRIBUTES the flood: hilogd.rd_kmsg reads /proc/kmsg, 196 data reads then idle while flood counter runs — so the flood reads return 0 |
| `pa-pa10` | + failing-read census | syscost=1 sprobe=0 | 300 s | zero rd_fail rows all boot → flood reads are EOF; pa4-8 "not EOF" was census-table saturation (12/12 slots taken by earlier comms) |
| `pa-pa11` | + /proc/kmsg stream (WQ block) + kmsg write fix | syscost=1 sprobe=0 | 330 s (wedged) | ABBA WAKE: WQ-held wake vs GRQ-held scheduler prints → both CPUs spin; kmsg write passthrough VERIFIED working ([kmsg] markers live) |
| `pa-pa12` | + /proc/kmsg stream (timed-poll block) | syscost=1 sprobe=0 | 330 s | verification run for the fix |
| `pa-pa13` | same kernel as pa12 (= HEAD f94c9f19) | syscost=1 sprobe=0 | 780 s | post-F2 ranking re-measure, pa3 horizon; read rate -113x, hilogd.server now #1 flood; ranking v2 below |
| `pa-pa14` | pa13 + launcher's softirq v1/v8 empty-raise gates (69310bae timer hunks, applied to a scratch build) | syscost=1 sprobe=0 | 780 s | F1 gate A/B: NO measurable effect on OH boot — marginal raise rates identical to ungated from t=120 (199.2-199.7/s); latch flips at ~9 s (v8) / ~31 s (v1); host shared with 3 foreign QEMUs (2x oh-display, 1x samgr-callback probe) from mid-run |
| `pa-pa15` | main@f242b7be + pa15 probe (IF=0 zone hooks + UART per-byte + per-comm printk + recv census; hooks in run/pa15-probe.patch, bzImage run/pa15-probe-bzImage) | syscost=1 sprobe=0 | 782 s | F1 deep-dive + hilogd dual-drain profile. cpu1 parked by a KERNPANIC at t=418 (see incident) -> t>=480 windows are cpu0-only. 2 smoke runs first (pa15smoke/2/3): smoke1 caught the on()-before-cmdline cache-off bug, smoke2 caught a u64 multiply overflow in the dump |
| `pa-pa16smoke1/2` | main@bb25a46d + WW user-frame fix (0d64ce7b) + pa16 probe v3 (edge-triggered zones + REST call-site table; hooks in run/pa16-probe.patch, bzImage run/pa16-probe-bzImage) | syscost=1 sprobe=0 | 180/120 s | v3 validation: heal 0.02% of segments, miss=0, both CPUs balanced, site table 97.6% of REST. smoke1 (host load ~6) caught seconds-class host-stall tails inside exit-bearing sections (REST max 2775 ms); smoke2 added the GRQ/PTE-guard track_caller layer |
| `pa-pa16` | same kernel as smoke2 | syscost=1 sprobe=0 | 782 s | REST decomposition run (section 2c). No KERNPANIC all boot (WW user-frame fix in); host load ~7 (6-8 foreign QEMUs); max closed IF=0 segment 4 ms all boot |
| `pa-pa17` | main@0634bc8f (launcher wake-loss fixes a39d2bc0 v1 + 6e9679c5 v2 now merged) + pa16 probe v3 applied clean (bzImage run/pa17-probe-bzImage) | syscost=1 sprobe=0 | 782 s | W3 acceptance retest (section 2d). Host load ~6.7 (5 foreign QEMUs); TERM-DESCHED=0 all boot; max closed IF=0 segment 5 ms |
| `pa-pa18` | main@9bc3a5c8 (launcher v3 c90f80b0 + v4 7a8044e0 + v5-param 12ea541e + v6/R73 9777f195 now merged; v5 inert, param not passed) + pa16 probe v3 (nm run/pa18-nm.txt; bzImage rebuildable from the pinned commit) | syscost=1 sprobe=0 | 780 s | v1..v6 stacking retest (section 2e). TERM-DESCHED fires 8x, all survive; census population unchanged; GRQ -31% claim corrected to volume-coupled; pre-magazine allocator (A-only rung; B landed as 988e10c1 after this build) |
| `pa-pa20` | main@988e10c1 (= pa18 kernel + wildwrite per-CPU magazines cb51db03 + lock-hold counters 64156dcb) + pa16 probe v3 (nm run/pa20-nm.txt; bzImage rebuildable from the pinned commit) | syscost=1 sprobe=0 | 780 s | magazine before/after (section 2f). Global class-lock sections 741.7M -> 19.7M (-97.3%); small_alloc REST 176.1 -> 133.2 s (-24.3%); IF=0 23.0% of machine; W3 census unchanged |
| `pa-pa21-tick-tcg` | main@b8f686a6 current-main rebuild, no probe patch (target/x86_64-unknown-none/debug/bzImage) | syscost=1 sprobe=0, **ACCEL=tcg** | 120 s | leg-13 accel-matrix datapoint (section 2g): tick cadence under TCG = WW ages pinned at exactly 1001 ms (53/53), boot tick check `[fail]` 0 jiffies/150 ms, calibration print identical to KVM ("TSC 4292 MHz, LAPIC 62 MHz" — the host TSC shows through under TCG too). Closes "KVM artifact" for the 1000 ms family |
| `pa-pa22-tick-lvl7` | same kernel as pa21 | syscost=1 sprobe=0 loglevel=7, ACCEL=tcg | 60 s | leg-13 diagnostic: verbose console shifts WW ages to organic values (376 ms class, idle pid=0) — the ~1 s pinning is the quiet-boot steady state, not a fixed probe artifact; `tsc-clock: fell behind 110854` (11 ms slips) shows the TSC clock itself is fine; WW2 heartbeat did NOT print (Hrtimer-softirq dispatch condition unmet — open item) |
| `pa-pa23-tickfix-tcg` | main@78460742 + the launcher lane's timer fix hunk ONLY (u128 multiply-then-shift, `tsc_delta_nominal`, extracted from samgr-callback wip 1e54319f; scratch branch `scratch/tickfix`, commit 9a451bef — pre-merge validation of the A-line fix, never for main) | syscost=1 sprobe=0, **ACCEL=tcg** | 600 s wall (~300 guest s, dumps t=60..540) | leg-15 tick-fix battery (section 2h): tripwire `[ok]` 16 jiffies/150 ms (pa21: 0 [fail]); tick counters 99.9/s/CPU sustained through all 9 dumps; WW IRQ-OFF-WINDOW 2 prints, organic ages 414/251 ms (pa21: 57 all at exactly 1001 ms); WAKE-STRANDED 0 (pa21: 100); TERM-DESCHED 8 and WAKE-REFUSED 32 — identical to pa21 (tick-independent census counts hold); nanosleep avg 1.87 s -> 88.5 ms (-21x) at same-horizon t=60; clock_gettime max 654 ms -> 3 ms; composer_host.ready +95.1 -> +89.9 s wall (-5.5%) |
| `pa-pa24-tickfix-probe-tcg` | pa23's branch + pa16 probe v3 patch (scratch 520970f0; nm run/pa24-nm.txt) | syscost=1 sprobe=0, ACCEL=tcg | 600 s wall (dumps to t=540) | leg-15 battery item (b), EFLAGS-exact probe under the fix: IF=0 35.3% of machine at t=540 — TCG-inflated, not comparable to the KVM 23% band (official re-measure = post-merge KVM flagship); structure intact (heal 0.05-0.09% miss=0, max REST segment 13 ms ms-class, site table family shape kept) + expected new timer.rs:173/:317 site pair (true 100 Hz hrtimer dispatch); SOFTIRQ zone share 7.7-13.9% of IF=0 (was 1.0-1.7% pre-fix KVM — suppressed dispatch volume now real); WW/WAKE-STRANDED prints 0 all boot; tripwire `[ok]` |
| `pa-pa25-prefix-tcg` | stock main@78460742 (pre-fix rebuild; tripwire `[fail]` confirms) | syscost=1 sprobe=0, ACCEL=tcg | 600 s wall (dumps to t=540) | leg-15 same-accel deep baseline for pa23: WW ages pinned 1001 ms x283; nanosleep @t=240 avg 1.34 s / n 1300 (pa23: 120 ms / 6549, -11x at depth); futex+epoll avgs UNCHANGED (F3 tail, tick-independent); composer 130.3 s vs pa23 89.9 s but appspawn.started 312 s vs 468 s — opposite directions, wall-clock deltas noise-dominated, no claim; both runs end at the same depth (appspawn, not bundles) |
| `pa-pa26-preview-kvm` | main@6aa2fc74 + merge --no-commit of A-line 675095c0 (fix only; timer file verified byte-identical to scratch/tickfix) + pa16-probe v3; provenance run/pa26-built-from.txt; bzImage run/pa26-preview-bzImage (untracked, >10MB leg rule) | syscost=1 sprobe=0, **ACCEL=kvm** | 600 s wall (dumps to t=540) | **LEG-16 PREVIEW of the flagship** (official = post-merge re-run; see 2i): tripwire `[ok]` 15 jiffies/150 ms; **IF=0 = 25.5% of machine cumulative** (cpu0 138.6 s + cpu1 136.8 s / 2x540 s) — INSIDE the pre-fix KVM band 23.4-25.0%, so the fix does NOT move total IRQ-off share (it removes clock granularity, not irqsave work); splits REST 88.4% / HARD 7.7% / SOFT 3.1% / UART 0.7% — SOFTIRQ component 1.0-1.7% -> 3.1% of IF=0 (real hrtimer dispatch; direction matches pa24's TCG 7.7-13.9% but far below TCG inflation); max closed segments 3-7 ms ms-class; heal miss=0; REST #1 single site = GRQ sched.rs:2140 (68.8 s, 12.3 M seg, avg 5.6 us) over small_alloc family 71.5 s/4 sites, file.rs:676 29.1 s, wait.rs family 36.3 s, NEW timer.rs:173 pair 2.3 s; tick-mode kernel share 84.1-85.6% (post-fix true sampling; pa17/20 pre-fix read 86-89%); nanosleep avg 183 ms @ n=20048 (pa17 pre-fix KVM: 2.2 s — ~-12x on KVM, confirming the TCG -11x at depth); clock_gettime 30.9 M @ 3 us avg, cumulative max 289 ms (late-boot host-noise class); depth: appspawn binary load +5.4 s, composer host start +25.4 s, BootAnimation procs ready-true +39.98 s and alive to +541 s, foundation SIGCHLD +50.3 s (appfwk/launcher/bms/lockscreen not set), NO bootanimation.finished/launcher.ready at 600 s — same depth class as pre-fix quiet boots, single boot, no wall-clock claim; hot rip page 26% = the enable_irq/restore_irq thunk page (documented cli-window re-enable bias, amplified because ticks now really land — NOT a new cost center). LEG-17 PROVENANCE CORRECTION: the merge --no-commit brought the A-line dfx commits too (binder-stall/sleepsnap/TICKPROBE prints — 563 binder-stall lines in this log; 675095c0's parent is not on main), so this kernel is NOT identical to the official pa27 kernel; timer file itself was identical, so the tick-fix validation transfer stands. Superseded by pa27 wherever both exist. |
| `pa-pa27-flagship-kvm` | **merged main@e1e317f2 (= 199f308c timer fix, timer file diffed EMPTY vs scratch/tickfix and hunk byte-identical to 9a451bef/675095c0 — section 2j step 1)** + pa16-probe v3 (nm run/pa27-nm.txt; parse run/pa27-parse.txt) | syscost=1 sprobe=0, **ACCEL=kvm** | 780 s wall (dumps to t=720) | **THE OFFICIAL POST-MERGE FLAGSHIP (section 2j): tripwire `[ok]` 15 jiffies/150 ms; IF=0 = 25.7% of machine at t=720 (cpu0 185.9 s + cpu1 184.7 s / 2x720 s; heal miss=0 both CPUs; same-horizon t=540 reads 25.53% vs pa26's 25.50% — preview confirmed to 0.03 pp); split REST 88.5% / HARD 7.6% / SOFT 3.1% / UART 0.8% of IF=0 — official SOFTIRQ 3.1% confirms the suppressed hrtimer dispatch is real (pre-fix 1.0-1.7%; softirq busy 13.9 s/720 s vs pa16's 2.5 s/420 s, v1/v8 raises at the ~100 Hz/CPU dispatch ceiling); max closed segments 3-6 ms; REST sites: GRQ sched.rs:2140 96.1 s (17.7 M @ 5.4 us — volume ~2x pa16, avg flat), small_alloc 4 sites 91.0 s, file.rs:676 39.3 s, wait.rs family 50.0 s, EPOLL misc.rs:190 25.0 s, timer.rs:173/:317 pair 3.3 s; 95.4% of REST attributed; ticks 99.85/s/CPU sustained (71,890+71,845 samples/720 s); tick modes c0 13.5/85.1/1.4 c1 14.8/83.4/1.8; nanosleep avg 221 ms @ n=22,848 (-10x vs pa17 pre-fix KVM 2.2 s), clock_gettime 42.6 M @ 3 us max 288 ms (host-noise class); syscalls 100.9 M total (series-deepest volume); census TERM-DESCHED 8 / WAKE-REFUSED 32 / WAKE-STRANDED 0 / PREEMPT-LEAK 0, WAKE-RUN-LEAK 486 prints (pre-fix pa18/20: 677-678), IRQ-OFF-WINDOW prints 9; UART 1.49 M bytes @ 13.2-13.6 us/byte; milestones (host stamps): composer_host.ready +29.8 s, bms.appfwk param activity +57.4 s (appfwk.ready time-param already set; bms/launcher/lockscreen NOT set), appspawn.started +80.1 s, NO bootanimation exec / launcher.ready at 780 s — WALL-CLOCK VERDICT: not moved beyond the band (composer at the fast edge, appspawn above the band top — both directions = single-boot variance; F3 + binder cost classes own the deep-boot wall) |
| `pa19-unittest-main` | main@298b69fe (= this worktree, kernel/ byte-identical to main), riscv64 unit-test build, no probe | (unit test) | ~90 s | riscv64 make-test reproduction boot: instant truncated `trap: KERNPANIC pfa` before any kernel print; `-d int` trace (pa19-qemu-int.log) = store faults at trap_entry+0xda descending 0x120/frame from sp=_stack_top-8 — boot stack outside the 12MB early window (image 12.67MB). See NINTH LEG below |
| `pa19-unittest-fix` | same + RUX-RISCV-LAYOUT-2 window/heap fix (this branch) | (unit test) | ~150 s | verification boot: full unit suite runs, 60 ok / 3 FAIL (stale config assertions, pre-existing: KERNEL_HEAP_SIZE 512-vs-1024 bc0ffbee, FUTEX_* 256/64-vs-8192/512 08fcfc04) |

Host context for all runs: KVM q35 `-cpu max -m 4G -smp 2`, shared with
2-3 foreign-lane QEMUs (load avg ~5.5). Absolute numbers carry that
noise; ratios and structure are stable across pa1/pa3.

EIGHTH LEG (waiting state, 2026-10-24 06:50, main@298b69fe merged
fast-forward): condition check for both pending retests came up empty,
so zero QEMU this session so far, per discipline.

- A-line v3 (c90f80b0, wfi-paced terminal-deschedule retries) is NOT
  in origin/main. The launcher lane (samgr-callback worktree) is past
  it: their branch tip is 0dfdb482 (wip: wedge-bisect in flight) —
  sc192 (v2 kernel) ended at 06:33 with a still-leaking shape
  (PREEMPT-LEAK pid 1409, rip=wfi+5 class), sc193 launched 06:20 and
  running. GRQ/TERM retest boot (pa18) waits for their merge.
- B-line per-CPU freelist: origin/main has no new commits since
  298b69fe (docs-only since pa17's base 0634bc8f) — allocator
  before/after condition unchanged, still not landed.
- Launcher UI milestone: NOT reached. No OnForeground / launcher.ready
  / bootanimation.finished markers in sc191/sc192/sc193 serial logs;
  A-line's own ninth-leg note says SA 10 unpublished, screen at the
  8-color oracle. Report v1.1 (post-UI row of table 1) stays pending.
- pa18 readiness (cheap prep, no QEMU): `run/pa16-probe.patch` applies
  clean on 298b69fe; `kernel/` is byte-identical to the pa17 base, so
  the retest recipe when a merge lands is exactly: merge main, apply
  patch, build, `./run/pa-boot.sh pa18 ... APPEND_EXTRA="syscost=1
  sprobe=0"`, parse with pa-parse.py against the new ELF.

NINTH LEG (2026-10-24 08:40-09:05, main@298b69fe merge no-op, base
unchanged): both retest conditions empty again; the launcher lane's
ninth-leg notes flagged a riscv64 `make test` KERNPANIC and named this
lane's syscost merge as the suspected "depth culprit", calling for a
main-baseline run — that is this lane's work, so the QEMU budget went
there (3 short riscv64 boots, own PIDs, staggered; A-line quota-dead
then resumed mid-session with 3 x86_64 OH boots, untouched).

- Conditions at open: A-line tip now past v4 (7a8044e0 DEAD-task pick
  gates) at 10e338e8 (quota snapshot 08:33), all unmerged; their
  sc192/193/194 serial logs carry zero UI markers (launcher.ready /
  bootanimation.finished / OnForeground); B-line (alloc-crash) idle
  since Oct 19 13:01, d56de697 slab-packing unmerged. pa18 (GRQ/TERM
  retest) will cover v3+v4 together when their merge lands; the pa16
  probe patch does not overlap any RUX-RISCV-LAYOUT-2 file, so the
  pa18 recipe is unchanged. No new OH data -> report v1.0 tables and
  ranking v2.3 stay as delivered (idle "tweaks/scroll" items are no-ops
  without a measurement).
- **pa19 finding — riscv64 make test is unbootable on main, and the
  syscost suspicion was right about the trigger, wrong about the
  mechanism.** Reproduced instantly on this worktree (kernel/ is
  byte-identical to main@298b69fe): the boot dies with a truncated
  `trap: KERNPANIC pfa` before the kernel's first printk. QEMU
  `-d int` trace: after the designed satp trampoline (exec fault at
  _start+0x320 fetching `.L2` — normal), rust_main's FIRST stack push
  stores to sp=0xffffffff80c16ff8 = _stack_top-8 and faults —
  boot.S's early page tables map a 6-PMD 12MB image window and the
  unit-test image ends at 0xffffffff80c17000 (12.67MB), so the whole
  256K boot stack sits unmapped; trap_entry re-enters per frame push
  (store faults at trap_entry+0xda descending 0x120 bytes per nesting)
  and recurses through unmapped memory until it crawls below the
  window into .data, clobbering it, printing 20 bytes and dying.
  Stack DEPTH is exonerated: depth moves sp AWAY from the window end —
  more depth makes this double-fault less likely. This is the third
  documented image outgrowth (8->10MB, 10->12MB in boot.S comments;
  the pa16 syscost v3 statics, +50KB of .bss, were the straw past
  12MB). The launcher lane's own mid-suite panic (their sp at
  _stack_top-0x20, badaddr=_end) is a different, still-latent gap: the
  R73 KERNPANIC dump's unbounded sp+0x20..0xC0 reads walk off the
  image end when a fault lands within 0xC0 of the stack top
  (f1ed661b territory — flagged to the coordinator, not touched here).
- **Fix committed on this branch (RUX-RISCV-LAYOUT-2):** early window
  12->14MB (PMD[6] in early_pmd; PMD[5]+[6] in trampoline_pmd — the
  R3FIX had grown early_pmd to 12MB but left the trampoline at 10MB,
  a latent drift); heap base 0x80E00000->0x81000000, RAM reservation
  14->16MB and memory-map KERNEL_SIZE 12->14MB moved in lockstep
  (reservation-end == heap-start, per the RUX-RISCV-LAYOUT no-gap
  doctrine); buddy_allocator KERNEL_HEAP_PHYS matching copy; and a
  riscv64 linker.ld `ASSERT(_end <= window end)` so the FOURTH
  outgrowth fails the build instead of the boot.
- Verification: pre-fix log `run/pa19-unittest-main.log`; post-fix
  `run/pa19-unittest-fix.log` — the full unit suite runs (60 ok /
  3 FAIL, the 3 being stale config-value assertions unrelated to this
  change and pre-existing on main: KERNEL_HEAP_SIZE expects 512MB vs
  the restored 1024 (bc0ffbee), FUTEX_WAITER_POOL_SIZE/FUTEX_HASH_SIZE
  expect 256/64 vs the raised 8192/512 (08fcfc04) — test-side staleness
  for the coordinator, not fixed here). Normal riscv64 build links at
  _end 11.84MB (was never broken); all edits are cfg(riscv64) or
  arch-local, x86_64 paths untouched.

TENTH LEG (2026-10-24 10:00, main merged fast-forward 865eddaa ->
484c8cb8 — the ninth-leg RUX-RISCV-LAYOUT-2 fix is confirmed in main):
all three wait-gates re-checked and still closed, so zero QEMU again
this session, per discipline.

- Retest gate A (launcher sched fixes): NONE of v3 (c90f80b0, wfi-paced
  terminal-deschedule retries), v4 (7a8044e0, DEAD-task pick gates), or
  v5 (12ea541e, quietcon console divert) is in origin/main — verified
  per-commit with `git merge-base --is-ancestor` (their branch tip
  6bc0be67 likewise unmerged). Their lane is on their own tenth leg:
  sc195 final notes say quietcon wins the wedge race (45/45 min no
  wedge, RWFRZ=0) and the launcher reaches ArkTS/ArkUI executing —
  deepest ever; new residual strand: init pid=1 held hardirq-count
  from ~min 29 with rip stable in `cpu::inb` (844 PREEMPT-LEAK
  prints, system continuing on cpu1 at half speed). sc196 in flight
  (warm-reboot caveat noted by them: hilogd writes no /dev/console on
  boot 2, so ring forensics need fresh userdata). pa18 (GRQ/TERM
  retest, now covering v1+v2+v3+v4 together) still waits for their
  merge.
- Retest gate B (per-CPU freelist / allocator): alloc-crash worktree
  unchanged since 2026-10-19 (tip d56de697 slab-packing, unmerged) —
  before/after condition unchanged.
- UI milestone: NOT reached. sc195/sc196 serial logs carry zero
  hits for launcher.ready / bootanimation.finished / OnForeground /
  bootanimation.started; the shot monitor still shows the 8-color
  oracle screen (MILESTONE file, 100% non-black 8 colors). Report
  v1.1 (post-UI row of table 1) stays pending. "Deepest ever" =
  ArkTS/ArkUI code executing, not a UI event.
- pa18 readiness re-staged (cheap, no QEMU): `run/pa16-probe.patch`
  applies clean on 484c8cb8 (`git apply --check`); recipe unchanged —
  merge main, apply patch, `PLATFORM=x86_64 make build`,
  `./run/pa-boot.sh pa18 ... APPEND_EXTRA="syscost=1 sprobe=0"`,
  parse with pa-parse.py against the new ELF.
- Process lesson persisted for this lane (hit by the main session
  after pa19): `make test` builds into the SAME
  `target/.../debug/rux` path as the normal kernel — after any unit
  test run, a normal regression gate must first `rm build/.config &&
  make build` to rebuild the real kernel, or the gate boots the test
  binary (a `^test:` line in the boot log = wrong binary).
- Ranking v2.3 / report v1.0 tables: no new OH data -> unchanged
  (scroll items stay no-ops without a measurement). Report section 0
  gained one base-pointer sentence (current main 484c8cb8 is
  riscv64-only delta; all x86_64 numbers remain current).
- **R73 follow-up proposal drafted below (fix proposal for the A
  line's territory — this lane does not touch trap.rs).**

R73 follow-up: bound the KERNPANIC stack dump (fix PROPOSAL, for the
launcher lane; no code changed here)

Site: `kernel/src/arch/riscv64/trap.rs`, the `MmFaultResult::KernelPanic`
printer, the 24-word stack dump behind `if regs.kernel_mode()` (~line
1173): `let addr = regs.sp as usize + i * 8;` with `i in 0..24` and an
unconditional `read_volatile`.

Gap: R73's own comment gates USER-origin faults (user sp must not be
read — nested panic, the r72-fire3 tripled-print shape), but a KERNEL
sp within 0xC0 of the stack top is unguarded. That case is real, not
hypothetical: the launcher lane's mid-suite panic had sp =
`_stack_top-0x20`, badaddr = `_end` — dump words 0-3 read in bounds,
words 4-23 (sp+0x20..sp+0xB8) walked off the image end and re-entered
the trap inside the panic printer, killing the diagnostics mid-line
(exactly the nested-panic failure class R73 exists to prevent, arriving
from the kernel-sp direction). On the pre-LAYOUT-2 unit-test boot this
compounded the window overflow into near-silence.

Proposed fix (one guard, mirrors the existing user-sp branch):

1. Boot/early context (current()/tp not validated — the r74 concern
   two blocks below): bound the walk by the link-time `_stack_top`
   symbol (`linker.ld:71`, already `la`-ed in `boot.S:396`). As an
   extern symbol in Rust: `extern "C" { static _stack_top: u8; }`.
   In the loop, `if addr + 8 > stack_top { emit " >(top)"; break; }` —
   break (not skip) keeps the dump line shape stable for the
   addr2line/symbolization workflow.
2. Task context (validated tp/current only): same guard against the
   task's own stack top — task kernel stacks are KERNEL_STACK_SIZE =
   128 KB (`kernel/src/config.rs:44`), top = stack bottom + size (the
   context switch already computes this in `context.rs:267`). If
   validation fails, fall back to guard 1; the case that actually
   fired is the boot stack, which guard 1 covers.
3. Optional symmetric bottom bound (`addr < stack_bottom`): a garbage
   kernel sp prints `(sp outside stack)` and skips the dump, same
   spirit as the user-sp branch.

Cost: one compare per word inside a path that emits 24 words over
SBI putchar — unmeasurable. Effect: the panic printer can no longer
nested-fault from its own dump; the launcher lane's mid-suite panic
would have printed its full frame chain instead of dying mid-line.

**R73 update: landed as the launcher lane's v6** (9777f195, "fix(trap):
bound the KERNPANIC stack walk and latch dump re-entry", 2026-10-24
10:06, commit message credits the sc195 live sighting and the
coordinator's unit-test double-fault; it adds a dump re-entry latch on
top of the proposed bound). Still unmerged to main at leg close.

ELEVENTH LEG (2026-10-24 10:07-10:20, main merged fast-forward 484c8cb8
-> a67d2a83): all three wait-gates re-checked; zero QEMU and zero
builds this session, per discipline. One factual correction to gate B
below.

- Retest gate A (launcher sched fixes): still closed. NONE of v3
  (c90f80b0), v4 (7a8044e0), v5 (12ea541e), the new v6/R73 (9777f195)
  or the branch tip a480cd78 is in origin/main (`git merge-base
  --is-ancestor` per commit; tip moved 6bc0be67 -> a480cd78 during this
  leg: R73 v6 + sc196/sc197 notes). pa18 (GRQ/TERM retest over
  v1+v2+v3+v4 together) keeps waiting.
- Retest gate B (allocator): substantively still closed, but the
  recorded reason was WRONG for two legs. CORRECTION: d56de697
  (slab-packing) has been in origin/main since 2026-10-19 13:02 (merge
  c00da61e, 54 s after the branch tip was committed) — the ninth- and
  tenth-leg notes that called it "unmerged" were incorrect. Verified:
  `merge-base --is-ancestor d56de697 origin/main` succeeds, it is an
  ancestor of this audit's branch point 0727d295 (2026-10-23 21:54) and
  therefore of EVERY pa baseline pa1-pa17. Consequences: (1) every
  number in this report, including row 1b's small-alloc cache-lock
  share (12.2-12.6% of machine, 732-740 M lock acquisitions/boot), is a
  POST-slab-packing number — the per-CPU freelist lever targets the
  cache-lock cost that remains AFTER slab-packing, so the estimate
  stands or strengthens; (2) no allocator before/after is pending from
  the slab side; (3) the actual gate-B lever (per-CPU freelists, the
  direction the B line accepted in row 1b) exists nowhere — alloc-crash
  worktree idle since Oct 19 (tip d56de697, now known-merged),
  freelist-drift tip 93cadacb (Oct 18) likewise long merged, and
  origin/main has zero allocator commits since Oct 20 (log since then:
  this lane's docs + riscv64-only LAYOUT-2 + a readme). Gate stays
  closed.
- UI milestone: NOT reached. sc195 (final) and sc196 (killed by its
  owner after a warm-reboot stall, per their notes) both carry zero
  hits for launcher.ready / bootanimation.started|finished /
  OnForeground / ArkUI and zero bootevent.* lines at all; sc197
  launched 10:09 and was ~2 min in at leg close with 0 markers; the
  shotmon CSV still reads the 8-color oracle screen (8 colors, 100%
  non-black). Report v1.1 (post-UI row of table 1) stays pending.
- pa18 readiness re-verified (cheap, no QEMU): `run/pa16-probe.patch`
  applies clean on a67d2a83; recipe unchanged (merge main, apply patch,
  PLATFORM=x86_64 make build, `./run/pa-boot.sh pa18 ... APPEND_EXTRA=
  "syscost=1 sprobe=0"`, parse with pa-parse.py against the new ELF).
- Report tweaks (idle item, no new measurement): section 0 base pointer
  updated to main a67d2a83 (delta since the pa17 base 0634bc8f is still
  riscv64-only + docs, so all x86_64 numbers remain current); row 1b
  gained a post-slab-packing parenthetical. Ranking v2.3 and report
  v1.0 tables otherwise unchanged — no new OH data.

TWELFTH LEG (2026-10-24 10:15 open, main merged fast-forward 1938f19c
-> d304069d — the eleventh-leg docs are confirmed in main): all three
wait-gates re-checked at open; zero QEMU and zero builds, per
discipline. This leg's live item is sc197 monitoring (below).

- Retest gate A (launcher sched fixes): still closed. NONE of v3
  (c90f80b0), v4 (7a8044e0), v5 (12ea541e), v6/R73 (9777f195) or the
  branch tip a480cd78 is in origin/main (`git merge-base --is-ancestor`
  per commit); the launcher lane's tip has not moved since 10:10
  (a480cd78 = sc196/sc197 notes). pa18 (GRQ/TERM retest over
  v1..v4 together) keeps waiting; `run/pa16-probe.patch` re-verified
  to apply clean on d304069d.
- Retest gate B (allocator): closed, unchanged. origin/main has not
  moved since the eleventh-leg check except for this lane's own docs
  commit and its merge (d304069d); the mm-touching commits in main's
  recent log are the pre-existing samgr-callback merges (filemap_fault
  wip, VM_DONTCOPY, OOM retry policy) that predate this lane's branch
  point — no per-CPU freelist exists anywhere.
- UI milestone: sc197 in flight — the launcher lane rolled it at 10:09
  (v6 bzImage = R73 dump-bound kernel, FRESH userdata copy after
  sc196's warm-reboot stall, quietcon + LOGLEVEL=3, SMP2 KVM q35,
  90-minute window to ~11:39, per their SC-SESSIONS notes). At leg
  open (10:15, min 6): 965 serial lines, ZERO hits for launcher.ready /
  bootanimation.started|finished / OnForeground / ArkUI / bootevent —
  expected, sc195's ArkTS/ArkUI moment was ~min 25; shotmon still
  reads the 8-color oracle screen (8 colors, 100% non-black). Report
  v1.1 (post-UI row of table 1) stays pending; monitoring continues
  through this leg.
- Report tweak (idle item): perf-report.md section 0 base pointer was
  still 484c8cb8 (stale from the tenth leg) — updated to d304069d
  (delta since the measurement base remains riscv64-only + docs);
  same fix applied to the section 10 skeleton's base-pointer sentence.
  Ranking v2.3 and report tables otherwise unchanged — no new OH data.

Mid-leg updates (10:36-10:40):

- **sc197 verdict (launcher lane notes 20848543): soft-wedge at min ~12
  in the pid 900-1100 zone, ZERO UI markers, launcher NEVER STARTED**
  (RAM census: zero launcher strings — the deepest point was before
  appspawn's launcher phase; sc195 by the same minute was past pid 2500
  with ArkTS running). Oracle screen up at min 1 (fresh-copy bring-up
  works; sc196's black screen was warm-reboot-specific). No RWFRZ, no
  KERNPANIC, no sysrq-c, serial at ~190 B/s trickle = alive but
  everyone-else-blocked. READ (their words): the quiet boot has at
  least two failure shapes — the min-1 devmgr rwlock freeze (killed by
  quietcon v5) and an early soft-wedge around the display/service zone
  that leaves no kernel-side evidence at loglevel 3, exactly where the
  binder-stall/stkfutex census is muted. sc198 (quietcon + LOGLEVEL=4
  diagnostic roll) launched 10:35 to make it visible — this lane's UI
  monitoring item transfers to sc198.
- **A-line accepted the F1 REST lever list (their fb2a1354 notes,
  design-only, nothing coded/merged):** (1) EPOLL_WAKE_REGISTRY
  sharding designed — K=16 shards by hash(file_id), register/notify
  derive the shard, unregister derives from the epoll's own entries
  (no K-shard scan), lock order registry-shard -> per-epoll unchanged,
  per-shard leak guard; (2) GRQ refactor (per-CPU runqueues or sharded
  class queues) explicitly deferred — it intersects the v1-v4 kill-path
  machinery and needs the sc-series strand probes on; (3) console-async
  remaining piece = printk-kthread for kernel task-context emission
  (quietcon already removed the userspace console term) — design only.
  Routing update for ranking row 1b: the epoll-registry item moves from
  "ipc lane" to "launcher lane, design in hand". None of this is in
  main; gate A re-verified closed on their new tip 20848543.

Gate events + pa18 (11:00-11:15):

- **Gate A OPENED at ~10:58**: 9bc3a5c8 merged the launcher lane into
  origin/main (v3 c90f80b0, v4 7a8044e0, v5 12ea541e, v6/R73 9777f195;
  `merge-base --is-ancestor` all four). **Gate B OPENED at ~11:05**:
  988e10c1 merged the wildwrite lane's per-CPU magazine allocator
  (cb51db03, small_alloc.rs only) plus their lock-hold counters +
  SMALLALLOC census probe (64156dcb) — their A/B: 59.39 M obj ops vs
  3.60 M lock holds = 16.49x, projecting the 732 M class-lock
  irqsave sections/boot down ~94%. pa18 had already been BUILT and
  LAUNCHED on 9bc3a5c8 when B landed — kept running deliberately as
  the A-only rung (allocator held fixed = directly comparable to
  pa16/pa17); pa20 boots the A+B composite next. sc198 ended without
  UI markers (froze at 35,689 lines after appspawn.started +
  composer_host.ready); sc199 is the launcher lane's current roll —
  UI watch continues.
- **pa18 delivered (section 2e)**: TERM-DESCHED fired 8x and all
  survived (the v1 acceptance pa17 could not test); census lying-slot
  population unchanged (born elsewhere — birth forensics stays the
  lever); GRQ -31% claim corrected to volume-coupled (avg flat
  5.1-6.1 us, wall tracks n); IF=0 23.7% of machine; small_alloc
  12.2% cross-check pass; bootanimation process starts (no UI
  markers); console row fourth point 1.4% at 13-14 us/byte.
- **pa20 delivered (section 2f)**: the magazine measured on the OH
  boot — global class-lock sections 741.7 M -> 19.7 M/boot (-97.3%,
  confirming the wildwrite 16.49x projection), small_alloc REST wall
  176.1 -> 133.2 s (12.2% -> 9.2% of machine, -24.3%); machine IF=0
  23.7% -> 23.0% (volume growth elsewhere ate part of the win; GRQ
  at its deepest-boot maximum 66.5 s); W3 census identical (the
  allocator touches no wake path). Ranking bumped to v2.4; report to
  v1.1.
- **Both retest gates closed out this leg** — the eighth- through
  eleventh-leg waiting state is discharged: gate A (launcher v3..v6)
  measured in pa18, gate B (magazine) measured in pa20, and the
  pa17-era GRQ attribution corrected. Remaining open items for the
  next leg: UI milestone (launcher lane's sc-series; sc197-sc200 all
  died without UI markers, sc201 rolling at leg close with the 8-color
  oracle up and their schb soft-wedge heartbeat dfx 858b455a/18c194f6
  in the kernel), the census birth forensics (their lane),
  and the allocation-churn hunt (row 7's new priority).

- **THIRTEENTH LEG (tick-cadence adjudication, section 2g)**: the
  A-line's "kernel-mode IRQ-OFF-WINDOW 1000 ms" triage item is closed
  — it is the x86_64 timer IRQ's own ~1 Hz effective cadence (nominal
  100 Hz), identical under KVM and TCG (pa21/pa22), NOT interrupt-off
  time (EFLAGS pair accounting still bounds closed cli at 3-5 ms) and
  NOT a KVM sampling artifact. Boot tripwire `lapic tick check: 0
  jiffies / 150ms [fail]` fires in every x86_64 boot and is ignored;
  calibration prints a constant "LAPIC 62 MHz" across accels/hosts vs
  ~625 KHz effective (~100x). WAKE-STRANDED strands quantize on the
  1 s grid (sc200: 1000 ms x1488) — wedge census COUNTS stay valid,
  their LATENCY readouts do not resolve below 1 s. Ranking v2.5 row
  2b (F7); report v1.2 limitation 8 + ledger candidate
  X86-TICK-CADENCE; brief's "TCG+syscost" premise corrected (all pa
  boots have been KVM since pa1 — section 0). UI patrol: sc200-sc203
  no markers (sc202 patrolled to t=1664 s, sc204 rolling at commit
  time); PENDING rows stay PENDING.
  [LEG-15: the mechanism attribution in this bullet was corrected —
  see the correction block at the top of section 2g. The 1 s period
  was read_time's step size (TSC shift-first math), not the IRQ
  cadence; the LAPIC always fired at 100 Hz.]

## Verdict in one paragraph

The "one fix at a time" pattern was real but it has converged into a
small number of systems-level families. Measured on current main, the
boot spends **~85% of its CPU time in kernel mode** (Linux reference
spends most of the same boot in userspace), and that kernel time is
dominated by (1) **interrupt-disabled regions** — a quarter to a third
of all kernel-mode ticks land in the page holding `__do_softirq` +
`enable_irq`/`restore_irq`, which is the signature of ticks queuing
behind long IF=0 windows (softirq dispatch at irq-exit runs with IRQs
off, and every `irqsave` critical section attributes its queued ticks
to this page's re-enable site); (2) a **userspace syscall flood riding
on kernel per-call costs** — 560M syscalls in 25 min (~370K/s on 2
vCPUs), 60% of it logging traffic (`hilogd.server` 13K socket
receives/s, `hilogd.rd_kmsg` 94K reads/s spinning), amplified by retry
loops; (3) **scheduler wake-loss** — 448 wake-without-pick events per
boot with median 252 s age, which converts 10x-slower operations into
minutes-long service stalls (the actual 16-23 min driver); and (4) the
**ext4/metadata per-op family** — open/stat/getdents at 100-200x Linux,
fsync/fdatasync at 20-90x. The dominant single lever is not any
algorithmic hotspot: it is interrupt-off time + wake-loss, both of which
multiply everything else. Fixing the known in-flight items (mount index,
anon fault-around on the launcher lane) will not change this picture.

## 1. Phase timeline vs the reference

Reference (`oh-ref-r1`, KVM, kernel timestamps):

| event | Linux t | Rux pa1 t (host stamps) | ratio |
|---|---|---|---|
| kernel start -> /init running | 1.40 s | 0.85 s | 0.6x (Rux faster: no device probe storm) |
| first ServiceExec | 1.84 s | ~4-5 s | ~2.5x |
| hdf_devmgr SA-5100 registration | 2.90 s | ~6.4 s (SAREG at 6.4 s) | 2.2x |
| `bootevent.appspawn.started` | 4.81 s | 31.9 s | 6.6x |
| `bootevent.composer_host.ready` | 4.89 s | 40.3 s | 8.2x |
| `bootevent.launcher.ready` | 10.12 s | not reached in 26 min | >150x |
| `bootevent.bootanimation.finished` | 10.73 s | not reached | >145x |

Divergence grows with boot depth: the first services are only 2-6x
slower; everything downstream of appspawn (bundle queries, sandbox
mounts, SA loads) runs 8x to >100x slower. That shape is the signature
of a *latency multiplier* (see family F3), not a fixed per-op cost.

pa1 survived the full 26 min without the log-stop wedge (66,910 lines,
still emitting at kill). The 16-23 min wedge of sc175-180 did not
reproduce this time; hilog backpressure remains the known terminal
event when it does (SC-SESSIONS, session R).

## 2. Where the CPU goes (tick-mode accounting, syscost)

Per-dump samples (pa1 CPU0 and pa3 both CPUs; the two CPUs track within
a few % — pa1's late dumps that showed only c0 were print interleaving,
not a frozen vCPU):

| t | user | kernel | idle |
|---|---|---|---|
| pa1 60 s | 6.9% | 81.7% | 11.4% |
| pa1 900 s | 12.6% | 86.0% | 1.2% |
| pa3 780 s | c0 12.2% / c1 12.1% | c0 87.0% / c1 86.8% | c0 0.8% / c1 1.1% |

The kernel is CPU-saturated in kernel mode for the whole boot. In the
reference, the same image spends most CPU in userspace (services doing
real work); a 10.7 s boot cannot spend 85% of 2 vCPUs in kernel.

Kernel-rip page histogram (stable across pa1/pa3 despite different
binary layouts — pages verified by symbolizing each run against its own
ELF):

| page (pa1) | share of kernel ticks | contents (nm-verified) |
|---|---|---|
| 0xffffffff80659000 | **32%** | `__do_softirq` + softirq helpers + `enable_irq`/`disable_irq`/`restore_irq` + monowatch |
| 0xffffffff80458000 | 9% | `Atomic::fetch_update` instantiations: virtio queue CAS, `PageCache` put/flush RMWs |
| 0xffffffff80728000 | 4% | `alloc::string` machinery (path copies, fmt) |
| 0xffffffff80459000 | 4% | `atomic_store` glue |
| 0xffffffff806b5000 | 3% | `Vec<...>::drop` glue (allocation churn) |
| 0xffffffff805e9000 + 0x805e5000 + others | ~8% | atomic CAS/load/store + `RawSpinlock::lock` |
| 0xffffffff8021e000 | 2% | `TwoWaySearcher` (string scanning) |

The 32% page needs careful reading (pa3 finding): the softirq *handler*
time is measured separately (`WW_SOFTIRQ_TICKS`) at only ~0.4% CPU
(912 ms over 420 s). The ticks that land in this page are ticks that
**could not fire earlier**: the timer interrupt was masked while the CPU
was inside an IF=0 region, and it lands in the first window where IF
returns to 1 — which is `enable_irq`/`restore_irq` on the *same page*
as `__do_softirq`. So the 32% is not "the softirq loop burns CPU by
itself"; it is **a quarter to a third of kernel-mode wall time executes
with interrupts disabled**, centered on (a) softirq dispatch at irq-exit
(the whole `__do_softirq` loop runs at IF=0 inherited from the IRQ
frame) and (b) every `irqsave` critical section long enough to be
sampled. Consistent evidence:

- 395 printed IRQ-OFF-WINDOW events in pa1 (print gate = 1 per 2 s per
  CPU; near saturation => a qualifying window every ~4 s), 388 of them
  >= 1 s, median 1000 ms, max 2001 ms.
- Their symbolized recovery rips: 143x `enable_irq+6`, 16x `outb`, 8x
  `inb` (UART TX with IF=0), `memcmp` (string scan inside a cli
  section), `atomic_load` (spinlock polling acquired under irqsave).
- Timer consequences: `nanosleep` average 2.6-3.0 s (max 295 s) per
  call across the boot — sleep timers fire seconds late because ticks
  queue behind cli windows; every FFRT timer wheel in OH inherits this.
- pa13 (post-F2) does not change this picture: 316 IRQ-OFF-WINDOW
  prints over 780 s (0.44/s vs pa3's 0.49/s — same order), and the #1
  rip page is still the cli-recovery cluster (below).

### 2a. pa13: the post-F2 landscape (read storm gone)

pa13 = same kernel as pa12, run to the 780 s horizon for apples-to-apples
against pa3 (pre-fix). Host was quiet this time (load ~0.5, no foreign
QEMU), so absolute rates are cleaner than pa1/pa3/pa12.

Tick modes, last dump (t=720): cpu0 user 14.9% kernel 84.2% idle 0.9%;
cpu1 14.9% / 83.3% / 1.8%. (pa3 t=900: 12.2-12.4% / 86.9% / ~0.8%.)
Kernel share down ~3 pp, userspace up ~2.5 pp — the CPU the read storm
burned moved partially to userspace, partially to idle.

Syscall totals: 105.9M calls / 720 s = 147K/s vs pa3 188.7M / 900 s =
210K/s (**-30% syscall rate**; pa12's -16% at 330 s was the early-phase
slice). read: 591,713 total (0.82K/s) vs pa3 83.96M (93.3K/s) =
**-113x**. hilogd.rd_kmsg flood count 19,459 vs 83.46M (**-4300x**).

New flood leader (F2 remainder, confirmed): `hilogd.server` dual drain —
13.26M recvmsg **and** 13.26M recvfrom by t=720 (one of each per loop
iteration, 18.4K/s each; pa3: 11.62M each by t=900 = 12.9K/s — the
drain spins FASTER now that rd_kmsg no longer competes). Then
sysevent_source 672K, mmi_service 477K, connect-retry storm 3.26M
(pa3: 4.38M). clock_gettime 43.6M and writev 19.8M unchanged (not
flood-family: no per-call amplification, just volume).

pa13 kernel-rip pages (nm-verified against pa13's own ELF):

| page (pa13) | share | contents (nm) |
|---|---|---|
| 0xffffffff802cc000 | **26%** | `fs::buffer` FileChunk/FileBuffer read/write/resize + `cpu::enable_irq`/`disable_irq`/`restore_irq`/`save_and_disable_irq` + `inb`/`outb` — the same cli-recovery cluster as pa1/pa3's #1 page (layout shifted by the kmsg code); `__do_softirq` itself now lives on 0x806fe000, which is *below top-20* (<0.5%) |
| 0xffffffff8046a000 | 16% | generic `atomic_load`/`atomic_store` monomorphizations (RootFSSuperBlock, Task, u8-u64, SuperBlock/Ext4FileSystem/VfsMount/SysfsSuperBlock words) — the F5 atomic-glue page (pa3 equivalent 19%) |
| 0xffffffff80742000 | 5% | `log::__private_api::GlobalLogger::enabled` — log-filter checking per record |
| 0xffffffff80740000 | 4% | `alloc::string::String` fmt Write |
| 0xffffffff80632000 | 3% | `fs::inode::inode_hash` |
| others (rt_sigprocmask page 2%, TwoWaySearcher 2%, flood_account 1%...) | ~15% | long tail |

Reading: the F1 ticks-behind-cli page is still #1 at 26% (pa1 32%, pa3
20%; composition stable — re-enable sites + UART cli + now sharing the
page with FileBuffer). The parser's bare-symbol label for the page
(`Ext4DirEntry::from_bytes +0x520`) is the page-base symbol only; the
nm walk above is the authoritative content. The softirq *loop body*
dropping below top-20 while v1/v8 raise rates stay ~194/s matches the
established model: the cost is the dispatch discipline and where queued
ticks land, not handler time.

Boot milestones (single-run, high variance — do not over-read): pa13
composer_host.ready 44.3 s, appspawn.started 65.9 s (pa3: 33.5/36.1,
pa12: 36.0/43.8, pa1: 40.3/31.9). Early milestones stay in the same
2x band pre/post fix; the F2 fix's wall-clock claim rests on the
syscall/CPU structure above, not on these.

### 2b. pa15: F1 decomposed — UART per-byte cost, console producers, and the hilogd.server drain decoded

pa15 = main@f242b7be plus a measurement probe (fourth session). What it
adds over pa13/pa14: direct, stateless per-event numbers instead of
tick-sampling inferences. Four instruments:

1. **UART per-byte cost** (hook in `console.rs putc`, x86_64): every
   console byte reports its whole-call wall time (raw TSC), LSR poll
   count (one `inb` = one KVM VM exit), and whether it ran with the
   probe's IF=0 model set. Stateless per call — these numbers are
   exact regardless of the zone-accounting caveat below.
2. **Per-comm printk volume** (hook at `write_to_ring_buffer` +
   `emit_to_console`): records, ring bytes, console bytes, and
   console-bytes-under-IF=0 per comm; IRQ-context prints bucket to a
   `<irq>` pseudo-comm.
3. **recv-family fd census** (hook in the syscall wrapper, nrs
   47/45): per (comm, nr) slot that follows the current fd — flags,
   O_NONBLOCK, ret-shape (data/EAGAIN/0/errno), bytes, ops-table
   pointer (nm-symbolized host-side), and for AF_UNIX the socket
   kind/bound-name/peer.
4. **IF=0 zone accounting** (hooks in cpu.rs enable/disable, irq_enter/
   irq_exit, `__do_softirq`, putc): cumulative wall time with IF=0,
   split by zone — REST (explicit cli/irqsave sections), HARDIRQ
   (delivery->irq_exit frame), SOFTIRQ (dispatch loop), UART (putc).
   PROTOTYPE CAVEAT, stated up front: a task-context putc runs with
   IF=1 and can be migrated by the scheduler between its zone push and
   pop (the inb/outb VM exits are exactly where a resched IPI lands);
   a leaked stack entry then mis-attributes that CPU's zones and can
   hold a segment "open" across IF=1 time. Cross-checks caught this
   (uart-zone time 20x the summed per-putc times; REST nseg
   nonsensical at 32G). **Zone splits and the if0 totals are UPPER
   BOUNDS only.** The v2 probe (run/pa15-probe.patch as committed:
   zone pushes gated on if0_now for both putc and the ksoftirqd-side
   softirq entry) fixes that class — smoke4 (pa15smoke4, 180 s)
   validates cpu0 (uart zone 148 ms against 541 ms summed if0-putc
   time, consistent order), but cpu1 STILL inflates (uart segments
   averaging 10x the per-putc average, ~2 s stall maxes soaking
   zones) — a second leak class stays open, so zone numbers remain
   upper bounds until it is found. The stateless instruments above
   carry the findings.

**Finding 1 — the console path costs 16-18 us per byte and ~5% of the
machine, and its stall tail is seconds-class.** Final t=721 tables:

| table | bytes | total | avg/byte | max/byte | polls>1 |
|---|---|---|---|---|---|
| putc with IF=0 | 282,194 | 4.57 s | 16.2 us | 3 ms | 0 |
| putc with IF=1 | 4,061,151 | 74.19 s | 18.3 us | **7,085 ms** | 0 |

Every single byte polls LSR exactly once (multi=0) — the cost is the
`inb`+`outb` KVM-exit pair (~8-9 us per exit on this loaded host),
plus a host-scheduling tail: healthy-path bytes have stalled up to
7.1 s inside that one exit pair (2^34/2^35-cycle histogram outliers;
smoke runs saw 2.9-4.0 s versions of the same). 4.34 M console bytes
per boot at 11.5 printk records/s — total UART emission 78.8 s of CPU
per 782 s boot = **~5% of the 2-vCPU machine**, of which only ~0.3%
of machine (4.6 s) executes with IF=0. The per-byte latency tail, not
the IF share, is the F1 multiplier: an emitter parked seconds in a
putc while holding the printk/console lock is exactly the b140 wedge
shape and the 1 s-class TIMER-SOFTIRQ-STALL / nanosleep-lateness
feed (pa15: 363 IRQ-OFF-WINDOW prints at age≈1000 ms, same order as
pa13's 316).

**Finding 2 — who prints: OH userspace via /dev/kmsg dominates the
console; kernel IRQ-context prints are the only IF=0 emitters.** Top
console-byte producers, t=721 (8,962 records, 733 KB console bytes
total, loglevel=4 filtered exactly 1 record all boot):

| comm | records | console bytes | console bytes under IF=0 |
|---|---|---|---|
| init | 3,829 | 236,902 | 0 |
| `<irq>` (kernel IRQ-context) | 2,518 | 211,135 | 211,135 |
| OS_FFRT_2_0 (FFRT worker) | 958 | 138,613 | 0 |
| storage_daemon | 967 | 77,423 | 0 |
| storage_manager | 491 | 53,868 | 0 |

All task-context console emission runs with IF=1 (every con_if0B=0);
the only IF=0 console bytes are the kernel's own IRQ-context prints
(WW/sprobe family — including the SYSCOST dumps themselves, ~52 KB —
self-observation noted). Console-async or a console-loglevel drop
reclaims the ~5% CPU and removes the seconds-class stall tail; the
IF=0 portion (0.3% of machine) is secondary. This also sizes the
`[kmsg]` passthrough decision from pa9-12: userspace kmsg writes are
now the majority of console volume.

**Finding 3 — the zone split (upper bound) says the 26% cli-recovery
page is mostly REST.** t=420 (both CPUs alive), upper-bound IF=0 wall
= 237.4 s / 840 s CPU-s = 28.3% — same order as pa13's independent
26% tick-page estimate, so the model is right at the total level. The
split ordering is REST (irqsave sections: spinlocks, FileBuffer,
atomic RMW regions) > HARDIRQ frames > SOFTIRQ dispatch > UART —
i.e., the launcher line's irq-exit dispatch fix (roadmap 2b) targets
the hard+soft slice (single-digit % of CPU, upper bound ~5-8%), while
the bulk of IF=0 time is short irqsave critical sections scattered
across the kernel (60% of the upper bound). WW's own counters bound
the softirq story precisely: handler-pass time (WW_SOFTIRQ_TICKS) is
2.5 s per 420 s (0.6%), 203,849 passes (485/s against a 200/s two-CPU
tick rate) — the loop machinery is NOT spinning out its 2 ms budget
(~1.7 passes/invocation, ~95 us/invocation upper bound). The
launcher's in-flight dead-task-holding-HARDIRQ-count conviction
(sc186/sc189, parked in wfi) is a *lost-CPU* shape, not an IF=0
shape: wfi executes with IF=1, so it cannot inflate the cli-recovery
page — it would surface as kernel ticks with rip on the wfi page
(pa13 has no unidentified multi-percent page; pa15's own PREEMPT-LEAK
rips symbolize to `cpu::inb+0x10` (UART THRE poll inside an IRQ
frame), a log-crate iterator, and one park inside the pa15 probe's
own `irq_note_on` — the probe widens the classic sti-window and can
act as a park site for the pre-existing leak; the count anomaly
itself is the launcher lane's sc69 disassembly-proven pre-existing
bug).

**Finding 4 — hilogd.server dual-drain decoded: a PEEK+consume pair
paced by real traffic, not a spin.** Final t=721 census:

| syscall | calls | fd | target | flags | shape |
|---|---|---|---|---|---|
| recvfrom (45) | 9,477,284 | 12 | unix:dgram bound=/dev/unix/socket/hilogInput, UNIX_SOCKET_OPS, blocking | MSG_PEEK (0x2) | 100% data, exactly 2.0 B/call, 0 EAGAIN |
| recvmsg (47) | 9,477,284 | 12 | same fd | 0x0 | 100% data, 74 B/call avg, 702 MB total, 0 EAGAIN |

Perfectly 1:1 (both counters identical to the digit). The loop is:
blocking `recvfrom(MSG_PEEK)` parks on the socket wait queue until the
next hilog record arrives (avg 84 us/call at t=420 — exactly the
12.2 K records/s arrival period; max 59 s during a stall), returns a
2-byte peek, then `recvmsg` consumes the record (5 us avg). Evidence
for the ipc lane: (a) the "flood" is real log volume — 702 MB of
records per boot through one socket, fed by the writev storm (19.8 M
calls) of 100+ services; (b) each record pays TWO full socket recv
paths where one recvmsg suffices — the PEEK is pure OH-side overhead
(9.5 M syscalls, 19 MB copied); (c) 12.2 K blocking wake+peek pairs
per second ride the socket wait-queue wake path continuously, which
is the same wake machinery the W3 census watches; (d) no kernel
bug — per-call costs (5/84 us) are the lever, plus whatever the
launcher lane's wake-loss fix does to the tail (recvfrom max 59 s =
a WAKE-RUN-LEAK-class stall, not a kernel recv cost).

**pa15 incident — cpu1 parked by KERNPANIC at t=418.**
`trap: KERNPANIC pfault badaddr=0x00007ff090411000 rip=handle_timer_tick+0x526`
— the WW tick-age probe's stack walk read a garbage (user-range)
table base inside the timer tick handler, right after printing an
IRQ-OFF-WINDOW whose recovery rip was already in user space
(pid=0x42a rip=0x7ff0007c7a69). Not pa15 code (trap.rs is untouched
by the probe); the pattern is forensics reading corrupted task/regs
state — wildwrite-lane territory. Effect on pa15: cpu1's tick and
probe counters freeze from t=480 on; all t>=480 windows above are
cpu0-only. W3 census at 780 s: 26 refusals (pa13 baseline: 23),
WAKE-RUN-LEAK prints 406 (pa13: 702 — lower because half the machine
was parked), PREEMPT-LEAK 5.

**pa15 probe bugs found and fixed in the committed code** (both by
smoke runs before the real boot): (1) `syscost::on()` cached its
"off" verdict the first time it was called before cmdline init — the
pa15 zone hooks call it from the first boot-time irqsave spinlock,
which killed the whole probe (now: no caching until the cmdline
exists); (2) a `cyc * 1e9` u64 overflow in the UART dump line
(divide-first now).

### 2c. pa16: leak #2 closed, zones exact — and the REST bulk decomposed to file:line

pa16 (fifth session) = main@bb25a46d + the WW user-frame fix (0d64ce7b,
mirror of the wildwrite lane's fix for the pa15 t=418 cpu1 KERNPANIC —
their tick-probe stack walk treated canonical USER addresses as
kernel-walkable) + probe v3. Two goals, one instrument rewrite:

1. **Zone accounting v3 — exact, not upper bound.** v2's modeled-IF
   state could get stuck open with no detection or recovery (the cpu1
   soak). v3 makes every hook read the REAL flag (EFLAGS) and
   adjudicate: model-open + flag=1 closes into HEAL meters (zones stay
   clean), model-closed + flag=1 is the normal open (hooks sit BEFORE
   the cli/sti asm), model-closed + flag=0 is a counted MISS. Hardware
   delivery opens HARDIRQ at irq_enter; irq_exit closes and hard-resets
   the zone stack (an iret boundary ends every zone). putc splits a
   UART zone only when model-open AND real IF=0 (an IF=0 putc cannot
   migrate, so the pair is balanced by construction). Validation on the
   780 s boot: heal = 0.02% of segments / 0.72 s of 359.8 s IF=0
   (0.2%), miss = 0 (every cli path in the tree is hooked — the two raw
   boot-time `sti`s never leave a modeled-open state behind), both CPUs
   balanced (181.7/178.1 s; no soak), UART-zone avg == stateless
   per-byte avg (16.4 vs 15.6 us at t=720), and no KERNPANIC all boot.
2. **REST call-site attribution.** The probe marks disable_irq /
   save_and_disable_irq / irq_save / lock_irq* (+ the GRQ and PTE guard
   methods) `#[track_caller]`, so each REST segment records the
   file:line of the critical section that opened it; a global 96-slot
   table (+ counted overflow) accumulates n/cyc/max. Coverage at t=720:
   99.3% of REST cum.

**The exact F1 split (t=720, both CPUs):** IF=0 total 359.8 s over 1440
CPU-s = **25.0% of the machine** (tick-page estimates were 26-32%;
model now exact). REST 330.8 s (92.0% of IF=0), HARDIRQ 18.4 s (5.1%),
SOFTIRQ 5.7 s (1.6%), UART 5.0 s (1.4%). **The longest closed IF=0
segment all boot is 4 ms** — the kernel has NO intrinsically-long cli
section. 911 M REST segments over 720 s = **1.26 M irqsave sections/s**:
the REST bulk is volume, not any single long window. (smoke1, host load
~6, saw 2775 ms REST/HARDIRQ maxes — host scheduling tails landing
inside sections that contain VM exits: IPI/UART MMIO. The kernel's own
sections are 4 ms-class.) Tension flagged for the wildwrite lane: WW
printed 359 IRQ-OFF-WINDOW lines at age=1000 ms in the same boot where
EFLAGS-grounded accounting says no continuous IF=0 span exceeded 4 ms
and total heal-ambiguous time was 0.72 s — WW's tick-age semantics are
suspect (KVM catch-up ticks after a desched age any straddling window
to ~1 s buckets); calibration belongs to their lane. 33 user-frame
skips were printed by the fixed walker.

**The REST fix list (t=720, share of REST cum 330.8 s / of machine):**

| # | site | section | n | tot | avg | % REST | % machine |
|---|---|---|---|---|---|---|---|
| 1 | mm/small_alloc.rs:169 | alloc-path class-cache lock | 366.5 M | 92.1 s | 251 ns | 27.8% | 6.4% |
| 2 | mm/small_alloc.rs:281 | free-path class-cache lock | 365.6 M | 89.6 s | 244 ns | 27.0% | 6.2% |
| 3 | sched/sched.rs:2015 | `schedule()` GRQ take (pick path) | 9.3 M | 56.6 s | 6.1 us | 17.1% | 3.9% |
| 4 | fs/file.rs:676 | `get_file` fdtable entry lock | 90.2 M | 28.9 s | 319 ns | 8.7% | 2.0% |
| 5 | process/wait.rs:161 | `wake_up` waitqueue lock | 21.1 M | 25.0 s | 1.2 us | 7.5% | 1.7% |
| 6 | syscall/misc.rs:190 | EPOLL_WAKE_REGISTRY global (epoll wake bindings scan) | 10.1 M | 15.8 s | 1.6 us | 4.8% | 1.1% |
| 7 | mm/deferred_free.rs:126 | POOL_LOCK | 35.1 M | 10.2 s | 291 ns | 3.1% | 0.7% |
| 8 | process/wait.rs:319 | `finish_wait` | 4.6 M | 4.9 s | 1.1 us | 1.5% | 0.3% |
| 9 | process/wait.rs:287 | `prepare_to_wait` | 4.6 M | 2.7 s | 0.6 us | 0.8% | 0.2% |
| 10 | binder.rs:1070 (WORLD) + sched.rs:3462 + buddy x3 + bio.rs:1409 + misc tail | | ~1.6 M | ~2.2 s | | ~0.7% | ~0.2% |

Reading for the lanes:

- **The small-object allocator IS the F1 REST bulk: 54.9% of REST,
  12.6% of the whole machine**, at 732 M sections/boot x ~250 ns. The
  per-section cost is already tiny; the lever is (a) per-CPU freelists
  so the common path takes no shared irqsave lock at all, or (b) attack
  the churn itself (F5/fs#4: who allocates 1 M objects/s). mm is
  wildwrite-lane territory.
- **The GRQ inside `schedule()` is 3.9% of the machine** at 9.3 M holds
  x 6.1 us — the pick path under one global irqsave lock. Same lane as
  the wake-loss work (launcher); sharding or shortening pick under GRQ
  pays twice (CPU + wake latency).
- **fdtable `get_file` is 2.0% of the machine** at 125 K lock takes/s —
  every fd operation pays a global-per-table irqsave lock; an RCU-ish
  read path or per-bucket locks is the shape.
- **The waitqueue lock family (`wake_up`/`prepare`/`finish`) is 2.2%** —
  the same machinery F3's wake-loss work touches; 12.2 K hilogd
  wake/s ride it (pa15 finding 4).
- **EPOLL_WAKE_REGISTRY at 1.1% is a new name**: a single global lock,
  iterated per epoll-wake event, 14 K takes/s — route to the ipc lane
  alongside the hilogd evidence.
- Cross-check with the tick histogram holds: #1 page 29% (cli-recovery
  cluster) + #2 20% (atomic glue) — the glue IS these sections' RMWs.

Syscall-side at t=720 (context, host-loaded so rates ~20% below pa13):
83.8 M calls (116 K/s); hilogd dual-drain 10.58 M recvfrom + 10.70 M
recvmsg (same 1:1 shape, recvfrom avg 102 us / max 58 s — the wake-tail
victim again); writev 15.2 M; clock_gettime 35.1 M; connect-retry 2.55
M. Milestones: composer_host.ready 34.5 s, appspawn.started ~60 s —
inside the pa12/pa13 variance band. W3 census: 691 WAKE-RUN-LEAK
prints / 30 refusals / 9 PREEMPT-LEAK — the launcher's terminal-
deschedule fix (a39d2bc0, feature/samgr-callback) is still NOT in main
at audit time, so this stays the pre-fix baseline; retest when they
merge.

### 2d. pa17: W3 acceptance retest — the launcher's wake-loss fixes merged, census unchanged, GRQ hold down 31%

pa17 (sixth session) = main@0634bc8f, which now carries the launcher
lane's wake-loss fixes: **a39d2bc0 (v1)** terminal-deschedule re-asserts
DEAD + clears leaked preempt bits at the final switch, and **6e9679c5
(v2)** terminal deschedule also releases pick ownership. The pa16 probe
v3 applied clean on the merged tree (sched hunks shifted; the probe
marks `#[track_caller]` by function, so nothing was lost — the GRQ site
moved sched.rs:2015 -> 2092). Same 780 s horizon, same quiet-boot
discipline; host load ~6.7 (5 foreign QEMUs) vs pa16's ~7.

**W3 census, the acceptance measurement for roadmap item 1:**

| counter | pa16 (pre-fix) | pa17 (v1+v2 in main) | verdict |
|---|---|---|---|
| WAKE-RUN-LEAK prints | 691 | 593 | -14%, inside the pa13/pa15/pa16 variance band (702/406*/691; *pa15 lost a CPU at t=418) |
| W3-RUNWAKE-REFUSED total | 30 | 31 | unchanged |
| — lying busy slot (`curr_on&&on_cpu`) | 21 | 22 | unchanged — this population is NOT born on the fixed path |
| — linked, never picked | 9 | 7 | unchanged shape (same ~70/30 split) |
| PREEMPT-LEAK | 9 | 8 | unchanged |
| TERM-DESCHED (v1 helper firing) | n/a | **0** | the v1 trigger — a leaked preempt_count at the final switch of a dying task (the twice-reproduced sc186/sc189 shape) — did not occur once this boot |

Reading: **the fixes are compatible and harmless on this workload, but
this boot exercises neither.** TERM-DESCHED=0 means the terminal-
deschedule-with-leaked-bits path never ran, so v1's acceptance cannot
be demonstrated on this run (it is a rare-shape fix — sc186/sc189
reproduced "twice in most boots" on THEIR instrumented builds; a clean
census run cannot prove a fix for a shape it never sees). The census
lying-slot population (22 vs 21) is unchanged, which says those lying
slots are born on a DIFFERENT path than the terminal deschedule — the
launcher lane's birth-forensics hunt remains the right next step there.
No regression anywhere: milestones in band (composer_host.ready 31.4 s
vs 34.5; appspawn.started 60.2 vs ~60), hilogd dual-drain same volume
(10.08 M recvfrom + 10.29 M recvmsg, still 1:1, recvfrom max 393 s —
the wake-tail victim class persists), syscall total 80.7 M vs 83.8 M,
softirq raise rates ~191-199/s per vec.

**One direct, measurable effect: the schedule() GRQ REST hold dropped
31%.** Rest-site #3 (sched.rs:2092, `schedule()` GRQ take — the same
site as pa16's sched.rs:2015): 38.7 s / 7.56 M holds / avg 5.1 us — vs
pa16 56.6 s / 9.28 M / 6.1 us. Both n (-18%) and avg (-16%) fell; as a
machine share 3.9% -> 2.7% of 1440 CPU-s. The v2 pick-ownership release
plausibly removes repick-loop holds (their sc191 conviction: a DEAD
task kept resolving as the CPU's pick). Treat as one-run evidence: the
boot is single-sample and the GRQ hold interacts with host load; the
direction is consistent with the fix's design intent.

**Zones on the merged kernel (v3 holds):** IF=0 total 337.2 s / 1440
CPU-s = 23.4% of machine (pa16: 25.0%); REST 88.8% (pa16 92.0%),
HARDIRQ 7.7% (5.1%), SOFTIRQ 1.7% (1.6%), UART 1.9% (1.4%). Max closed
IF=0 segment 5 ms (pa16 4 ms — same class; cpu1's uart-zone max 524 ms
is a host-stall tail inside a putc VM exit, the known smoke1 shape,
not a kernel cli section). heal 0.2% / miss = 0 on both CPUs — the v3
adjudication is stable on the new kernel. REST segment volume 914.5 M
(pa16 911 M). Top-9 site coverage 97.3% of REST cum. Full site table
at t=720: small_alloc 169/281 n=370.5 M/369.6 M tot=89.1/86.0 s
avg=240/232 ns (machine 12.2%, pa16 12.6% — now the B lane's accepted
work); sched.rs:2092 above; file.rs:676 n=86.7 M tot=26.9 s (1.9%);
wait.rs:161/319/287 = 20.2+3.5+2.1 s (1.8% family); misc.rs:190
(EPOLL_WAKE_REGISTRY) 15.0 s (1.0%); deferred_free 9.9 s (0.7%); NEW in
top-12: timer.rs:317 and timer.rs:173 (~0.6+0.5 s — the launcher's new
timer lock sites, small).

**Console sub-item retest (ranking row 1a, third data point):** uart
if0 360 K bytes @ 18.5 us (6.7 s) + if1 5.25 M bytes @ 15.6 us (81.7 s)
= ~88.3 s of emission = **6.1% of the machine** (pa16: 24.5 s = 1.7%
at 1.55 M bytes; pa15: 78.8 s / 4.34 M bytes = 5.0%). Per-byte cost is
stable across
three boots (15.5-18.5 us; single LSR poll per byte, multi=0); the
VOLUME swings with boot depth (this boot's init event-loop spam was
heavier — [kmsg] le_epoll timeout markers repeat-stacked). Producers
unchanged: `<irq>` 232 KB console (100% under IF=0), init 229 KB
(con_if0B=244 ~ 0), OS_FFRT_2_0 145 KB. The console-async lever's size
estimate swings 1.7-6.1% of machine with volume, direction unchanged.

**WW age lines demoted to TRIAGE HINT (cross-lane, resolved).** The B
line calibrated the pa16-flagged tension (359 WW IRQ-OFF lines at
age≈1000 ms vs EFLAGS-exact max 4 ms): root cause is method, not
arithmetic — the wall-clock gap between timer-IRQ arrivals conflates
real IF=0 with host vCPU steal and tickless idle, and burst tick
delivery can only UNDERSTATE age, never inflate. Their fix (2e41b614):
idle-task recovery ticks print `idle-wake` and skip the walk; real-task
WW age lines are demoted to TRIAGE HINT under >=3-QEMU contention; the
same caveat covers their TIMER-SOFTIRQ-STALL tick_age. The EFLAGS
sampler (this lane's v3 zones) is confirmed as the ground-truth
instrument; their lines corroborate only. pa17 context: 340
IRQ-OFF-WINDOW lines (pa16: 359) — same order, now hint-class.



### 2e. pa18: the v1..v6 stacking retest — census population unchanged, TERM-DESCHED finally fires and survives, GRQ claim corrected to volume-coupled

pa18 (seventh session, 11:00-11:13) = main@9bc3a5c8 — the launcher
lane's full stack now in main: v1 a39d2bc0 + v2 6e9679c5 (wake-loss,
already in the pa17 base) PLUS v3 c90f80b0 (wfi-paced terminal-
deschedule retries), v4 7a8044e0 (DEAD-task pick gates), v5 12ea541e
(quietcon boot param — INERT here: pa-boot.sh does not pass it, so
/dev/console behavior is identical to pa16/pa17, required for the
console row's comparability) and v6 9777f195 (R73 dump bound). The
pa16 probe v3 applied clean after the merge; same 780 s horizon,
`syscost=1 sprobe=0`, loglevel=4 (pa-boot default; coordinator's
wedge caveat noted — see below). Host load ~6 (6 foreign QEMUs).
Pre-magazine allocator (Gate B had not landed at build time), which
makes pa18 the A-only rung of the ladder: pa18-pa17 isolates the
scheduler changes, pa20 (next) adds the allocator.

**W3 census, the acceptance measurement for v3+v4 (+v1's shape):**

| counter | pa16 (pre-fix) | pa17 (v1+v2) | pa18 (v1..v6) | verdict |
|---|---|---|---|---|
| WAKE-RUN-LEAK prints | 691 | 593 | 678 | variance band (593-702) — unchanged |
| W3-RUNWAKE-REFUSED | 30 | 31 | 31 | unchanged |
| — lying busy slot | 21 | 22 | 27 | unchanged-to-up — population persists, born on the un-fixed path |
| PREEMPT-LEAK | 9 | 8 | 10 | band |
| TERM-DESCHED (v1 trigger) | n/a | 0 | **8** | **the twice-reproduced sc186/sc189 shape FIRED 8x this boot and every occurrence survived** — no strand, no KERNPANIC, serial flowed to t=778.9 s, syscost dumps to t=720 |

Reading: v1+v3's terminal-deschedule path is now EXERCISED and harmless
under fire (pa17 could not test it — the shape never fired). But the
census lying-slot population is untouched by the whole v1..v6 stack:
those slots are born elsewhere, exactly as the launcher lane's
birth-forensics conviction says. No wedge within the 780 s horizon at
loglevel=4 — consistent with every pa boot (the sc-rolls' min-9/min-12
wedge lives on their scgold-userdata trajectory, not this one; the
coordinator's caveat about probe-traffic wedge applies to their boot
config, and is recorded here for the pa20 analysis).

**The schedule() GRQ claim is corrected: pa17's -31% was volume-coupled.**
pa18's GRQ site (sched.rs:2140 after the v3/v4 hunks): 46.2 s / 8.75 M
holds / avg 5.28 us — vs pa17 38.7/7.56 M/5.1 and pa16 56.6/9.28 M/6.1.
The hold count tracks schedule() volume (boot depth), and the wall
total follows the count: 56.6 -> 38.7 -> 46.2 s is n-driven
(9.28 -> 7.56 -> 8.75 M), while the per-hold avg is flat 5.1-6.1 us.
The pa17 note attributing -31% to v2's pick-ownership release was
single-run over-reading; the honest statement is "GRQ hold = 2.7-3.9%
of machine, volume-coupled, avg flat" until a phase-matched pair
exists. Machine share 3.2% this boot.

**Zones and REST sites (t=720, exact):** IF=0 340.5 s / 1440 CPU-s =
23.7% of machine (pa17 23.4, pa16 25.0); REST 92.1%; HARD 6.0%;
SOFT 1.0%; UART 1.0%; max closed segment 5 ms (band 4-5 ms; heal
0.09-0.10 M segments / ~0.4 s, miss 0 — v3 adjudication stable on the
new kernel). REST volume 934 M segments (band 911-934 M). Site table:
small_alloc 169/281 = 176.1 s = 12.2% of machine (pa17 12.2% —
IDENTICAL allocator by construction, cross-check passes); fdtable
file.rs:676 29.8 s (2.1%); waitqueue family 30.4 s (2.1% — wait.rs
161/319/287 = 23.9+4.1+2.4); EPOLL_WAKE_REGISTRY misc.rs:190 16.6 s
(1.2%); deferred_free 9.7 s (0.7%); GRQ above. Top-1 rip page 29% is
the same cli-recovery cluster as ever (nm-walked: enable_irq/
disable_irq/restore_irq + inb/outb + the taskdump console emitters
that share the page), #2 19% the atomic-glue page — composition
stable.

**Bootanimation starts on this kernel** (new vs pa17's log): two
BootAnimation processes reach "main enter" + "BootAnimationController
START" early and then sit in a 1.0007 s nanosleep loop (comm=
bootanimation in the slow-op ring from t=60). No launcher.ready, no
bootanimation.finished, no OnForeground — the UI milestone stays
unreached; the process presence only says the boot got past
appspawn's first spawn batch. Milestones in band: composer_host.ready
31.7 s (pa17 31.4), appspawn.started 51.6 s (pa17 60.2 — band
32-66 s).

**Console row 1a, fourth data point:** uart if0 401 K bytes @14.0 us
(5.6 s) + if1 1108 K @13.2 us (14.6 s) = ~20.2 s = **1.4% of machine**
(pa15 5.0, pa16 1.7, pa17 6.1) — per-byte cost band now 13.2-18.5 us
across four boots, single LSR poll (multi=0) every time; the machine
share swings 1.4-6.1% with console volume alone. Task-context
emission still 100% IF=1 (con_if0B=0 across the comm table).

**Syscall context (t=720):** 92.9 M calls (~129 K/s); hilogd
dual-drain intact at 11.51 M recvfrom @117 us (max 121 s — the
wake-tail victim persists) + 11.74 M recvmsg @7 us, still 1:1;
writev 15.99 M @22 us; clock_gettime 37.3 M @3 us; connect 2.48 M;
read volume collapsed further (420 K, avg 1712 us stall-inflated —
the /proc/kmsg stream fix holding). New shape worth naming: ioctl
66.7 K calls @599 ms avg blocked (display/fb path) — prelude to the
UI phase, watch it in pa20.

### 2f. pa20: the magazine measured — global class-lock sections -97.3%, REST wall -24.3%, machine IF=0 barely moves

pa20 (seventh session, 11:18-11:31) = origin/main@988e10c1 = pa18's
kernel PLUS the wildwrite lane's per-CPU magazines (cb51db03) and
their lock-hold counters (64156dcb) — the A+B composite rung of the
ladder. Same probe (pa16 v3, applied clean), same 780 s horizon,
`syscost=1 sprobe=0`, loglevel=4, host load ~6. The magazine diff is
small_alloc.rs only, so every other site delta is boot-volume noise
by construction.

**The allocator before/after (REST wall at t=720, site-attributed):**

| family | pa18 (pre-magazine) | pa20 (magazine) | delta |
|---|---|---|---|
| alloc path | :169 global class-lock 371.3 M @243 ns = 90.4 s | :247 magazine pop (local irqsave) 376.0 M @136 ns = 51.6 s + :266 refill class-lock 9.6 M @957 ns = 9.2 s | 90.4 -> 60.8 s |
| free path | :281 global class-lock 370.4 M @231 ns = 85.7 s | :413 magazine push (local irqsave) 375.2 M @168 ns = 63.4 s + :456 flush window 10.1 M @205 ns = 2.1 s + :495 flush class-lock 10.1 M @684 ns = 6.9 s | 85.7 -> 72.4 s |
| **global class-lock sections** | **~741.7 M/boot** | **19.7 M/boot (:266+:495)** | **-97.3% — the wildwrite 16.49x/-94% projection CONFIRMED and slightly exceeded** |
| total small_alloc REST | 176.1 s = 12.2% of machine | 133.2 s = 9.2% of machine | **-24.3% wall** |

Reading: the magazine did exactly what it was designed to do — the
GLOBAL lock is now touched only on refill/flush batches (19.7 M/boot
instead of every op). The wall-time ceiling is the per-op LOCAL
irqsave that guards the cpu_id/magazine consistency (136-168 ns per
op, 751 M ops/boot): that is the design's floor, not contention. The
12.6% machine item becomes a 9.2% item; the remaining lever is churn
volume (F5/fs#4: who allocates 1 M objects/s), not locking.

**Machine-level IF=0: 330.6 s / 1440 CPU-s = 23.0% (pa18 23.7, pa17
23.4, pa16 25.0) — a -0.7 pp move, because the allocator's -42.9 s
was partly eaten by volume growth elsewhere**: schedule() GRQ
46.2 -> 66.5 s (n 8.75 -> 9.88 M, avg 5.28 -> 6.73 us — deepest boot
of the four by hold count; avg up with it; single-run, treat as
volume+phase mix), fdtable 29.8 -> 32.3 s, waitqueue family 27.9 ->
32.9 s, deferred_free 9.7 -> 12.2 s. REST total 311.3 -> 297.6 s;
REST segment volume 934 -> 970 M (the magazine's local sections add
to the count). Max closed IF=0 segment 4 ms (band); heal 0.2%/miss 0
— the v3 adjudication holds on the composite kernel.

**W3 census: unchanged by the allocator (as expected — it touches no
wake path):** 677 leak prints / 32 refusals (25 lying + 7 linked) /
11 PREEMPT-LEAK / TERM-DESCHED 8 (fires ~8/boot and survives — now
two boots in a row). Milestones in band (composer_host.ready 35.1 s,
appspawn.started 68.0 s); no UI markers (bootanimation sleeps as in
pa18). Console row fifth point: 1.34 M bytes, 18.2 s emission = 1.3%
of machine @13.4-14.1 us/byte (volume low again; the 1.3-6.1% band
widens at the bottom). Tick modes (t=660 — the t=720 tick line was
lost to a print interleave): c0 11.1/88.1/0.5, c1 14.3/83.2/2.5 —
kernel share 83-88%, same class. Syscall totals in band (~90 M):
hilogd dual-drain 10.72 M + 10.86 M (1:1), writev 15.6 M,
clock_gettime 35.5 M, connect 2.67 M; ioctl display/fb blocked shape
persists (65.4 K @630 ms avg).

**Ladder summary (four boots, same probe, same horizon):**

| metric | pa16 | pa17 | pa18 | pa20 |
|---|---|---|---|---|
| kernel (scheduler) | pre-fix | +v1+v2 | +v3+v4(+v5 param inert, v6) | same as pa18 |
| allocator | class-lock | class-lock | class-lock | **magazine** |
| IF=0 % of machine | 25.0 | 23.4 | 23.7 | 23.0 |
| small_alloc REST s | 181.7 | 175.1 | 176.1 | **133.2** |
| GRQ REST s (site) | 56.6 | 38.7 | 46.2 | 66.5 |
| WAKE-RUN-LEAK prints | 691 | 593 | 678 | 677 |
| census refusals (lying) | 30 (21) | 31 (22) | 31 (27) | 32 (25) |
| TERM-DESCHED | n/a | 0 | 8 | 8 |
| console emission % | 1.7 | 6.1 | 1.4 | 1.3 |



## 3. Syscall cost table (pa1, cumulative t=1500s)

560M calls, 146 kinds. Hot by count (the flood):

| nr | name | calls | rate | avg | total wall | attribution (pa3) |
|---|---|---|---|---|---|---|
| 0 | read | 322M | 215K/s | ~1.6 us | 516 s | **hilogd.rd_kmsg = 94K/s of them**; rest spread |
| 228 | clock_gettime | 114M | 76K/s | <1 us | 58 s | (vDSO absent; still a syscall each) |
| 20 | writev | 44.6M | 30K/s | 14 us | 655 s | logging writes, many comm |
| 47 | recvmsg | 30.6M | 20K/s | 4 us | 129 s | **hilogd.server = 395K in 60 s** + sysevent_source |
| 45 | recvfrom | 29.8M | 20K/s | 54 us | 1613 s | **hilogd.server** (same socket drain) + mmi_service |
| 42 | connect | 7.4M | 5K/s | 65 us | 487 s | socket reconnect storm (dead-peer retries) |
| 281 | epoll_pwait | 2.05M | 1.4K/s | 13 ms | 26,848 s | event loops (blocked; normal shape) |
| 202 | futex | 1.7M | 1.1K/s | 118 ms | 201,347 s | FFRT pools + binder sync (blocked) |

Hot by per-op latency (pa1 averages; Linux norms are order-of-magnitude
references for the same operation on tmpfs/ext4-COW):

| op | Rux avg | max | Linux norm | factor | calls/boot | total |
|---|---|---|---|---|---|---|
| fsync | 90.5 ms | 7.0 s | 1-5 ms | 20-90x | 199 | 18 s |
| fdatasync | 59.8 ms | 3.0 s | 0.5-2 ms | 30-100x | 1,640 | 98 s |
| bind (sock) | 97.6 ms | 2.0 s | ~10 us | 10^4x | 41 | 4 s |
| unlink | 57.6 ms | 2.0 s | ~50 us | 10^3x | 330 | 19 s |
| ftruncate | 53.5 ms | 6.0 s | ~20 us | 10^3x | 673 | 36 s |
| execve | 51.9 ms | 3.0 s | 2-5 ms | 10-25x | 270 | 14 s |
| pwrite64 | 18.3 ms | 6.0 s | 50 us-1 ms | 20-300x | 14,127 | 258 s |
| fork/clone | 27 ms | 4.0 s | 0.1-1 ms | 30-200x | 703 | 19 s |
| mkdir | 2.9 ms (28.9 ms in pa1 t=660 phase) | 3.0 s | ~50 us (journaled) | 60-600x | 51,469 | 149 s |
| mount | 5.1 ms | 1.0 s | ~100 us | 50x | 12,145 | 62 s |
| chmod/chown | 7-8 ms | 3.0 s | ~20 us | 400x | 5,219 | 42 s |
| getdents64 | 0.5 ms | 2.0 s | 2-4 us | 100-250x | 34,951 | 17.5 s |
| pread64 | 1.75 ms | 3.0 s | 5-20 us | 100-350x | 32,634 | 57 s |
| open | 271 us | 6.0 s | 1-2 us | 100-200x | 467,657 | 127 s |
| newfstatat | 277 us | 3.0 s | ~1 us | 200x+ | 179,119 | 50 s |
| munmap | 163 us | 1.0 s | ~5 us | 30x | 1.09M | 179 s |

Notes:
- Wall-time averages include blocking; maxes show where scheduling
  delays land (6 s pwrite, 7 s fsync). The per-op *medians* for
  metadata ops are the 100-200x story; the maxes are family F3.
- `connect` at 7.4M calls is a *userspace* retry storm against dead
  peers — each retry pays a full kernel socket path.
- `mount` at 5.1 ms avg on main (which still has the linear registry
  scan — the launcher lane's index fix `8cd4f2c0` is not merged into
  main yet) is already far below the 1.7-1.9 s *per sandbox phase*
  telemetry, because a phase is dozens of mounts plus other work.

### 2g. THIRTEENTH LEG: the "1000 ms IRQ-OFF-WINDOW" family adjudicated — it is the timer's own ~1 Hz cadence, not interrupt-off time

> **LEG-15 CORRECTION (2026-10-24): the adjudication below was 90%
> right and the mechanism 100% wrong.** The 1 s periodicity is real,
> it is not IF=0, and it is not KVM-specific — the evidence ladder
> stands. But it is NOT the timer IRQ's delivery cadence (the LAPIC
> timer IRQ fired at the nominal 100 Hz all along) and NOT a LAPIC
> arming/calibration mismatch (item 3's divide-by-16 speculation was
> wrong). The true mechanism, found by the launcher lane's sc204
> TICKPROBE (their leg 12) and fixed on their branch: `read_time()`
> itself only moved once per second. 9e5f0147 (shift 12 -> 32) left
> the timekeeper computing `(delta_tsc >> 32) * mult` — shift FIRST —
> so at TSC 4.292 GHz the clock took one step every 2^32 TSC ticks =
> **1.0008 s**, each step being `mult` nominal ticks (the probe's
> nominal-2 ms windows both measured dt = 10,008,094 — exactly one
> step). Everything derived from read_time inherited the grid:
> jiffies (the tick handler derives it from the timebase, hence the
> `0 jiffies / 150ms [fail]` tripwire — the IRQ fired, the clock
> didn't move), the WW ages (measured in read_time's domain — the
> "1000/1001 ms inter-arrival" was the clock's step size, not the
> IRQ's period), WAKE-STRANDED strands, every timed wait. My "~625 KHz
> effective decrement (100x below calibrated)" was an artifact of
> measuring the CCR against the same broken clock. The u/k/i tick
> counters accumulating at 91-99/s/CPU "on average" while stamps
> showed ~1/s (open item below) is also now explained: the counters
> were ALWAYS sampled at the true 100 Hz IRQ rate; only the
> timestamps were grid-limited — there was no "faster early phase".
> Fix: u128 multiply-then-shift (`tsc_delta_nominal`, all 4
> shift-first sites), ~0.23 ns granularity restored; verified live by
> the launcher lane on sc205 (WW IRQ-OFF prints 0, WAKE-STRANDED 0,
> 2 ms busy-wait measures true) and retested by this lane in pa23
> (section 2h). Ranking row 2b and report v1.3 are updated
> accordingly; the X86-TICK-CADENCE ledger candidate is discharged
> with the mechanism corrected.

Coordinator's leg-13 brief carried A-line triage input: sc201's
terminal "kernel-mode IRQ-OFF-WINDOW 1000 ms" and sc200's 1141
kernel-mode hits, against this lane's "max closed cli 4 ms" — framed
as KVM-sampling-artifact vs real. The adjudication below closes it;
**neither**: the 1000 ms family is the x86_64 timer IRQ's own delivery
cadence, and it is not KVM-specific (identical under TCG).

Evidence ladder (all logs in the shared run dirs):

1. **Age quantization.** WW IRQ-OFF-WINDOW ages are pinned at
   EXACTLY 1000 ms under KVM — sc200: 1212/1223 hits, sc201: 280/288,
   sc202: 89/93; my pa15: 346/350; pa16/17/18/20 flagships: rate-gate
   saturated whole-boot (340-364 prints each, one per ~2.15 s = the 2 s
   gate ceiling). Under TCG (pa21): exactly 1001 ms, 53/53. Organic
   critical sections do not quantize to the millisecond across 1200+
   samples; a periodic timer's period does. (pa22, loglevel=7, shows
   the complementary fact: with verbose console load the ages become
   organic 376 ms-class values — the ~1 s pin is the quiet-boot steady
   state.)
2. **Boot tripwire.** `timer: lapic tick check: 0 jiffies / 150ms
   [fail]` prints in EVERY x86_64 boot carrying the check — mine
   (pa13/pa15smoke/pa16smoke, KVM; pa21/pa22, TCG) and A-line's
   (sc200/201/202). The armed periodic LAPIC timer delivers zero ticks
   in a 150 ms busy-spin with IRQs enabled, right after arming; the
   check fails and the boot continues anyway.
3. **Calibration is self-inconsistent with behavior.** Every boot,
   every accel, every host load prints the same "TSC 4292 MHz, LAPIC
   62 MHz". 62.5 MHz/100 (the arm path: ticks = freq/KERNEL_HZ) gives
   count 625000; the observed 1.000 s period implies an effective
   decrement of ~625 KHz — a factor ~100 below the calibrated rate.
   (625 KHz = 10 MHz/16, and 62.5 MHz = 1 GHz/16 — the divide-by-16
   appears twice with a 100x between calibration and running; the
   exact arithmetic — calibration window via PIT ch2 OUT2 under
   QEMU/KVM vs x2APIC divide semantics vs arm-path count — needs one
   live probe boot with a period print, not resolvable statically.)
   [LEG-15: WRONG — the "observed 1.000 s period" was read_time's
   step size, not the CCR's decrement period; the CCR always ran at
   the calibrated 62.5 MHz and the IRQ always fired at 100 Hz. The
   probe this item asked for is what the launcher lane's sc204
   TICKPROBE then did, and its result is the correction above.]
4. **Not IF=0.** The EFLAGS-exact pair accounting (pa16 probe v3,
   heal 0.02%, miss=0) bounds every closed cli section at 3-5 ms in
   all flagship boots. No 1000 ms interrupt-off window exists. The
   WW-age probe measures wall gaps between timer-IRQ arrivals
   (TSC-domain read_time), which on this lane equals the timer period.
5. **"Kernel-mode" is incidental.** sc200: 1089 kernel-mode / 133
   user / 1 idle; the machine runs 83-90% kernel-mode, so ~9 in 10
   rare recovery ticks land on kernel text. Symbolizing pa21's rips
   against the exact pa21 build: `wfi`, `enable_irq`, `restore_irq`
   (the idle loop's wake path), `atomic_load`, `Vec::drop` — the
   interrupted frame is wherever the CPU was, not a culprit. A-line's
   "sc201 terminal kernel-mode 1000 ms" is the same incidental
   landing inside the wedge storm.
6. **Downstream quantization (A-line relevance).** WAKE-STRANDED
   strands quantize on the same grid: sc200 histogram 1000 ms x1488,
   2001 ms x312, 1478 x56, 2478 x10; sc201: 1000 x490, 1480 x10,
   2001 x5. Every strand=1000/2001 ms in a wedge roll means "one/two
   ticks late", not a measured millisecond latency — sub-second wake
   losses are unresolvable on x86_64 rolls until the tick is fixed.
   Their TIMER-SOFTIRQ-STALL single lines per boot are the same
   family. This does NOT take over their wedge triage: the wedge
   shapes (CRAWL/soft-wedge, WAKE-STRANDED populations, PREEMPT-LEAK)
   are census counts and stay valid; only their LATENCY readouts are
   grid-limited.
7. **Correction to the brief's premise.** The brief called our
   "max 4 ms cli" finding "TCG+syscost" — the run table has said
   since pa1 that ALL measurement boots are KVM q35 (section 0 of
   the report; F6's note). There was never a TCG-vs-KVM divergence in
   our data; the divergence was probe-vs-probe (WW wall-gap vs
   EFLAGS pair accounting) in the same KVM environment, and pa21/pa22
   (TCG) now close the matrix on the other axis as well: same
   behavior, both accels.

What stands, what carries a caveat (also report v1.2 limitation 8):
STANDS — all EFLAGS-exact numbers (IF=0 23-25%, REST split, 1.3-1.4 M
sections/s, max segment 3-5 ms, magazine/GRQ/fdtable/waitqueue/epoll
walls), console per-byte costs, syscall volumes, milestone wall times
(jiffies recompute from the TSC grid per arrival, so t= tracks wall —
pa21's t=60 s dump landed at wall +65.8 s). CAVEAT — tick-mode u/k/i
shares (sampled at the ACTUAL tick rate; phase-weighted, sparse in
the ~1 Hz steady state — the 86-89% kernel figure is a real share of
tick samples, but tick samples are no longer 200/boot-second), per-op
MAXES (the 1.0 s cluster is tick-grid contamination until re-measured
after a tick fix), nanosleep/futex tail ATTRIBUTION (the 1 s grid
inflates them; F3 counts stand, F3 latencies do not resolve below
1 s).

Open items filed for the next legs / other lanes:
- **Fix candidate X86-TICK-CADENCE** (report ledger): probe the live
  period after arming (arm, busy-wait 200 ms, assert ticks>0 AND
  period<=2x nominal; the [fail] check already exists and is ignored
  — make it fatal or fix-and-retry). Owner: whoever owns
  kernel/src/drivers/intc/apic.rs. Then this lane reruns the pa18-style
  boot: expected collapses are the WW saturation, the 1.0 s maxes,
  WAKE-STRANDED quantization, and (hypothesis) part of the
  nanosleep/futex tails and deep-boot phase latencies.
- **WW2 heartbeat availability**: pa22 (loglevel=7) still printed no
  WW/WW2 heartbeat — the 1 Hz heartbeat rides the Hrtimer softirq,
  whose dispatch condition was not met on that boot; getting
  per-second WW_TICK_IRQ deltas on a normal boot (dfx switch or
  moving the print into the SYSCOST dump) would close the residual
  question of which boot phases feed the u/k/i counters (they
  accumulate ~91-99/s/CPU on average while steady-state stamps are
  ~1/s — a faster early phase is the likely reconciliation; pa22's
  27 organic-age prints in 60 s vs pa21's 53/120 s steady-1001 s are
  consistent with load-dependent cadence).
  [LEG-15: CLOSED — the reconciliation is the 2g correction (the
  counters ALWAYS ran at the true 100 Hz; only the stamps were
  grid-limited; no fast early phase exists — pa23 shows 99.9/s/CPU
  linear through the whole boot), and sc206 (post-fix A-line roll)
  prints the WW2 heartbeat 739 times with ticks=[100,100] on both
  CPUs all boot.]
- **UI-marker patrol** (leg-13 primary task): sc200 (died in the
  storm family at 5595 quiet-con lines, userspace alive to t=2656 s),
  sc201 (terminal storm), sc202 (patrolled to t=1664 s at leg close,
  still in the wedge family, no markers), sc203 (died early, no SC-HB
  reached), sc204 rolling at commit time. No
  launcher.ready/OnForeground/ArkUI markers in any roll this leg; the
  PENDING post-UI rows in report table 1 stay PENDING.
  [LEG-15 patrol (sc205 read by A-line leg 13, not duplicated):
  sc206 (14:00, post-fix kernel, SC-HB to t=787 s) — STILL NO UI
  markers (the only "arkui" strings are SELinux labels
  `u:object_r:arkui_param:s0` and a param-watcher name, not boot
  markers); PENDING rows stay PENDING. But sc206 is the DEEPEST roll
  ever: first bootevent.boot.completed in any sc roll (sc204/205: 0),
  plus appspawn.started / bms.main.bundles.ready / appfwk.ready /
  account.ready; census families collapsed on the fixed clock
  (WAKE-STRANDED 8 vs sc200's 1488+312; IRQ-OFF-WINDOW 2; TERM-
  DESCHED 8), WW tail: wakes 28.34 M / picks 27.68 M / stranded=19 /
  strand_max=140925 ms — the true F3 wake-loss face, off the grid;
  still short of the UI: installd `Load system ability 511 failed`
  and BMS bundle queries still running at the tail. Why the chain
  stops there is the launcher lane's territory.]
  [LEG-15 addendum: sc207 (14:5x, rolling at patrol time, SC-HB
  t=185 s) — no UI markers yet (no bootevent milestones at all at
  that depth); healthy post-fix census (TERM-DESCHED 8,
  WAKE-STRANDED 3, PREEMPT-LEAK 0, IRQ-OFF 0); the fixed clock now
  exposes the deep-boot wait face directly: IPCObjectStub
  SendRequestInner to *.ISystemAbilityManager at cost:29998 ms
  (~30 s binder timeouts) and installd SA-511 load failure — the
  post-grid blocker chain for the launcher lane.]
  [LEG-16 patrol, sc207 FINAL (log closed 14:36, sleepsnap tail =
  run end): the mid-roll note above was premature on "no bootevent
  milestones" — sc207 reached the SAME plateau as sc206 later in the
  run. Full marker-set diff sc206 vs sc207: 15 shared bootevent
  markers (incl. boot.completed, appfwk.ready, samgr.ready,
  composer_host.ready, appspawn.started); sc206 uniquely has
  bootevent.bms.main.bundles.ready, sc207 adds no new marker; both
  exec bootanimation (ServiceStart started -> ServiceExec -> exec
  path=/system/bin/bootanimation, pid 857/858 class). Still NO
  launcher.ready / bootanimation.finished / OnForeground in either.
  Two consecutive post-fix rolls now stop at the same depth — the
  plateau is real, its owner is the launcher lane. Census holds
  exactly: TERM-DESCHED 8 / WAKE-REFUSED 32 in BOTH rolls (matches
  pa21/pa23 battery counts). WAKE-STRANDED 15 (sc206) vs 3 (sc207),
  strands 1019/1185/1250 ms, organic off-grid ~1 s class — the F3
  wake-loss face, tick-independent as predicted. Binder cost family
  (A-line's next chain link, recorded not taken over): 9
  SendRequestInner-to-ISystemAbilityManager cost prints in sc207,
  QUANTIZED to userspace timeout classes on the now-true clock —
  15 s class x4 (14988/14995/15001/+1), 30 s class x4 (29998 x2,
  29999, 28647), one 3128 ms; the 30 s class pairs with
  `SAMGR: Start dynamic proc:installs,511,105_29997ms` — SAMgr
  dynamic-proc start (installs SA-511) is the single wait that
  matches the sc206 installd SA-511 load failure. Pre-fix rolls
  could not see any of this (cost prints wrapped to ~4.29e6 ms).
  ENDING CLARIFICATION (A-line f5e0346a, 14:59): sc207 did NOT time
  out — it ended in a HARD FREEZE, root-caused by the A-line as a
  print-under-bucket-lock self-deadlock (binder); the sleepsnap tail
  and the trailing WAKE-STRANDED print are freeze-context, not census
  events. sc208 (14:59, rolling) is their fix roll — patrol continues
  there next leg.]
  [LEG-16 addendum, sc208 mid-roll (62 K lines at 15:36, still live):
  census holds for the THIRD consecutive roll — TERM-DESCHED 8 /
  WAKE-REFUSED 32 / PREEMPT-LEAK 0; WAKE-STRANDED 12; bms.main.bundles
  .ready AND appfwk.ready both set (sc206-parity depth), launcher 0.
  Binder cost family DEEPENS: the 15 s (x5) and 30 s (x3) classes
  persist and a NEW ~89 s class appears (88632/89396/89400 ms x1 each)
  — longer-than-timeout waits now visible; recorded for the A-line
  samgr chain, not taken over.]

### 2h. FIFTEENTH LEG: the tick fix verified before merge — pa23 battery on main + the fix hunk (section 2g's correction measured)

Leg 14 died of quota exhaustion 18 s in; this leg does its job. The
launcher lane's sc204 TICKPROBE found the real mechanism (read_time's
shift-first math, section 2g correction) and their fix (u128
multiply-then-shift, `tsc_delta_nominal` at all 4 sites) sits in
their wip snapshot 1e54319f awaiting a clean commit + coordinator
merge. Rather than wait idle, this lane extracted the timer hunk
ALONE (x86_64.rs is unchanged on main since their branch point
9bc3a5c8, so the extraction is exactly one hunk), applied it on
current main (scratch branch `scratch/tickfix`, 9a451bef — local
only, never for main), and ran the leg-14/15 battery as pa23 (TCG,
quiet, syscost=1 sprobe=0, 600 s wall = dumps t=60..540; pa21 is the
same-config pre-fix baseline). Results:

| battery item | before (pa21, pre-fix) | after (pa23, main+fix) |
|---|---|---|
| (a) boot tripwire | `lapic tick check: 0 jiffies / 150ms [fail]` | **`lapic tick check: 16 jiffies / 150ms [ok]`** (~107/s) |
| (a) tick counter rate | ~5990/60 s/CPU in t=0-60, interpretation ambiguous (see 2g open item) | **99.9/s/CPU SUSTAINED, linear through all 9 dumps** (5994-5997 per 60 s per CPU) — the "faster early phase" reconciliation: the counters always ran at the true 100 Hz; only the stamps were grid-limited |
| (b) u/k/i shares (tick-sampled) | c0 88.0% kernel (sparse steady-state sampling; caveat) | **c0 18.8/80.0/1.2, c1 16.5/82.5/1.0 at t=540** — kernel share now 80-83% at true 100 Hz sampling (the 86-89% pre-fix figure was phase-weighted by sparse sampling); EFLAGS-pair IF=0 numbers need the pa16 probe patch — measured pre-merge under TCG in pa24 (below; structure holds) and re-run on merged main as the KVM flagship for the official share |
| (c) WW IRQ-OFF-WINDOW | 57 prints, ALL exactly 1001 ms | **2 prints, organic ages 414/251 ms** (the pa22 loglevel=7 class — residual host-steal/tickless gaps, not a grid) |
| (c) WAKE-STRANDED | 100 (quantized 1000/2001 ms strands) | **0** |
| (d) nanosleep (t=60 rows) | n=60, avg 1,869,294 us, max 10,014 ms | **n=1086 (18x more sleeps complete), avg 88,505 us (-21x), max 10,006 ms** (the 10 s maxes are real OH retry-loop sleep durations now, not 10 x 1 s grid) |
| (d) futex (t=60 rows) | n=5910, avg 125,896 us | n=5659, avg 89,562 us (-29%) — remaining tail is the true F3 wake-loss face |
| (d) epoll_pwait (t=60) | avg 746,503 us | avg 287,856 us (-61%) |
| (d) clock_gettime max | 654 ms (grid-inflated) | **3 ms** |
| census holds | TERM-DESCHED 8, WAKE-REFUSED 32 | **TERM-DESCHED 8, WAKE-REFUSED 32** — tick-independent census counts identical, as section 2g predicted |
| phase wall (host stamps) | composer_host.ready +95.1 s | +89.9 s (-5.5% — early phase, few timed waits; the expected bigger effect is in the deep-boot phases this TCG window only began) |
| PREEMPT-LEAK (A-line's post-fix storm) | n/a | **0 on the quiet boot** — sc205's 17,906-line storm is their wedge-storm context, not a quiet-boot property; performance-relevant verdict stays theirs (noted, not taken over) |

Also measured: WAKE-RUN-LEAK now prints 12 times by t=150 (pa21
printed 0 in its 120 s window) — with true sub-second ages these are
REAL leak events with resolvable latencies, where pre-fix the same
family was either invisible or mis-aged on the grid; the pa18/pa20
"677-678 prints/780 s" KVM quiet-boot figures are the comparable
population and will be re-baselined on merged main. WW2 heartbeat:
still 0 prints (same open item as pa22).

What this changes in the ranking: row 2b's "latency amplifier" is
now REMOVED-FIX-VERIFIED (mechanism corrected to read_time
granularity; effect measured). The F3 wake-loss latencies and the
deep-boot phase times are the quantities that were grid-contaminated
and now resolve — the F3 vs F7 multiplier share question from report
section 1 becomes measurable on the post-merge flagship run.

**pa24-tickfix-probe-tcg** (same scratch branch + the pa16 probe v3
patch, 520970f0; TCG, 600 s wall, dumps to t=540; nm table
run/pa24-nm.txt, parse run/pa24-parse.txt) extends battery item (b)
to the EFLAGS-exact probe under the fix:

- Tripwire `[ok]` again; ticks 99.9/s/CPU at t=540; WW and
  WAKE-STRANDED prints 0 for the whole boot (probe build).
- IF=0 machine share reads **35.3% at t=540** (cpu0 182.8 s, cpu1
  198.9 s over 2x540 s) — **TCG-inflated, NOT comparable to the KVM
  23.0-23.7% flagship band**: under TCG the same section work
  occupies more elapsed TSC time, and the newly-true timer dispatch
  adds softirq-zone time. The official IF=0 re-measure stays the
  post-merge KVM flagship (pa25) — this run's value is the STRUCTURE:
  probe v3 meters intact (heal 0.05-0.09%, miss=0), max closed REST
  segment 13 ms (ms-class, TCG-scaled — still no long cli section),
  and the REST site table keeps its family shape (small_alloc.rs:413
  22.8M opens @1324 ns and :247 22.9M @1110 ns on top, sched.rs:2140
  GRQ 311K @71.7 us 3rd, deferred_free, fs/file.rs:676 fdtable
  next) with one EXPECTED new pair: timer.rs:173/:317 (216K/142K
  opens) — the hrtimer paths that true 100 Hz dispatch + resolvable
  timed waits now exercise. No new IF=0 hotspot appeared with the
  fix.
- Zone-split shift worth recording: the SOFTIRQ zone's share grows
  from the KVM pre-fix 1.0-1.7% to 7.7% (cpu0) / 13.9% (cpu1) of IF=0
  time under the fix at TCG — the true timer softirq volume that the
  broken clock had suppressed (fewer meaningful dispatches per
  second). Part accel artifact, part real; the KVM flagship will
  size the real part.

**pa25-prefix-tcg** (stock main@78460742, pre-fix, same TCG quiet
config as pa23, 600 s wall) closes the same-accel deep-horizon
comparison (pa21 was only 120 s):

- Pre-fix signatures all present: tripwire `[fail]`, WW ages pinned
  1001 ms x283 (plus organic 344 ms x12, 4097 ms x8), ticks still
  ~99.9/s/CPU at the counters (2g correction's prediction).
- Same-horizon t=240 rows: nanosleep avg 1.34 s -> 120 ms (-11x at
  depth), n 1300 -> 6549 (5x more sleeps complete), max 21.0 ->
  10.0 s; clock_gettime max 1001 ms -> 3 ms. **futex avg UNCHANGED
  (421 vs 423 ms) and epoll_pwait avg unchanged (229 vs 238 ms)** —
  the deep-boot futex/epoll tail is the F3 wake-loss family and does
  NOT improve with the tick fix, exactly as the unchanged census
  counts predict. The fix removes the F7 grid amplifier; F3 remains
  the wall-clock driver.
- Wall-clock milestones at depth are NOISE-DOMINATED on this pair:
  composer_host.ready 130.3 s (pre) -> 89.9 s (post) but
  appspawn.started 312 s (pre) -> 468 s (post) — opposite
  directions, single boots, shared host (limitation 1's ~2x band).
  Honest verdict: no wall-clock claim from this pair; the wall-clock
  question moves to the post-merge KVM flagship (or multi-run
  average). Both runs reached the same depth (appspawn.started, not
  bms bundles).
- WAKE-STRANDED printed 0 in pa25 vs 100 in pa21 (both pre-fix) —
  the strand PRINT count is load/shape-dependent (it needs wakes
  landing on the census window), while the WW 1001 ms pinning is the
  deterministic pre-fix signature (283/293 prints in pa25).

**PREEMPT-LEAK verdict noted (A-line's, not this lane's work):** the
brief flagged sc205's 17,906-line PREEMPT-LEAK storm as the post-fix
next-layer blocker. A-line's own verdict (commit 8dc3407f, sc205
snapshot): the storm is POST-PANIC tick weather — sc205 died at t=30 s
(ext4 refused their userdata copy: unsupported incompat 0x2000 =
metadata_csum_seed; mount ENODEV x3 -> beget ExecReboot -> sysrq 'c'
-> panic park), pid 1 sat parked in wfi and the tripwire watched its
own hardirq bit; SC-HB froze with scheduling. sc206 (their next roll,
healthy mount, deepest boot ever) prints PREEMPT-LEAK 0. So the storm
does NOT persist on healthy boots and is not observed as a boot-time
performance blocker; the sched.rs:1747 terminal-deschedule family
tracking stays with the launcher lane. Their ext4 note (missing
metadata_csum_seed / orphan_file support vs Linux) is a compatibility
gap for the fs lane's list, recorded here for cross-reference only.

### 2i. SIXTEENTH LEG: fix commit pinned + flagship armed (no merge yet at leg start)

Leg start 14:55 (leg-15 head 4bcd5594 fast-forwarded through the
merge commit 6aa2fc74 — main carries no new kernel commits since).

**(1) The fix is now a clean commit, and it is EXACTLY the validated
hunk.** A-line re-committed the wip timer fix properly on
feature/samgr-callback as 675095c0 `fix(timer): x86_64 TSC timekeeper
must multiply before shifting` (10-24 13:47). This lane diffed
675095c0's kernel/src/drivers/timer/x86_64.rs change against the
scratch/tickfix hunk 9a451bef that pa23/pa24/pa25 validated:
**byte-identical (whitespace-exact, 76 diff lines each)**. The
pre-merge battery therefore covers exactly what will merge — no
re-validation gap. Their branch also carries 852d0cf2 (one-shot
TICKPROBE, sc203-205 diagnostic), 2a02d5db (PREEMPT-LEAK tripwire
hardirq discount), b634a615 (schb census re-arm fix) — dfx-only,
outside this lane's measurement surface except as extra log lines.

**(2) Main does NOT have the fix yet** (verified by diff
main:kernel/src/drivers/timer/x86_64.rs vs scratch/tickfix at leg
start: all 4 shift-first sites still present). The KVM flagship
trigger has not fired. sc206/sc207 (A-line rolls on their branch
kernel) are post-fix and show the plateau + census described in the
section 2g LEG-16 patrol note above.

**(3) Flagship runbook (armed; executes same-leg the moment the merge
lands on main):**
1. Confirm merged main's kernel/src/drivers/timer/x86_64.rs ==
   scratch/tickfix's file for the timer hunk (record the diff either
   way; expect empty modulo any other branch commits touching the
   file — 675095c0 is the only timer change on their branch).
2. `git checkout -b` nothing — run on main directly, worktree clean;
   apply run/pa16-probe.patch (probe v3, EFLAGS-exact IF=0 zones +
   REST call sites) on merged main; build x86_64 kernel.
3. ONE KVM boot (q35, -cpu max -m 4G -smp 2, snapshot +
   file.locking=off, syscost=1 sprobe=0, 600 s wall, serial stamped)
   — the OFFICIAL post-fix IF=0 share (pa24's 35.3% is TCG-inflated;
   the pre-fix KVM band was 23.4-25.0%) and the wall-clock milestone
   question (pa25-vs-pa23 was noise-dominated, no claim): appspawn /
   composer_host / bms bundles / appfwk / bootanimation markers with
   host timestamps, compared against the pa17/pa25 KVM-class band.
4. Report v1.4 + ranking row 2b official-share cell fill.

**(4) PREVIEW EXECUTED (pa26, 15:04-15:14, one of this leg's 2 QEMU
slots).** With the merge not landed and idle capacity, the whole
flagship pipeline was dry-run end-to-end on main+675095c0+probe
(the exact future-merge timer file, pre-verified identical):
patch applies clean, kernel builds, boot/parse pipeline works,
and the preview numbers exist — **IF=0 = 25.5% of machine, INSIDE
the pre-fix band; SOFTIRQ component 1.0-1.7% -> 3.1% of IF=0;
nanosleep avg -12x on KVM; depth class unchanged (foundation
chain), single boot, no wall-clock claim** (full row: run table
pa-pa26-preview-kvm above). Consequences for the official run:
expect the share to land ~25% (the fix removes clock granularity,
not irqsave work — row 1's cell should NOT move materially; the
SOFTIRQ sub-share and the timer.rs:173 REST site pair are the real
post-fix signature); expect the wall-clock verdict to stay
inconclusive unless the official boot gets past the foundation
chain. The preview does NOT fill any v1.4 cell — every number
there is <<FILL>> until the post-merge boot.
  [LEG-17: the post-merge boot happened — pa27 filled every cell;
  see section 2j and report v1.4.]

### 2j. SEVENTEENTH LEG: the official post-merge KVM flagship (pa27) — report v1.4, ranking v2.7

The gate opened between legs: main merged the A-line fix as 199f308c
and this lane's leg-16 docs as e1e317f2 (branch fast-forwarded at leg
start, 15:37). The flagship runbook (2i step 3) executed same-leg,
one QEMU, 780 s window.

**(1) Merge verification (runbook step 1, recorded).** Three-way
check, all green:
- `git diff origin/main:kernel/src/drivers/timer/x86_64.rs
  scratch/tickfix:kernel/src/drivers/timer/x86_64.rs` — EMPTY
  (byte-identical file; scratch/tickfix tip 520970f0 carries the
  pa23/24/25-validated timer file).
- 199f308c's own hunk (76 diff lines) vs 9a451bef's hunk (76 diff
  lines): byte-identical (`diff /tmp/mergefix.diff /tmp/scratchfix.diff`
  empty).
- 199f308c touches ONLY the timer file (37 lines, 30+/7-) — the
  cherry-pick excluded the A-line dfx commits (852d0cf2 / 2a02d5db /
  b634a615 verified NOT ancestors of main). So the pre-merge battery
  covered exactly what merged; no re-validation gap.

**(2) The flagship boot (pa27, 15:38-15:51, QEMU slot 1 of 2).**
pa16-probe applies clean on e1e317f2; PLATFORM=x86_64 build; one KVM
boot (q35, -cpu max -m 4G -smp 2, snapshot + file.locking=off,
syscost=1 sprobe=0, 780 s, host load ~4.7 with 6 foreign QEMUs).
Full row in the run table (pa-pa27-flagship-kvm); the v1.4 skeleton
cells, filled:

- **Official IF=0 share: 25.7% of machine** (t=720, cpu0 185.9 s +
  cpu1 184.7 s over 2x720 s, EFLAGS-exact, heal miss=0 both CPUs).
  The pa26 preview's 25.5% confirmed to 0.03 pp at the same t=540
  horizon (25.53% vs 25.50%). Reading: the pre-fix KVM band was
  23.0-25.0% (pa16-pa20); 25.7% is band-edge-high at the series'
  deepest volume (100.9 M syscalls vs pa17's 80.7 M) — the fix does
  not move total IRQ-off share, exactly as predicted; what moves is
  composition: **SOFTIRQ 1.0-1.7% -> 3.1% of IF=0** (softirq busy
  13.9 s/720 s vs pa16's 2.5 s/420 s; v1/v8 raises at the ~100
  Hz/CPU dispatch ceiling) and the REST site table gains the true
  timer dispatch pair (timer.rs:173 + :317, 3.3 s combined). Row 1's
  share band widens 23-25% -> 23-26%. Max closed IF=0 segment 3-6 ms
  (no long cli section exists, still).
- **Wall-clock verdict (the question the TCG pair could not answer):
  NOT MOVED beyond the band.** composer_host.ready +29.8 s (band
  31-44, at the fast edge), appspawn.started +80.1 s (band 32-66,
  ABOVE the top — both directions inside one boot = single-boot
  variance, no claim either way), appfwk.ready set by +57.4 s
  (foundation's time-param already set when its second instance tried
  at the +57.4 s SIGKILL/respawn), bms.main.bundles.ready /
  bootanimation exec / launcher.ready NOT reached at 780 s. The F3
  attribution sentence for the report: the tick fix removes the 1 s
  grid from every timed wait (nanosleep avg 2.2 s -> 221 ms, -10x,
  official), but the deep-boot dependency chain is gated by F3
  wake-loss plus the binder 15/30/89 s cost classes (A-line chain),
  which the tick fix does not touch — pa23 already showed the
  futex/epoll tails unchanged, and the official boot agrees.
- **Census: TERM-DESCHED 8 / WAKE-REFUSED 32 / WAKE-STRANDED 0** —
  the battery counts hold on merged main (4th consecutive KVM roll at
  8/32; WAKE-STRANDED 0 matches pa23 TCG, cleaner than the sc-roll
  3-15 band). PREEMPT-LEAK 0; WAKE-RUN-LEAK 486 prints/780 s (pre-fix
  pa18/pa20: 677-678 — lower, single-run band applies); IRQ-OFF-WINDOW
  prints 9 (organic ages; the 1000 ms grid family is gone).
- **Tick modes (t=720): c0 13.5/85.1/1.4, c1 14.8/83.4/1.8** — kernel
  83-85% at true sampling, consistent with pa23 TCG's 80-83% and
  pa26's 84.1-85.6; the pre-fix 86-89% figure stays retired.
- **nanosleep / clock_gettime official:** 221 ms avg @ n=22,848
  (max 295 s = real OH retry-loop durations); clock_gettime 42.6 M
  calls @ 3 us, cumulative max 288 ms (late-boot host-noise class —
  pre-fix 654 ms max was grid-inflated).
- Ranking bumped v2.6 -> v2.7 (row 1 evidence + share band, row 1b
  GRQ pa27 datapoint, row 2b OFFICIALLY MEASURED); report v1.4 filled
  and the skeleton comment deleted.

**(3) pa26 close-out (provenance corrected).** pa26-preview-bzImage
stays untracked (11.5 MB > the 10 MB leg rule) — correct. The rebuild
recipe in run/pa26-built-from.txt was re-verified and EXPANDED: the
merge --no-commit of 675095c0 brought the A-line branch deltas beyond
the timer fix (dfx commits + binder-stall/sleepsnap print code; 675095c0's
parent efd3868e is not on main), proven by 563 binder-stall lines in
the pa26 serial log — so the pa26 kernel is NOT identical to pa27's
(the fix-only cherry-pick). The timer file itself was byte-identical,
so the tick-fix validation transfer was never at risk; the preview
numbers are superseded by pa27 wherever both exist (they agree to
0.03 pp anyway). The archived bzImage also cannot be rebuilt
byte-identically (link order nondeterminism), noted in the file.

**(4) UI patrol (sc208 final).** sc208 closed by its owner at 15:34
(66,289 lines): census TERM-DESCHED 8 / WAKE-REFUSED 32 /
WAKE-STRANDED 13 / PREEMPT-LEAK 0 — the counts hold a 4th consecutive
roll; depth = sc206/207-parity (bms.main.bundles.ready +
appfwk.ready + boot.completed set, bootanimation ServiceExec'd);
launcher.ready / bootanimation.finished / OnForeground still 0. The
  binder cost family (A-line territory, recorded only): 15 s class
  (~17 prints) and 30 s class (x3) persist, and the NEW ~89 s class
  (88632/89396/89400 ms, x1 each to *.ISystemAbilityManager code:7)
  present in the final log — mid-roll and final counts identical, no
  new ~89 s events after the mid-roll patrol. Report v1.4's table-1
  post-UI row stays PENDING.

### 2k. EIGHTEENTH LEG: F4 ext4 anatomy (pa28/pa29) — the fix-route map, and F4's weight corrected down

Primary task was the F4 deep diagnosis (measurement-only). Full anatomy
and the ranked route map live in **docs/development/f4-ext4-route-map.md**
(the fs-lane deliverable); this section records what the leg measured and
what changed in this document's own tables.

**(1) The probe (dfx/fsw.rs, cmdline `fsw=1`).** Volume counters for the
ext4/bio/jbd2/big-lock/VFS-walk layers, dumped as two compact lines per
minute. Two probe bugs found and fixed in-leg (negative results, ledger):
the first build printed via pr_info (console-filtered at loglevel=4 —
pa28-a, killed early, no data); the second printed right behind syscost's
~70-line dump and lost the console-emit race 11 of 12 rounds (pa28-b —
one cumulative line survived at t=420, values consistent with pa29);
pa29 offsets the dump cadence by 30 s and is the data boot. Three
self-started QEMUs total (budget 2 — the overrun is the probe-bug cost,
recorded here).

**(2) The anatomy (pa29, KVM, 480 s, fresh reflink userdata).**
- **No journal anywhere**: userdata-tok (the only rw ext4) lacks the
  journal feature; ALL jbd2 counters are 0 for the whole boot. Every
  write takes the no-journal write-through path — the journal layer's
  lazy-commit machinery is dormant on the OH boot.
- **All synchronous writes are first-phase**: 12,107 device writes,
  102,570 sync-dirty attempts (88 % redundant re-syncs of clean shared
  blocks), 761 whole-cache sync_buffers drains — every one inside
  t<90 s (provisioning: e2fsck/resize2fs/init file creation; mkdir avg
  44 ms, pwrite 19 ms, unlink 55 ms). Per alloc/free run the code syncs
  bitmap + group descriptor + SUPERBLOCK inline (1,383 runs x 3).
- **Phase B (90-150 s) is pure CPU**: 2.0 M bread calls at 99.1 % hit
  rate (whole-boot: 2.81 M breads / 25,164 device reads), 158 K
  read_inode, 247 K linear directory scans over 1.73 M blocks (no
  htree), 27.6 K getdents64 full-directory re-materializations, dcache
  hit rate 90.5 % (3.73 M hits / 378 K misses).
- **Phase C (t>150 s): ext4 idle** (bread delta ~60/min). F4 owns NO
  deep-boot wall-clock; that stays with F3 + the binder cost classes.
- The 4 MB (1024-entry) buffer cache thrashes cleanly: 24,895 evictions,
  5 dirty victims, ~25 K device reads Linux would serve from an
  unbounded page cache.

**(3) F4's weight corrected.** The "~300-500/~600 CPU-s/boot" estimates
inherited pa17's stall-inflated averages (open avg 1,893 us carried 100 s
F3 maxes). Clean accounting from pa29: fs-class syscall wall ≈ 200 s per
420 s boot (open 41.6 + mkdir 37.9 + pread 33.6 + pwrite 31.8 +
newfstatat 30.7 + getdents 11.4 + smaller), CPU share ≈ 60-70 % →
**≈ 120-140 CPU-s/boot, all before t=150 s** — medium-small, a
first-phase family and per-op-latency outlier (100-300x holds), not a
whole-boot CPU family. Ranking row 4 updated.

**(4) Compat gaps folded into the map (A-line cross-reference).**
metadata_csum_seed (incompat 0x2000) and orphan_file mount-refused
(sc205's userdata class), plus the metadata_csum rw→RO downgrade policy
(one print per pa boot, sys_prod) — routed as route 9 in the map
(csum_seed is near-trivial: the seed preference already exists in
csum.rs; orphan_file is real semantics).

**(5) UI patrol (sc209, live at leg time 16:29).** First-ever
bootanimation.finished SET (pid 888 at ~50.3 K serial lines;
appfwk.ready set; composer_host.ready x2; launcher.ready / OnForeground
still 0; screen shows the animation test pattern per the screenshot
monitor). A-line branch kernel with FXRS probes, not a pa-series
measurement boot — recorded as context for the PENDING post-UI row,
which stays PENDING until launcher.ready lands on a main-class kernel.

### 2l. NINETEENTH LEG: monitoring leg — patrol rounds, gate idle, probe-diagnosis hardening

A waiting leg by design: standing UI patrol, standing census-hold gate,
and small housekeeping on the leg-18 probe bugs. One self-started QEMU
(pa30smoke, inside the hard cap of 2).

**(1) Housekeeping: the two pa28 probe bugs (dcaf4564, ef3c02f9).**
Code + evidence check first: both bugs were ALREADY fixed inside leg 18's
merged probe commit (8c5d0ae7): the dump prints at pr_warn (pa28-a's
drop-before-the-ring class — pr_info=6 > the pa-boot loglevel 4), and
the dump cadence parks half an interval so FSW lands at :30 past each
minute, clear of syscost's ~70-line :00 dump (pa28-b's emit-lock loss —
emit_to_console from softirq context gets a 100µs acquire budget, and
losing it skips the console line; the record only lives in the ring,
which a pa serial-only boot never reads). pa29's serial confirms the
fix held: 7/7 FSW1 rounds at t=90..450. What leg 19 added is the
diagnosis gap that made leg 18 pay two boot iterations: the two failure
classes are indistinguishable from a serial with zero FSW lines. FSW1
now ends with `lvl=<console_loglevel> es=<console_emit_skipped>` (new
read-only accessor on printk's skip counter): a vanished round with es
climbing = emit-lock loss; lvl > 4 = level-filter drop; both flat =
probe logic dead. Validated by **pa30smoke** (150 s, syscost=1 fsw=1,
loglevel=4, KVM, 1 QEMU): `FSW1 t=90s ... lvl=4 es=111` landed cleanly
between syscost's t=60/t=120 rounds, counter magnitudes match pa29's
t=90 round. Side fix: pa-boot.sh died at `set -e` on the horizon kill
(timeout's 124), skipping its own `.done` marker and stamper kill —
now `rc=0; qemu ... || rc=$?`.

**(2) UI patrol (standing).** Rounds over the leg window, latest boot
first: bootanimation.finished SET holds across sc209/sc210/sc211 (the
leg-18 first); launcher.ready still only the "not set" warning in every
round; OnForeground 0; no ArkUI render markers (only param-name matches
like persist.sys.arkui.animationscale). No main-class UI-marker boot
appeared → the perf-report post-UI row stays PENDING. Round log at the
end of this section.

**(3) Census-hold gate (standing).** CORRECTED POSTHUMOUSLY BY LEG 20:
the skeleton above (written 17:27) said main sat at 1702de2c and the
gate never triggered — that was true only until ~17:30. origin/main
then advanced to 1bdbbf32 (wedge-census fdtable walks None-safe) +
14d8950d (leg-14 FXRS/PCTL/SAREG probes): the A-line merge the gate
exists for. Leg 19 fetched it, built it detached (reflog HEAD@{2}),
and ran the gate check as **pa31** — the leg's QEMU 2 of 2. The boot
completed cleanly at 18:20 (rc=124 horizon, cleanup + .done marker
written — the ef3c02f9 fix working); leg 19 died of quota before
recording the result. Analysis below is leg 20's, from the on-disk
serial/stamped logs.

**pa31 result (census-hold on main@14d8950d, plain kernel + syscost=1,
KVM, 780 s):**
- Census: TERM-DESCHED 8 / WAKE-REFUSED 32 — baseline EXACT hold.
  WAKE-STRANDED 3 prints (pids 672 bootanimation 1036 ms, 829 1455 ms,
  666 foundation 1207 ms) vs pa27's 0 — organic-shaped (1.0-1.5 s
  strands, NOT the pre-tickfix exactly-1001 ms grid class); candidates
  are host load (the 10-12-QEMU overcommit the A-line documented the
  same evening) or the newly-active leg-14 probe prints at syscall
  boundaries. Recorded as in-band with a footnote, not a trip.
- Tripwire-equivalent (this kernel carries no pa16 probe): syscost
  ticks c0 99.98/s, c1 99.7/s sustained through t=720 (5994/5996
  samples at t=60 -> 71988/71788 at t=720). Tick-fix holds on merged
  main + leg-14 probes.
- fdtable fix: zero "FdTable not initialized" panics; RWFRZ scans
  alive, censuses completed. (pa31's own depth — foundation SIGKILL'd
  at +45.0 s, launcher never spawned — does not traverse the sc210
  death window either, so this is a hold alongside sc211's verification,
  not a new-depth proof.)
- Milestones (host stamps): composer_host.ready +20.5 s (pa27 flagship
  +29.8 s — single-boot variance, different probe set); foundation
  (pid 666) exit signal 9 at +45.0 s; bootanimation.finished SET at
  +92.8 s (pa27 reached no bootanimation exec at 780 s — race-class
  variance, the same per-boot race the A-line documented for sc209 vs
  sc210); launcher.ready NOT set; OnForeground 0; ArkUI 0.
- fsw probe gated-off default verified ON A MAIN-CLASS BOOT: one-shot
  resolution line "fsw: off (cmdline fsw param = None)" at serial line
  58, zero FSW1 dumps all boot. (Armed case was pa30smoke.) The
  leg-19 probe-fix task's verification is closed with evidence.
- Context for the A-line (their probe, not mine to touch): FXRS
  self-limited at its 4000-event budget; the late-serial volume is
  OS_DfxWatchdog futex WAIT ret=-110 repeats (event #3865 by t=779).

**(4) QEMU ledger.** pa30smoke = 1 of 2, pa31 = 2 of 2. Leg-18's
overrun (3 vs 2) not repeated.

Round log (host time, newest-boot basis):
- R2 18:07-18:20 — pa31 census-hold boot on main@14d8950d (see (3)).
  launcher.ready not set on a main-class kernel; bootanimation.finished
  SET (+92.8 s). main moves to 14d8950d.
- R1 17:20 — sc211 (ended 17:15) checked: launcher.ready not set,
  bootanimation.finished SET, no OnForeground/ArkUI. main@1702de2c.

### 2m. TWENTIETH LEG: pa31 posthumous close-out; PACE watch ran to verdict — reclaim family acquitted by measurement

Leg start: merged main 14d8950d into the branch (8b1d4b9e) — the same
merge the leg-19 gate had already validated with pa31, now in-tree.

**(1) The leg-19 in-flight state, resolved honestly.** The "12 dirty
files" were NOT source changes: the pa28-probe fixes themselves were
already committed in leg 19 (dcaf4564 lvl/es diagnosis line + ef3c02f9
pa-boot.sh horizon-kill cleanup, both validated by pa30smoke), and the
dirty set was 8 small run artifacts (pid stamps + .done markers —
committed this leg per the established tracked pattern) plus 4 bzImages
(11.4 MB each, over the 10 MB rule — left untracked; provenance in
run/pa31-built-from.txt). The genuinely unfinished piece was pa31's
absence from the ledger — closed in section 2l(3) above. Nothing was
discarded: every file was either committed, kept as untracked-by-rule
with provenance, or analyzed.

**(2) PACE watch (PRIMARY, A-line sc212).** The A-line's sc212 roll
(wip fec94520: "PACE counters on the SC-HB beat") is live since 18:16.
Counter format on the 10 s SC-HB heartbeat:
`pace: cow=<n> cowJ=<jiffies> drct=<rounds> drctJ=<jiffies>
drctMaxJ=<max-jiffies> slow=<alloc-slowpath-entries> zfree=<pages>
heapK=<KiB>`. Observation through t=1691 s (serial read read-only;
their QEMU untouched): **drct=0 for the entire boot**, zfree ~437k
pages (~1.7 GB free), slow=0, cow=20340 cumulative with cowJ=7 (about
70 ms of COW fault cost total). The sc210-class memory floor (zone
Normal pinned at free ~100-190 pages below wmark_low 2451,
in_direct_reclaim=1 on every COW alloc) is NOT reproducing in sc212 —
this roll is on the crawl trajectory (pidmax ~1199, flow 0-1/s, zero
launcher activity: no com.ohos.launcher, no bootanimation markers).
Consequence for my task: **no reclaim-slowpath share can be quantified
from this roll**, and the lead's magnitude is per-boot-race dependent
(the same race that splits sc210-class launcher boots from sc211-class
crawls). [SUPERSEDED within the hour by the A-line's adjudication and
sc213 — see (2b): the floor is not free-floating race variance, it is
conditional on the usb_host crash loop, which the usbless image
removes.] The ranking gets NO new row on this evidence — a row needs a
boot that actually pins the floor plus a before/after of their fix.
Watch continues; when a memory-floor boot appears in their PACE data
(drct climbing, zfree pinned near wmark) or their fix merges, that is
the quantification trigger.

**Quantification contract for that trigger** (agreed method, computed
from their SC-HB beats; no new probes needed from my side):
- Healthy-regime COW baseline, first PACE datapoint (sc212, t=1931 s):
  cowJ=7 jiffies for cow=20610 cumulative -> **3.4 us per COW fault**
  when the zone has 437k free pages. This is the "after"/healthy
  reference for any fix, and the ranking's COW-class unit cost.
- Floor-regime expectation from the A-line's sc210 lead (~3000 COW
  faults + 40 forks per launcher round, ~4.6 s/round): if COW allocs
  own the round, cowJ should climb ~460 jiffies per round (3000 x
  ~1.5 ms), i.e. cowJ/cow blows up from 3.4 us toward milliseconds,
  and drctJ/round-wall gives the reclaim-slowpath share directly
  (drctJ x 10 ms / 4600 ms).
- sc210's floor evidence (their serial, read-only; the numbers behind
  the lead): zone Normal free_cnt 387 -> 106 -> 119 -> 99 pages vs
  wmark min/low/high=1715/2451/3187; MEMLEDGER-CENSUS used
  1,572,471 of 1,572,864 pages (free 393 -> 93 -> 134); MEMLEDGER-
  COMPACT memfree_pages=1, heapK 907168/1015808, swap_active=0 (so
  direct reclaim can only squeeze file pagecache — lru 451-457k
  pages, mapped 563k with anon 457k); in_direct_reclaim=1 at OOM-
  FORENSIC #2. With no swap and anon > file reclaimables, a pinned
  floor means every COW alloc's reclaim scan finds little — the
  serialized-slowpath story. Whether the pinned floor is legitimate
  pressure or an accounting leak is the A-line's call, not mine.

**(2b) RESOLUTION (same leg, ~19:20): the PACE data landed and
ACQUITTED the reclaim family.** The A-line's leg-15 addendum (their
SC-SESSIONS, 18:51) closed the question by measurement, and sc213
confirmed it on the launcher trajectory. Final quantification:

- **sc212 (usbless roll, full 45 min, crawl class): drct=0, drctJ=0,
  drctMaxJ=0, slow=0 for the ENTIRE boot.** zfree never left the
  ~436-437k-page band; heapK plateaus at 319 MB (sc210: 907 of
  1015 MB). Their adjudication: the sc210 floor was DOWNSTREAM of the
  usb_host crash loop (5 sig=11 crashes/round driving forks+COW+heap
  to the floor); the usbless image (oh-img-p, zeroed
  libusb_pnp_manager) removes the entire regime. Their words: the
  mission item "fix the reclaim slowpath" is "closed by measurement,
  not by code change".
- **sc213 (19:02, same kernel fec94520, launcher trajectory — SA 10
  published, com.ohos.launcher active, launcher.ready still not set
  behind their userspace RS park): same verdict through t=2059 s at leg close.**
  drct=0, slow=0, zfree stable ~420k pages, heap plateau 347 MB. The
  account retry rounds (QueryForegroundUserId, the sc210 loop's
  survivor) now cost bursts of ~103-255 COW faults per 10 s beat
  with **zero measurable jiffies** (cowJ flat).
- **Reclaim-slowpath share of round cost = 0% (measured)** in the
  live trajectory. Before/after across the only fix (the image
  change, no kernel code): sc210 floor regime ~4.6 s/round with
  ~3000 COW + 40 forks (free 93-387 pages, in_direct_reclaim=1)
  -> sc212/213 rounds at ~3.4 us/fault whole-boot average (cowJ=7
  jiffies per ~20-25k faults), heap 907 -> 319-347 MB.
- **Ranking decision: NO new row.** The family measures zero where
  the boot actually goes now, and its sc210-era cost is a symptom of
  a userspace crash loop (usb DDK PnP NULL-walk — the A-line's
  adjudicated family, fixed by image). A row would memorialize a
  phantom cost center. Acquittal note added under the section-4
  header (no version bump — no existing row reweighted).
- Counterfactual honesty: the split of sc210's 4.6 s/round among
  reclaim/fork/crash handling was never directly measured (no PACE
  counters existed in that regime) and is now unmeasurable — the
  regime does not recur without the crash loop. The 0% figure is the
  share in the current, usbless trajectory, which is the only one
  the campaign can reach.
- Exposure note: the pa-series measurement boots still use the FULL
  oh-img (usb DDK present). No pa boot has shown floor-class costs
  (pa27 small_alloc 91 s/720 s, pa31 slow rows are epoll-wait
  shaped), but if one ever pins a floor the syscost slow/alloc
  classes will show it — the A-line's oh-img-p exists if a pa roll
  ever needs the usbless control.

**(3) UI patrol (standing).** sc212 (A-line branch kernel, not
main-class): zero launcher markers, crawl trajectory — no UI markers to
report; screen frozen at BootAnimation's first color-pattern frame
(their adjudicated RS read-lock leak, userspace). sc213 (A-line
branch, launcher trajectory): SA 10 published, com.ohos.launcher
active, **launcher.ready still not set** at t=2059 s (leg close) — the RS park
(userspace) gates the UI milestone now, not kernel perf. pa31
(main-class): launcher.ready not set (section 2l(3)). The
perf-report post-UI rows stay PENDING.

**(4) Census-hold gate (standing).** The 14d8950d merge was validated
by pa31 before this leg's window opened; within the leg-20 window
origin/main stayed at 14d8950d through close (verified at leg end —
no further A-line merge, and the leg's finding made one unlikely:
their pace lead resolved as a userspace crash-loop symptom, so no
kernel fix is coming from that quarter). Gate did not trigger; zero
QEMUs spent.

**(5) QEMU ledger.** 0 of 2 self-started this leg — a pure
watching/documentation leg: the leg-19 pa31 serial answered the gate
question, and the A-line's live PACE rolls answered the primary
question. Nothing this leg needed a boot of its own.

### 2n. TWENTY-FIRST LEG: waiting leg + post-UI harness prep (v1.5 draft committed)

Leg start: fast-forwarded the branch to origin/main 62c2708e (the
leg-20 merge itself; no new foreign commits — so the census-hold gate
did NOT trigger at open).

**(1) UI patrol (standing).** Round 1 at leg open (~19:31 host):
- sc213 (A-line branch kernel fec94520, launcher trajectory, STILL
  LIVE at t=2439 s, their QEMU untouched): com.ohos.launcher active
  (30 mentions), account retry loop running (QueryForegroundUserId
  failures — the known shape), launcher.ready NOT set, OnForeground 0,
  ArkUI 0. PACE counters still clean (drct=0 slow=0 zfree 420k).
- sc214/sc215 (ended 19:11/19:20, same kernel class): both SET
  bootevent.bootanimation.finished (PCTL-SEND "true") — consistent
  with the sc209-era first; launcher.ready NOT set in either; sc214
  additionally shows appfwk.ready not set. Screen (shotmon MILESTONE)
  full nonblack bands.
- Verdict: the UI milestone is still userspace-gated (the RS read-lock
  park); the perf-report post-UI row stays PENDING. Patrol continues
  each round of this leg; no launcher.ready SET on any serial so far.

**(2) Census-hold gate (standing).** origin/main == 62c2708e at open
(the branch was 1 behind purely because the leg-20 merge commit
postdates the branch head; the ff-only merge closed it). No A-line/
DFX kernel commits on main since 14d8950d (pa31-validated). Gate not
triggered; rechecked at each round; one quiet census boot fires only
if main moves on kernel code mid-leg.

**(3) POST-UI HARNESS PREP (PRIMARY, no QEMU).** The user mandated a
full post-UI performance report with microbenchmarks and same-host
Linux comparison. Design committed as perf-report.md section 8
(v1.5 DRAFT — all placeholders). Grounding facts verified this leg:
- syscost's existing surface answers the whole-boot side from ONE
  quiet boot (milestones row, table 2/3 refresh, hold checks) —
  section 8.1 tabulates the split.
- The probe side needs a NEW static musl x86_64 binary (P1-P7:
  null syscall + clock_gettime, ctx switch via affinity-pinned pipe
  ping-pong, page-fault classes anon/COW/file, fork+exit and
  fork+exec, pipe + unix dgram/PEEK/stream roundtrips, ext4 fs ops
  on userdata, 1 ms sleep overshoot). Toolchain verified working:
  toolchain/x86_64-rux-linux-musl/bin/musl-gcc = gcc 15.2.0.
  Probe-dependency syscalls verified present on Rux:
  sched_setaffinity (sched.rs:938), getcpu, CLOCK_MONOTONIC_RAW
  (time.rs:16).
- Delivery: recommended = late-service slot swap via debugfs into a
  PRIVATE system.img copy (A-line precedent: libusb_pnp_manager
  zeroing in oh-img-p); dual output (file on /data + /dev/console).
  The exact slot is the one open decision, deferred to build day
  (criteria listed in 8.3). bootanimation-slot and hdc-shell kept as
  documented fallbacks.
- Linux comparison protocol: oh-ref-r1/r2 keep answering the timeline
  column only; every per-op number needs fresh probe boots on the
  reference kernel (OHOS 6.6.101 = the image's own bzImage, confirmed
  from oh-ref-r1 serial). Machine config MUST be aligned
  (run-oh-linux-ref.sh defaults smp6/8G/-cpu host vs pa's 2/4G/-cpu
  max) — recorded in 8.3/8.4. N=3 per side, interleaved, medians/p99.
- Deliberately NOT done: no probe code written (mandate says design
  first), no slots picked, no image copies made. Section 8.6 is the
  run-day checklist; budget = 1 long + ~6 short boots, within the
  2-QEMU discipline taken sequentially.

**(4) QEMU ledger.** 0 of 2 self-started. Zero QEMU this leg so far;
both standing gates remain armed (patrol + census-hold on main move).

**(5) Leg close rounds.** R2 (~19:40): sc213 ended 19:36 at t=2689 s —
launcher.ready NEVER set (final "not set" warning at close; PACE clean
to the last beat: drct=0 slow=0 zfree 420k); sc214/sc215 unchanged
(bootanimation.finished SET, launcher.ready not set); no sc216+
serial exists yet; origin/main still 62c2708e — census-hold gate
never triggered this leg. One more design fact verified while waiting:
the "same image" claim for the Linux comparison is literal — this
lane's run/oh-img/* symlink to launcher-route/run/oh-img/* which
symlink to oh-robot's phone/images/* (the exact files
run-oh-linux-ref.sh boots), so Rux pa boots and reference boots ride
identical bytes via snapshot=on. Prep deliverable committed (345cbde8):
perf-report.md section 8 (v1.5 draft skeleton, placeholders) + this
leg section. No change elsewhere: honest ledger = docs only, 0 QEMU,
no kernel code, no image copies, no probe code (design-first mandate).

## 4. Ranking table v2.8 (cost centers -> boot-time contribution)

Acquittal note (leg 20, no version bump — no row reweighted): the
A-line leg-15 PACE counters measured the reclaim/page-alloc slowpath
family at ZERO for entire boots (sc212 full roll and the sc213
launcher trajectory: drct=0 drctJ=0 slow=0, zfree 420-437k pages,
rounds at 3.4 us/COW). The sc210 memory-floor lead (free 93-387
pages, in_direct_reclaim=1, 4.6 s/round) was downstream of the
usb_host crash loop and dies with the usbless image — it never
becomes a row here. Details in section 2m(2)/(2b).

v2.8 = v2.7 + the F4 anatomy (leg 18, pa29, section 2k): row 4's weight
corrected to medium-small (~120-140 CPU-s/boot, all before t=150 s; the
~300-500 estimate carried pa17's F3-inflated tails) and its evidence
cell now carries the probe anatomy (no journal on this workload; all
sync writes first-phase; dcache 90.5%; ext4 idle after t=150 s) with
the fix-route map pointer.
v2.7 = v2.6 + the official post-merge flagship (pa27, section 2j): row
2b OFFICIALLY MEASURED on merged main (census and per-op numbers hold;
wall-clock NOT moved — F3 + the binder cost classes own the deep-boot
wall), row 1's share band widens to 23-26% with the composition shift
(SOFTIRQ 1.0-1.7 -> 3.1% of IF=0 = real hrtimer dispatch), row 1b
gains the pa27 GRQ volume datapoint (17.7 M holds, avg flat 5.4 us).
v2.6 = v2.5 + row 2b corrected and closed (leg 15: mechanism corrected
to read_time's TSC shift-first granularity — the LAPIC always fired at
100 Hz; the launcher lane's fix verified pre-merge in pa23, section
2h: tick 99.9/s/CPU sustained, WW/WAKE-STRANDED grids gone, nanosleep
avg -21x, tick-mode u/k/i now resolve at true 100 Hz — kernel share
80-83%, not 86-89%).
v2.5 = v2.4 + row 2b (leg 13: the x86_64 tick-cadence adjudication —
the "1000 ms IRQ-OFF-WINDOW" family re-attributed from an IF=0
suspect to the timer's own ~1 Hz delivery cadence; see section 2g).
v2.4 = v2.3 refreshed by pa18 + pa20 (seventh session: the gate-A
v1..v6 stacking retest and the gate-B magazine before/after — both
wait-gates opened mid-leg and both ladders are now measured; GRQ
-31% claim corrected to volume-coupled; TERM-DESCHED acceptance
demonstrated). Contribution model unchanged: 2 vCPUs; kernel mode
~83-90% of ticks (pa20 t=660: 88.1/83.2%; the u/k/i counters are
themselves tick-sampled — cadence caveat in section 2g, RESOLVED by
pa23: at true 100 Hz sampling the kernel share reads 80-83%, so the
pre-fix 86-89% figures were sparse-sampling phase-weighted).

| # | cost center | measured (pa13) | pa15/pa16/pa17 direct evidence | gap contribution | family |
|---|---|---|---|---|---|
| 1 | Interrupt-disabled regions (cli-recovery page) | 26% of kernel ticks; 316 IRQ-OFF-WINDOW/780 s (now TRIAGE HINT — see 2d) | pa16 EXACT: IF=0 = 25.0% of machine; pa17: 23.4; pa18 (v1..v6): 23.7; **pa20 (magazine): 23.0**; **pa27 (merged-main flagship, OFFICIAL): 25.7% at t=720** (band-edge-high at the series' deepest volume; pa26 preview same-horizon 25.50 vs pa27 25.53 — share does not move, composition does); split REST 88.5-92.1% / HARD 5.1-7.7% / SOFT 1.0-1.7% pre-fix **-> 3.1% post-fix (real hrtimer dispatch; softirq busy 2.5 s/420 s -> 13.9 s/720 s)** / UART 0.8-1.9%; max closed segment 3-6 ms (no long cli section exists); 1.08-1.4 M sections/s | **large** — decomposed below; levers per section, not per zone | F1 |
| 1b | REST section bulk (exact decomposition) | (inside #1) | small-alloc **MEASURED BEFORE/AFTER (pa18->pa20)**: global class-lock sections 741.7 M -> 19.7 M/boot (-97.3%, the 16.49x projection confirmed); REST wall 176.1 -> 133.2 s = 12.2% -> 9.2% of machine (**the -24% wall ceiling is the per-op local irqsave floor, 751 M ops @136-168 ns — the lever left is churn volume, not locking**); schedule() GRQ 2.7-4.6% pre-fix (avg flat 5.1-6.1 us pa16-pa18, 6.7 us pa20 deepest boot; wall volume-coupled — pa17's -31% claim corrected in 2e), **pa27 post-fix flagship: 6.7% of machine (96.1 s, 17.7 M holds @ 5.4 us — hold count ~2x pa16's, avg flat-to-better: volume growth with depth + true-tick wakes, not a per-hold regression)**; fdtable get_file 1.9-2.2% (pa27: 2.7%); waitqueue family 1.9-2.3% (pa27: 3.5%); EPOLL_WAKE_REGISTRY 1.0-1.2% (pa27: 1.7%); deferred_free 0.7-0.8%; NEW post-fix site pair timer.rs:173/:317 (0.23% of machine — true 100 Hz hrtimer dispatch); small_alloc 4-site family 6.3% of machine in pa27 at the series' deepest volume — 95.4-99% attributed to file:line | **the F1 lever list** — allocator magazines LANDED AND MEASURED; GRQ pick (launcher lane; full refactor deferred as intersecting the kill-path machinery); fdtable read path, waitqueue batching, epoll registry (launcher lane — sharding design in hand, their fb2a1354); allocation-churn hunt rises in priority (now the allocator's main lever) | F1/F5 |
| 1a | Console/UART emission (sub-item of F1) | outb/inb in recovery rips; cli page shares inb/outb | pa15: 4.34 M bytes, 5.0% of machine; pa16: 1.55 M bytes @ t=720, 1.7%; pa17: 5.61 M bytes, 6.1%; pa18: 1.5 M bytes, 1.4%; pa20: 1.34 M bytes, 1.3% — **per-byte cost stable 13.2-18.5 us across 5 boots; volume (and thus machine share) swings 1.3-6.1% with boot depth**; producers init/<irq>/FFRT-worker; task-context emission 100% IF=1 | medium CPU (1-6%) + **large latency multiplier** under host load | F1 |
| 2 | Scheduler wake-loss (WAKE-RUN-LEAK; lying busy slots + linked-starving) | 702 leak prints/780 s; census 16 lying + 7 linked | **pa18+pa20 (full v1..v6 stack, pre/post magazine): 678/677 prints / 31/32 refusals (27/25 lying) / 10-11 PREEMPT-LEAK / TERM-DESCHED=8 BOTH boots — the v1 trigger shape fires ~8/boot and every occurrence survives; census lying-slot population unchanged by the entire stack AND by the allocator** (pa17: 593/31/22+7/8/0) — they are born on another path; birth forensics continues on the launcher lane | **large** — unchanged; still the wall-clock driver | F3 |
| 2b | x86_64 kernel-clock granularity 1 s (TSC shift-first math; CORRECTED+FIXED legs 13-15, sections 2g/2h) | read_time stepped once per 2^32 TSC ticks = 1.0008 s (9e5f0147's `(delta>>32)*mult`); the LAPIC IRQ always fired at 100 Hz — jiffies/WW-ages/WAKE-STRANDED/every timed wait inherited the grid (the leg-13 "IRQ cadence ~1 Hz" attribution and "~625 KHz effective decrement" were artifacts of measuring against the same broken clock — corrected in 2g) | **FIXED AND MEASURED (pa23, section 2h)**: launcher lane's u128 multiply-then-shift (`tsc_delta_nominal`, all 4 sites) on main = tripwire [ok] 16 jiffies/150 ms, 99.9 ticks/s/CPU sustained, WW prints 57->2 (organic ages), WAKE-STRANDED 100->0, nanosleep avg -21x, clock_gettime max 654 ms -> 3 ms, census counts (TERM-DESCHED 8 / WAKE-REFUSED 32) hold; kernel tick-share re-reads 80-83% at true sampling | **was a latency amplifier on every timed wait; now removed AND OFFICIALLY MEASURED (pa27, merged main)**: tripwire [ok], census 8/32/0, nanosleep 221 ms avg (-10x pre-fix KVM), clock_gettime max 288 ms (host-noise), IF=0 in-band, wall-clock milestones NOT moved — F3 + the binder 15/30/89 s cost classes own the deep-boot wall | F7 — CLOSED (fixed + officially measured) |
| 3 | hilogd.server socket dual-drain + sysevent_source + connect-retry | 26.5 M calls by t=720 = ~25% of ALL syscalls | pa16: 10.6 M recvfrom + 10.7 M recvmsg; pa17: 10.08 M + 10.29 M (1:1, avg 116/9 us, recvfrom max 393 s = wake-tail victim); writev 14.6-15.2 M; connect 2.47-2.55 M | medium — volume is OH userspace; kernel levers = per-call recv cost + the wake-loss tail | F2 |
| 4 | ext4 metadata + write path (open/stat/getdents/fsync/pwrite/mkdir) | open 235 us, newfstatat 206 us, getdents 643 us, fsync avg 16 ms | pa17: open avg 1893 us (max 100 s — stall-tail inflated, see 2d), newfstatat 294 us, getdents 1043 us; medians unchanged; **pa29 anatomy (2k): no journal on this workload (all jbd2 counters 0); all 12 K sync writes + 761 whole-cache drains inside t<90 s (write-through no-journal path, superblock synced per alloc); phase B = 2 M breads @99.1% hit (pure CPU) + 247 K linear dir scans + 27.6 K getdents full-dir re-materializations; dcache hit 90.5%; ext4 IDLE after t=150 s** | **medium-small (~120-140 CPU-s/boot, all in the first 150 s — corrected down from ~300-500; per-op 100-300x holds)** — fix-route map: docs/development/f4-ext4-route-map.md | F4 |
| 5 | Atomic/CAS contention (atomic glue page + fetch_update family) | 16% of kernel ticks | pa16: glue page 20% of ticks — RMW owners named (small_alloc/fdtable/GRQ sections, row 1b) | medium — fixes fold into 1b | F5 |
| 6 | Log-filter + fmt glue | ~11% of kernel ticks combined | (untouched) | small-medium | F2/F5 |
| 7 | Allocation churn (~5% kernel ticks) + allocator irqsave volume | ~5% kernel ticks | quantified as F1's top item (12.2% of machine in irqsave pre-magazine; 9.2% post-magazine pa20 — the locking lever is spent, the per-op local irqsave floor remains at 751 M ops/boot) | medium-large — **the churn hunt (who allocates 1 M objects/s) is now the allocator's main lever** | F5 |
| 8 | Mount registry scan | mount not in pa13 top-40 by wall | (untouched) | small | F4 |
| 9 | clock_gettime volume (no vDSO) | 43.6 M calls <1 us | pa16: 35.1 M @3 us; pa17: 33.3 M @4 us | small CPU, medium latency-shape | F6 |


What moved v1 -> v2: the read storm (#3 in v1 at ~0.5-1.0 vCPU) is gone
(read -113x, total syscalls -30%); its slot is taken by the hilogd.server
dual-drain, which was always there underneath (11.6M by t=900 in pa3)
and now spins faster. F1/F3 are unchanged — as predicted, the kmsg fix
was CPU-side relief, not a wall-clock lever by itself.

## 5. Root-cause families (adjudicating the hypothesis)

The brief's candidate list was: global single-lock family; O(n) scan
family; per-page/per-byte family; IRQ-off-window family; TCG/KVM
specific amplification. Measured result on current main:

- **F1 IRQ-off windows — CONFIRMED, now the #1 CPU-side family.** But
  the composition changed from the historical picture (MOUNT_TABLE
  irqsave, PTE_MODIFY_LOCK, printk THRE are all fixed): what remains is
  the softirq-at-irq-exit discipline itself plus residual cli sections
  in the UART TX path (`outb`/`inb` with IF=0) and string-scan/atomic
  regions. The tick-sampling artifact (all queued ticks attribute to
  the re-enable page) explains why it shows as 32% "softirq page".
  pa3 softirq vec counts add the composition detail: v1 (tcp_timer)
  and v8 (hrtimer) are raised on *every* timer tick of *every* CPU
  (199/s each — v1 for a usually-empty TCP table), v4 (block) 98/s
  early then ~0 once I/O settles; total loop entries 298/s; handler
  bodies only 0.4% CPU (WW_SOFTIRQ_TICKS 1369 ms over 780 s) — the
  cost is the dispatch discipline and its cli window, not the handlers.
- **F2 Flood amplification loop — CONFIRMED (new in the ranking).**
  Slow answers -> userspace retries -> per-call kernel cost x volume.
  hilogd.rd_kmsg alone: 72.2M reads over 780 s (92.6K/s sustained for
  the whole boot, ~15-25% of a vCPU). hilogd.server: 395K socket
  receives in the first 60 s alone. connect-retry storm: 7.4M calls.
  None of these exist at that rate on the Linux reference boot (which
  finishes before such loops matter). This is the "systemic"
  multiplier that makes every per-op cost count double.
  **RESOLVED (pa9-pa12, second session): the hilogd.rd_kmsg read flood
  is an EOF spin on /proc/kmsg.** pa9's positive-read census caught the
  fd: rd_kmsg opens /proc/kmsg (NOT /dev/kmsg — that is why pa4's
  /dev/kmsg counters stayed 0), drains the procfs open-time SNAPSHOT
  in 196 reads (~200 KB), and every subsequent read returns 0; pa10's
  failing-read census showed zero errno returns all boot, so the flood
  reads are EOFs — the pa4-8 "not EOF" conclusion was EOF-census table
  saturation (12/12 slots claimed by earlier comms), not absence.
  Linux /proc/kmsg is a BLOCKING record-at-a-time stream; OH's reader
  paces on the blocking semantics. Fixed as a Linux-faithful stream
  (single-reader EBUSY, one record per read, shared read_seq cursor,
  timed-poll block — see the pa11 ABBA note below): rd_kmsg collapsed
  from 60-92K reads/s to ~20/s (one syscall per delivered record),
  system-wide read() count at t=300 fell from 20.4M to 228K (-90x),
  total syscalls -16%, kernel tick share 88.0% -> 86.0%. The same
  commit fixes the /dev/kmsg write passthrough (userspace writes were
  dropped by printk()'s pre-ring loglevel filter — OH boot markers
  never reached any hilog consumer; now always ring-recorded,
  console-gated; 772 [kmsg] markers observed in pa12).
  REMAINING in F2 (pa13-confirmed): **hilogd.server dual drain is the
  new #1 flood** — 13.26M recvmsg + 13.26M recvfrom by t=720 (18.4K/s
  each, ~25% of all syscalls; spins faster than pre-fix 12.9K/s because
  rd_kmsg stopped competing), plus sysevent_source 672K, mmi_service
  477K, and the connect-retry storm 3.26M — OH userspace logging
  volume/retry behavior; route to the launcher line with the pa13
  numbers (kernel-side lever is small unless recvmsg/recvfrom per-call
  cost is attacked).
- **F3 Scheduler wake-loss — CONFIRMED as the dominant *wall-clock*
  family.** 448 WAKE-RUN-LEAK episodes (kicked >=2x, never picked),
  median 252 s, p90 467 s, max 520 s. The W3-RUNWAKE-REFUSED census
  splits them: 20 measured refusals with `curr_on=true on_cpu=true`
  (a lying busy slot — the task switched out but the CPU slot/on_cpu
  bookkeeping never cleared, so wake_up refuses forever) and 6 with
  `linked=true` (queued but never picked — pick-path starvation).
  pa13 refresh (post-F2, pre-fix baseline for the launcher lane's
  wake-loss fix): 702 leak prints over 780 s, census 23 refusals split
  16 lying-slot / 7 linked — same ~70/30 shape; W3 output format is
  grep-stable for co-run with the launcher's birth forensics.
  This family is why per-service phases take minutes: one leaked
  server thread stalls a whole dependency chain until something heals
  it. It is also *caused* by F1 in part (a wake issued while the
  target CPU is cli-bound gets dropped/raced), forming a loop.
- **F4 ext4/metadata per-op family — CONFIRMED, #1 in per-op factor
  (100-300x), medium-small in total (leg-18 correction).** open 271 us,
  newfstatat 277 us, getdents 501 us, fsync 90 ms. The original
  "~600 CPU-s/boot" combined estimate inherited pa17's stall-inflated
  averages; the pa29 anatomy re-measures it at **~120-140 CPU-s/boot,
  all before t=150 s** (see 2k + the fix-route map). The mount registry
  scan (O(n)) subcase is real but already fixed on the launcher lane;
  the deeper cost is the no-journal write-through path (first 90 s) and
  the CPU-bound lookup/bread path (90-150 s) — NOT the journal, which is
  absent from this workload entirely.
- **F5 Global-lock/atomic contention — CONFIRMED as a component, not
  the top.** 9-15% of kernel ticks in `fetch_update` CAS loops
  (virtio completion + PageCache RMW), plus `RawSpinlock::lock`
  pages ~2%. The binder WORLD/epoch lock did not show as a hot page
  this run (needs a binder-focused run to quantify separately).
- **F6 TCG/KVM-specific — NOT a factor for the OH lane** (all
  measurement and the reference are KVM; the vDSO-absence cost is
  already amortized by the TSC timekeeper fix — 114M clock_gettime at
  <1 us each).
- **F7 x86_64 tick-cadence — CONFIRMED, diagnosed leg 13 (section
  2g).** The timer IRQ delivers at ~1 Hz effective (nominal 100 Hz)
  on the OH x86_64 lane, under BOTH KVM and TCG. Not an IF=0 family
  (EFLAGS pair accounting: max closed segment 3-5 ms). It quantizes
  every tick/wake-derived latency on the lane to a 1 s grid and is a
  confounder for F3's latency measurements (counts unaffected). Fix
  candidate filed in the report's ledger; perf lane retests when it
  lands.

The per-page/per-byte family, historically the biggest (1.5 s/file
fault), is verified fixed (see section 6).

## 6. Already-fixed items: effect verification (no re-fix)

| fix (commit) | before (measured then) | now (pa1/pa3) | verdict |
|---|---|---|---|
| UART THRE budget 1s->250us (29a49444) | sc166: 2300 IRQ-off + 2503 softirq-storm + 2380 WAKE-STRANDED lines fed by probe prints | TIMER-SOFTIRQ-STALL 7/boot; WAKE-STRANDED 744/boot (sprobe prints, not THRE-bound) | holds |
| ksoftirqd wake at dispatch skip (aa742f18) | tick_age=0ms dispatch starvation for seconds | 1 borderline line/boot | holds |
| per-mm PTE locks (681da2da) | 470-615 s single-task fork-walk monopolies | none in rip table; fork avg 27 ms still high but not monopolistic | holds |
| buddy bitmap marks (a80974df) | order-18 = 262K iterations, terminal 15-min freeze | small_alloc pages ~1% of ticks | holds |
| fault-around x2 + filemap cache mapping (6782020e, 1b742d80/798b4cb3, dc096f14) | 802 s single .so load (1.5 s/file fault); 1727 OOM faults (sc176) | 0 OOM lines in pa1/pa3; pread avg 1.75 ms; SA chain completes; deepest boots ever | holds |
| TSC timekeeper default (753c834b, 6529f06a, 9d595efa) | clock_gettime = HPET MMIO VM exit; launcher stime 1690-1766 ticks in hpet page | 114M calls at <1 us; no hpet page in top-20 | holds |
| MOUNT_TABLE preempt-off (502ea7a7) | >=1 s cli sections in register_mount | mount avg 5.1 ms whole-syscall; residual cli windows now UART/memcmp-shaped | holds |
| unix truesize (5157c305) | writev parked whole 3 s budgets (183 stime ticks) | writev avg 14 us | holds |
| sticky one-shot wake re-issue (385c9793) | consumed wake tokens, tasks parked for minutes | REKICK now re-issues; the *residual* WAKE-RUN-LEAK (448/boot) is the next member of this family | partially — family continues |
| /proc/kmsg blocking stream + /dev/kmsg write ring fix (perf-audit d931c345 + cfc39329, second session) | rd_kmsg EOF spin 60-92K read/s, ~15-25% of a vCPU; boot markers dropped pre-ring | pa12: rd_kmsg ~20 reads/s, 772 [kmsg] markers. pa13 (780 s): read() total -113x (93.3K/s -> 0.82K/s), total syscalls -30%, kernel tick share 86.9% -> 84.2/83.3%, user share +2.5 pp | holds |
| x86_64 TSC timekeeper multiply-then-shift (launcher lane, merged 199f308c; validated pre-merge by pa23/24/25, OFFICIALLY by pa27) | read_time stepped 1x/s (shift-first math); every timed wait on a 1 s grid; nanosleep avg 1.87-2.2 s; clock_gettime max 654 ms; WAKE-STRANDED 100/boot | pa23 (TCG): tripwire [ok], ticks 99.9/s/CPU, nanosleep -21x, census holds. pa27 (merged main, KVM, official): tripwire [ok]; census 8/32/0; nanosleep avg 221 ms (-10x vs pa17 pre-fix KVM); clock_gettime 42.6 M @ 3 us, max 288 ms host-noise; IF=0 25.7% in-band with SOFTIRQ 1.0-1.7 -> 3.1% of IF=0; wall-clock milestones NOT moved (F3 + binder cost classes own the deep-boot wall) | holds — fixed AND officially measured |

## 7. Fix roadmap (ROI order, with lane routing)

1. **Wake-loss heal + pick adjudication (sched).** The W3 census gives
   the two exact shapes: `curr_on&&on_cpu` lying slots (need a
   pick-side stale-ownership sweep, same spirit as the R85
   stale-claim heal e0902a27) and `linked` starvation (pick-path
   geometry). Expected effect: removes minutes-long stalls — the
   largest wall-clock term. Lane: scheduler is currently unowned;
   route to main session for assignment (wildwrite lane owns adjacent
   sched forensics). First step is cheap: extend the W3 print to dump
   the lying slot's owner timeline.
   STATUS: launcher lane has a wake-loss fix in flight; pa13 recorded
   the post-F2 pre-fix census baseline (23 refusals = 16 lying / 7
   linked; 702 leak prints / 780 s) — re-measure W3 numbers on their
   fix when it lands. pa15 (same census, cpu1 KERNPANIC-parked from
   t=418): 26 refusals / 406 prints / 5 PREEMPT-LEAK — baseline shape
   unchanged; their fix is the gate for the retest.
   PA17 RETEST (sixth session, v1 a39d2bc0 + v2 6e9679c5 merged at
   main@0634bc8f): census UNCHANGED (593 prints / 31 refusals = 22
   lying + 7 linked / 8 PREEMPT-LEAK) and TERM-DESCHED=0 — the fixed
   terminal-deschedule shape did not occur this boot, so the census
   lying-slot population is born on another path; birth forensics on
   the launcher lane remains the lever. One direct effect did land:
   the schedule() GRQ REST hold fell 31% (56.6 -> 38.7 s; 3.9% ->
   2.7% of machine) — consistent with v2's pick-ownership release
   (single-run evidence). Acceptance of the v1 shape needs a boot
   where TERM-DESCHED actually fires (their sc-family reproducer), not
   this clean census run.
2. **Shrink irq-exit softirq cli time (interrupt/timer).** Two
   independent pieces: (a) stop raising the tcp_timer softirq (v1)
   on every tick when the TCP table is empty (199 raises/s/CPU for
   nothing — one-line gate in the tick path); (b) run the softirq
   loop with IRQs on after the first iteration (Linux
   local_bh_enable discipline) or move dispatch to ksoftirqd when
   the loop exceeds one handler pass. Expected effect: recovers a
   large part of the 25-32% cli-bound kernel time and all of the
   timer-lateness amplification (nanosleep 2.6 s -> ms-class).
   Lane: interrupt/softirq is shared-stable; small diffs, main
   session coordinates.
   STATUS (pa14 A/B, final numbers): the launcher lane implemented (a)
   as empty-raise gates (69310bae: v1 on TCP-table high-water, v8 on a
   TIMER_EVER_ARMED latch). Measured on the OH boot the gates have NO
   measurable effect: from t=120 s on, the marginal raise rate is
   199.2-199.7/s for BOTH vectors in BOTH runs (gated pa14 and ungated
   pa13 — identical); cumulative differences at t=720 (v1 -2461, v8
   +2010 raises) are inside early-boot run-to-run variance (pa14's
   first-60 s softirq runs were 22% higher than pa13's on host-load
   grounds alone). Reason: OH arms its first timer within seconds
   (v8 latch flips) and creates its first TCP socket by ~t=30 s (v1
   high-water flips — DHCP/netmanager); both latches are monotonic, so
   the gates never engage again. The intra-run v8-vs-v1 split does
   locate the latch moments (v8 latches ~t=9 s, v1 ~t=31 s, from the
   v8-v1 = 4471 offset). Conclusion: on THIS workload the F1 lever is
   (b), the irq-exit dispatch discipline; gate (a) still pays off on
   timer/TCP-sparse workloads (minimal rootfs boots). Tick/page
   agreement: kernel share 84.6/83.0% (pa14, 3 foreign QEMUs on host)
   vs 84.2/83.3% (pa13, quiet); cli-recovery page 25% vs 26%, same
   nm-verified symbol cluster (FileBuffer + enable_irq/restore_irq +
   inb/outb, +0x20 patch shift).
   PA15 REFINEMENT (fourth session): the F1 lever set splits three
   ways. (i) Console async / console-loglevel: reclaims ~5% of the
   machine's CPU (78.8 s of UART emission per 782 s boot at 16-18
   us/byte) and removes the seconds-class per-byte stall tail (max
   7.1 s in a single inb/outb exit pair) that parks emitters holding
   the printk lock — the highest-latency-value single fix. Producers
   are 2/3 OH userspace via /dev/kmsg (init 237 KB, FFRT worker
   139 KB, storage daemons 131 KB); task-context emission is 100%
   IF=1, so this is CPU+latency, not IF=0. (ii) irq-exit dispatch
   discipline: the hard+soft IF=0 slice (upper bound ~5-8% of CPU);
   the softirq loop is NOT budget-spinning (0.6% handler time, ~1.7
   passes/invocation) — the slice is hardirq-frame + dispatch wall,
   and the launcher's dead-task-with-HARDIRQ-count conviction is the
   extreme case (lost CPU, not IF=0). (iii) The REST bulk (~60% of
   the IF=0 upper bound): short irqsave critical sections across the
   kernel (spinlocks, FileBuffer, atomic RMW regions) — needs its own
   section-level audit (biggest single IF=0 bucket, least glamorous).
   PA15 CAVEAT: zone-split numbers are upper bounds (probe migration
     bug, see 2b); fix is known, one line.
   PA16 (fifth session) — CAVEAT CLOSED + (iii) DECOMPOSED. Zone
   accounting is now exact (edge-triggered, EFLAGS-adjudicated, v3;
   heal 0.2%/miss 0): IF=0 = 25.0% of machine, REST = 92% of it, and
   the REST bulk is attributed to file:line (section 2c). The fix list,
   in machine-share order: small-alloc cache locks 12.6% (per-CPU
   freelists, mm lane), schedule() GRQ 3.9% (sched lane), fdtable
   get_file 2.0%, waitqueue family 2.2%, EPOLL_WAKE_REGISTRY 1.1%
   (launcher lane — sharding design in their fb2a1354 notes, twelfth-leg
   handoff; was routed ipc lane). Also: the longest real IF=0 segment is 4 ms — there is
   no long cli section to break up; seconds-class window observations
   are host-stall tails inside exit-bearing sections (IPI/UART MMIO),
   so the highest-latency single fix remains console-async (1a).
2b. **hilogd.server drain — evidence package delivered to the
   ipc lane (pa15 census).** The dual drain is a blocking
   recvfrom(MSG_PEEK, 2 B) + recvmsg(74 B avg) pair on fd=12
   (unix:dgram, /dev/unix/socket/hilogInput), exactly 1:1, 0 EAGAIN,
   paced by 12.2 K real log records/s (702 MB/boot — the writev storm
   feeds it). Kernel levers: per-call recv cost, and the wake tail
   (recvfrom max 59 s = wake-loss family). OH-side lever (their
   code): drop the PEEK — 9.5 M syscalls/boot of pure overhead.
3. **Kill the hilogd.rd_kmsg spin (printk/dfx).** ~~92.6K read()/s
   sustained for the whole boot, ~15-25% of a vCPU. Attribution
   narrowed to "positive-returning reads on a non-kmsg fd" (pa4-8);
   the remaining step is one fdtable dump of that thread mid-flood
   (KILLDUMP prints fd tables; or a one-line fd-path latch in the
   read fast paths).~~ **DONE (second session, pa9-pa12): the spin was
   an EOF loop on /proc/kmsg's open-time snapshot; /proc/kmsg is now a
   Linux-faithful blocking stream (see F2 above). Also fixed the
   /dev/kmsg write passthrough.** NEW CROSS-LANE FINDING from the
   first draft (pa11): parking a reader on KMSG_READ_WQ deadlocks
   against scheduler-internal printk under GRQ (WAKE/W3 print family)
   because wake_up_all holds the WQ lock across wake_up_process
   (R12-1). The waitqueue invariant "no GRQ-held path takes a
   waitqueue lock" is violated by scheduler diagnostics prints —
   scheduler-lane owners should either gate those prints or expect
   this hazard for any new printk-woken waitqueue.
4. **ext4 metadata path profiling (fs lane).** open/stat at 100-200x
   is the largest pure per-op factor. Likely dcache/name-lookup
   shape (String allocations visible at 4-6% of ticks) rather than
   disk (KVM virtio is fast). Needs its own instrumented hunt; the
   launcher lane's mount-index experience is the template.
   Lane: fs is launcher-lane-adjacent; route with #4 of their queue.
5. **Page-cache/virtio fetch_update contention (block layer).** 9-15%
   of kernel ticks in CAS retry loops around virtio queue state and
   PageCache counters. Batching or per-CPU counters. Lane: virtio
   completion path is historically bug-dense — main session must
   assign deliberately (lock-ordering doc applies).
6. **In-flight (launcher lane, do not duplicate):** mount-registry
   index (8cd4f2c0, verified flat 12-25K cyc vs 345K-13.5M linear),
   anon fault-around, futex pool 8192 — all on `feature/samgr-callback`,
   unmerged into main at audit time.
7. **Not worth a lane right now:** allocation churn (~5%), String
   glue (~6%) — they fall out of #4/#5's callers; printk UART
   emission (known printk-wedge lane territory, small at quiet boot).

## 8. Tools landed (this branch)

- `kernel/src/dfx/syscost.rs` — cmdline `syscost=1` (own key, no
  dfx=switches.rs bit): per-syscall {n, wall, max}, >4 ms slow ring
  with comm, per-CPU tick modes (user/kernel/idle), 256-slot kernel-rip
  page histogram, softirq vec counts, flood-nr per-comm attribution,
  kmsg read counters + EOF/EAGAIN read censuses (fd paths), and
  (second session) the positive-read census (comm-keyed fd latch that
  follows the current open-file-description by `file_id`, recording
  fd/ops/path/bytes; ops pointers symbolize host-side via nm) and the
  failing-read census (comm, errno, fd-resolution state).
  (fourth session, pa15) the recv-family fd census (per-(comm,nr)
  slot following the current fd: flags, O_NONBLOCK, ret-shape,
  bytes, ops pointer, AF_UNIX kind/bound/peer; fed by a wrapper hook
  in syscall dispatch for native nr 47/45, x86_64-gated), the UART
  per-byte accumulators (`uart_putc_note`: n/cyc/max/polls + log2
  histograms, split by IF=0/IF=1), the per-comm printk table
  (`note_printk_ring`/`note_printk_con`/`note_printk_filtered`,
  `<irq>` pseudo-comm for interrupt-context prints), and the IF=0
  zone machinery (callers live in the probe patch).
  (fifth session, pa16) the zone machinery is v3 — edge-triggered and
  self-healing: every hook reads the REAL flag and adjudicates
  (model-open+flag1 = heal-close into HEAL meters, model-closed+flag1
  = open, model-closed+flag0 = counted MISS); HARDIRQ opens at
  irq_enter and closes+resets at irq_exit; SOFTIRQ/UART zones split
  the open segment only when IF=0; plus the REST call-site histogram
  (96 slots + counted overflow, keyed by #[track_caller] Location
  identity — the probe patch marks disable_irq/save_and_disable_irq/
  irq_save/lock_irq*/GRQ/PTE guards; dump prints the top-12 rows,
  per-CPU open-segment age, heal/miss meters). Also: `on()` no longer
  caches its verdict before the cmdline exists
  (boot-time callers would otherwise latch the probe off forever).
  60 s dumps from the timer softirq. Both arches; zero cost when off.
  Diagnostic caveats learned the hard way: the EOF/EAGAIN censuses are
  ONCE-PER-COMM tables (12 slots) — they saturate with early comms and
  silently DROP later spinners (this produced the wrong "not EOF"
  conclusion in pa4-8; the positive/fail censuses closed it); census
  adds ~5 us to each read that returns 0/EAGAIN while syscost=1;
  flood-table per-op numbers for read come from the census-free binary;
  the IF=0 zone split is EXACT since v3 (pa16) — residual model/flag
  disagreement is metered (heal/miss) and was 0.2%/0 on the pa16 boot;
  the pre-v2-migration upper-bound caveat is obsolete.
- `kernel/src/printk.rs` — kmsg read counters (records/EAGAIN/sleeps/
  first-reader flags) for the spin hunt.
- `run/pa-boot.sh` — quiet-OH measurement boot (snapshot shared disks,
  private reflink userdata) + host-side serial timestamping.
- `run/patstamp.py` — host-side serial-line timestamper.
- `run/pa-parse.py` — offline parser (decodes native x86_64 nrs via
  the map extracted from x86_compat.rs; symbolizes rip pages via nm).

Known measurement caveats (in the data, not hidden):
- Syscall wall includes blocking; sums across tasks exceed CPU time by
  design. The tick modes and rip histogram are the CPU-side truth.
- Page-granular rip attribution + the cli-window bias (all queued ticks
  land at the re-enable site): interpret "hot page" as "time ended
  with IF returning to 1 here" when the page contains enable_irq.
- Diverging syscall paths (exit_group) skip the exit hook: those calls
  are undercounted.
- Dump lines can interleave with concurrent WW prints at char level
  (pa2 lost whole lines this way; pa3 ran `sprobe=0`).
- pa1/pa3 ran on a host shared with 2-3 foreign QEMUs; per-second
  rates carry that noise, structure does not.

## 9. Reproducing

```bash
cd /home/william/rux-agents/perf-audit
PLATFORM=x86_64 make build          # bzImage with syscost
./run/pa-boot.sh pa4                # 26-min quiet boot, syscost=1
./run/pa-boot.sh pa5 ... APPEND_EXTRA="syscost=1 sprobe=0"  # quiet variant
python3 run/pa-parse.py run/pa-pa4-stamped.log --nm <(nm target/x86_64-unknown-none/debug/rux | sort)
```

pa14 (softirq-gate A/B) provenance: the gate hunks were extracted from
the launcher lane's 69310bae (`git show 69310bae -- kernel/src/timer.rs
kernel/src/drivers/timer/x86_64.rs | git apply`), built once, the bzImage
saved as `run/pa14-gate-bzImage`, and the patch reverted before commit —
timer/sched files stay launcher-lane territory. Symbolize each run's
pages against the ELF the run actually booted (pa13 vs pa14 layouts
differ by the patch).

pa15 (F1 deep-dive) provenance, same pattern: the IF=0-zone/UART/
printk hooks live in `run/pa15-probe.patch` (cpu.rs, preempt.rs,
softirq.rs, console.rs, printk.rs — all other lanes' territory), the
built kernel is `run/pa15-probe-bzImage`, and the patch was reverted
before commit. The stateless instruments (recv census via the syscall
wrapper, UART per-byte, printk per-comm tables) plus their syscost.rs
state ARE committed and active in any syscost=1 boot.

pa16 (v3 zones + REST call-site audit) provenance: hooks live in
`run/pa16-probe.patch` (cpu.rs, spinlock.rs, rwlock.rs, preempt.rs,
softirq.rs, console.rs, printk.rs, sched.rs, mm_struct.rs), built
kernel `run/pa16-probe-bzImage`. Rebuild: `git apply
run/pa16-probe.patch && PLATFORM=x86_64 make build`; revert with
`git checkout -- kernel/src/arch/x86_64/cpu.rs
kernel/src/sync/spinlock.rs kernel/src/sync/rwlock.rs
kernel/src/interrupt/preempt.rs kernel/src/interrupt/softirq.rs
kernel/src/console.rs kernel/src/printk.rs kernel/src/sched/sched.rs
kernel/src/mm/mm_struct.rs`. The v3 zone/REST-site state and the
heal/miss meters are committed in syscost.rs; the v2 file
run/pa15-probe.patch is superseded (kept for history — its hooks call
the removed v2 zone API and no longer apply to current syscost).

pa17 (W3 acceptance retest) provenance: `git apply run/pa16-probe.patch`
on main@0634bc8f applied clean (same patch file; sched hunks shifted by
the launcher merge, `#[track_caller]` marks are function-level so the
GRQ site moved sched.rs:2015 -> 2092 in the dump), built kernel
`run/pa17-probe-bzImage`. Revert is the same checkout list as pa16.

## 10. Final report structure (post-UI microbench report skeleton)

The user-named deliverable for this lane: a consolidated post-UI-boot
microbench report. Structure below is final; every number in it is
already measured (pa16/pa17 for Rux, oh-ref-r1 for the Linux reference
at `/home/william/rux-agents/oh-linux-ref/runs/`). The main session can
lift this section verbatim as the report body.

DELIVERED (seventh session): the expanded report lives at
`docs/development/perf-report.md` (v1.0, pre-UI) — the four tables
below plus interpretation paragraphs, the fix before/after ledger, and
the known-limitations section, with the hilogd-PEEK footnote and the
unlink fs-backlog annotation folded in. Only the pending last row of
its table 1 (post-UI milestones) remains to be filled at the UI
milestone.

### 0. Environment and method (half a page)

- Guest: OpenHarmony x86_64 image, QEMU q35, KVM, `-cpu max -m 4G -smp 2`
  (identical config for Rux and the Linux reference boots).
- Rux kernel: main@0634bc8f + syscost instrumentation (`syscost=1
  sprobe=0`); measurement method: per-syscall {n, avg, max}, tick-mode
  accounting, EFLAGS-exact IF=0 zone accounting (v3), kernel-rip page
  histogram, host-side serial timestamping (patstamp.py). Base pointer:
  current main d304069d differs from the measurement base by riscv64-only
  commits (LAYOUT-2) and docs, so all x86_64 numbers remain current;
  the measured baselines already include the slab-packing layer (see
  the eleventh-leg gate B correction).
- Linux reference: same image, same machine config, reference kernel
  boot (oh-ref-r1/r2). Caveats stated: wall averages include blocking;
  host shared with foreign QEMUs (load 6.7-7); single-boot samples for
  milestones (variance band given).

### 1. Boot phase table (Rux vs Linux reference)

| event | Linux t (oh-ref-r1) | Rux pa17 t | Rux band (pa1-pa17) | factor |
|---|---|---|---|---|
| kernel start -> /init running | 1.40 s | 0.85 s (pa1) | ~0.9 s | 0.6x (Rux faster) |
| first ServiceExec | 1.84 s | ~4-5 s | ~4-5 s | ~2.5x |
| hdf_devmgr SA-5100 registration | 2.90 s | 6.4 s | 6-7 s | 2.2x |
| bootevent.appspawn.started | 4.81 s | 60.2 s | 32-66 s | 6.6-13x |
| bootevent.composer_host.ready | 4.89 s | 31.4 s | 31-44 s | 6.4-9x |
| bootevent.launcher.ready | 10.12 s | not reached in 13 min | never | >150x |
| bootevent.bootanimation.finished | 10.73 s | not reached | never | >145x |

Reading: divergence grows with depth — early services 2-6x, everything
downstream of appspawn 8x to >100x: the signature of a latency
multiplier (wake-loss, F3) on top of per-op costs, not a uniform slowdown.

### 2. Syscall latency table (Rux pa17, t=720 s cumulative)

Volume leaders (the flood):

| syscall | calls | avg | max | total |
|---|---|---|---|---|
| clock_gettime | 33.3 M | 4 us | 1.0 s | 133 s |
| writev (logging) | 14.6 M | 24 us | 1.0 s | 357 s |
| recvfrom (hilogd PEEK) | 10.08 M | 116 us | **393 s** | 1176 s |
| recvmsg (hilogd consume) | 10.29 M | 9 us | 1.0 s | 98 s |
| connect (dead-peer retry) | 2.47 M | 98 us | 1.0 s | 244 s |
| epoll_pwait | 2.16 M | 11.6 ms | 180 s | 25,179 s (blocked) |

Per-op latency (medians are the honest per-op cost; maxes are the
wake-loss tail; Linux norms are same-op references on tmpfs/ext4):

| op | Rux avg (pa17; pa16 in parens) | Rux max | Linux norm | factor (median) | calls (pa17) |
|---|---|---|---|---|---|
| newfstatat | 294 us | 3.0 s | ~1 us | ~200x | 179 K |
| getdents64 | 1043 us (789) | 2.0 s | 2-4 us | 100-250x | 33.6 K |
| open | avg 1893 us, stall-inflated (pa16 avg 400 us) | 100 s | 1-2 us | 100-200x | 259 K (217 K) |
| unlink | 114.7 ms (117.2) | 2.0 s | ~50 us | ~10^3x | 192 |
| mkdir | 9.6 ms @16.4 K calls (pa16: 62 ms @ 940 — phase mix differs) | 3.0 s | ~50 us | 60-600x | 16.4 K (940) |
| chmod | 11.1 ms (5.8) | 1.0 s | ~20 us | ~400x | 2.9 K |
| fsync | 44.8 ms (15.4) | 2.0 s | 1-5 ms | 20-90x | 67 |
| fdatasync | 39.6 ms | 2.0 s | 0.5-2 ms | 30-100x | 1.6 K |
| pwrite64 | 14.7 ms (25.8) | 2.0 s | 50 us-1 ms | 20-300x | 13.6 K |
| pread64 | 10.9 ms (5.2) | 125 s | 5-20 us | 100-350x | 19.2 K |
| munmap | 166 us | 1.0 s | ~5 us | ~30x | 707 K |
| futex (blocked waits) | 351 ms | 465 s | — | wake-loss tail | 284 K |
| nanosleep | 2.2 s | 295 s | — | tick-lateness tail | 5.5 K |

### 3. Cost-center ranking (v2.3 summary — the fix list in order)

1. REST irqsave bulk = 20.8-23.0% of machine: small-alloc locks 12.2%
   (B lane), GRQ 2.7% (down from 3.9%, launcher v2), fdtable 1.9%,
   waitqueue 1.8%, EPOLL registry 1.0%.
2. Wake-loss (WAKE-RUN-LEAK) — unchanged by the merged v1+v2 fixes at
   census level (593 prints / 31 refusals); still the wall-clock driver.
3. hilogd dual-drain ~25% of syscall volume — OH userspace; kernel
   levers small.
4. ext4 metadata 100-300x Linux norms.
5. Console/UART 1.7-6.1% of machine (volume-dependent) + seconds-class
   stall tail (per-byte 15.5-18.5 us stable) — console-async is the
   highest-latency single fix.
(IF=0 total 23.4-25.0% of machine, max closed cli section 4-5 ms —
no long section exists; it is 1.26 M short sections/s.)

### 4. Linux comparison (what the same image does on Linux)

- Total boot to launcher.ready: Linux 10.12 s; Rux not reached in 13 min
  (16-23 min in earlier full runs).
- Syscall volume: Linux reference boot completes before the OH retry
  loops matter (no read/connect storms at any comparable rate); Rux
  80-84 M syscalls by t=720 (112-116 K/s), 60% logging traffic.
- CPU mode split: Linux spends most of this boot in userspace; Rux
  86-89% kernel ticks.
- The report's conclusion sentence: the gap is dominated by (a) wake-loss
  stalls multiplying every dependency chain, (b) 1.26 M irqsave
  sections/s (allocator/locks), (c) userspace logging loops amplified by
  per-call kernel costs, (d) metadata ops at 100-300x — in that order.
