# Wild-write producer hunt (wildwrite-hunt line)

Branch: feature/wildwrite-hunt (worktree /home/william/rux-agents/wildwrite-hunt,
cut from main bc0ffbee). Goal: ATTRIBUTION — catch the wild kernel write
engine in the act, symbolize the writer, name the trigger scenario.

## Evidence recap (from prior archives, not re-collected)

Fires still happening on ~current main (latest: wt1, 2026-10-19):

- b77 (Oct 17): `hpet::BASE` single-byte smash 0xfe -> 0x4e (ASCII 'N'),
  one byte of the fixed MMIO alias address. Post-smash: every clock read
  dereferenced an unmapped physmap VA, boot graph froze.
- wt1 (Oct 19): BASE **and** adjacent BASE_LOCKED taken in one smash
  (blast radius >= 2 qwords or a wide store). read_time() returned
  boot-epoch garbage; CPU wedged in hpet::r32 (GDB post-mortem:
  RIP=hpet::r32, RDI=0xffff89004ed000f4, IF=0).
- b81 (Oct 17): sched `PER_CPU[1]` slots — current=nil, idle=<random
  heap task ptr>. Heal-at-entry now in main (sched.rs __schedule).
- Read-side immunity for all three victims is in main (value+complement
  pairs, monotonic clamp, re-derivable slots). Producer never caught.

Engines already convicted and fixed (do NOT re-hunt):
- R70: user EFLAGS.DF=1 through trap entry -> backwards rep movsb.
- 963832aa: copy_kernel_mappings low-half clone -> PT UAF -> text
  scribble (structural fix: kernel-half device alias).
- R83/R85 (riscv64 trap.S): ti_user_sp / ti_kernel_sp smear anchors.
- 56fc7e8b: mremap interior-range semantics (gnome-shell heap).
- a6219f85: 32KB kernel stack overflow (children-list zeroing).
- R64 13482899: rsp0 ceiling / cross-stack family.

Residual engine signature: x86_64, fires under OH-load boots (post-sdc,
softirq/vsync storms). Not every boot (b150-b155 clean). Victim value
patterns: ASCII bytes ('N', 'AppI', 'e ticks') and adjacent-qword wide
smashes into pointer-bearing statics.

## Hunt plan

1. **Primary: QEMU gdbstub hardware watchpoints on the live victim
   statics — zero kernel change.** Boot my x86_64 build on the OH image
   (read-only: -snapshot + file.locking=off, TCG thread=multi, smp2, 4G)
   with `-gdb tcp:127.0.0.1:<private port>`. gdb batch script arms
   `watch` on the hpet quiet statics + canary page (addresses
   nm-derived; see run/ww-gdbscript.py). Every watchpoint hit logs rip +
   symbol + regs + stack to the gdb log and continues; legit writers
   (hpet::init pre-attach, heal paths) are a known small rip set —
   anything else IS the wild writer.
2. **Kernel-side always-on tripwire (my territory): dfx=wildwatch
   (switch bit 26) — LANDED and end-to-end validated (commit 42eb2006).**
   - The quiet-after-init timekeeper statics moved into a page-exclusive
     2-page `WildGuardCluster` (canary page + members page) in hpet.rs;
     hot statics (BASE, LAST_GOOD) stay outside.
   - After init the guard pages go read-only in every reachable alias
     (kernel-half link VAs, linear map, low identity), splitting the 2MiB
     image leaf. A store to the frames page-faults at the writer's RIP
     and the existing kernel-#PF printer carries rip + rbp walk +
     KERNPANIC dump → addr2line names the writer.
   - Canary page verified from the 1 s timer cadence (DMA-class writers
     bypass PTEs).
   - Self-test (`wildwatch-selftest=1`): deliberate smash → trap fires →
     dump symbolizes to `rux::dfx::wildwatch::x86::arm` ← `dfx::init` ←
     `rust_main_tail` — attribution chain proven.
3. **If neither fires across a boot series:** broaden with a shadow
   checksum scan of the hpet/PER_CPU neighborhoods from the timer tick
   (correlation-only; attribution then needs the watchpoint boot to
   repeat on the same seed).

## Findings so far

