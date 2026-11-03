# dfx-trace lane: unified trace-event framework, fully aligned with the reference kernel

## Charter (user directive 2026-11-02)
"Your dfx has many points — implement them fully aligned with the
reference kernel's trace events; the current bespoke-switch approach is
too messy." Build the unified framework and migrate onto it. The 57
bespoke `dfx=` switches stay working as legacy compat while other lanes
finish their hunts, but NEW observability stops adding bespoke switches
and adds tracepoints instead.

## What "fully aligned" means (concrete)
1. **Tracepoint sites**: static event sites with reference naming
   (subsystem:event like sched:sched_switch), disabled cost = one
   relaxed atomic load per site (same class as the existing switch
   table), enabled via the event table.
2. **Per-CPU ring buffers**: SPSC per-CPU rings, global tracing_on
   master switch, overwrite semantics, timestamps in the reference's
   spirit (local clocks acceptable round 1, note the divergence).
3. **Consumer interface = the reference's file layout and semantics**:
   /sys/kernel/tracing/{available_events,set_event,tracing_on,trace,
   trace_pipe,trace_marker,options,...} and
   events/<subsys>/<event>/enable per-event files. Text formatting on
   read (that is also what the reference does). If mounting a real
   tracefs needs filesystem support we lack, implement the tracefs
   mount — the paths and behaviors must match, not a /proc lookalike.
4. **First event wave** (semantics must match the reference events):
   sched family (sched_switch, sched_wakeup, sched_wakeup_new,
   sched_process_fork, sched_process_exit, sched_migrate_task),
   irq/hardirq entry-exit context markers, printk:console, and
   raw_syscalls:sys_enter/sys_exit (this subsumes the SysRing forensic
   ring long-term).
5. **Overhead gate** (guarded domains sched + interrupts): disabled
   sites = one load (verify, not assume); ENABLED overhead measured and
   reported vs disabled in syscost/pbench terms; census bands and
   pbench P2 within baseline band with events off. Image size delta
   reported (the 12MB kernel-image window is tight — kallsyms history).

## Migration ledger (round 2+, do not delete instruments mid-hunt)

The bespoke `dfx=` switch table (59 entries, kernel/src/dfx/switches.rs)
is FROZEN legacy: it keeps working untouched while hunting lanes close
their rounds, and new observability goes through tracepoints instead
(AGENTS.md policy). Replacement map:

