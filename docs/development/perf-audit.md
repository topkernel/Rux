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

## 4. Ranking table v2.3 (cost centers -> boot-time contribution)

v2.3 = v2.2 refreshed by pa17 (sixth session: launcher wake-loss fixes
merged into main, W3 acceptance retest, console sub-item second data
point, allocator routed to the B lane). Contribution model unchanged:
2 vCPUs; kernel mode ~86-89% of ticks (pa17 t=720: 89.4/88.4%; pa16:
86.5/86.3%).

| # | cost center | measured (pa13) | pa15/pa16/pa17 direct evidence | gap contribution | family |
|---|---|---|---|---|---|
| 1 | Interrupt-disabled regions (cli-recovery page) | 26% of kernel ticks; 316 IRQ-OFF-WINDOW/780 s (now TRIAGE HINT — see 2d) | pa16 EXACT: IF=0 = 25.0% of machine; pa17: 23.4%; split REST 88.8-92.0% / HARD 5.1-7.7% / SOFT 1.6-1.7% / UART 1.4-1.9%; max closed segment 4-5 ms (no long cli section exists); 1.26 M sections/s | **large** — decomposed below; levers per section, not per zone | F1 |
| 1b | REST section bulk (exact decomposition) | (inside #1) | small-alloc cache locks 12.2-12.6% of machine (~55% of REST, 732-740 M/boot; **accepted by the B line, per-CPU freelist direction**); schedule() GRQ 2.7-3.9% (**pa17: -31% wall after launcher v2, 38.7 s vs 56.6 s**); fdtable get_file 1.9-2.0%; waitqueue family 1.8-2.2%; EPOLL_WAKE_REGISTRY 1.0-1.1%; deferred_free 0.7% — 97.3-99.3% attributed to file:line | **the F1 lever list** — allocator per-CPU freelists (B lane), GRQ pick (launcher lane, partially landed), fdtable read path, waitqueue batching, epoll registry (ipc lane) | F1/F5 |
| 1a | Console/UART emission (sub-item of F1) | outb/inb in recovery rips; cli page shares inb/outb | pa15: 4.34 M bytes, 5.0% of machine; pa16: 1.55 M bytes @ t=720, 1.7%; pa17: 5.61 M bytes, 6.1% — **per-byte cost stable 15.5-18.5 us across 3 boots; volume (and thus machine share) swings 1.7-6.1% with boot depth**; producers init/<irq>/FFRT-worker; task-context emission 100% IF=1 | medium CPU (2-6%) + **large latency multiplier** under host load | F1 |
| 2 | Scheduler wake-loss (WAKE-RUN-LEAK; lying busy slots + linked-starving) | 702 leak prints/780 s; census 16 lying + 7 linked | **pa17 (v1 a39d2bc0 + v2 6e9679c5 in main): 593 prints (-14%, in variance band) / 31 refusals (22 lying + 7 linked, unchanged) / 8 PREEMPT-LEAK / TERM-DESCHED=0** — fixes harmless, their trigger shape absent this boot; lying slots are born on another path; birth forensics continues on the launcher lane | **large** — unchanged; still the wall-clock driver | F3 |
| 3 | hilogd.server socket dual-drain + sysevent_source + connect-retry | 26.5 M calls by t=720 = ~25% of ALL syscalls | pa16: 10.6 M recvfrom + 10.7 M recvmsg; pa17: 10.08 M + 10.29 M (1:1, avg 116/9 us, recvfrom max 393 s = wake-tail victim); writev 14.6-15.2 M; connect 2.47-2.55 M | medium — volume is OH userspace; kernel levers = per-call recv cost + the wake-loss tail | F2 |
| 4 | ext4 metadata + write path (open/stat/getdents/fsync/pwrite/mkdir) | open 235 us, newfstatat 206 us, getdents 643 us, fsync avg 16 ms | pa17: open avg 1893 us (max 100 s — stall-tail inflated, see 2d), newfstatat 294 us, getdents 1043 us; medians unchanged | medium (~300-500 CPU-s/boot at this horizon) | F4 |
| 5 | Atomic/CAS contention (atomic glue page + fetch_update family) | 16% of kernel ticks | pa16: glue page 20% of ticks — RMW owners named (small_alloc/fdtable/GRQ sections, row 1b) | medium — fixes fold into 1b | F5 |
| 6 | Log-filter + fmt glue | ~11% of kernel ticks combined | (untouched) | small-medium | F2/F5 |
| 7 | Allocation churn (~5% kernel ticks) + allocator irqsave volume | ~5% kernel ticks | quantified as F1's top item (12.2-12.6% of machine in irqsave alone) — **B lane's queue** | medium-large — merges with 1b | F5 |
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
  (100-300x), medium in total.** open 271 us, newfstatat 277 us,
  getdents 501 us, fsync 90 ms. Combined ~600 CPU-s/boot. The mount
  registry scan (O(n)) subcase is real but already fixed on the
  launcher lane; the deeper cost is the name-lookup/journal path.
- **F5 Global-lock/atomic contention — CONFIRMED as a component, not
  the top.** 9-15% of kernel ticks in `fetch_update` CAS loops
  (virtio completion + PageCache RMW), plus `RawSpinlock::lock`
  pages ~2%. The binder WORLD/epoch lock did not show as a hot page
  this run (needs a binder-focused run to quantify separately).
- **F6 TCG/KVM-specific — NOT a factor for the OH lane** (all
  measurement and the reference are KVM; the vDSO-absence cost is
  already amortized by the TSC timekeeper fix — 114M clock_gettime at
  <1 us each).

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
   (ipc lane). Also: the longest real IF=0 segment is 4 ms — there is
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
  histogram, host-side serial timestamping (patstamp.py).
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