- **F1 (significant side-finding): the kernel has never set CR0.WP**
  (verified live: `cr0=0x80000013`, WP clear). With WP=0, ring-0 stores
  IGNORE page-table RW bits — every "read-only" supervisor mapping is
  decorative (COW-on-kernel-side, .rodata hardening, my RO trap alike).
  Linux sets WP early in boot for exactly this discipline. wildwatch
  arms WP per-CPU when enabled (arm() on the BSP, the verify cadence on
  the others). Candidate mainline follow-up: set WP=1 permanently in
  boot (needs its own soak — any latent legit RO-write would surface).
- **F1b (handoff, user-half corruption line): with WP=0, supervisor
  uaccess writes go THROUGH COW/share protections.** A `read(fd, buf)`
  into a post-fork COW page or a file-backed shared page writes the
  FORK-SHARED / PAGE-CACHE page directly instead of breaking COW —
  parent/child divergence and cross-mapper content bleed, with zero
  kernel-side trace. This is a first-class suspect for the OH
  heap-corruption timeline's user-victim families ("kernel-side smoking
  gun", PTE-OVERWRITE-LIVE, foreign-content meta). Suggested probe for
  that line: boot with WP=1 + `dfx=wildwatch` arming only WP (or a
  dedicated wp=1 boot flag) and a supervisor-COW tripwire print.
- **F2: the b77 "smash" value is the CORRECT device-alias VA.**
  `device_alias(0xFED00000)` = 0xffff8900_4ed00000 (alias window base
  0xffff8900_00000000 + (0xfed00000 − 0xb0000000)). The b77 narrative's
  "0xffff8900fed00000 → 0xffff89004ed00000" byte-smash reading compares
  against a WRONG baseline; 0xffff8900_4ed00000 is the value hpet::init
  legitimately stores (verified live via gdb on the running kernel).
  The wt1 GDB wedge showed RDI=0xffff89004ed000f4 = the normal
  counter-HI offset — that reader was at the RIGHT address. What the
  engine actually did in b77/wt1 needs re-reading against this
  correction from the oh-display raw captures. Read-side immunity
  stands regardless.
- **F3: gdbstub hardware watchpoints work cleanly on this QEMU/TCG**
  (8-point set armed through 45+ min OH boots, storm-rate serial
  unaffected). This is the cheapest attribution surface for the NEXT
  captures: any store to the watched statics logs rip + regs + stack.
- **F4: WP=1 is not bootable on the OH image today.** Arming WP
  (dfx=wildwatch) collapses the OH boot into init's LoopEvent timeout
  loop before any service starts (ww-long2/long3: 21/5 init-forks vs
  103 in a control boot; serial becomes a 987k-line cow-flood until the
  81dfcb42 rate-limit, then a pure LoopEvent spin). The COW faults
  themselves are user-mode; the collapse is a mix of extra supervisor
  COW work (F1b) and UART pressure. Minimal-rootfs boots with WP=1 are
  fine (selftest boots ran to the trap). The in-kernel RO trap is
  therefore validated but NOT usable as the long-run rider on OH boots;
  use it on minimal-rootfs load generators, or fix F1b first.

## Exclusions narrowed this session

- The gdb watchpoint set (7 statics, 45 min, full OH storm in r1/r4)
  observed ZERO writes — the engine did not fire in these windows
  (consistent with the b150-b155 clean runs; the engine is intermittent
  at a rate lower than 1/boot-hour under this driver's load profile).
- No false positives from the trap on the HPET read path (300 s runs).

## Run log

- ww-smoke (240 s, watchpoints pre-relocation): armed, no hits, clean boot.
- ww-r1 (2700 s, watchpoints, pre-relocation build): 0 hits, 0 heals,
  full storm reached — engine did not fire this window.
- ww-armtest2..11 (wildwatch bring-up): walk bug (missing PDPT level)
  found via gdb cross-check; arming then clean (2 pages RO + 4 alias
  PTEs), 300 s boots healthy, zero false positives on the HPET read path.
- ww-selftest4 (wildwatch-selftest=1): TRAP VALIDATED —
  `WILDWATCH-SELFTEST firing` → `KERNPANIC pfault badaddr=0xffffffff807c2000
  rip=0xffffffff8056e42b` → 24 stack words + 3 rbp frames →
  addr2line: `rux::dfx::wildwatch::x86::arm` ← `rux::dfx::init` ←
  `rux::rust_main_tail` ← `boot_stack_continuation`.
- ww-long2 (WP=1, 10 min): cow-flood wedge (987k cow lines), 21 init
  forks — F4. ww-ctrl (same binary, no wildwatch): healthy storm, 7.5k
  cow lines (baseline), 103 init forks.
- ww-long3 (WP=1 + 81dfcb42 rate-limit, 45 min): cow flood fixed
  (1.1k lines) but the LoopEvent spin persists — 5 init forks, no
  storm. WP stays off for OH hunts.
- ww-r4 (2700 s budget, watchpoints on the GUARD cluster, current
  build): full OH storm reached (107 init forks, media_service +
  appspawn churn, wakewatch heartbeats healthy). Boot died at ~35 min
  from the KNOWN kernel-heap exhaustion family (ALLOCTHROW 256 KiB with
  248 KiB free of the 128 MiB kernel heap, appspawn context — separate
  line, not a wild write). Watchpoints: ZERO hits, zero heals. Bonus
  observation for the heap-forensics line: the USER-half
  Permission-denied family was ACTIVE in the same boot — 5 victims
  (pids 387/500/605/612/652/705 family) all at rip=0x3fbf08ad30
  (ld-musl, the memcpy/validation site from the corruption timeline),
  minutes before the heap exhaustion. The kernel-static engine did not
  fire in any of this session's four storm windows (r1, r4, plus the
  b150-b155 history) — its rate under this driver is below 1 per
  boot-hour.