| bespoke instrument(s) | replacement in the unified framework | status |
|---|---|---|
| schedtrace, schb, periodic, taskdump, parkdump, sleepsnap | sched:sched_switch / sched_waking / sched_wakeup / sched_wakeup_new / sched_process_fork / sched_process_exit / sched_process_wait / sched_process_free / sched_migrate_task (all present) | ready to port |
| syscalltrace, sysforensic, sysring | raw_syscalls:sys_enter / sys_exit (present; sysring forensic ring is subsumed long-term per charter) | ready to port |
| sigtrace | signal:signal_generate (present) | ready to port |
| bindertrace, dbtrace, txnhunt | binder:binder_transaction / _received / _alloc_buf / _buffer_release / wait_for_work (present) | ready to port |
| memwatch | oom:mark_victim (present) + reference mm event family (mm_page_alloc/free, round-2 events) | partial |
| allocwatch, leakhunt, mmforensics, vmap, hole, poolcensus, monowatch | reference kmem:kmalloc/kfree + mm_page_* family (round-2 events; kmem exists on the parked feature/dfx-align branch, port after reconciliation) | round 2 |
| syncguard, syncwatch, thpark, futextrace | reference lock:lock_acquire/lock_release + futex event family (round-2 events) | round 2 |
| sfddbg, exthunt, filerd | reference ext4:/jbd2: event family (round-2 events) | round 2 |
| epollscan, epolltrace | no reference analog; round-2 rux: epoll tracepoints in the unified table | round 2 |
| tcptrace, socksnap | reference net event family (tcp:*, round-2 events) | round 2 |
| inputtrace | reference input event family (round 2) | round 2 |
| writetrace | exceptions:page_fault_kernel (present) + block:block_rq_issue/complete (present) | partial |
| execcost, ohhunt, treehunt, driftwatch | sched:* + raw_syscalls:* timelines answer the same questions (present) | ready to port |
| acqdiv, scmtap, datatap, ksprobe, ripsample, steptrace, claiminject, oncpuinject, epollscan-tap | debug taps without a reference event analog; keep frozen until their hunts close, then retire with the hunt | keep frozen |
| scribble, scribblesweep, scribblepark, wildwatch, wwcensus | wild-write TRIPWIRES (active detectors, not passive observability) — no reference event analog; keep as forensics, frozen | keep frozen |
| watchdog, softlockup, hung_task | DFX SAFETY NETS (act, don't just observe) — out of the tracepoint migration by design | permanent |
| ledger, ledger-bookkeeping | internal accounting of the above, not an instrument | n/a |

## Method
- Read the reference implementation shape first (tracepoint site
  macros, event registration, ring layout, tracefs file semantics);
  replicate semantics exactly, keep our internal implementation free.
- Boot on x86_64+KVM to exercise: echo > set_event, cat trace, marker.
- x3 builds rc=0 + make test green before every push (origin/dfx-trace;
  kill your suite's lingering riscv64 QEMU after make test — it holds
  rootfs.img's write lock; match by exact cmdline).

## Round 1 verification record (2026-11-02/03, x86_64+KVM, this host)

- **Disabled cost**: VERIFIED IN RELEASE ASM — each off site compiles
  to one `movzbl <enabled>` + `test` + `je` (syscall entry at
  ffffffff8056102d, exit at ffffffff805610a7 in the round-1 build);
  tracing_on moved inside record() — the reference's split (static key
  at the site, ring reserve under the master).
- **Enabled overhead**: guest differential, 4000x8K zero-reads, 3 reps
  each way, min-of: off 957.7-986.7 µs vs on 1107.8-1119.2 µs →
  **~37 ns/read = ~18-19 ns per record** (sys_enter+sys_exit armed).
  (run/tr1-kvm-proof-serial.log, [P5].)
- **Image delta**: x86_64 release bzImage 9,904,128 (origin/main) →
  9,916,416 (round 1) = **+12,288 bytes (+0.12%)**.
- **Serial proof** (run/tr1-kvm-proof.sh + run/tr1-proof-init.c on a
  static-init ext4 image, run/tr1-rootfs.img): P1 layout OK at
  /sys/kernel/tracing AND /sys/kernel/debug/tracing (same instance);
  P2 catalog/format files reference-shaped, bare-name set_event arms a
  whole group, unknown token = EINVAL; P3 interleaved trace with
  correct fields — sched_switch state letters (R/S/Z), a natural
  sched_migrate_task (orig_cpu=0 dest_cpu=1), irq_handler_entry
  "irq=11 name=virtio-pci" with d.h.. hardirq flags, printk:console
  from a /dev/kmsg write, tracing_mark_write markers, full
  fork→wait→exit(Z)→free lifecycle; trace_pipe consumes; per-CPU stats
  report entries/overrun/read-events (overrun 23018 under the P5 flood
  — the overwrite path exercised); P4 options read the reference
  defaults and context-info=0 drops the header.
- x3 builds rc=0; make test 1297/1297 (riscv64).
- Known divergences (all noted here, none silent):
  - printk:console records the Rux printk STORE text; /dev/kmsg writes
    carry Rux's "[kmsg]" origin tag in that text (reference stores the
    raw text).
  - Dynamic-array events (console, irq name) cap at 80 bytes per
    record (fixed-size ring record).
  - The x86 legacy-PIC line handlers are void; their irq_handler_exit
    always reports ret=handled.
  - The x86 LAPIC timer tick and the R83 scribble-heal stamp do not
    fire tracepoints (no action / reference uses untraced
    __set_task_cpu there).
  - options/ carries only the four options the renderer and ring
    consume (context-info, overwrite, sym-addr, sym-offset);
    tracer-specific reference options are omitted, not faked.

## Session log (append per session)

### 2026-11-02, round 1 — rs suite ported, 2 kernel bug families fixed, 2 stressors re-run clean

**Ported** (stress-ng stressor logic as standalone static musl probes,
per charter method; sources archived in userspace/tests/probes.tar.gz
per the 394c16ce policy, host driver test/stressng/run-class-x86.sh):
14 classes — malloc/mmap/bigheap (memory churn), fork/exec, fsmeta
(metadata storm with read-back verify), pipe/sock/sem/msg/shm (IPC),
futex/sched (wakeup storms), timer (create/settime/delete + CLOCK_
MONOTONIC discipline + nanosleep latency), plus --spawn passthrough
for minimal reproducers. Vehicle: the x86_64+KVM LTP rootfs (copy of
the permanent-store image in run/, rs/rsrun/renameco injected via
debugfs); PID-1 rsrun runs cmds.txt lines with per-class budgets.

