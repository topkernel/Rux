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

## Session 5 (continuation agent — buddy marking fix, wedge re-verification, per-mm lock design)

Inherited: the ww-s2 chain was mid-r9 with the driver alive and
self-continuing. Claimed as-is — r9 completed its full 2700 s (0
watchpoint hits, 0 heals) and the driver advanced to r10 on its own at
06:19; no restart needed. 0 WW-HIT across all ten armed runs so far.

### The buddy marking fix (a80974df)

The session-4 design bomb, defused on the marking-cost side. The heap
buddy marked every page of a block on each add_to_free_list /
remove_from_free_list / init_block via per-page `meta.get_mut()` calls
(bounds assert + UnsafeCell deref per page) under the single irqsave
heap lock. On the 1 GiB heap (2^18 pages, seeded as one order-18 block
since init_sized takes the largest aligned order that fits) an
order-scale add/remove walked 262144 heavy iterations per level; under
MTTCG with the other CPU spin-waiting on the same lock word (TCG
exclusive-access amplification), that is the minutes-scale,
effectively-terminal grind gdb caught in ww-v2. The net-side R50 note
(tcp_timer.rs) is the same pathology observed at order-14.

Fix (buddy_allocator.rs only):

- Per-page free state moves to a word-strided `AtomicU64` bitmap
  (`free_bits`, INV-HEAP-3): an order-18 fill is 4096 masked word RMWs.
  `BlockMeta.free` stays authoritative for list LEADERS; the R9-14
  double-free check consults the bitmap, which is exact for aliased
  interior pages too (previously interior bytes, best-effort).
- Interior `order` bytes are kept via a tight raw-pointer byte fill
  (single up-front bounds assert, no per-page call): the ORDER_MISMATCH
  arm and report fidelity of the R9-14/R17 tripwires are preserved.
- Refusal-kind labels follow the bitmap authority.

Leader metadata, free-list operations, and the merge logic are
byte-for-byte unchanged. MAX_ORDER stays 20 (config test asserts it);
no order above 18 can exist on this heap because get_buddy_idx(p, 18)
exceeds MAX_PAGES and coalescing stops. Known follow-up, deliberately
NOT in this commit: the split path double-fills each buddy half
(init_block + add_to_free_list) and the retained-half init_block fills
are dead writes — all cheap now, worth folding into the per-mm-lock
rework if that file is touched again.

### Wedge re-verification (the merge gate for 7b7a82b5)

Controlled boots, ww-boot.sh conditions (q35/smp2/4G/snapshot,
dfx=taskdump,watchdog,periodic), worktree kernel = HEAD with the buddy
fix on top of the chunking:

| boot | kernel | outcome |
|------|--------|---------|
| ww-v1 (s4) | chunked | froze ~23 min, terminal |
| ww-v2 (s4) | chunked | froze ~19 min, terminal (strand_max 1079s) |
| ww-bf-sanity (5 min) | chunked+buddyfix | healthy storm, wakes 540k, 0 panics |
| ww-bf-v1 (45 min) | chunked+buddyfix | FULL budget, wakes 11.35M advancing to the end, avg_run 4ms, 0 KERNPANIC/ALLOCTHROW, strand_max 470s |
| ww-bf-v2 (45 min) | chunked+buddyfix | FULL budget, wakes 11.64M, avg_run 4ms, 0 KERNPANIC/ALLOCTHROW, 1115 forks; strand_max 615s (one late single-task monster, wakes never stalled) |