## Session 2 (continuation agent)

- Merged origin/main 3ab8f09a (no conflicts; main's kernel-side changes
  — build.rs, trap.rs, xattr, procfs/pid, unix — do not overlap my files).
  1326066f confirmed: the r4 kernel was built with the stale build.rs that
  resolved platform from Kernel.toml (riscv64) even for x86_64 feature
  builds, baking in the 128 MiB heap clamp. **r4's "kernel-heap exhaustion
  at 128 MiB" was a build artifact, not a real leak.** Post-merge rebuild:
  config.rs TARGET_PLATFORM=x86_64, KERNEL_HEAP_SIZE=1073741824,
  bzImage 10.79 MB (matches pre-clamp working kernels). The committed
  config.rs on main still carried the stale 128 MiB value; regenerated and
  committed on this branch (e432831e). OH boots on this branch now run
  with 1 GiB kernel heap — r4's 35-min death should not recur, and heap
  observations from r4 should be discarded.

### F1b FIXED (12f82297) — CR0.WP permanently on + kernel-mode COW resolved

Root cause of the F4 collapse, and the root-cause fix for the F1b
supervisor-COW-piercing family:

- The x86_64 `kernel_data_fill` branch in mm/exception.rs only accepted
  `Handled | Fixed` from handle_mm_fault; `CowPending` fell through to
  the exception-table fixup → short copy → EFAULT. With WP armed, every
  read()/write() into a post-fork COW buffer returned EFAULT, init's
  param reads failed, and the boot collapsed into the LoopEvent spin —
  the exact ww-long2/long3 signature. The riscv64 twin has had the
  inline `handle_cow_fault` resolution since R82; x86_64 now mirrors it.
- CR0.WP set permanently: BSP at early_boot_init (cpu::
  enable_write_protect), every AP in the smp CD/NW-clear RMW. Safe
  because the kernel image, linear map, and device windows are all P|RW
  — WP only bites user-side COW/RO pages and wildwatch guards.
- uaccess.rs: fault_in_write ported from riscv64 (Linux
  fault_in_pages_writeable parity) — copy_to_user/clear_user retry once
  after resolving the range.

Validation (ww-f1b-a, 10-min OH boot, q35/smp2/4G, snapshot):
- 803 "fork: parent" lines vs 18 in the collapsed WP=1 boots (ww-long2)
  and 422 in the WP=0 control — no collapse, boot reaches further than
  the old control (main's xattr fixes contribute).
- Full service storm (474 appspawn/media_service lines), healthy timer
  heartbeats (WW: wakes=1.84M).
- 0 KERNPANIC, 0 kernel faults, 0 Permission-denied victims (r4 had 5
  at rip=0x3fbf08ad30 — the predicted EFAULT-driven family; gone).
- cow lines 8071/10min ≈ control baseline 7487 — no COW flood.

Consequences for the hunt:
- F4 is resolved: WP=1 boots OH without collapse. Every future boot on
  this branch carries the COW/uaccess discipline; a wild supervisor
  write to any RO user page now takes a loud #PF with attribution
  instead of silently piercing protection.
- The r4-era note "OH long runs must not enable WP" is obsolete.
- wildwatch's per-CPU enable_write_protect stays (idempotent, harmless
  belt-and-suspenders if a future path clears WP).