**Round-1 campaign** (baseline 819a1be2, one family per boot,
dfx=allocwatch,taskdump,watchdog + syscost=1, logs in
run/stress-runs/): ipc / sched / timer / memory PASS (0 panics, 0
wedges, 0 probe errors); fsmeta FAIL (SP-2); forker survived but lit
up the SP-1 storm. Verdicts + census numbers: run/stress-runs/
findings.md.

**SP-1 (fixed, verified)**: PREEMPT-HEAL false-fires stripped live
guard bits of fresh fork/exec tasks (15,577 heals + 169 underflows per
120 s of fork churn). Two producers: (1) stale_200's j==0 arm treated
never-stamped fields as instantly stale — fixed with arm-on-first-
observation (sched.rs); (2) new_task_at did not zero the four
ti_pc_*_zero_jif stamps on recycled Task pages — same class as R68a
(task.rs). Verification boot (fork 8 + exec 4, full 100 s budgets):
**0 heals, 0 underflows, 0 panics** (was 15,577/169/0).

**SP-2 (fixed, verified)**: ext4 setattr corrupted freshly-written
files — the success tail ran page-cache invalidate_inode for EVERY
attr flavor (chmod dropped dirty unflushed pages) and metadata-only
setattrs wrote back the disk-read inode's stale i_size (0 for pending
writeback). renameco minimal reproducer isolated chmod (3/300 fails,
reads returned EOF; rename alone clean). Fix: invalidate gated to
ATTR_SIZE, metadata setattrs carry the live VFS size (ext4/mod.rs).
Verification boot: renameco 0/300 on every sequence; fsmeta 4-worker
class full 120 s rc=0 (was: all 4 workers corrupt on cycle 1).

**Driver bug found+fixed in-flight**: rsrun killed classes at an
absolute 60 s (RES rc=137 truncation in the first campaign's memory/
forker boots); now parses --timeout per class.

**Tier-1-under-load evidence** (syscost, memory boot): IF=0 cpu0
17.6 s/60 s (rest 17.2 s, hard 408 ms, uart 3 ms), top rest-site
sched.rs:2568 (37.4M ops avg 834 ns); no ALLOCTHROW / DEADLOCK /
KERNPANIC classes in any boot. Follow-up hunts queued: IRQ-OFF-WINDOW
idle-wake prints 65-142 ms under load (pre-existing, guarded domain
interrupts); WAKE-STRANDED 118 s face under memory churn (SP-1
family; re-check on fixed kernel in round 2).

**Round 2 queue**: dirstorm-style shared-tree churn + fsync pressure;
pipe/socketpair under CPU contention (2 vCPU); LTP robustness subset
long-loop (fork12, mmapstress, mmap-corruption); brk-churn probe via
glibc-static (musl pins brk); make test + full LTP spot-scan on the
fixed kernel to guard the setattr/preempt changes.


### 2026-11-02, round 2 — SP-3 fixed, IRQ-OFF-WINDOW adjudicated artifact, residual family censused, brk covered

**Sync**: merged origin/main aa43f52e (round-1 SP-1/SP-2 content landed
there); branch descends from main. Round-2 commits: 126e99b9 (kernel),
ec9a0079 (probes).

**Queue item 1 (SP-2 deep soak) + SP-3**: new rs classes dirstorm
(shared-tree legal races, strict fd-pinned verify) and dirsync (16 KB
write + fdatasync/fsync + fchmod + rename + dir-fsync storm). fsdeep
boot: dirstorm 6x150s PASS (2.02M ops, 0 corrupt); fsmeta 8x180s PASS
(1.45M ops — SP-2 area holds at 2x round-1 intensity); dirsync caught
**SP-3: fsync(dirfd) on ext4 returned EINVAL** (sys_fsync's blanket
!is_regular_file check; the reference kernel's ext4_dir_operations
carries .fsync, and our own namei defers entry-block durability to
fsync(dirfd)). Fixed (is_directory + ext4 ops -> ext4_sync_file);
rerun clean: dirsync 615,856 ops rc=0, dirstorm 2.55M ops rc=0 with
strict dir-fsync checks. make test green after fix.