bf-v1 detail: through the 19-23 min wedge window wakes advanced ~6k per
heartbeat continuously. strand_max ratcheted 90s → 206s → 470s at
wakes 6.2-8.3M while avg_run STAYED at 4ms and wakes never stalled —
single-task stranding (the fork-walk monopoly / structural-wall
residual), NOT the system-wide freeze of v1/v2 (both of which froze
wakes entirely and never recovered; v2 produced only ~1k further wakes
across its last 25 min vs bf-v1's 8M). bf-v2 mirrors it: a 615s
single-task monster near the end with wakes advancing normally
throughout.

Verdict: the heap-grind freeze mechanism is fixed by the marking
change; the 2/2-vs-0/8 correlation that blocked the chunking merge is
resolved at 0/2 freezes on the fixed kernel (bf-v1 + bf-v2), with the
wedge mechanism gdb-proven pre-existing and untouched by 7b7a82b5.
The mm chunking (7b7a82b5) + a80974df are MERGE-READY AS A PAIR —
main should take both together (chunking alone still carries the
buddy-grind wedge; the fix alone leaves the multi-second fork-walk
strands).

### Per-mm lock design (the wall endgame — spec for the implementing session)

Current shape: ONE global `PTE_MODIFY_LOCK` (Spinlock, per-arch static
in arch/*/mm/mm_ops.rs) serializes every leaf-PTE write + rmap update
across ALL mms. The chunking (7b7a82b5) reduced hold granularity to
per-leaf-table; the residual monsters (bf-v1's 470s strand, baseline's
400s class) are same-lock cross-mm queueing plus no kernel preemption.

Key discovery: `MmStruct.pgd_lock` (RwSpinlock) is VESTIGIAL — zero
callers of pgd_read()/pgd_write(). It can be repurposed/replaced by the
per-mm PTE lock without touching any existing user.

Site census (x86_64 + arch-neutral): page_fault.rs x6 (demand fault
map+rmap, COW fault), mmu_init.rs x8 (teardown chunks, munmap/mprotect
leaf clears, huge-leaf frees), mm_ops.rs x18 (brk, unmap, map helpers,
set_pte, fork-walk chunks), mm/rmap.rs x1 (unmap clearing), mm/
compact.rs x1 (migration PTE), syscall/memory.rs x3 (mmap/mprotect/
remap), drivers/ashmem.rs x1, ipc/binder.rs + ipc/sysv_shm.rs (IPC
line's territory — coordinate).

Hard constraints:

1. All-or-nothing per exclusion domain, per arch: if the demand fault
   takes mm-A's lock while the fork walk takes the global, they do not
   exclude each other. Every x86_64 site must switch in ONE change
   (riscv64 keeps the global until its own migration — gate through an
   arch hook, e.g. `pte_lock_for(mm)` returning the global on riscv64).
2. Cross-mm sequences: rmap-driven unmap (reclaim/swap/migration) can
   clear PTEs in SEVERAL mms under one logical operation — needs
   per-mm locks taken one at a time (never nested) or ordered by
   MmStruct address, with each mm Arc-pinned across its section.
   Kernel-half writes (copy_kernel_mappings, device alias installs)
   stay on a global kernel-table lock.
3. Fork: parent-side COW downgrades take the PARENT's per-mm lock per
   chunk; child-side writes go to unpublished private tables and need
   no lock (already true for the chunked walk).
4. Teardown: dying mm's own lock, per chunk — uncontended by
   construction (last-Arc + synchronize_rcu drain at mm_struct.rs:1017).
5. irqsave variants must survive the migration (the fault paths use
   plain lock(); rmap/compact/ashmem use lock_irqsave() — the per-mm
   lock needs both).
6. Lock ordering: vma_manager (RwSpinlock) → per-mm pte_lock →
   page_desc locks; document in docs/architecture/lock-ordering.md
   before the first hunk.

Suggested staging for the implementing session: (1) add the lock field
+ arch hook, keep the global everywhere (compile-only change); (2)
one-patch switch of all x86_64 sites + lock-ordering doc; (3) OH
45-min boots x2 + LTP fork/mmap families (the mm oh-suites) before
push. The IPC-line sites (binder/sysv_shm) should be migrated in
coordination with that line, not unilaterally.

### Chain status this session

- r9 (inherited, driver alive): full budget, 0 hits, 0 heals; driver
  self-advanced to r10 at 06:19 — the claim-then-watch protocol works.
- r10 (FROZEN = PRE-chunking kernel): wedged ~15 min in — serial
  frozen 06:34 onward, wakes stuck at 3.00M, si_busy=2000ms+2016ms
  (softirq running 2 s straight), ticks halved, 0 KERNPANIC. This is a
  heap-grind freeze ON THE BASELINE KERNEL (the same family ww-v2's
  gdb sample proved): tally now 1 terminal wedge in 11 frozen-kernel
  chain boots, vs 0/2 on chunked+buddyfix. Further evidence the grind
  pre-dates and is independent of the chunking commit.
- The chain driver tree (wrapper -> ww-chain.sh -> ww-hunt.py -> qemu)
  was killed externally again ~07:04 (session-infra cleanup, same class
  as the 05:34 kill in session-4's record). Relaunched from r11 at
  07:07 (`./run/ww-chain.sh ww-s2 100 12371 11`, session-5 log).
  0 WW-HIT in every armed run to date (r1-r10 + all manual boots).

## Session 6 (continuation agent — per-mm PTE locks implemented per the session-5 design)

Inherited: chain mid-r11 (driver alive, self-continuing). Claimed as-is;
r11 completed its full 2700 s (0 hits, 0 heals) and the driver
self-advanced to r12 at 07:52. Merged origin/main (10499dd0 — the
launcher line's binder/sched-preempt work; the handoff's pair-merge of
7b7a82b5+a80974df into main had not landed yet at merge time).

### The per-mm PTE lock (681da2da, on top of the merge)

The session-5 spec, executed with one design extension the spec left
open: what to do about the IPC sites (ipc/binder.rs, ipc/sysv_shm.rs)
that must keep excluding against everything but live in ANOTHER line's
forbidden territory. Solution: make the lock two-level.

- `GlobalPteLock` (RwSpinlock<()>) replaces the `PTE_MODIFY_LOCK`
  Spinlock static in BOTH arches' mm_ops.rs. An EXCLUSIVE holder
  (`.lock()/.lock_irqsave()`) gets exactly the old whole-domain
  semantics. The IPC sites' code is untouched — they still call
  `PTE_MODIFY_LOCK.lock()` — and now exclude against every per-mm
  section through the epoch.
- `PteLock` per MmStruct (the vestigial `pgd_lock` RwSpinlock, zero
  callers, repurposed as the spec prescribed). On x86_64 its section =
  epoch SHARED + the mm's spin: same-mm writers serialize, different-mm
  writers run CONCURRENTLY — the fork-walk monopoly breaker.
- Arch hook `pte_lock_for(mm)` in both arches: x86_64 returns the mm's
  PteLock; riscv64 returns ONE global-mode handle (every section
  degrades to the exclusive epoch — behavior identical to the old
  single lock, as the spec's constraint 1 required).

Switched to per-mm sections (x86_64 + arch-neutral, all in one change
per the all-or-nothing constraint): page_fault x5 (demand/COW/swap +
stack growth), brk, unmap/zap, set_range_permissions, the fork walk's
parent chunks (copy_page_table_cow now takes the parent MmStruct),
handle_cow_fault (takes the owning mm; child-settid path uses the
child mm from the task), mmap/mprotect/mremap/mlock eager sections,
rmap unmap + compact migration (per-mm, one section at a time, mm
Arc-pinned — the loop structure already existed), ashremap, and mm
teardown via the DYING mm's own lock from MmStruct::drop (per the
spec's constraint 4). `free_user_page_tables` grew a `pte` param;
exec's unpublished error-path tree and the synthetic test trees use
`FRESH_TREE_PTE_LOCK` (arch-neutral static).

Kept GLOBAL deliberately: `map_user_region` + kernel-root maps (root-
PPN-only interface, callers are exec/fresh-table/device cold paths) and
the IPC sites (territory). Documented as follow-ups.

Deadlock-freedom transfer argument (in lock-ordering.md, INV-LOCK-7):
every nesting edge that the new scheme forbids (write-inside-read,
same-mm self-nesting) would ALSO have self-deadlocked on the former
global Spinlock, so no live path can regress; vma_manager -> PTE
section -> page_desc ordering is unchanged at every site. The one
deliberate semantic narrowing, argued in the code comment: teardown
chunks no longer globally exclude OTHER mms' faults from reallocating
just-freed frames — safe because a frame's last teardown access is the
free itself and the buddy heap lock serializes free vs alloc.

Builds: x86_64 and riscv64 both clean (no new warnings; the codebase
carries ~2300 pre-existing).

### Verification

- riscv64 (global-mode PteLock paths — the neutral files'
  pte_lock_for): minimal-rootfs boot to mrsh shell, 0 panics; in-kernel
  unit tests 1085/1088 — the 3 failures are pre-existing
  config_test.rs size asserts (expect heap 512 MB; Kernel.toml carries
  1024 from main's merge; unrelated). PREEMPT-LEAK prints at idle
  symbolize to `cpu::wfi` — the launcher line's tripwire noise from
  the merged main, not the PTE path.
- x86_64 sanity (5.5 min, ww-boot.sh conditions + dfx=taskdump,
  watchdog,periodic): 0 panics, wakes 525k, storm healthy.
- x86_64 v1 (45 min): run by session 7 (session 6's attempt was killed
  ~10 min in by the user pause) — see Session 7.
- x86_64 v2 (45 min): run by session 7 — see Session 7.

### Chain status this session

r11 full budget clean (self-advanced); r12 started 07:52, armed,
storming. 0 WW-HIT cumulative (r1-r12 + all manual boots). Process
tree recorded for the external-kill watch: wrapper 1542328 ->
ww-chain.sh 1542339 -> ww-hunt.py -> qemu (per-run pid in
run/ww-chain-s5.log).

## Session 7 (continuation agent — per-mm PTE lock validation completed)

Inherited: pause snapshot 9ed98546 (session-6's v1 attempt killed ~10 min
in by the user pause — healthy partial: wakes 1.5M, 0 panics, strand_max
46s at kill time; chain driver stopped mid-r12). Nothing was running at
claim time; working tree clean.

### Build verification at HEAD

- x86_64: `PLATFORM=x86_64 make build` is a cargo no-op reproducing the
  byte-identical bzImage (md5 f9a733cd...), so the session-6 sanity boot
  and this session's v1/v2 all ran the committed 681da2da code.
- riscv64: clean rebuild (1 pre-existing manifest warning). riscv
  `pte_lock_for` re-inspected: returns the single `PTE_GLOBAL_HANDLE`
  (global-mode degradation) as designed.

### The 45-min OH boots (ww-boot.sh conditions + dfx=taskdump,watchdog,
### periodic, KERNEL = worktree bzImage, no gdb)

| boot | budget | wakes | panics | strand events | >5s | >60s | strand_max |
|------|--------|-------|--------|---------------|-----|------|------------|
| bf-v1 (global lock baseline) | full | 11.35M | 0 | 26584 | 230 | 5 | 470s |
| bf-v2 (global lock baseline) | full | 11.64M | 0 | 25359 | 447 | 3 | 615s |
| ww-pml-v1 (per-mm) | full | 10.67M | 0 | 21781 | 191 | 2 | 66s |
| ww-pml-v2 (per-mm) | full | 9.23M | 0 | 25681 | 364 | 1 | 202s |

- v1: strand_max ratcheted to 43s by minute 7 and stayed flat for 27+
  minutes (the 66s max landed late); wakes never stalled, avg_run 3-5ms,
  0 KERNPANIC/ALLOCTHROW, 1059 forks.
- v2: one early-boot single-task monster (pid 303, 202s; holder chain
  pc=0/pc=1 — no preempt-count leak, no lock parking; wakes advanced
  continuously through it, avg_run 4ms), then flat. 1017 forks,
  0 panics.
- Forensics: USEGV-F victims 0 in both boots, MM-DROP-CTX 0,
  PREEMPT-LEAK 3 (v1, wfi-class launcher-line noise) / 0 (v2).
- Verdict vs the gate: both boots ran the full budget with wakes
  advancing and 0 panics; strand_max 66s/202s is 2.3-9x below the
  470-615s residual baseline. The fork-walk cross-mm monopoly is
  broken — no 400-600s class monster in either boot, no system-wide
  stalls. 681da2da is VALIDATED for merge (on top of the already
  merge-ready 7b7a82b5 + a80974df pair).

### riscv64 side

- Session-6's records verified against the raw logs: smoke boot to
  mrsh, 0 panics; in-kernel unit tests 1085/1088 (3 failures are the
  pre-existing config_test.rs size asserts).
- bindmkdir musl driver (test/bindmkdir_boot.py, launcher-line sc28
  probe): first run SIGILL'd at a `vsetivli` (0x1c01a) — HOST TOOLCHAIN
  issue, not kernel: the worktree lacks toolchain/riscv64-rux-linux-musl
  (built artifact, gitignored) so the probe fell back to distro glibc
  static with RVV ifuncs on a V-less QEMU cpu. Fixed by symlinking the
  main checkout's SDK (read-only).
- With the musl probe: BMD-DONE reached, 22/32 steps rc=0, 0 panics,
  deterministic across runs. The 10 non-rc0 steps are all in the
  chroot/string-prefix-jail block (BMD 21+ cascade from
  `chroot("/a")` inside the /a2 jail) plus the two negative-test
  primes (which return the CORRECT ENOENT but the driver's crude
  rc-filter counts them). A/B against a merge-base build (49f015f8,
  no per-mm locks; temp worktree, same SDK): failure set
  byte-identical. 681da2da touches no VFS/path code, so this is the
  pre-existing mount-family semantics state (Session K territory), NOT
  a regression. All mm-exercising steps (bind mounts, negative-dentry
  creates, self-bind, /dev bind, chroot /a2 jail, open /dev/null in
  jail) pass.

### Chain status this session

- r13 (relaunched at claim, frozen kernel, armed): full 2700 s, 0
  WW-HIT, 0 heals; driver self-advanced to r14.
- r14 (armed, storming): killed externally at ~22 min (~13:04, task
  infra stopped the background chain task; my v2 boot task survived
  the same window). Clean partial: 0 WW-HIT, 0 panics. Kill event +
  pid tree recorded in run/ww-s7-proctree.txt (same class as the
  session-4/5 kills; parents are the zcode-cli shell, so
  session-infra cleanup remains the killer).
- Relaunched from r15 at 13:06 (run/ww-chain-s7b.log); r15 storming.
  Cumulative: 0 WW-HIT across r1-r14 (r12 was a pause-kill partial).

## Session 8 (reconstructed from artifacts — quota death at 14:36, no doc written)

- 0c29a23d `fix(mm): skip foreign (boot-reserved) frames in every user-PTE
  walk` — the sc83-deadlock-direction fix (full story in the commit
  message): drm GEM dumb buffers mmap(2)'d from kernel-heap phys were
  treated as ordinary user pages by the PTE walkers (fork get_page on
  foreign frames; teardown put_page + free_pages → the one-loud-print-
  per-frame REFUSED storm from a free context, the sc83 both-CPU spin
  ingredient). Fix generalizes the framebuffer special cases into
  `is_foreign_user_frame` at every leaf walk site (teardown, unmap,
  COW-copy) + rate-limits the REFUSED/WILD prints.
- r15 (chain): full 2700 s, 0 WW-HIT / 0 heals, driver self-advanced to
  r16 (run/ww-chain-s7b.log).
- r16 (chain): killed externally at ~8 min (14:05) — clean partial:
  armed (8-point set), storm active, 0 WW-HIT / 0 panics. The kill
  timing matches the predecessor freeing the QEMU slot for the riscv64
  verification boot (discipline chain-1 + verify-1), not infra cleanup.
- ww-ff1 (verification boot of 0c29a23d, 14:02, worktree bzImage built
  13:58): still running at quota death — verdict collected by session 9.
- In-flight at death: the buddy split-path fold (session-5 follow-up)
  edited 14:03:45, frozen unreviewed in snapshot 39bc68d9. A riscv64
  `make test` (unit-test feature, built 14:04:20, started 14:04:14) was
  also left running with its serial piped to the dead session (output
  unrecoverable).

## Session 9 (continuation agent — fold review/landing, 0c29a23d verdict, chain resumed)

### Inheritance reconstruction

- Claimed BOTH leftover QEMUs after cmdline verification (both cwd =
  this worktree): 1624529 = ww-ff1 (the 0c29a23d OH verification) and
  1625109 = the riscv64 make-test run. The make-test run was reaped:
  its output pipe led to the dead session (no recoverable results) and
  its binary was ambiguous w.r.t. the fold edit (source changed at
  14:03:45, 35 s before that build finished) — zero information value;
  superseded by my own captured run below.
- Working tree was clean (the snapshot commit carried everything).

### 0c29a23d VERIFIED (ww-ff1, full 45-min budget, kernel-enforced kill)

45:45 wall, 73749 serial lines, full storm to the end:
wakes=13.31M picks=12.99M avg_run=2ms, **strand_max=2982 ms** — the
best strand profile ever recorded on this line (previous best 66 s on
ww-pml-v1; global-lock baselines 470-615 s). Plausible contributor:
the foreign-frame skip removes drm-GEM teardown/copy work from the
PTE-walk critical sections. 0 KERNPANIC / 0 ALLOCTHROW / 0 kernel
faults, 0 REFUSED / foreign-frame prints (no printk flood — the fix's
wedge vector is gone), 1005 forks, cow 8435 lines (baseline), USEGV-F
5 victims at the known rip=0x7ff082996bac errno-as-pointer shape,
MM-DROP-CTX 0, PREEMPT-LEAK 0, 0 heals. **Gate passed: merge-ready.**

### Buddy split-fold: reviewed, validated, landed (28630426)

Review of the 39bc68d9 hunk (alloc_blocks split loop):

- The removed `init_block(buddy, .., true)` before `add_to_free_list`
  repeated the full marking byte-for-byte (leader meta, interior order
  bytes, free bitmap) — genuinely redundant.
- The removed per-iteration `init_block(retained, .., false)` was
  fully covered by later writes: the union of the successive
  add_to_free_list calls (split-off suffixes) plus the loop's final
  `init_block(page_idx, order, false)` (kept prefix) rewrites every
  page of the block exactly once.
- Unobservability argument verified in code: every alloc_blocks /
  free_blocks caller (GlobalAlloc::alloc, alloc_page_raw,
  dealloc_untracked, free_page_raw) and every free-bits / order-byte /
  leader-meta consumer (R9-14 tripwire, merge-loop buddy check,
  buddy_stats) runs under the same irqsave heap lock; the bitmap is
  cleared for the whole block by remove_from_free_list before the loop.
  The early-break corner (buddy_idx >= MAX_PAGES) only changes
  diagnostic labels on a region that path already leaks (pre-existing).

Validation: riscv64 in-kernel unit tests **1085/1088** (run/
ww-fold-rv-test.log; the 3 failures are the pre-existing config size
asserts, byte-identical failure set to the pre-fold session-6
baseline); x86_64 OH sanity boot (ww-fold1, 5.5 min, full storm):
wakes 689k, avg_run 10ms, strand_max 2384 ms, 0 panics / 0 refusals.
Committed as 28630426 (the wip snapshot 39bc68d9 was soft-reset away
— unpushed, so no history rewrite escaped the worktree; the quota-
death marker lives in this record instead).

### Chain status this session

- Relaunched from r17 at 14:50 (`./run/ww-chain.sh ww-s2 100 12371 17`,
  run/ww-chain-s9.log): r17 armed (gdb pid 1636024) and storming.
  Process tree for the external-kill watch: bash -c 1635836 →
  ww-chain.sh 1635847 → ww-hunt.py 1635852 → qemu 1635853.
- Note reconstituted for future sessions: the chain boots
  `run/ww-hunt-frozen-bzImage` (frozen pre-mm-rework kernel, 02:35
  Oct 21) BY DESIGN — the watchpoint set must stay paired with
  `run/ww-hunt-frozen-rux.elf` for symbolization, and a mid-chain
  kernel change would silently invalidate the hunt. mm fixes landing
  on the branch do NOT ride the chain; they soak via controlled boots
  and main.
- Hunt-kernel REFREEZE at 15:10 (after 17 armed zero-hit runs, per the
  escalation plan): old pair archived as `run/ww-hunt-frozen-*.{s2a}`
  (the r1-r17 logs' addresses symbolize against those), new pair =
  the 28630426 build (bf779126 — mm three-piece + 0c29a23d + fold).
  ww-gdbscript.py re-derives the 8-point set from the new ELF
  (GUARD_CANARY_LO moved 0xffffffff80a65000 → 0xffffffff80a7a000);
  r18+ arm against the current kernel. Hardware watchpoints fire on
  the wild STORE itself, so the mm fixes do not mask captures — they
  only make the boots healthier (longer storm windows per 45-min
  budget). r17 (running on the old kernel, already loaded in memory)
  is unaffected by the file swap.
- Cumulative: **0 WW-HIT across r1-r17** (r12 pause-kill, r16 slot-
  free kill partials). The engine did not fire in any armed window.

## Session 10 (continuation agent — chain reclaimed alive, watchset expanded to 10)

### Inheritance reconstruction — the handoff's "driver died" was stale

At claim (15:38) the session-9 chain tree was STILL ALIVE under the
main session's zcode-cli (wrapper 1635836 → ww-chain.sh 1635847 →
ww-hunt.py → qemu): r17 had self-completed its full 2700 s (15:35) and
the driver had self-advanced to r18, which armed correctly against the
REFREEZEN pair (WW-ARMED 8, GUARD=0xffffffff80a7a000 — the swap landed
as designed) and was storming. No restart was needed at claim; the
handoff's "relaunch from r19" was only executed later, after the tree
died (below). Lesson for handoffs: check `ps` before trusting the
driver-dead assumption — the chain has now survived its owning
session's death once (the bash wrapper under a subagent-shell snapshot
outlived session 9's window by 15+ min).

- r17 (LAST run on the .s2a pair, full 2700 s): 73062 lines, wakes
  11.32M, strand_max 30.9 s, 0 KERNPANIC/ALLOCTHROW, 0 WW-HIT, 0 heals,
  120 USEGV-F lines (known errno-as-pointer family), 0 MM-DROP-CTX,
  0 PREEMPT-LEAK. Clean.
- r18 (FIRST run on the refrozen 28630426 pair, killed externally at
  15:50:00 = 15/45 min): 53029 lines, wakes 3.60M, full storm, strand
  profile = the healthy new-kernel shape (one 9.2 s single-task
  outlier pid 765, everything else <= 2 s — consistent with ww-ff1's
  2.98 s baseline, no 400 s-class signal), 0 panics, 0 WW-HIT, 0 heals.
- The 15:50:00 kill: whole-tree (wrapper/chain/hunt/qemu/gdb all gone
  in the same second, logs truncated mid-storm, no teardown lines) —
  the known session-infra cleanup form, this time reaping session-9's
  leftover tree 12 min into this session. My capacity-probe experiment
  overlapped the window (15:48-15:50) but killed only its own PIDs
  (verified: the signal-15 attribution line names my script's bash as
  the killer of my own QEMU). Kill datum recorded in
  run/ww-s10-proctree.txt.
- Chain relaunched from r19 at 15:52 as THIS session's background task
  (run/ww-chain-s10.log).

### Watchset expansion to 10 points (the archive's next-step #2, done)

Gap found by re-reading the victim history: the static the engine
smashed TWICE (b77/wt1 — hpet::BASE at 0xffffffff80fc8888) sat OUTSIDE
the GUARD cluster and was watched by NOTHING; all 8 hardware points
were on cluster members/canary spots. r1-r18 therefore ran with the
one twice-confirmed victim unwatched.

Capacity probe (run/ww-cap1: bare boot of the frozen kernel + OH
ramdisk, own QEMU port 12399, 10 hw watchpoints for 30 s):

- All 10 arm cleanly on this QEMU 10.2.1 gdbstub — no insertion errors.
- Positions 9-10 (JIFFIES, timer::LAST_TICK — both 100 Hz hot) each
  fired 2687x in 30 s: firing works at every armed position. This
  RETROACTIVELY validates the 8-point chain set (armed since r1, but
  firing at positions 5-8 had never been observed).
- 0 hits on the 8 quiet candidates during the idle bare boot — no
  post-init writers on hpet::BASE, console::UART, TSC_MULT,
  PHYS_RAM_BASE, CMDLINE_PTR, GDT_PER_CPU, virtio MMIO_OFFSET,
  JIFFIES_BASE.

New points (deployed from r19; per-run cmd files regenerate from
ww-gdbscript.py):

- Point 9 HPET_BASE: only post-attach writers are the heal paths
  (validated_read_base / read_time_nominal — recognizable rips). A hit
  is a corruption event either way; the rip classifies engine vs heal.
- Point 10 TSC_MULT (0xffffffff80be8fa8): timekeeper-class middle
  qword of the TSC trio (OFFSET/BASE/MULT at 0xffffffff80be8f90/8fa0/
  8fa8) — the wt1 fingerprint was a WIDE smash into a timekeeper
  neighborhood, so the trio is the highest-affinity victim geometry.
  Only store is the HPET-gated TSC-fallback calibration, which returns
  early forever once hpet::ready() — zero legit writers on these boots.

r19 verified armed with the 10-point set (points 9/10 at 0x...fc8888 /
0x...be8fa8). PER_CPU slots stay unwatchable as always (legit writer
every schedule).

### Cross-line incident (main-session coordinator): shared-rootfs lock

Session 9's riscv64 unit-test QEMU (1625109-class leftover, PID
1636556, started 14:51) outlived its session with stdout already fully
written (TEST SUMMARY 1085/1088 at log line 20491 — results long since
recorded). Because THIS worktree's test/rootfs.img was a SYMLINK to
/home/william/workspace/Rux/test/rootfs.img (created by session 7
alongside the toolchain symlink) and that boot had no -snapshot and
default locking, it write-locked the MAIN repo's shared rootfs and
blocked the main-session regression gate twice — the coordinator
initially misread the lock refusal as a kernel regression and reverted
a good merge (since re-landed on main as 9dc61140 via revert-of-revert;
the mm three-piece PASSED their full gate on an image copy).

- Machine reaped at 15:39 after cmdline+cwd verification.
- Durable fix at 15:42: the worktree's test/rootfs.img symlink is now
  a PRIVATE 1.3 GiB copy (own inode) — no path from this worktree
  touches main-repo mutable images anymore. Discipline rule for this
  line: worktree-local symlinks into main's MUTABLE images are
  forbidden (read-only SDK symlinks are fine).
- The only other machine this line runs (the ww-s2 chain) boots only
  its own frozen bzImage + oh-display images, all -snapshot +
  file.locking=off.

### Commits this session

- cf775210: land the refrozen kernel pair binaries (session 9's
  e8e742d4 recorded them but the swap itself was uncommitted at quota
  death) + r17/r18 artifacts.
- 223c76e5: watchset expansion + capacity probe scripts/artifacts.
- Both pushed via ssh.github.com:443 (e8e742d4..223c76e5).

## Where this leaves the hunt (handoff)

The attribution machinery now exists and is proven end-to-end; the
remaining variable is the engine's firing rate under the reachable load
profiles. Next captures should:

1. Ride ww-hunt.py boots (Mode B) on long soaks — e.g. overnight chains
   of 45-min OH boots; any WW-HIT line in the gdb log is the capture
   (rip + regs + stack in the same log, symbolize with addr2line).
   Since r19 the set is 10 points (cluster 8 + hpet::BASE +
   TSC_MULT). Consider also chaining via fire6.py-style load (sdc
   cycles were the b77-era trigger context).
2. Watchset room remains: capacity is proven >= 10 (ww-cap1); if hits
   stay at zero, next candidates from the quiet probe are
   console::UART, PHYS_RAM_BASE, CMDLINE_PTR, GDT_PER_CPU,
   virtio MMIO_OFFSET (all verified 0 post-init writers at idle —
   re-verify storm-quietness before arming, a flood auto-stops the
   chain). PER_CPU slots stay unwatchable (legit writer every
   schedule) — for those, complement shadow + heal-report
   correlation remains the only signal.
3. F1b is the highest-value lead surfaced: supervisor uaccess writes
   bypassing COW/share protection (needs WP=1 to even observe). Whoever
   takes the user-half corruption line should own it.
4. b77/wt1's raw captures deserve re-reading against F2 before more
    effort goes into the "hpet BASE smash" framing.

## Session 12 (continuation agent — mm handoff landed: VMA fd/offset bookkeeping)

### Inheritance reconstruction

- Pause snapshot 5d5a621e carried the mm-lane handoff item (file-LOAD
  VMA fd/offset, from launcher Session L) mid-flight: exec-time
  per-segment interp/exe VMAs + procfs pin-first path resolution +
  mremap file-attr carry + a loader_vma probe, all built but only
  partially validated (the 20:07 probe run predates the probe's final
  check fixes).
- Chain: r20 died externally at 15/45 min, r21 died with the user
  pause at ~26 min — both clean partials (armed 10-point, 0 WW-HIT,
  0 panics). Relaunched from r22 at 23:50 (run/ww-chain-s12.log;
  process tree in run/ww-s12-proctree.txt). r22 ran its full 2700 s
  clean (0 WW-HIT, 0 heals, 0 panics) and the driver self-advanced.

### Ground truth before touching the mmap overlay path

Re-read the samgr-lane forensics (oh-sc96-serial.log RWFRZ dumps):
EVERY unstamped-VMA sighting (66 park frames across 10 distinct rips)
is the SAME VMA — the kernel-exec-mapped flat interpreter span
0x3fbf000000-0x3fbf138000 fd=-1 baseoff=0x0 filesz=0x0. The park rips
(0x116dea/0x12143d/0xd3466/...) are ld-musl's own code (the ripbytes
0x00cab80c75daf883 = futex-park sequence; sys=0xca is futex on
x86_64) — Session L's "park-site lib not in the images' .so set"
mystery was the INTERPRETER all along, invisible because the old exec
path mapped it as one flat anonymous VMA. Userspace-loader-mapped libs
were NOT broken (their data-segment pointers resolved — the MAP_FIXED
overlay surgery in MmStruct::mmap preserves fd/offset, and the
merge/stamp window is closed by the fd comparison in can_merge). So
the fix is the exec path — exactly what the snapshot WIP implements.

### What landed this session (on top of the snapshot WIP)

- vfs.rs `open_file_arc_nofd`: the WIP's boot-exec pins (PID-1 exe +
  interp path identity) never took because open_file_arc went through
  file_open, whose fd install needs a current-task fdtable — at the
  init= exec none exists. The new resolver builds the File Arc via
  path_lookup + the same inode/dentry/ops wiring as file_open, with no
  fdtable interaction (and no inotify noise — kernel-internal open
  semantics). open_file_arc (execve path) now delegates to it, one
  code path for boot and post-boot exec.
- Loader-overlay sequence locked in by probe P5 (musl ld.so shape:
  anon PROT_NONE MAP_FIXED reservation + adjacent same-flags file LOAD
  overlays with discontinuous offsets + fd closed): both LOADs keep
  path + exact offset, the reservation holes survive as ---p lines.
  This is the regression guard for the "loader reservation overlay"
  half of the handoff.
- mremap_carry_file_attrs doc-block reattachment (the b93 sys_mremap
  trace note had been swallowed into the helper's doc comment).

### Validation

- test/loader_vma_boot.py (x86_64, dynamic probe as init):
  LVMAP-PASS — P1 boot-exec interp 4 lines with path + 3 non-zero
  offsets, P2 boot-exec exe 4 lines with path, P3 mremap keeps
  path+offset+content, P5 loader overlay fidelity, P4a/P4b execve'd
  child interp+exe with paths (child maps: per-segment offs
  0/0x76000/0x136000/0x138000 with /lib/ld-musl-x86_64.so.1).
  Before open_file_arc_nofd the boot instance printed empty paths for
  both exe and interp (LVMA-DBG pin-miss on every segment).
- riscv64: in-kernel unit tests 3 FAIL — the pre-existing config size
  asserts (KERNEL_HEAP_SIZE/FUTEX_*), byte-identical to the session
  6/7/9 baseline; minimal-rootfs boot to /bin/sh 0 KERNPANIC (the
  first smoke attempt accidentally booted the make-test feature
  kernel, whose post-test PANIC is the test harness itself).
- OH verification boot (ww-lvma1, worktree bzImage, 17 min, killed at
  budget by own PID — note: ww-boot.sh's SECS env is stale, it execs
  QEMU unbounded): 57403 lines, 861 forks through the modified exec
  path, full storm (wakes 3.79M, avg_run 4ms, strand_max 9.6s —
  healthy new-kernel shape), 0 KERNPANIC/ALLOCTHROW, 0
  Permission-denied victims, 0 MM-DROP-CTX.

### Forensic payoff (for both lines)

With the interp now a set of FileBacked VMAs carrying a pinned file
and Linux-parity pgoff, every 0x3fbf-region rip converts to a file
position: file_off = vma_off + (rip - vma_start) into
/lib/ld-musl-x86_64.so.1 (or the exe) — the samgr line's holder-chain
symbolization and this hunt's park-site decoding both stop being
blind there. The maps path column survives fd close (pin) on all
three paths: boot-exec (PID 1), execve, and loader overlays.

## Session 13 (continuation agent — watchset to 15, metawatch landed, file-fault path dedup)

### Chain stewardship

- Claimed with the s12 chain tree ALIVE (r23 armed, 10-point set,
  storming — the handoff's "may have died" was again stale; ps first).
- r23 verdict: EXTERNALLY KILLED clean partial at 15/45 min. The whole
  tree (wrapper/chain/hunt/qemu/gdb) died in the s10-pattern silent
  sweep at ~00:50 (serial last write 00:50:xx, no teardown lines, no
  budget-spent line). The guest was HEALTHY to the last line (ticks
  [100,100], si_busy 112ms, wakes advancing, strand_max 3.6s); the final
  `STALE-CLAIM-RESET` line is healthy-boot noise (the qs1 probe boot
  prints the same mid-storm). 0 WW-HIT, 0 panics. Cumulative: 0 WW-HIT
  across r1-r23 partials.
- Relaunched from r24 at 01:19 as this session's background task
  (run/ww-chain-s13.log; pids in run/ww-s13-proctree.txt).

### Watchset expanded to 15 points (archive next-step #2, done)

- ww-qs1 (storm-quietness probe, full OH disk boot, own port 12398,
  15.5 min): the five candidates that run/ww-cap1 had only
  idle-verified — console::UART, PHYS_RAM_BASE, CMDLINE_PTR,
  GDT_PER_CPU (arch::x86_64::trap), virtio-pci MMIO_OFFSET — stayed
  silent through a full storm (864 forks, wakes 3.57M, strand_max
  9.48s, 0 panics, 0 hits). All five are chain-safe.
- ww-cap2: 15 SIMULTANEOUS hardware watchpoints arm completely on this
  gdbstub (15/15 confirmations, WW-ARMED 15, no insertion errors) —
  verified against a fresh guest before riding the chain.
- r24+ rides the 15-point set (ww-gdbscript.py regenerates per-run cmd
  files; r23 kept its already-generated 10-point cmd by design).
- Ops lesson (recorded for every future session): a `rm && qemu ... &`
  compound backgrounds the WHOLE list — the captured $! is a subshell,
  `kill $!` orphans the QEMU (one held port 12398 for 20 min and served
  a stale guest to a later probe's gdb). Start experiment QEMUs from a
  script file as a background task, kill only verified own PIDs.

### buddy-meta shadow scan landed (dfx=metawatch, commit 434c2e69)

The unified-engine hypothesis's buddy face (the ww-v2 order-18 grind)
now has an in-kernel witness, per the session-4 next-step list:

- mm/buddy_allocator.rs grows `buddy_meta_shadow_scan`: derive the free
  set from the free lists into a claim-guarded scratch bitmap and
  cross-check INV-HEAP-1/2/3 (link bounds, leader order/free bytes,
  2^order alignment, prev-mirror links, exact free_bits equivalence, no
  double listing). Pass 1 lock-free from the timer cadence (racy field
  reads tolerated, census precedent); any dirty pass 1 re-audits UNDER
  the heap irqsave lock; only a PERSISTENT mismatch reports. Findings
  classify into 9 kinds (LinkOob / OrderMismatch / FreeByteClear /
  Unaligned / PrevBroken / BitmapSetUnlisted / BitmapClearListed /
  DoubleListed / HopOverflow).
- dfx/metawatch.rs (switch bit 27) drives it every 10 s from the timer
  softirq: one METAWATCH-ARMED line per boot, ZERO traffic until a hit;
  transients counted silently. A confirmed hit prints META-SMASH with
  finding details — the buddy-face capture. NOTE: hunt boots that want
  it must add it to EXTRA_APPEND explicitly; the ww-s2 chain boots do
  NOT ride it (frozen-kernel purity).
- Validation: metawatch-selftest=1 PASS on first bare boot (injected
  free_bits bit flagged as BitmapClearListed under the heap lock,
  restored, re-audit clean; METAWATCH-SELFTEST chain
  buddy_meta_selftest <- metawatch::tick <- timer softirq). OH soak
  (ww-mwoh1, 15 min full storm): **0 META-SMASH — zero false
  positives**; the under-lock confirm gate absorbed every mid-mutation
  race under millions of allocs/frees. A/B control (ww-mwctl, same
  kernel/flags/contention, metawatch OFF): si_busy tops 2279 ms vs
  mwoh1's 3916 ms, strand_max 45.5s vs 44.3s — same class, i.e. the
  scan cost is invisible at boot granularity. Incidental finding:
  si_busy 1-2s classes appear whenever >=3 QEMUs share the host
  (bf-v1's 2422ms historical value had the same shape with the chain
  concurrent) — treat softirq-busy readings as host-load-sensitive.

### Launcher-relay item: file-fault path audit + lookup dedup

Coordinator relayed launcher Session M's finding: post-exec launcher
burns its whole 3s watchdog in KERNEL time loading ~100MB of ArkUI libs
(their side: readahead/fault-around/page-cache policy in bio.rs/ext4;
Session N). This line's side — per-fault mm work — audited and the
cheap pure-win subset LANDED (see commit; validated):

- Audit of the x86_64 file-fault path (page_fault.rs demand fault):
  per fault it did THREE full VMA-tree finds (entry, EOF/SIGBUS gate,
  fill arm), up to FOUR vm_file pin-table resolutions, TWO
  sched::current() round trips, and an unconditional 4KiB pre-zero
  before the read. x25600 faults per 100MB lib load, multiplied by
  every spawn.
- The dedup (mm territory, riscv64 twin untouched): capture vma_start /
  vm_file Arc / file_offset / effective_size ONCE under the entry guard
  (dropped before any read_at — R73 rule intact); the SIGBUS gate, the
  SHARED-devfs path, and the fill arm all use the captures. Pre-zero is
  skipped only when a full-page read is planned; short/failed reads
  re-zero (no stale kernel data can reach a user page). The fill arm's
  bounds now use the devfs-aware effective size (devfs files grown
  after mmap serve their new pages; regular files identical to before).
- Validation: test/loader_vma_boot.py LVMAP-PASS (P1-P5: boot-exec
  interp/exe per-segment faults, mremap file-attr carry, loader overlay
  — all through the new path); OH boot ww-fd1 (below).
- NOT done (documented for the implementing session, the 16x-class
  win): fault-around — map up to 16 pages per fault from the page
  cache in one PTE-lock section (Linux filemap_fault/do_fault_around
  shape; MAP_PRIVATE around-pages map without write). Compounds
  directly with Session N's readahead: 25,600 faults per 100MB lib
  collapse to ~1,600. Needs its own LTP mmap-family + OH validation.
- ww-fd1 (OH boot, dedup kernel + metawatch rider, 15 min): full storm
  (830 forks, wakes 2.67M, 136k serial lines), 0 KERNPANIC /
  ALLOCTHROW, **0 SIGBUS forensics prints** (the relocated gate fires
  nothing false), 0 Permission-denied victims, 0 META-SMASH (second
  clean metawatch soak). strand_max 81s is the >=3-QEMU host-contention
  class again (see the si_busy note above); wake rate and fork count
  match the mwoh1/mwctl controls. A clean-host stime A/B for the
  launcher relay's 3s-burn question belongs to a dedicated window —
  the dedup is a per-fault constant-factor win (3 finds + 4 pin
  lookups + 2 current() trips + 1 memset removed per fault), the 16x
  class win is fault-around + readahead, still open.

### Session 13 close-out

- Commits: 434c2e69 (metawatch), 1bbc3fbe (watchset 15 + probe
  scripts), fault-path dedup + this archive (see git log).
- Chain at close: r24 (first full 15-point run) in flight; 0 WW-HIT
  cumulative r1-r24.
- Health baseline confirmations this session: the 9.5-9.7s strand_max
  class reproduces on clean windows (qs1, r24); LVMAP-exec-path boots
  fork at control rates (864/15min, 830/15min).

## Session 14 (continuation agent — mm-side fault-around landed)

### Chain stewardship

- The handoff's "r25 died with session 13" was STALE AGAIN (third
  occurrence of the pattern): at claim the whole tree was alive
  (wrapper 2053720 → chain → hunt → qemu 2361211), r25 armed with the
  15-point set and storming 4.5 min into its budget. Claimed as-is, no
  restart.
- r24 verdict (closed mid-flight by session 13): FULL budget clean —
  77660 lines, wakes 12.39M, ticks [100,100], strand_max 9705 ms (the
  9.5-9.7 s new-baseline class), 0 WW-HIT / 0 heals / 0 panics. Driver
  self-advanced to r25 at 02:04.
- r25: EXTERNALLY KILLED clean partial at ~15/45 min — serial froze
  02:19 mid-storm, no teardown lines, whole tree gone in one sweep
  (the s10 15:50 / s13 00:50 infra-sweep class; it also reaped
  session-13's wrapper). Guest healthy to the last line (ticks
  [100,100], wakes 3.46M, strand_max 9.7 s), 0 WW-HIT, 0 panics.
  Relaunched from r26 at 02:40 (run/ww-chain-s14.log); r26 armed
  (WW-ARMED 15, GUARD=0xffffffff80a7a000) and storming. Cumulative:
  **0 WW-HIT across r1-r25** partials.

### mm-side fault-around LANDED (6782020e) — the session-13 16x item

The session-13 archive left fault-around as "documented for the
implementing session, the 16x-class win"; the handoff greenlit the mm
side while the launcher line owns the fs side. Implementation
(x86_64 page_fault.rs only, riscv64 twin untouched):

- Read/exec demand faults into FileBacked VMAs fill and map up to 16
  pages (64 KiB, Linux fault_around_bytes default) from the faulting
  page: one VMA find, one fill pass, one PTE-lock section. ~25600
  faults per 100 MB lib load collapse to ~1600 events. The per-page
  sequential read_at stream feeds the ext4 readahead detector, so the
  fs side classifies and prefetches the window — composition, not
  duplication.
- Semantics held to the single-page arm: past-EOF pages stay unmapped
  (SIGBUS gate re-faults them — mmap13 parity), window clipped at VMA
  end, write faults opt out (do_fault_around parity), SHARED-devfs /
  anon / device / shmem excluded, per-page walk re-checks free race
  losers.
- Safety: the batch re-verifies the VMA snapshot (start/end/offset/
  fd/flags) under a RE-TAKEN vma read guard held across the single
  PTE-lock map section (INV-LOCK-7 vma→pte nesting, same as the
  marker path) — munmap/mprotect/MAP_FIXED all need the vma write
  lock, so the stale-capture race (mapping file content into a
  concurrently replaced region) is closed for the whole window at
  once, strictly stronger than the single-page arm.
- Bonus fix inside the extracted fill helper: a NEGATIVE errno from
  read_at on a pre-zero-skipped page fell through the old
  `bytes_read == 0` check — recycled-buddy content could reach a user
  page on a failed read. Now any read not verifiably covering the page
  zeroes the tail.

### Validation

- test/faultaround_boot.py (new, loader_vma shape): **FAW-PASS x6** —
  FA1 64-page sequential content (4 full windows), FA2 EOF clip →
  SIGBUS in a forked child, FA3 write-fault opt-out + COW privacy +
  writable around pages, FA4 partial-tail zero fill, FA5 adjacent-VMA
  boundary clip (two files at neighboring MAP_FIXED addresses, no
  cross-contamination), FA6 fork COW over an around-mapped window.
- test/loader_vma_boot.py LVMAP-PASS (regression: boot-exec interp/exe
  segments, mremap file-attr carry, loader overlay — all through the
  batch path).
- ww-fa1 (fault-around kernel, 15-min OH soak, TCG smp2/4G,
  dfx=taskdump,watchdog,periodic,metawatch): full budget, 822 forks
  (fd1 control class), wakes 4.40M advancing to the end, avg_run 4ms,
  ticks [100,100], **0 KERNPANIC / ALLOCTHROW / META-SMASH / SIGBUS
  forensics / Permission-denied / MM-DROP-CTX**. strand_max 144 s with
  wakes never stalling — classified against the A/B control below.
- ww-fa2ctl (control, HEAD~ kernel = identical except no
  fault-around, same script/flags/budget, same concurrent-host
  conditions — hunt chain r26 + samgr-line sc110/sc111 ≈ 3 QEMUs
  throughout both soaks):

  | metric (15 min) | ww-fa1 (fault-around) | ww-fa2ctl (control) |
  |---|---|---|
  | serial lines | 142,760 | 138,496 |
  | forks | 822 | 853 |
  | final wakes | 4.40M | 2.85M |
  | avg_run | 4 ms | 6 ms |
  | strand_max | 144 s | 69 s |
  | stranded events | 55,834 | 52,464 |
  | si_busy > 500 ms samples | many (max 1064 ms) | 181 |
  | KERNPANIC/ALLOCTHROW/SIGBUS/PermDenied/MM-DROP-CTX/META-SMASH | 0 | 0 |

  Classification: strand_max 144 s vs 69 s is within the
  host-contention family's spread (the 15-min controlled-boot class
  itself ranged 44-81 s at 3 QEMUs in session 13; bf-v1/v2 spread
  470-615 s on IDENTICAL kernels) — both soaks ran against ~3
  concurrent QEMUs, both show hundreds of >500 ms si_busy samples,
  both never stalled wakes. The throughput signal points the right
  way: the fault-around boot completed 1.54x the wakes at LOWER
  avg_run (4 vs 6 ms). The definitive quantification (spawn-to-spawn
  stime on the launcher workload) belongs to the launcher-line relay
  once Session N's readahead rides main.

Two probe-development lessons recorded for the line: (a) a forked
child must judge only its own checks — the FA6 child inherited the
parent's global failure counter and "failed" while every child-side
check passed, which cost a false-alarm reproduction chase; (b) printf
from short-lived forked children is lossy on this console — raw
write(1, ...) diagnostics are reliable (FA2/FA6 shape).

### Probe side-observation (pre-existing, not touched)

MAP_FIXED at mid-low addresses (e.g. 0x50000000) fails EINVAL from
the x86_64 arch mmap brk-conflict guard (`addr < MMAP_START && end >
BRK_DEFAULT` = 0x20000000): any fixed mapping ending above 512 MB and
below MMAP_START is refused, far beyond the actual brk region. Linux
allows these placements. Divergence noted; the guard is deliberate
brk-heap protection (mm_ops.rs MAP_FIXED branch), left alone.

### Session 14 close-out

- Commits: 6782020e (fault-around + probe), archive update (this
  section) + chain artifacts.
- Chain at close: r26 in flight (15-point set); 0 WW-HIT cumulative.
- Next: (1) chain continues — watchset 15 + metawatch both live;
  (2) if the launcher line's Session N readahead lands on main, an
  OH A/B of spawn-to-spawn stime would quantify the composed win
  (16x fewer faults x batched reads); (3) watchset room remains
  (capacity proven >= 15; next quiet candidates listed session 10).