### F2 archaeology (b77/wt1 raw captures re-read against the F2 correction)

- b77 (oh-display/run/oh-b77-serial.log, 6387 lines): the freeze point
  is the **sdc fork/exec boundary** — last serial lines are init's
  `Sync exec: /system/bin/sdc`, `fork: parent=1, child=318`, execve,
  and one final child-stack COW fault (0x7fffffffd088), then silence.
  Immediately BEFORE that: a wall of param-service failures
  (`Create directory ... failed err=2/5`,
  `mmap err 19 file /dev/__parameters__/`, persist-parameter
  rename/open failures). Confirms the handoff note that sdc cycles were
  the b77-era trigger context. F1b lens: the param workspace is the
  devfs-shared-mapping + fork + supervisor-memcpy surface — precisely
  the family the WP fix now disciplines.
- wt1 (wake-timer/run/oh-wt1-serial.log): deep-storm context — pids
  ~1000+, binder 1M mmap churn (0x7ff08a…), BIGMUNMAP, and
  WW: TIMER-LATE-FIRE with ~349 s accumulated lateness
  (hres_fires=908988 late_n=894045). The lateness is post-smash
  symptomology (BASE already bad → read_time garbage → deadlines
  miscomputed), not the trigger.
- Net: two distinct trigger contexts (early-boot sdc exec vs deep
  binder/mmap storm) hitting the same victim neighborhood. The
  GUARD-cluster watch set covers it either way; the chain's Mode B
  boots traverse both phases (45-min budget spans the storm).
- User-side families in the WP=1 boots: the r4-era "Permission denied
  at rip=0x3fbf08ad30 (ld-musl)" family is GONE (0 occurrences).
  main's new USEGV-FOR forensics show a different, non-corruption
  family: pid-restart loops dying at cr2=0x0 with rax=-16 (EBUSY
  consumed as a pointer by a strlen-class call) — a service-side
  errno-as-pointer bug, not heap corruption; noted for the OH
  bring-up line, not this hunt.

### Session 2 chain (ww-s2): overnight Mode B boots on the F1b-fixed kernel

- ww-s2-r1 (45 min, watchpoints armed): full budget survived, 84752
  serial lines, full storm, 0 KERNPANIC / 0 ALLOCTHROW / 0 kernel faults
  (r4 died at ~35 min from the 128 MiB heap artifact; with 1 GiB the
  boot-hour is clean), 0 WW-HIT, 0 heals. Chain continues (run/ww-chain.sh
  ww-s2, stop with run/ww-s2.stop; any WW-HIT auto-stops the chain and
  keeps run/ww-s2-rN-gdb.log for addr2line symbolization).
- WP arming verified live via gdbstub probe (ww-cr0probe): cr0=0x80010013
  [PG WP ET MP PE] on BOTH the BSP and the AP (was 0x80000013 pre-fix).

## Session 3 (continuation agent)

- ww-s2-r2 (interrupted): session-2's window died 15 min into r2's 45-min
  budget (serial last write 01:34:37, boot started ~01:19:39). Verdict
  after full-log grep: counts as a CLEAN PARTIAL — 53245 serial lines,
  storm fully active (cow faults, WW heartbeats, wakes=3.26M), 0 WW-HIT,
  0 KERNPANIC, 0 ALLOCTHROW in serial and gdb logs; watchpoints were
  armed (8-point set, WW-ARMED in gdb log). Interruption was external
  (window death kills the chain), not a kernel event.
- Chain resumed from r3: ww-chain.sh gained a `first` (4th) arg so
  restarts continue numbering instead of overwriting run/ww-s2-rN-*
  history. ww-s2-r3 verified armed and storming before this session's
  other work started.
- Branch push: port 22 and github.com:443 both time out from this host,
  but ssh.github.com:443 is reachable. Pushed with a one-shot URL
  (`git push ssh://git@ssh.github.com:443/topkernel/Rux.git ...`);
  origin now at 5aed2ed6. Host key added to known_hosts (TOFU via
  ssh-keyscan). Shared origin URL left untouched.