**Queue item 2 (IRQ-OFF-WINDOW idle-wake)**: adjudicated MEASUREMENT
ARTIFACT, not an interrupts-domain violation. Evidence: periodic LAPIC
tick + idle halts with IF=1 (gap = halt-exit delivery latency);
user-frame gap lines prove steal-magnitude delays with IF=1; syscost's
cli/sti zone accounting measured organic REST maxima <=2 ms in the same
boots (the only >7 ms segments were ONE host-stall window quantized on
both CPUs: 2619 ms REST @ file.rs:581 cpu1 + 2620 ms UART cpu0, fired
once, never again). Probe re-tagged: IRQ-DELAY-IDLE / IRQ-DELAY-STEAL
for artifact frames (kernel change 126e99b9), verified live. No
interrupts fix needed; guarded-domain band holds organically.

**Queue item 3 (WAKE-STRANDED re-census, SP-1-fixed kernel)**: the
residual family STANDS and is deterministic. memory2 boot: malloc rc=1
(worker 308 wedge, needed SIGKILL — campaign's first wedge), 1x
WAKE-STRANDED 100,469 ms/22,262 visits, 21 heals, 3 underflows. Chain:
rs malloc parent (pid 307) carried preempt_count 0x10004 for 200.7 s ->
wakeups refused -> heal -> induced underflow at a page-allocator lock
drop (.sdata near mm::pglist::NODE_DATA). leakhunt rerun reproduced
everything (wedge + 100.3 s strand + 200.8 s leak) and captured
false-fire ORIGIN sites mm/zone.rs:567 + mm/mm_struct.rs:958 with
oage_jif=1 (live acquires healed instantly) — steal-window false-fire
producer: the ti_pc_* stamps refresh only on the task's own CPU tick,
so a task parked mid-guard through a >2 s host steal window resumes
"stale-stamped + bits present" and is healed live. Numbers are for the
OH lane (same family); steal-aware exemption for the 200-tick bar is
the argued fix direction.

**Queue item 4 (LTP robustness long-loops)**: rsrun gained an
`exec <budget> <path> [args]` line form (fixed a stale-tail argv bug
found in-flight: NULL terminator placed before two token shifts — LTP
new-API binaries rejected the duplicated path arg; host-verified).
ltplong sweep (49 entries: mmapstress01-10 x3, mmap-corruption01 x3,
msgstress01 x3, msg_comm, sem_nstest, fork_procs, fork_exec_loop,
growfiles): mmapstress01 x3 + mmap-corruption01 x3 rc=0. Adjudicated
NOT bugs (host reference identical): mmapstress02/03/05/08 brk-ENOMEM
(1990s static binaries; their brk roundup fails on the modern
reference kernel too). Usage-shape (my cmds lacked args):
mmapstress06/09/10 — rerun with canonical args in ltplong3. REAL
suspect family: fork_procs/fork_exec_loop/growfiles/msg_comm/sem_nstest
+ 1 of 3 msgstress01 -> rc=254 with EMPTY output = rsrun's waitpid
ECHILD path: exec'd glibc-static children die silently before
userspace output, sometimes (msgstress01 mixed 2xTBROK+1x254), under
fork-heavy tests. fork_procs passes on the host reference (rc=0).
ltplong3 rerun (8 entries incl. 2 of the same binaries): 0 recurrences
— the family is INTERMITTENT (~1 boot in 2 today); mmapstress09/10
PASS with canonical args, fork_procs -n 100 x2 + sem_nstest + growfiles
all rc=0. Functional datum for the LTP lane: msg_comm TFAIL "messages
leak between namespaces".

**Queue item 5 (brk via glibc)**: brkchurn.c (glibc-static, host gcc
— host/guest both x86_64): PASS. 864,320 raw sbrk waves (212 GB) +
114.9M glibc malloc blocks (1.15 TB), enomem=0, verify 0 mismatch.
The sys_brk paths musl physically cannot reach: covered clean.

**Gates**: x86_64 build rc=0 (final 22:23), riscv64 build rc=0 (clean
config), make test GREEN (1096-suite verdict green), SP-3 stressor
rerun clean. Pushed origin/stress-port.

**Round-3 queue**: (1) the rc=254 silent-child-death family (minimal
repro: fork+exec loop of a glibc-static hello + waitpid audit; suspect
interaction with the strand/leak family — same boots); (2) heal
bar steal-awareness (200-tick bar vs >2 s host steal windows — design
per the census above, coordinate with OH lane); (3) ltplong long-horizon
loops (hours) once the 254 family is fixed; (4) mmapstress07 with
tmpfile args; (5) doubled direct-console interleave on heal prints
(two CPUs, no console lock) — cosmetic but garbles the heal lines.

### 2026-11-03, round 3 — heal-bar root-caused and rebuilt (observation
### counting), steal question decided, execaudit landed, 4x soaks PASS

**Sync**: merged origin/main 12acd3f6 (binder tcomplete deferral, ext4
published-size, x86_64 128KB stacks, LTP r22) — merge 3383cf79. Round-3
commits: ac0ff7f6 (kernel heal bar), f527ac16 (execaudit probe) + this
session's docs/driver commit.

**Q1 (the "200.7 s 0x10004 leak") — ROOT-CAUSED: not a missed put.**
Evidence chain: the first heal's leakhunt ORIGIN showed oage_jif=1 with
held_jiffies=0x4e6c — an outermost 0->1 acquire ONE jiffy before the
"200 s hold" heal (a solid base=4 hold cannot rewrite the o-record;
inner 4->5->4 pairs never touch it) — the base was churning live
guards. held_jiffies is LEAK_WIN window AGE, not hold time. Three bar
defects produced every observed face:
(a) absence-stamp AGE accrued while off-CPU (stamps refresh only on
    the task's own CPU tick) — the 100 s strand victim resumed with a
    200+-tick-old bar and its first tick healed the resumed syscall's
    LIVE guards -> spinlock.rs:0x1b8 underflow at the page-allocator
    zone lock;
(b) jiffies is timebase-DERIVED, so >2 s host steal windows inflate
    stamp age with zero ticks delivered;
(c) the pb3 release-side re-arm stored the SUPERSEDED ti_pc_zero_jif
    field (zero readers in the tree) — the bar never re-armed on legal
    1->0 drains, and lock-hot tasks formed 200-present-sample streaks
    (held_jiffies=0 storm, oage_jif=1 on every line).
Plus (d): heals never re-armed anything, so post-heal presence was
judged against the pre-heal stale bar — the storm amplifier.
**Fix (ac0ff7f6)**: the bar now counts CONSECUTIVE-PRESENT OBSERVATIONS
(ti_pc_{pirq,sirq,hirq}_present_run): bumped only by the task's own-CPU
tick when the field is present; reset on absent samples, legal drains
(release-side now writes the field the gate reads), every external
zeroing (all heal paths + underflow clamps), and task creation. No
tick, no increment — off-CPU time and host steal cannot inflate it.
Verification (leakhunt boots): memory-fix1 + memory-fix2 = 0 heals / 0
underflows / 0 strands / 0 PARKED-LEAK (round-2 same-boot: 21/3/1 + a
storm); forker-v4 (fork 8 + exec 4) = 0 heals / 0 underflows (SP-1
families stay dead). HEAL lines now print prun=/srun=/hrun=.

**Q2 (steal-aware exemption) — DECIDED, no exemption needed**: the new
bar's unit is the OBSERVATION (a delivered tick that sampled the field
present). Host steal delivers no guest ticks -> contributes zero by
construction. An exemption on the old bar would have masked defect
(c): the storm was churn-caused (oage_jif=1), not steal-caused.

**NEW parkwalk census (dfx=leakhunt)**: PARKWALK-CENSUS walks/obs/
parkednz printed every 1000 walks. fix1/fix2: walks=10,000,
obs=82,629, parkednz=0 — the walk is healthy and NO task parked with a
nonzero count in either boot. This answers round-2's "why did
PARKED-HEAL miss the 100 s strand": the victim parked CLEAN; the
0x10004 face arose at RESUME from the stale bar. The strand itself
(R85 orphaned-mark/lost-park) remains the OH lane's family — its
wake-side face no longer corrupts counts on this kernel.

**Q3 (rc=254 silent-child-death)**: execaudit.c probe landed
(userspace/tests/probes.tar.gz; driver injects it): fork storm +
immediate exec of a glibc-static target (brkchurn) + strict waitpid
audit (ECHILD/wrong-pid/signaled/127/nonzero), sibling fork-pressure,
rsrun's WNOHANG discipline via -w, EA lines to /dev/console. In-guest
boot: 560 audited execs (280 per discipline), events=0, echild=0 — the
family did NOT fire this boot (matches its ~1-in-2 intermittency;
next session: concurrent burst + LTP-shaped targets + multi-boot
sweep). Probe is the standing reproducer now.

**Q4 (soaks)**: dirstorm 12x150 (4x round-2 intensity) PASS — 2,139,796
ops, 0 RS-ERR; fsmeta 8x180 PASS — 1,375,896 ops; brkchurn pressure
rerun (two driver lessons: everything after --spawn is the probe argv;
the spawn class keeps rs's default 60 s budget unless --timeout
precedes --spawn) — ~90 s of glibc brk churn under leakhunt:
sbrk_ops 226k+ / 55+ GB, malloc_ops 29M+ / 290+ GB, enomem=0,
0 heal-family events. 0 heal-family events in every soak boot.

**NEW mm finding (SP-5 candidate, memory lane's guarded domain)**:
memory-fix1 boot — malloc class 0 ops for 100 s (round-2 boots: 4.45M);
zone genuinely exhausted (used=524213/524252), 2 workers self-SIGKILL
on pagefault OOM, parent sat in direct reclaim with
in_direct_reclaim=<pid> gate_exit_releases=0 across 4+ OOM-FORENSIC
dumps WHILE a later dump showed zone_free=33462 pages returned. Heal
side not involved (round-2 malloc windows had 0 heals too — first heal
fired only after the class). fix2 boot completed malloc normally:
INTERMITTENT, same cadence class as the fake-OOM history. Shape:
reclaim's exit gate does not release (or holder never re-checks) once
memory returns. Reproducer: the memory class boot itself.

**Gates**: x86_64 build rc=0 (post-fix, twice), riscv64 build rc=0
(clean config), make test GREEN (ALL TESTS PASSED, rc=0; suite QEMU
killed after verdict per lock discipline), stressor reruns clean.
Pushed origin/stress-port.

**Round-4 queue**: (1) SP-5 reclaim-gate livelock — coordinate with
the mm owner; the stress vehicle reproduces it; add a gate-age census
to name the holder's wait site; (2) execaudit multi-boot sweep
(concurrent burst, LTP binaries as targets) to catch the intermittent
ECHILD; (3) ltplong long-horizon loops (hours) on the fixed kernel;
(4) mmapstress07 with tmpfile args; (5) SPLIT o-record divergence
under fork churn (forker-v4: 24 budgeted lines, sched.rs:2580/6041) —
input for the stale-tp booking hunt (omis), zero wedges alongside.

### 2026-11-02/03, round 1 (this session)
- Branch dfx-trace off origin/main; commits: tracefs mount + event wave
  + one-load sites, then the reference-exact set_event grammar.
- Found substantial prior trace work already on main (tracepoint!
  table, rings, tracefs files under debugfs) — round 1 completed the
  charter's missing pieces: /sys/kernel/tracing as a real tracefs
  mount (+ the debugfs alias of the same instance), irq/printk/migrate
  events, options/, one-load sites, the reference set_event grammar,
  and the whole verification battery above.
- feature/dfx-align (local branch, 19 commits over main) carries a
  kmem event wave but is entangled with other lanes' vmscan/ext4/perf
  merges — NOT merged here; reconcile in round 2 when porting the
  kmem/mm families.
- Artifacts: run/tr1-kvm-proof.sh (KVM driver), run/tr1-proof-init.c +
  run/tr1-rootfs.img (static-init proof image — mkfs WITHOUT
  metadata_csum_seed/orphan_file, the ext4 gate rejects those),
  run/tr1-kvm-proof-serial.log (full transcript).
- test/tracefs-smoke.sh updated: per-event set_event writes (the
  reference applies ONE token per write — trace_get_user).

### 2026-11-03, round 4 (in progress) — negmemo validated 2/2, execaudit
### clean, SP-5 root-cause rewritten twice (holder-strand -> fake-OOM kill
### chain -> frozen-clock question), reference-parity mm fixes landed

**Sync**: merged origin/main d2b2672a (negmemo + chroot fast base + vm-op
fast paths) — merge a409deaa. Commits this round: 63e80291 (negstorm
probe + driver fix), 02c7... discriminator, b5a6dc3d (reference-parity
reclaim-wait fixes + swap census), + clock discriminators (see git log).

**Baseline re-verify on new main**: fsmeta 8x180 PASS rc=0 (~1.34M ops).

**Negmemo validation (queue 4) PASS x2**: new negstorm class (strict
ENOENT-prime -> create/link/rename-in/mkdir/symlink -> verify oracle,
common-set churn; needs cmdline negmemo=1): 8-worker boot 1.9M ops
neghit=274,763 negstale=479,832 ZERO events; 16-worker boot rc=0
neghit=200,402 negstale=483,127 ZERO events. d2b2672a holds.

**execaudit (queue 2) no recurrence**: 5 variants one boot (burst 16,
both disciplines, growfiles+fork_procs targets): 3,520 execs 0 events
(4,080 across both boots incl. r3). Family stays rare-intermittent.

**SP-5 (queue 1) — the hunt log so far** (boots: sweep 6 + swapcensus +
rawj + clock):
1. 6-boot sweep: the face is deterministic (every boot: holder caught
   phase=5 SWAPW rounds=0 freed=0, exhausted=4..8, selffight=0; workers
   die by kernel SIGKILL = rs's "needed SIGKILL (wedge)" mislabel — the
   15s-unreapable line never fires).
2. Discriminator fields killed the "minutes-long hold" reading:
   taken_age=0-1j, acqs=1-2 TOTAL, longholds=0 — holds are sub-second;
   round-3's "stuck across 4+ dumps" was a snapshot artifact.
3. Real damage: allocation FAILURE via 64-attempt gate-wait exhaustion
   while the holder's round is in flight -> pagefault fake-OOM kills
   (dump site: "alloc_pages(order=0) failed with free_cnt>0"; ledger
   shows 253MB free / zone refills 0->104k pages around the kills).
4. Reference-parity fixes LANDED (b5a6dc3d): progress-aware gate wait
   (RECLAIM_PROGRESS from balance_pgdat), fatal-signal bails (waiter +
   holder rounds + deep drain; current_is_dying exported),
   PF_MEMALLOC-parity holder-nested skip. Class verdict UNCHANGED
   (kills persist) — the fix is correct semantics but not the breaker.
5. swapwrite census: n=0 completed swap writes at EVERY dump — the
   anon-reclaim engine produces zero (lru[0]=225,700 anon pages,
   file-lru=27: reclaim is swap-only territory at exhaustion).
6. RAW-JIFFIES CONTRADICTION (open): now_j=50 frozen at both dumps
   (HZ=100 -> 0.5s) while SYSCOST printed t=60s nearby and negstorm
   boots' jiffies-derived PC2 advanced to 270s. Either read_time broke
   backward on some CPU or timer IRQs stopped during the cascade —
   the clock boot (OOM-FORENSIC: clock tb= irq0= irq1=) discriminates.
   If the tick dies during the cascade, the 1Hz census silence AND the
   kill-window misbehavior (strikes-only kills) both resolve.
RESOLVED by the clock boot: "clock tb=6173251 irq0=46 irq1=50" ->
"tb=7051524 irq0=46 irq1=50" — tb ADVANCES, tick-IRQ counters frozen:
the TIMER IRQs STOP on both CPUs ~0.5 s in (exhaustion ramp). jiffies
died with them (increment runs only from the timer IRQ); every
jiffies-derived silence during the cascade is the dead clock. NOT
reproduced by fork-only / mmap-churn / negstorm / fsmeta / timer
boots (tick alive through t=60-270s in all). Trigger is in the
reclaim/OOM-slowpath machinery — timers/interrupts-domain handoff
(LAPIC LVT/TMICT/IRR dump + last-re-arm stamp on the
frozen-counter-while-tb-advances signature). Swap reclaim still
produces zero completed writes at every dump (n=0) — second open
thread for the mm owner (per-page skip-reason instrument in
reclaim_anonymous_pages).

**ltplong4 long boot (queue 3) RUNNING**: mmapstress07 x2 TPASS with
canonical args (/tmpfile 4096 1 0) — the round-2 usage gap closed.
Rest of the battery in flight (mmapstress01, mmap-corruption01,
fork_procs -n 200, growfiles, mmapstress09 -p 40 -t 0.4).