- USEGV-F observation (r1, full 45-min WP=1 boot): 5 victim pids
  (502/708/768/799/1044), each dying exactly 3 times at the IDENTICAL
  rip=0x7ff082996bac rsp/rbp (pid-restart loop, 3 attempts then give up),
  always cr2=0x0 rax=-16, faulting instruction `48 8b 03` (mov rax,[rbx],
  rbx=0) in a FileBacked lib page (fd=6, voff 0x15bac). Matches the
  archived errno-as-pointer reading; unaffected by the F1b kernel-side
  discipline, as expected for a service-side bug. Rate: ~5 victims /
  boot-hour, stable vs session-2 notes.
- Capture-protocol validation (the one leg never exercised before — no
  WW-HIT had ever fired anywhere): (a) gdb -batch `commands` blocks print
  rip/regs/symbol/stack and continue correctly on every hit (local
  inferior: 5/5; 8/8 lines survive a SIGTERM kill — ww-hunt.py's budget
  teardown cannot lose buffered hits); (b) REMOTE hit path on TCG: bare
  2G/2cpu q35 boot (kernel+ramdisk only, own PID, killed after), gdb
  attached +6s watching `rux::timer::LAST_TICK` — 1448 WW-HIT lines in
  15 s at the 100 Hz tick cadence, `info symbol` resolving the writer
  (atomic_store in the timer path), target resumed cleanly through every
  hit. A real engine hit on the chain WILL be recorded with full
  attribution. Side note from the bare-boot attempts: -m 1G boots die at
  ALLOCTHROW (heap region sits at 0x40000000, needs >=2G) — keep 2G+
  for any ad-hoc x86_64 boot of this kernel.

- ww-s2-r3 (45 min, session-3, watchpoints armed): full budget survived,
  82828 serial lines, full storm (wakes=13.3M, ticks=[101,100]), 0
  KERNPANIC / 0 ALLOCTHROW / 0 WW-HIT / 0 heals. Second full clean
  boot-hour on the F1b-fixed kernel. USEGV-F: 5 distinct pids x 3
  attempts each — same shape and rate as r1. Chain auto-advanced to r4.

## Session 4 (continuation agent — mm chunking verification + wedge attribution)

Inherited state: session-3's window died 03:02 mid-verification of 7b7a82b5
("chunk the fork COW walk and mm teardown"). Reconstructed: chain r5 had run
its full 45 min (02:39-03:24) and r6 died externally at 15 min (03:39, no
kernel event, no host OOM record); the predecessor's manual boots survived as
/tmp/wwtest/mm1-serial.log (bare ramdisk boot, idle 2h, 0 panics — killed by
me after data extraction) and mmoh-serial.log (OH boot of the chunked kernel,
~15-17 min, session-killed).

### Build verification

`PLATFORM=x86_64 make build` on the clean HEAD tree is a cargo no-op that
re-produces the byte-identical bzImage (md5 56b1be30...): the binary mm1/mmoh
booted IS the committed chunking code. config.rs regenerates to x86_64 over
the merged riscv64 copy (same generated-artifact quirk as session 2's
e432831e; recommitted here). Branch push: the handoff's "push pending" was
stale — remote feature/wildwrite-hunt already sat at 7b7a82b5.

### mm chunking: measured effect (before the wedge finding)

Strand distributions (WAKE-STRANDED events, sed-extracted):

| boot | kernel | window | total | avg | max | 1-5s | >5s |
|------|--------|--------|-------|-----|-----|------|-----|
| ww-s2-r5 (chain) | frozen, no chunking | 45 min | 17902 | 473ms | 200932ms | 2075 | 56 |
| mmoh (inherited) | chunked | ~15-17 min | 16305 | 133ms | 10019ms | 55 | 2 |
| ww-s2-r7 (chain, concurrent control) | frozen | 45 min | — | — | 424624ms | — | — |

Chunking cut multi-second strands ~14x per hour and the max monster 20-200x
in the mmoh window, but the metric is NOT zero, and r7 shows the baseline
still rolls 400s-class monsters while progressing. Consistent with the
SC-SESSIONS wall analysis: chunking is step 1; per-mm locks + kernel
preemption are still required.

### The wedge (blocks the verdict)

ww-v1 (45-min controlled OH boot, chunked kernel, ww-boot.sh conditions,
no gdb): at ~23 min the boot fully froze — wakes frozen at 3.09M (+4 per
90s), 169 runnable, both CPUs' incumbents parked at preempt_count
257/258-260 for minutes, ticks halved, 0 KERNPANIC/ALLOCTHROW/forensics.
Killed at ~28 min.

ww-v2 (same binary, + dfx=taskdump,watchdog,periodic): wedge REPRODUCED at
~19 min (wakes frozen at 3.38M). The kernel probes named it, then a live
gdbstub attach (port 12391, both CPUs interrupted) settled it:

- cpu1: `tcp_timer_tick` (timer softirq) → drop `TcpTxBatch` → Vec<u8>
  dealloc → `BuddyAllocator::free_blocks` → `add_to_free_list` — grinding
  the R17-A mark-every-page loop, sampled mid-iteration with 0x2c24a
  iterations remaining = ORDER-18 (whole-heap-scale) block marking, under
  the heap lock (irqsave).
- cpu0: same `tcp_timer_tick` → `TcpTxBatch::reserve` → Vec grow →
  `BuddyAllocator::alloc` — spinning on the same irqsave heap lock (this
  is the TIMER-SOFTIRQ-STALL IF=0 window; its pt_regs rip fossil showed
  enable_irq, the task-dump pids — hilogd.rd_kmsg read, sysevent_source
  munmap — were only the interrupted tasks, not the mechanism).

The wedge LIFTED after ~15 min (v2 wakes resumed; strand_max had ratcheted
to 253s during the stall) — a slow grind, not a deadlock. The deadlock
watchdog never fired (no single lock HOLD threshold crossed; the holder was
making progress).

Root shape: a kernel-heap buddy pathology — BUDDY_MAX_ORDER=20 over a
2^18-page heap, and the free/merge path's add/remove marking is O(2^order)
PER LEVEL under one global irqsave lock. An order-18-scale marking pass in
a debug (unoptimized) kernel under TCG holds the ONLY heap lock for tens
of seconds to minutes while the other CPU's TCP timer spins on it. Both
CPUs run tcp_timer softirqs, so once one grinds the other piles on.

Correlation vs chunking: 2/2 chunked boots (v1, v2) hit the full freeze;
0/8 frozen-kernel boots (r1-r8) did — but the mechanism touches no code the
chunking commit changed (buddy_allocator, net/tcp timer). The concurrent
baseline r8 spent its own multi-minute deep slump (avg_run 16ms, 47s
strand_max, ~40x slowed wake rate) — the pathology exists on the baseline
too, at lower intensity. A timing-mediated amplification via the chunked
kernel's longer mm-critical sections is plausible but unproven.

### Code review findings on 7b7a82b5 (from the diff read)

- Teardown chunking: every structural free (leaf table, PMD, PUD) completes
  inside one PTE-lock chunk; the exec.rs free_user_page_tables caller is an
  error-path RAII guard over a FRESH address space (no concurrent users);
  the Drop caller is last-Arc with the mm_struct.rs:1017 synchronize_rcu
  drain. Sound.
- Fork chunking exclusion: verified that ONLY free_user_page_tables frees
  intermediate tables (grep: all free_page_table* sites are in
  mmu_init.rs's teardown walk) — munmap/mprotect clear leaf PTEs only, so
  the "faults only ADD tables" argument holds.
- REOPENED WINDOW (documented risk, 0 observed occurrences): fork's
  snapshot-first re-pairs OLD VMAs with NEW PTEs — a concurrent PROT
  DOWNGRADE mprotect landing between the snapshot era and the walk gives
  the child (PROT_NONE marker PTE + writable snapshot VMA), which can trip
  the fault handler's "already mapped + no perms" kill — the same
  R-WRITESTORM-3 SIGSEGV/EACCES family. 0 Permission-denied lines in
  mmoh/v1/v2 serial. Worth hardening if this shape ever prints.
- Teardown root-table free is now lockless (was under the whole-walk lock)
  — benign: the tree is dead (last-Arc + rcu drain).

### Verdict

7b7a82b5 is NOT validated for merge on this evidence. It delivers a real
~order-of-magnitude strand-latency improvement (mmoh window) and its
exclusion analysis survives code review, but (a) the strand metric does
not reach zero (expected — per-mm locks + preemption still missing), and
(b) both controlled boots hit a full-system heap-grind freeze that the
8 baseline chain boots dodged. The freeze itself is attributed to a
pre-existing buddy/TCP-timer pathology (gdb-proven), so it is not a
regression mechanism in the commit — but the correlation stands at 2/2
vs 0/8 and needs either more boots or the buddy-hold fix before the mm
chunking rides into main. Candidate mm-side fixes (my territory, next
session): cap/trim the per-level O(2^order) marking (the interior marks
exist for the R9-14/R17 tripwires — they can be a debug/dfx-gated check
instead of an always-on hot-path cost), and/or split the heap lock or
make the marking amortized. The net-side trigger (TcpTxBatch growth to
~10^5 elements) belongs to whoever owns net/tcp.

USEGV-F (service-side errno-as-pointer): 120 victims in both mmoh and r5
windows — unchanged by chunking, as expected.

### Session-4 close-out addendum

- v2 never recovered: after the 19-min wedge it produced only ~1k more
  wakes across its remaining ~25 min (final wakes 3.38M, strand_max
  ratcheted to 1079s). The heap-grind freeze is effectively terminal for
  a boot, not a transient. 0 KERNPANIC/ALLOCTHROW to the end.
- The chain driver task was killed externally at 05:34 (r8 died with it
  at 32/45 min — clean partial: 0 WW-HIT, 0 panics, armed, storming).
  Relaunched from r9 at 05:35; chain continues on the frozen kernel.
- Cross-line convergence (launcher line Session I / A, relayed): the
  preempt_count leak producer is fingerprinted as a SpinlockGuard whose
  add/drop executes in TWO task contexts (hit sites: fs::inode::ICACHE,
  fs::locks::RECORD_LOCK_TABLE, VFS heap spinlocks) — same engine class
  as this hunt's wild writer. Their IPC wedge is shape-DISTINCT from my
  buddy grind (four binder holders INTERRUPTIBLE-parked in
  ioctl(BINDER_WRITE_READ) with todo=0 — lost-work, not lock-grind), but
  their MM-DROP-CTX witness (an mm owned by a live pid dropped in
  another pid's context — mm_struct.rs, THIS line's territory) fired in
  the same seconds as their 116s strand. Emerging picture: one engine,
  two downstream faces (buddy-meta corruption → lock-grind here;
  park/state corruption → lost-work there).
- Negative checks from my boots: MM-DROP-CTX = 0 in v1/v2/r7/r8/mmoh
  serials; sleeping tasks with leaked preempt counts = 0 in v2's
  periodic taskdumps (pre- and post-wedge). My windows show no engine
  fingerprint — the wedge stands on the buddy grind alone. The order-18
  coalescing anomaly (impossible with 229MB in use unless free-mark
  metadata lied) remains the one place engine corruption could enter my
  mechanism; correlation only, unproven.
- Next-session mm work, in order: (1) buddy free/merge marking cost —
  the O(2^order) mark-every-page loops under the single irqsave heap
  lock are the grind; make interior marking a dfx-gated debug check or
  amortize it (careful: R9-14/R17 tripwires rely on the marks);
  (2) per-mm locks (the wall endgame) once the wedge correlation is
  resolved; (3) if the engine capture lands, audit buddy meta and
  MmStruct Arc refcounts for blast radius; MM-DROP-CTX is the
  in-kernel witness already in place.

## Where this leaves the hunt (handoff)

The attribution machinery now exists and is proven end-to-end; the
remaining variable is the engine's firing rate under the reachable load
profiles. Next captures should:

1. Ride ww-hunt.py boots (Mode B) on long soaks — e.g. overnight chains
   of 45-min OH boots; any WW-HIT line in the gdb log is the capture
   (rip + regs + stack in the same log, symbolize with addr2line).
   Consider also chaining via fire6.py-style load (sdc cycles were the
   b77-era trigger context).
2. Extend the watch set to OTHER quiet statics if hits stay at zero —
   the same GUARD-cluster trick (page-exclusive region + watchpoints)
   applies to any never-written-after-init globals; PER_CPU slots stay
   unwatchable (legit writer every schedule) — for those, complement
   shadow + heal-report correlation remains the only signal.
3. F1b is the highest-value lead surfaced: supervisor uaccess writes
   bypassing COW/share protection (needs WP=1 to even observe). Whoever
   takes the user-half corruption line should own it.
4. b77/wt1's raw captures deserve re-reading against F2 before more
    effort goes into the "hpet BASE smash" framing.
