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

## Session 15 (continuation agent — MAP_FIXED/brk placement parity)

### Chain stewardship

- The handoff's "r26 died with session 14" was STALE for the FOURTH
  time: the whole tree was alive at claim (wrapper 2371420 → chain →
  hunt → qemu 2371447), r26 armed with the 15-point set and storming
  19 min into its budget. Claimed as-is; no restart.
- r26 verdict: FULL budget clean — 82262 serial lines, wakes 12.55M,
  avg_run 2ms, ticks healthy, strand_max 13.1s (the >=3-QEMU
  host-contention class: the main session's regression gate + riscv64
  boots ran concurrently), 0 WW-HIT / 0 heals / 0 KERNPANIC /
  0 ALLOCTHROW. Driver self-advanced to r27 at 03:25 (WW-ARMED 15,
  storming). Cumulative: **0 WW-HIT across r1-r26** partials.
- Cross-line context: the main session merged feature/wildwrite-hunt
  into main during this window (the mm three-piece + fault-around).
- Infra find: toolchain/riscv64-rux-linux-musl was a SELF-REFERENCING
  symlink loop (the main repo's own copy, created Oct 22 00:40),
  breaking the SDK for both trees. Repointed THIS worktree's symlink
  to devfs-grow's real SDK (read-only use, session-7 precedent). The
  main repo's broken symlink is still there — main session should
  repoint it before any riscv64 probe build.

### MAP_FIXED low-placement + brk-mirror parity (session-14 next-step item, LANDED)

Probe: test/mmapfix.c + test/mmapfix_boot.py (dual-platform dynamic
musl, faultaround_boot.py shape). Pre-fix x86_64 run documented the
divergence live: MAP_FIXED EINVAL at 0x50000000 / 0x30000000 /
0x1fff0000-straddle (MF1-3) — the blanket guard
`addr < MMAP_START && end > BRK_DEFAULT` rejected every fixed mapping
between 512MB and MMAP_START.

Changes (both arches + syscall layer, commit cc091b57):

1. **Guard narrowing (arch/*/mm/mm_ops.rs mmap fixed path)**: refuse
   only the LIVE heap span [align_down(start_brk), align(brk)).
   MAP_FIXED over malloc's own storage stays EINVAL — deliberate,
   documented divergence (Linux unmaps whatever is covered; we never
   silently free brk-managed pages). Everything else below MMAP_START
   now places like Linux: the fixed-path surgery (split/keep/zap/
   replace) is VMA+PTE complete, and brk growth into foreign VMAs is
   refused by sys_brk's existing VmaManager overlap check.
2. **sys_brk mm.brk mirror sync (syscall/memory.rs)**: sys_brk is the
   ONLY live brk writer and it updated only task.brk — mm.brk stayed
   at the exec-time image end forever, so the guard read an EMPTY
   heap and the live-heap protection was decorative for any process
   that grew its heap (the probe's MF6 caught this: a fixed mapping
   landed ON the live heap after growth). mm.brk is now synced at all
   three sys_brk mutation sites; exec/fork already kept both
   consistent.

Two falsified theories worth recording (both killed by the probe):

- "PIE brk is dead": the arch MmStruct::set_brk/do_brk range check
  (ceiling TASK_SIZE/3 < PIE base TASK_SIZE*2/3) looked like it killed
  every PIE's brk — but that function is DEAD CODE (zero callers; the
  live path is sys_brk's VMA-surgery + demand-fault pages with only
  user_end/RLIMIT_DATA checks). sys_brk always allowed PIE brk growth;
  glibc malloc's brk works. The dead set_brk now carries a
  LEGACY/UNUSED banner.
- "riscv64 fork-child cold-page corruption": the riscv64 probe run's
  MF8 child died of SIGILL with the child PC inside the ELF-header
  page — looked like a fill/return-address bug. objdump exonerated the
  kernel: the death instructions were the probe's OWN code — the
  distro riscv64-linux-gnu-gcc emits Zce code-size insns (C.SB et al.)
  by default and QEMU's rv64,zbb,zba,zbs cpu SIGILLs on them. Same
  class as session 7's RVV lesson; mmapfix_boot.py now pins
  -march=rv64gc (bindmkdir precedent), after which MF8 passes. A
  standalone no-mmap variant (test/forkcold.c) reproduced the same
  SIGILL shape, ruling out the probe's mmap legs as the trigger; the
  objdump + march-pin pair settled it — no kernel bug, no regression.

Probe lessons: musl's sbrk is a STUB (src/linux/sbrk.c returns ENOMEM
for any nonzero increment — musl malloc is mmap-only); any brk probe
MUST use raw SYS_brk calls (MF4/MF5 do).

### Validation

- test/mmapfix_boot.py: **MMF-PASS x9 on BOTH x86_64 and riscv64** —
  MF1-3 fixed placements + content, MF4 baseline brk growth + refused
  growth into a fixed mapping above the heap (returns old break,
  Linux shape), MF5 growth to one page below the mapping + writable
  top, MF6 live-heap MAP_FIXED EINVAL, MF7 MAP_FIXED_NOREPLACE EEXIST,
  MF8 fork COW isolation over a fixed mapping, MF9 NULL-hint mmap.
- riscv64 in-kernel unit tests: **1085/1088**, byte-identical failure
  set to the session 6/7/9/12/13 baseline (the 3 pre-existing
  config_test.rs size asserts).
- ww-mmf1 (OH soak, 15 min, worktree kernel, dfx=taskdump,watchdog,
  periodic,metawatch): full budget, 127289 serial lines, wakes 2.59M
  advancing to the end, avg_run 5ms, 801 forks (fa1/fd1 control
  class), strand_max 262s single-task monster with wakes never
  stalling — the >=3-QEMU host-contention class (chain r27 + main
  session QEMUs concurrent; fa1's own A/B spread was 69-144s on
  identical kernels), **0 KERNPANIC / 0 ALLOCTHROW / 0 META-SMASH /
  0 SIGBUS forensics / 0 Permission-denied / 0 MM-DROP-CTX**,
  METAWATCH-ARMED (third clean metawatch soak on this branch).
- Residual divergences (documented in code comments): non-fixed hints
  below MMAP_START still not honored (falls back to find_free_area —
  Linux honors free hints anywhere); brk may grow to exactly TOUCH an
  adjacent foreign VMA (Linux's sys_brk leaves one page via
  vm_start_gap); live-heap MAP_FIXED is EINVAL where Linux unmaps;
  brk SHRINK unmaps pages (sys_brk munmap path) but the arch legacy
  set_brk does not — irrelevant while it stays uncalled.

### Session 15 close-out

- Commits: cc091b57 (mm parity fix + probes), d86c0ea3 + this
  addendum (archive + chain artifacts + the riscv64 SDK symlink
  repoint). Pushed via ssh.github.com:443.
- Chain: r27 was killed EXTERNALLY at ~15/45 min (03:40, mid-storm,
  no teardown lines, whole tree gone in one sweep — the s10/s13/s14
  infra-sweep class; guest healthy to the last line: ticks [100,100],
  strand_max 10.5s healthy class, wakes 3.41M advancing; 0 WW-HIT,
  0 kernel panics — the 3 'panic' grep hits are OH userspace service
  stderr). Relaunched from r28 at 03:54 as this session's background
  task (run/ww-chain-s15.log; wrapper 2399284 → chain → hunt → qemu
  2399301, WW-ARMED 15 verified, process tree in run/ww-s15-proctree.txt).
  Cumulative: **0 WW-HIT across r1-r27** partials.
- Next: (1) chain continues toward the capture; (2) the narrowed guard
  + brk mirror ride to main with this branch's next merge; (3) low-hint
  honoring is the remaining known placement divergence needing its own
  validation window; (4) main repo's toolchain/riscv64-rux-linux-musl
  symlink is still a self-loop — fix before any riscv64 probe build.

## Sessions 16-17 (reconstructed from snapshot 2e2dd3bb — quota deaths,
## no archive written at the time)

- Chain stewardship through r30: r28 full-clean, r29 externally killed
  at 15/45 (clean partial), r30 full-clean. Cumulative 0 WW-HIT r1-r30,
  0 META-SMASH; r31 was killed with the user pause at ~10 min (armed,
  0 hits — verdict confirmed by session 18 from the artifacts).
- Landed on the snapshot (unreviewed at pause, reviewed by s18):
  low-hint adoption in both arches' non-fixed mmap paths (hint rounded
  down, honored anywhere in the user range when its gap is free;
  ignored below MIN_MAP_ADDR / on the live brk span / past USER_END —
  Linux arch_get_unmapped_area parity; also closes the unaligned-hint
  Vma::new assert panic vector) and the sys_brk one-page-gap guard
  (Linux find_vma_intersection(oldbrk, newbrk+PAGE) parity: growth is
  refused when a VMA starts inside [ceil(oldbrk), newbrk+PAGE); a
  VMA straddling the boundary is caught by add()'s overlap refusal —
  same keep-old-brk outcome). Probe: test/mmapfix.c grew MF5b + MF10-13
  (14 legs, MMF-PASS x14 both platforms per the handoff; re-validated
  by session 18 after the merge).
- render_service investigation started (run/rs-repro-boot.sh, KVM q35
  4G, launcher-line oh-img-p private images + private userdata copy):
  rs-repro1 captured the deterministic crash on this branch's kernel —
  pid 690 rip 0x3fbf08ad30 (rep movsq memcpy), rdx=0x3a8, dst=rsp+0x478,
  cr2=0x7ff00067c000, vma(cr2)=[0x7ff00067c000,0x7ff00067e000) flags=0x10
  Anonymous off=0x35000 (launcher's sc114/116: off=0x19000 — same shape,
  different boot layout).

## Session 18 (continuation agent — render_service offset decode, main merge, PT-alloc panic attribution)

### render_service crash decode (mm lane, from launcher Session O handoff)

Ground truth collected across 8 repro boots (rs-repro1..10):

- The fault VMA is a PROT_NONE ANONYMOUS remnant (flags=0x10 = PRIVATE
  only — no R/W/X) whose nonzero `off` is bookkeeping-consistent: it is
  EXACTLY the split arithmetic every split site carries (tail off =
  old_off + (split_end - vma_start)) — the value measures distance from
  the region's ORIGINAL mmap base, carried through carves. An anon VMA
  never gets a nonzero offset at creation (mmap-anon path never calls
  set_offset); the offset anomaly is a SYMPTOM of the region's carve
  history, not itself a fault-path bug (no path consults offset for
  Anonymous faults).
- The OH stack-pool anatomy (decoded from syscall traces): per process,
  508KB-8MB PROT_NONE anon reserves (PROTNONE-MAP), worker stacks carved
  as bare RW anon MAP_FIXED mmaps inside them (0x7000-0xa000 class;
  render_service showed a 0x37000-class carve implied by the crash
  geometry), 2-page PROT_NONE mprotect guard toggles everywhere
  (717 len=0x2000 in one 8-min window), and the big stacks (0x803000
  reserves + 0x801000 RW mprotect at base+0x2000 = guard-below).
- The crash geometry fully decoded: rsp sits at the top of an RW carve
  [0x7ff000647000, 0x7ff00067c000); the memcpy (0x3a8 = sizeof(struct
  pthread)-class context copy at thread start) targets [rsp+0x478,
  +0x3a8) = exactly the 2-page remnant above the carve — i.e. the
  runtime believed the stack region extended to 0x7ff00067e000 while
  the kernel's carve ended 2 pages short (or a 2-page guard-toggle
  mprotect landed at the live carve's top). Either way the thread dies
  at its FIRST frame write — deterministic per boot layout.
- Boots 3/5/6/7 with surgical traces (mprotect/carve prints as pr_err):
  render_service WEDGED (no visible crash) — its OS_IPC threads park in
  futex with "owner tid 2 NOT FOUND" (exited-while-holding shape), the
  same park family launcher Session O decoded for OH app children. The
  visible crash and the silent wedge are two faces of the same
  thread-creation failure class.

### The boot-killer: map_page PT-alloc panic (fake-OOM family)

repro2 (loglevel=7), repro5, repro7, repro9 all died early (2-8 min) at
`PANIC: map_page: failed to allocate PT table` (mmu_init.rs:1419 —
`alloc_page_table().expect(...)`): the zone allocator returns None for
an ORDER-0 page. alloc_pages has NO watermark gate — it returns None
only when every free list is EMPTY, i.e. the zone is genuinely drained
(or its pages leaked). This is the same family as the GNOME image's
"page allocator reports OOM at ~342MB/2G" (main-session active goal)
and the F12 clone heap-reserve drain (launcher line, bfc35fd2 probes,
now merged). Trace/print volume shifts the timing (UART irqsave
slowdown changes the allocation interleaving), which is why repro1 and
repro8/10 survived. ACTION ITEM for the mm side (next session or the
main fake-OOM effort): the `.expect` panic turns a recoverable
allocation failure into a kernel panic — at minimum map_page should
fail the mapping and deliver SIGBUS/OOM-kill the task like Linux, and
the underlying zone-drain needs the fake-OOM accounting audit (the
SEED-ALLOC-BAD-POP detector in zone.rs is the in-place tripwire).

### Merge + verdict

- Merged origin/main (e2b3e3a6 — the samgr-callback merge: binder
  rework, TSC shift-32, mount preempt-off lock, taskdump 30s, clone-gate
  refusal probes). mm/ untouched by main between 2e2dd3bb and e2b3e3a6.
- Post-merge: repro8 (20 min) booted render_service with NO crash and
  NO wedge; repro10 (30 min) reached ~14 min clean, then render_service
  WEDGED in the OS_IPC futex-park family (binder-stall owing pid 537,
  "owner tid 2 NOT FOUND") — the crash itself did not reproduce on the
  merged kernel in ~50 min of exposure (repro9 died of the PT-alloc
  panic first). The binder rework plausibly changed the trigger
  context; the geometry decode above stands.
- VERDICT: handed back to the launcher line with the decode — on
  merged main the deterministic crash is not reproducing; if it
  resurfaces, the capture recipe is: surgical pr_err traces (commit
  2f54454b has them: MPROTECT-NONE all, ANONFIXED-MAP >=16KB or
  PROT_NONE, PROTNONE-MAP >=64KB, MPROTECT-RW/FAIL, BIGMUNMAP) + the
  nearvma dump (kept, below) — one crash boot then fully reconstructs
  the carve sequence. The PT-alloc panic is the louder blocker on this
  image family now.

### Permanent forensic addition

- USEGV-FOR (fatal-SIGSEGV forensics, arch/x86_64/trap.rs) now dumps
  the VMA neighborhood around cr2 (+/-16MB window, capped at 12
  entries) — the single vma(cr2) line could not show what borders a
  mis-flagged region (the render_service decode needed exactly this).

### Hint adoption + brk one-page gap: landed

- The snapshot's WIP (sessions 16-17) reviewed this session: hint
  semantics sound (round-down, MIN_MAP_ADDR/heap-span/USER_END
  rejections, conflict fallback to find_free_area), brk guard sound
  (find_vma_after(ceil(oldbrk)) can never return the heap VMA itself —
  vma_start is always the existing heap VMA's END; straddling VMAs are
  caught by add()'s overlap refusal).
- Validated on the MERGED kernel (post origin/main e2b3e3a6):
  **MMF-PASS x14 on BOTH x86_64 and riscv64** (MF1-9 + MF5b + MF10-13;
  re-run on the final landing binary after the experiment prints were
  reverted), **LVMAP-PASS** (exec/loader VMA regression), riscv64
  in-kernel unit tests **1085/1088** (run/ww-s18-maketest.log; the 3
  pre-existing config size asserts, byte-identical to the session
  6/7/9/12/13/15 baseline). OH exposure: repro8/repro10 (~50 min) ran
  the hint path live with no VMA-geometry regression.
- The render_service trace recipe lives in commit 2f54454b (surgical
  pr_err prints); revert to landing form done in the landing commit.

### Chain stewardship

- r31 (relaunched at claim after the pause): FULL budget clean —
  0 WW-HIT / 0 heals. r32: killed at 15/45 by the ~11:30 infra sweep
  (whole tree, clean partial); relaunched from r32 at 11:40 — that
  full run clean too, driver self-advanced to r33. r33: FULL budget
  clean (survived to completion before the next sweep), driver
  self-advanced to r34; the ~13:3x sweep killed the wrapper and r34's
  first minutes, relaunched from r34 at 13:40 (WW-ARMED 15 verified).
  Three infra sweeps this session (~11:30, ~12:44, ~13:3x), each
  taking the whole background-task set; the r31-r33 artifacts show the
  claim-then-watch protocol keeps working across them. Cumulative:
  **0 WW-HIT across r1-r33**, 0 META-SMASH.
- The riscv64 unit-test run was also swept once at ~60 min (results
  lost with the task's pipe); the rerun with file-captured output
  completed: 1085/1088 baseline confirmed (above). Ops note: pipe a
  long make test to a FILE, not a task pipe — a sweep kill loses the
  pipe buffer but the file survives.

### Session 18 close-out

- Commits: 2f54454b (experiment traces — the capture recipe),
  05119912 (origin/main merge), 8a1b8020 (landing: hint/brk/nearvma +
  archive). Pushed via ssh.github.com:443 (2e2dd3bb..8a1b8020).
- Open for the next session, in order: (1) the PT-alloc panic
  (map_page .expect at mmu_init.rs:1419) is the loudest blocker on the
  OH image family — recoverable-failure policy + the zone-drain
  (fake-OOM accounting) audit; coordinate with the main session's
  fake-OOM effort and the launcher line's F12 gate probe. (2) If the
  render_service memcpy crash resurfaces, cherry-pick 2f54454b's
  prints + nearvma dump and boot once — the decode in this session
  pins the geometry; the remaining unknown is which syscall (carve
  short vs top-guard mprotect vs hinted placement) produced it.
  (3) Chain continues; watchset 15 + the hunt for the wild writer
  stays the line's ultimate milestone.


## Sessions 19-21 (reconstructed from artifacts + commit messages — quota deaths and user pauses, no archive written at the time)

### Session 19 (13:52-14:30): PT-alloc survivable + first fake-OOM audits

- 40920349 (14:11) landed the map_page panic fix (session 18's loudest
  item): alloc_page_table's Late stage gets a bounded reclaim retry
  (clean page-cache eviction only — swap-out would self-deadlock on the
  held per-mm PteLock), map_page* return bool, demand-fault sites convert
  failure to MmFaultResult::OutOfMemory (SIGKILL, Linux pagefault-OOM
  semantics), data pages freed on the failure path.
- 7fea5944 (14:24) added the raw-UART forensics after pt1/pt2 lost every
  printk line of the OOM dumps (printk wedges during exactly these
  storms): MEMLEDGER-CENSUS/BUCKETS/OOMFX-RAW lines ride the raw path.
- pt1 (14:17): 0 panics but NO OOM either (UART-slowdown timing shift).
  pt2 (14:24): fake-OOM captured with full forensics — memfree_pages=0,
  zone_free=0, zone_managed=736006, pagecache 126k, heap 206MB of 992MB —
  the drain is REAL, not accounting drift (census sums reconcile exactly:
  reserved 836858 + managed 736006 = total descriptors 1572864).
  pt3: PANICKED at ledger.rs:86 (see session 22 — the Line OOB bomb).

### Session 20 (14:30-16:38, quota death at ~16:00 mid oom_kill work)

- b663c6f8 snapshot carried unreviewed oom_kill.rs + trap.rs changes;
  998e1c01 (16:12) reviewed and corrected them: pagefault-OOM retry
  policy — strike burned only when a kill window is WON (the snapshot
  burned one per fault and killed the faulting task alongside the
  victim), 10s quiet episode reset, riscv64 kernel-mode arm gets the
  global killer a chance before panicking (livelock fix).
- pt4 (16:20, pre-walk-fix): walk_fp_chain read the RISC-V frame layout
  on x86_64 — every allocation interned a UNIQUE garbage chain, PSITE
  slots filled with ASCII junk (0x49657069505d737d), and the boot
  PANICKED at the same ledger.rs:86 OOB.
- a2a7e91f (16:30) fixed walk_fp_chain to the SysV layout ([rbp]=caller
  rbp, [rbp+8]=ra). pt5 (16:31-16:37, killed by the user pause): fake-OOM
  at ~3 min (pagefault OOM x2 → oom-killer Killed process 305 (ueventd)
  score 556 → zone refilled to 1042 pages) and the boot SURVIVED ~5 more
  minutes of full storm to the pause kill, 0 panics. The survivable path
  works end-to-end on the first clean test.
- 39fc7619 (16:38) pause snapshot.

### Session 21 (22:13-22:15)

- Relaunched the chain from r36 (ww-chain-s21.log); the user pause at
  22:15 killed it ~2 min in (0 WW-HIT, clean partial). 6553ae01 snapshot.

## Session 22 (continuation agent — fake-OOM survivable-path validation + the zone-drain audit)

### Inheritance reconstruction

- Nothing running at claim (chain dead with the pause, r36 partial).
  Relaunched the chain from r37 at 00:08 (run/ww-chain-s22.log; proctree
  in run/ww-s22-proctree.txt; WW-ARMED 15 verified, frozen pair
  unchanged). Cumulative 0 WW-HIT r1-r36.
- Build verified: PLATFORM=x86_64 make build at the pause snapshot HEAD
  is a cargo no-op reproducing the pt5 bzImage byte-identical (md5
  96fc5282) — pt5's verdict rode the committed code.

### The fake-OOM numbers (pt2/pt5 re-read — the audit's ground truth)

At the OOM moment (both boots, same shape):

- Machine: 4 GiB usable (e820 low 2 GiB + above-4G 2 GiB), 1572864
  descriptors over the 6 GiB physical span; the ~2 GiB PCI hole
  descriptors report reserved → reserved=836858 (hole 524355 + kernel
  heap 253952 + slab/image/etc.), zone_managed=736006, census sums
  reconcile EXACTLY (no accounting drift, orphan~0, inverse=0).
- zone_free=0 walkable=0 — the drain is genuine: managed pages are all
  held. Decomposition: other_used=558600 (76%!), lru_file=125947 (17%,
  CLEAN page cache, dirty=0), anon=45713, pt=4586, cow/mapped small.
- The two headline questions this session: (a) the survivable path holds
  under repeated fake-OOM (pt5 says yes once; needs longer exposure);
  (b) WHO holds the 558600 unflagged pages, and why the slowpath's
  direct reclaim does not evict the 126k CLEAN page-cache pages first.

### Instrumentation fixes found necessary and landed (8127f1cf)

1. ledger Line::emit OOB — the pt3/pt4 boot killer, STILL LIVE at HEAD:
   push() filled the [u8;160] and emit() stored buf[160]. pt5's BUCKETS
   line summed to 159 — one byte of luck. Fixed (newline byte reserved).
2. psite attribution unusable: the fp walk stores trailing ras
   UNCHECKED, so one caller interned as 4 slots (identical f1..f7,
   junk-divergent f8); 64 slots saturated at boot; the runtime
   allocators never interned — MEMPSITES top-10 nets summed to ~96k
   against the 558600 other_used census. chain_key sanitizes
   non-kernel-text ras (x86_64 image window), PSITE_SLOTS 64→256, dump
   top-16.
3. High-RAM blind spot: the PFN→site table covered 2 GiB; q35's
   above-4G half (2 GiB of 4) was untracked — high-RAM frees never
   attributed (churn nets inflated by their high half; leaks correct).
   Widened to 6 GiB/786432 entries.
4. MEMLEDGER-OWRS: other_use(d) census by physical range (12×512 MiB,
   descending so above-4G buckets survive truncation).

Kernel md5 after: fd6a9a37 (bzImage). Chain r37 unaffected (frozen pair
by design).

### Early psite read (pt6 partial, ~4 min, pre-fix kernel)

Top family net=69154+6421+2247+1841+848+821+658 ≈ 82k through
alloc_pages ← alloc_user_phys_page ← handle_mm_fault ← do_page_fault —
the anonymous-demand-fault arm (ledger anon=45713 at OOM + the
high-RAM-frees inflation explains the gap). Side sites: mremap
copy_old_to_new_pages (1.8k), devfs shared_grow (0.5k). NO site for the
ext4 page cache (should be ~126k net) and nothing near 558k — both
expected consequences of the saturation/blind-spot bugs above; pt7 (the
fixed-kernel audit boot) is the decisive run.

### The zone-drain audit: ROOT CAUSE ATTRIBUTED (pt7, fixed instrumentation)

pt7 (30-min rs-repro audit boot, kernel 8127f1cf, dfx=taskdump,watchdog,
periodic,memwatch,ledger) survived TWO fake-OOM rounds (oom-killer killed
ueventd score 556, then watchdog_servicene score 436) with zero panics —
and the widened psite table + OWRS histogram named the drainer:

- **psite #1: net=639009 pages (a=713290 f=74281) through alloc_pages ←
  alloc_user_phys_page ← handle_mm_fault+0x311d (call at 0x...cdebd) ←
  do_page_fault ← trap_handler** — the MAIN single-page demand-fault arm
  (page_fault.rs:885). Contains both the properly-flagged anon sub-arm
  and the unflagged FileBacked sub-arm.
- **psite #2: net=125716 (a=138615 f=12899) — handle_mm_fault+0x2dfa
  (call at 0x...cdb96, the loop shape)** — the fault-around batch arm
  (page_fault.rs:767).
- OWRS histogram at OOM #1: other_used=539290 spread r0=64808 r1=109711
  r3=5217 r8=110842 r9=99434 r10=84068 r11=65210 — uniform across the
  zone's two DRAM halves (no phys-range funnel; the high half carries
  359k, the low 174k).
- Census at OOM #1: other_used=539290 lru_file=149632 anon=41469
  pt=4446 mapped_file=1022 cow=132 — sums reconcile exactly. Pages in
  other_used have rc=1, mapcount=0, NO flags (census precedence:
  mapped_file would have caught mapcount>0).

**Root cause (code-level, both arms verified):**

1. page_fault.rs:1074 — the main arm's FileBacked sub-arm tail is
   literally `_ => { // File-backed: rmap not wired yet }`: after
   allocating a FRESH page, read_at()-ing the file content into it, and
   mapping it into the user PTE, it sets NO page-descriptor state — no
   Anonymous/SwapBacked, no mapcount, no rmap record, no LRU add.
2. The fault-around arm (731-880) does the same for every page of its
   16-page window — no descriptor state anywhere on the path.

Consequences, in order of size:

- **Unreclaimable by construction**: vmscan only sees Lru members.
  Every file-backed demand fault pins its private copy FOREVER (freed
  only by process-exit PTE teardown). The OH workload's file mappings
  (ArkUI libs etc.) accumulate 539-558k such pages = 2.1-2.18 GiB of
  the 2.87 GiB zone — the drain.
- **Double memory**: the same file data ALSO lives in the page cache
  (lru_file 126-150k pages): read_at fills the fresh page THROUGH the
  cache. Linux maps the CACHE page directly (filemap_fault) — one copy,
  shared, reclaimable; MAP_PRIVATE goes RO+COW. Rux copies per fault.
- **The "fake" in fake-OOM**: the ledger census shows 76% of the zone
  as "other_used" (nobody's pages) — accounting-side invisibility of
  the same bug. The GNOME image's main-session "OOM at ~342MB/2G" is
  almost surely the same family (their used-counter also cannot see
  these pages).
- The anon arm is correctly flagged (SwapBacked+LRU per the
  page_fault.rs:233 comment family) but swap_active=0 on these boots,
  so anon (41k) is de-facto unreclaimable too — minor next to the file
  copies.
- The clean page cache is the only reclaimable pool at OOM time and it
  did NOT shrink across the two OOM dumps (149632 → later dump same
  class) — whether vmscan's shrink_inactive_list actually evicts under
  this pressure is a separate open question (pages ARE on
  LRU_INACTIVE_FILE; all file lru_add variants land there).

**Fix directions (next session's mm item, needs design + probes):**

- Real fix (Linux parity, the big one): filemap_fault shape — read/exec
  faults on FileBacked VMAs map the PAGE-CACHE page directly (refcount
  +1, rmap), RO even on writable VMAs; write faults COW (alloc, copy,
  flag anon+SwapBacked, map RW). Kills the double-copy AND makes the
  pool reclaimable (unmap → refault). Touches both arms, the fork COW
  walk's file-page handling, mprotect downgrades, and unmap/teardown
  (put the cache ref). Must ride with loader_vma/faultaround/mmapfix
  probes + OH soak.
- Interim accounting fix (small): set Anonymous+SwapBacked+mapcount+
  rmap+anon-LRU on both file arms' copies (Linux marks private file
  pages SwapBacked after COW). Makes them census-visible and
  swap-eligible, does NOT fix the double-copy or non-swap reclaim.
  NOTE: this moves them into the `anon` census bucket and changes
  oom-killer badness inputs — coordinate with the main-session
  fake-OOM effort either way.

### Survivable-path validation state (task 1)

- pt5 (HEAD@pause kernel): 1 fake-OOM survived (ueventd killed, zone
  refilled 0→1042, storm continued to the pause kill). 
- pt7 (8127f1cf): 2 fake-OOMs survived in the first ~20 min (two
  distinct victims), 0 panics — and the Line-emit fix is PROVEN LIVE:
  pt7's first BUCKETS line is WIDER than pt3/pt4's (extra digits) and
  truncated cleanly where those boots panicked.
- Remaining legs: riscv64 unit tests + minimal-rootfs boot; the 45-min
  OH 0-panic gate boot.

### Task-1 legs completed (session 22 close-out)

- pt7 (30-min audit boot): FULL budget, 29270 lines, **0
  KERNPANIC/ALLOCTHROW**, 2 oom-killer rounds survived (ueventd 556,
  watchdog_servicene 436), storm to the end.
- pt8 (45-min OH 0-panic GATE): FULL budget (timeout kill as designed),
  38090 lines, **0 panics**, same 2 oom-kills survived, WW heartbeats to
  the last line. **GATE PASSED.**
- riscv64 in-kernel unit tests (make test, file-captured per the s18
  lesson): **1085/1088** — byte-identical failure set to the s6/7/9/12/
  13/15/18 baseline (the 3 pre-existing config_test.rs size asserts);
  no regression from 998e1c01's riscv OOM arms or this session's dfx
  changes. The post-summary PANIC is the test harness itself (main.rs:984).
- riscv64 minimal-rootfs smoke (150 s console boot): /bin/sh loaded,
  mrsh prompt reached, **0 panics** (PREEMPT-LEAK lines = the known wfi
  idle noise from main's launcher-line tripwire).
- Ops: the make-test QEMU outlives the harness after the post-summary
  panic and holds test/rootfs.img's write lock (session-10 incident
  class) — reaped after cwd+cmdline verification (it was my own), then
  the smoke ran clean. Sweeps: one infra sweep at ~01:08 killed the
  chain task mid-r38 (clean partial: 57593 lines, 0 WW-HIT, 0 panics;
  pt8's task survived the same window); relaunched from r39 at 01:15
  (run/ww-chain-s22b.log, armed verified). config.rs restored to the
  committed x86_64 flavor after the riscv legs (generated-artifact
  quirk, s2/s4 precedent).

### Where this leaves the line (handoff)

1. **The fake-OOM hunt is ANSWERED on the attribution level**: the
   drainer is the file-backed demand-fault copy semantics (both x86_64
   arms; the riscv64 twin has the same shape and needs the same audit
   run — its psite sanitizer is the one dfx gap). The fix decision
   (filemap_fault parity vs interim flag hygiene) belongs to the next
   mm session + the main-session fake-OOM effort — the archive above
   carries the full evidence chain (psites, OWRS, census, code lines).
2. Open vmscan question (second-order): lru_file stays ~150k at the OOM
   moment — evict_one's cache-entry ref_count pinning under the
   concurrent-read storm, or Referenced activation churn, or the
   balance_pgdat loop shape; worth one probe boot with eviction
   counters before anyone touches vmscan.
3. Chain continues at r39 (15-point set, frozen pair from 28630426).
   Cumulative **0 WW-HIT across r1-r38** partials, 0 META-SMASH.
4. The wild-write engine itself remains uncaptured; the hunt's
   machinery (watchpoints + metawatch + WP/COW discipline) is all live.

## Session 23 (continuation agent — filemap_fault alignment: the fake-OOM fix)

### What landed (the session-22 "real fix" direction, implemented)

Private file demand faults now map PAGE-CACHE pages directly instead of
allocating a private copy per fault (Linux filemap_fault/do_read_fault/
do_cow_fault semantics). Both x86_64 arms converted:

- **Read/exec faults** (single-page + the fault-around window): pin the
  cache frame (`get_phys`, on-miss warm the cache via a new Discard-
  destination ext4 read that drives the existing readahead/fault-around
  machinery with no bounce copy), map it RO — writable private VMAs get
  the COW bit instead of W (Linux writenotify). Holes and non-ext4
  files fall back to the private-copy fill, now with full page
  bookkeeping. The frame is shared by every mapper and refault; the pin
  (plus one descriptor ref) is owned by the PTE.
- **Write faults** keep the read-into-fresh-copy fill and are now
  flagged Anonymous+SwapBacked+mapcount+rmap+anon-LRU (the "rmap not
  wired yet" arm — do_cow_fault parity).
- **PTE ownership bit** `cow_flags::PCACHE` (bit 10, beside COW bit 9):
  marks a PTE that owns a cache pin. Carried through mprotect rewrites
  and PROT_NONE markers; dropped with `put_by_key` at unmap, mm
  teardown, and the COW break; duplicated with `pin_by_key` by the fork
  walk's copied leaves. A PCACHE leaf NEVER gets W in place — an
  mprotect W-upgrade forces the COW bit so the first write copies.
- **Pin protocol**: every PCACHE PTE owns exactly one CachedPage pin
  (eviction guard — evict_one skips pinned entries, so a mapped frame
  can never be freed under a PTE) plus one descriptor get_page
  (balanced by unmap/teardown/COW put_page). `page_remove_rmap` skips
  the LRU-unlink for PageType::PageCache frames (the file-LRU
  membership and mapping/index fields belong to the cache — clobbering
  either corrupts eviction). No page_record_mapping/set_index on cache
  frames (index is the cache's BTreeMap key).
- Lock order: pte -> page-cache inodes lock (fork pin, unmap/COW put);
  fault fill phase pins BEFORE taking the pte lock (R73 intact). No
  path nests inodes -> pte, no cycle.

### Validation (x86_64)

- faultaround_boot.py **FAW-PASS x6**, loader_vma_boot.py **LVMAP-PASS**,
  mmapfix_boot.py **MMF-PASS x14** — through the new cache-mapping path
  (the probes' rootfs is mkfs.ext4, so `is_ext4_file` gates in).
- **fm1** (30-min rs-repro audit boot, dfx=taskdump,watchdog,periodic,
  memwatch,ledger): FULL budget, 97,123 lines, **0 KERNPANIC /
  0 ALLOCTHROW / 0 OOM events** (pt7 at the same wall-clock: TWO
  oom-killer rounds; pt5 OOM'd at ~3 min). The ledger series line at
  ~20 min: `memfree_pages=365037 pt_pages=28112 pagecache_pages=158051`
  — 1.4 GiB FREE with a 617 MB shared cache, vs pt7's OOM moment
  `memfree=0, other_used=539290`. **The drain is gone; the fake-OOM
  does not occur on the reproducer that hit it twice in 30 minutes.**
  0 REFUSED/SEED/dblfree (all MEMREFUSE counters zero), 0 MM-DROP-CTX,
  0 META-SMASH, 0 FRAME-DOUBLE-MM; 1 USEGV-FOR victim at the baseline
  1-per-boot rate (this boot: the render_service thread-creation crash
  family, session-18 decode; pt7/pt8 each had 1 victim of the
  errno-as-pointer shape).
- **fm2** (45-min OH 0-panic gate, pt8 shape): FULL budget (timeout kill
  as designed), 43,590 lines, **0 KERNPANIC / 0 ALLOCTHROW / 0 OOM /
  0 kernel faults / 0 MM-DROP-CTX**; 51 legible ledger samples, last
  memfree_pages=568032 (2.2 GiB free at boot-end). 4 USEGV-FOR victims
  in 3 families, all documented service-side shapes (accountmgr
  cr2=0x38 3-attempt restart loop; the render_service thread-creation
  crash; OS_IPC errno-as-pointer garbage derefs) — no kernel-fault or
  permission-kill family among them. **GATE PASSED.**
- riscv64: in-kernel unit tests **1085/1088** (byte-identical failure
  set — the 3 pre-existing config size asserts); minimal-rootfs smoke
  to /bin/sh, 0 panics. The riscv64 fault TWIN keeps the old
  private-copy shape for MAP_PRIVATE — port notes below.

### Instrument note (dfx, not the fix)

The MEMLEDGER 30-s series lines are heavily garbled on these boots (the
raw-UART writes interleave mid-printk — one line survived legible in 30
min; grep on clean "MEMLEDGER" text finds nothing, search
"memfree_pages=" fragments instead), and the series fired far less
often than its 30-s period. The single legible line carried the
decisive numbers; the cadence question belongs to the dfx line.

### Session 23 close-out (chain stewardship + landing)

- r39: FULL budget clean (78,267 lines, 0 WW-HIT, 0 panics), driver
  self-advanced. r40: FULL budget clean (54,035 lines, 0 WW-HIT,
  0 panics) — then the chain DRIVER died in an infra sweep (log frozen
  02:00, no r41 start); relaunched from r41 at 03:24
  (run/ww-chain-s23.log, WW-ARMED verified). Cumulative: **0 WW-HIT
  across r1-r40** partials.
- The make-test riscv QEMU outlived its harness again (post-summary
  panic) holding test/rootfs.img's write lock — reaped after
  cwd+cmdline verification (own process), then the smoke ran clean.
- config.rs restored to the committed x86_64 flavor after the riscv
  legs (generated-artifact quirk).
- Landed as dc096f14 (wip snapshot ed444e08 beneath it); pushed via
  ssh.github.com:443. MERGE-READY for main with its gates: probes x3,
  fm1 no-OOM audit, fm2 45-min 0-panic, riscv 1085/1088 + smoke.

### Handoff notes

1. **riscv64 twin port** (next mm session, GNOME fake-OOM is the same
   family): mechanical — same helpers (the arch-neutral
   put_by_key/pin_by_key/Discard-warm are in place), pick an RSW PTE
   bit for PCACHE beside the riscv COW bit, and convert the twin fault
   path + its mm_ops unmap/teardown/fork/COW/mprotect sites the same
   way. The riscv MAP_SHARED ext4 alias (shared_file_fault_page)
   already pins; MAP_PRIVATE is the gap.
2. MAP_SHARED ext4 on x86_64 remains private-copy (pre-existing; the
   riscv twin aliases the cache frame). If taken, map SHARED ext4
   leaves W WITHOUT the cow bit and with a permanent fault-time pin
   (riscv64 discipline) — but note there is no writeback for
   mapped-cache dirty data, so writes through the mapping would be
   lost on eviction; needs a coherence plan first.
3. The session-22 open vmscan question stands but shrinks: lru_file
   now contains the pinned working set; eviction skips pinned entries
   by design (no file rmap yet — the deliberate minimal choice; mapped
   cache pages are unevictable until unmapped, exactly like the
   MAP_SHARED aliases).

## Session 24 (continuation agent — riscv64 filemap_fault twin port)

### Base + stewardship at claim

- Merged origin/main = e486c3c3 (fast-forward; main had merged the
  session-23 branch). Chain tree ALIVE at claim (fifth occurrence of
  the stale-driver-dead handoff): r41 armed (15-point set) and
  storming, driver self-continuing.
- Kernel implementation located: dc096f14 carried only docs/run
  scripts; the x86_64 kernel changes live in the wip snapshot ed444e08
  beneath it (page_cache put_by_key/pin_by_key, ext4 ReadDst::Discard +
  ext4_fault_populate_cache, rmap PageCache skip — all arch-neutral,
  reusable by the twin as-is).

### The riscv64 port (6e76430b)

Mirror of the x86_64 session-23 fix, both directions of the protocol:

- **cow_flags::PCACHE = RSW bit 9** (beside COW bit 8). A PTE carrying
  it owns one CachedPage pin + one descriptor get_page for its mapping
  lifetime.
- **Fault arm** (page_fault.rs): MAP_PRIVATE read/exec faults on
  FileBacked VMAs pin the page-cache frame via `filemap_pin_cache_page`
  (is_ext4_file gate; get_phys hit, else Discard-warm + pin) and map it
  R-only — writable VMAs get COW instead of W (writenotify parity; the
  first write COWs in handle_cow_fault). Write faults, non-ext4 files,
  and holes keep the private-copy fill, now flagged
  Anonymous+SwapBacked+mapcount+rmap+anon-LRU (the "rmap not wired
  yet" census-invisible copy — the drainer's accounting face). The
  gnome-sigill text-stamp forensics on the fill arm are PRESERVED.
  Cache-mapped frames take NO rmap/set_index/LRU state (the
  descriptor's mapping/index are the cache's eviction keys;
  page_record_mapping's slot-0 set_index would clobber them — the
  MAP_SHARED alias arm's unconditional page_record_mapping on a
  page_index-0 frame remains the known pre-existing hazard).
- **Release sites**: release_cache_mapping_pin (put_by_key via the
  descriptor's eviction keys) after the descriptor put at unmap_pages,
  unmap_one_user_leaf_locked, mm teardown (free_user_page_tables L0
  walk), and handle_cow_fault's copy path. The COW exclusive fast path
  defensively strips PCACHE (a cache frame's descriptor refcount can
  never hit the exclusive window — the cache base ref keeps it >= 2).
- **Fork**: copy_page_table_cow's L0 leaf arm takes pin_by_key for
  PCACHE leaves before its get_page (the child's copy owns its own
  pin — the parent's unmap must not strand the child's frame).
- **PTE rewrites**: sys_mprotect's riscv walk and the fault path's
  PROT_NONE re-present carry PCACHE like COW and force the COW bit on
  any W-upgrade (a cache-backed leaf never gains W in place).
- **Bonus fix** inside the demand arm: the map_page OOM path used to
  free_page() the MAP_SHARED alias frame (cache-owned, pinned) — now
  gated like the lost-race path. set_range_permissions (which drops
  software bits wholesale) confirmed DEAD (zero callers) — no pin-leak
  path.
- Lock order inherited: pin in the fill phase (no PTE lock), releases
  under the PTE lock — pte -> inodes everywhere, no cycle.

### Validation (riscv64, all on 6e76430b)

- In-kernel unit tests **1085/1088** (run/ww-s24-rv-test.log;
  byte-identical failure set: the 3 pre-existing config size asserts).
  The post-summary make-test QEMU outlived the harness again — reaped
  after cwd+cmdline verification (own process; s22/s23 class).
- **MMF-PASS x14** (mmapfix, riscv64) — first run accidentally booted
  the make-test feature kernel (s12 lesson relived: `make test`
  overwrites the target binary; rebuild before probes).
- **FMAP-PASS x7** — NEW probe (test/filemap.c + filemap_boot.py,
  mmapfix mechanics): FC1 cache content, FC2 write-after-read COW
  privacy + fresh-mapping coherence (the cache frame is never
  scribbled), FC3 fork isolation + shared-page coherence over cache
  windows, FC4a partial-tail zero fill, FC4b past-EOF SIGBUS (probe
  bug fixed en route: the touch must be INSIDE the mapping but past
  EOF — outside-VMA touches SIGSEGV), FC5 mprotect W-upgrade COW
  divert, FC6 PROT_NONE round trip, FC7 write(2) invalidation
  freshness. Also the regression guard for the port's mprotect/
  marker handling.
- Minimal-rootfs smoke (run/fm24-rv-smoke.sh, 150 s): /bin/sh reached,
  **0 panics** (PREEMPT-LEAK lines = known wfi idle noise).
- x86_64 build clean (shared-file edits are cfg-gated); config.rs
  restored to the committed x86_64 flavor at commit (generated-artifact
  quirk, s23 precedent).

### The GNOME-image dead end (asset, not kernel)

The handoff's fake-OOM validation path (/home/william/rux-agents/
gnome-img/gnome.img + fire6.py) DOES NOT EXIST on disk anymore — the
directory is gone, and every fire-r* cold copy was reclaimed per-run
(fire-r84/r85 summaries name the dead path as their image). The
candidates on disk are NOT the GNOME image: oom-accounting's work/
gnome-riscv.img is a plain Ubuntu server rootfs (no Xorg, no
gnome-session, no /root/start-gnome.sh — its /usr/sbin/init is a
symlink into the udesk family), and riscv-gnome's work/ubuntu-gui.img
is the udesk desktop image (no /root chain either). fm24-gnome
(45-min budget, repro-full shape, dfx=...,ledger) booted the fresh
Ubuntu rootfs: kernel reached /sbin/init load, then wedged in early
userspace behind a PREEMPT-LEAK idle flood (1857 lines) — 0 panics,
0 OOM events, no GNOME signal. Boot killed at ~11 min (own pid). The
GNOME-image 342MB/2G verdict needs the image REBUILT (the gnome-int
line's preparation scripts, or main-session restore) — noted for the
main session.

### The filedrain A/B (fake-OOM, minimal-rootfs reproducer)

With the GNOME image unavailable, the drain is reproduced directly:
test/filedrain.c + filedrain_boot.py — N rounds of mmap(160MB data
file, MAP_PRIVATE) + full read-touch + checksum, every mapping KEPT
ALIVE (the GNOME shape: every spawn's libs stay mapped), then fork
children reading mapping 0 (pin duplication + teardown under load),
-m 2G smp2 MTTCG, dfx=taskdump,watchdog,periodic,memwatch,ledger.
Pre-port expectation: N x 160MB of unreclaimable unflagged private
copies + 160MB cache → fake OOM near N=10. Post-port: all rounds
share ONE 160MB pinned cache set (PTE tables only) → clean pass.

- 5-round shakeout (800MB): BOTH kernels pass, 0 OOM markers — the
  2G zone absorbs 960MB; not decisive (both serials also overwritten
  by the shared tag — fixed with --tag).
- **10-round A/B verdict** (run/fm24-fdr-ab.log + /tmp/fdr-work/
  fdr-{pre,post}-serial.log; control = main's e486c3c3 riscv build,
  post = 6e76430b): both complete FDR-PASS (checksums intact, fork
  children fine) — but the memory ledger tells the real story.
  - CONTROL (old private-copy semantics): `used=505848 free=18410` of
    524288 pages — **72 MB from the fake OOM** at 10 live mappings,
    with the copies sitting census-visible as mapped=200017 and the
    evictable 160MB pagecache already squeezed out.
  - PORT (cache-frame mapping): `used=95706 free=428582` — **1.67 GB
    free**, pagecache_pages=40960 and mapped=40961: exactly ONE shared
    160MB set serving all ten mappings, no per-fault copies. 23x the
    headroom under identical load; the same shape as the x86 fm1
    result (365037 free vs pt7's 0-at-OOM). (Both script rc=1s are the
    poller missing the FDR-PASS before QEMU powers off — the serial
    tails carry the marker.)
  - The control surviving at all shows the 2G minimal-rootfs zone
    absorbs ~1.8GB of drain before the edge; the GNOME image's
    342MB/2G OOM fires earlier because the desktop's steady stream of
    NEW mapped libs (never munmapped) compounds the copies with real
    anon+heap. The drain mechanism itself is the one measured here.

### x86_64 MAP_SHARED ext4: writeback-coherence design (task 2, design-first)

Ground truth in main today: x86_64 MAP_SHARED on ext4 falls to the
private-copy path (mappers do NOT see each other's stores; nothing
persists). The riscv64 twin aliases the cache frame (mappers coherent,
PERMANENT pin — never dropped — stores never reach disk; write(2) on
the file invalidates the inode cache and diverges disk vs mapped
frame). Neither is Linux: Linux maps the cache page W, tracks it
dirty, and writes it back through the page cache (msync/fsync/
reclaim), with write(2) going THROUGH the same cache pages.

Design (staged; implement behind probes, mm territory only):

1. **Reuse the PCACHE protocol for SHARED leaves** (the port's second
   payoff): the x86_64 SHARED ext4 fault pins exactly like the private
   arm (get_phys/Discard-warm) and maps the frame with W (NO COW bit),
   PCACHE set. Fork: the existing COW-exemption branch copies the PTE
   and get_pages — add the same bit-keyed pin_by_key the riscv walk
   gained. Unmap/teardown/mprotect: the PCACHE release/carry sites
   from session 23 already handle it. This alone fixes mapper-vs-mapper
   coherence AND retires the riscv alias's PERMANENT pin (riscv can
   then migrate shared_file_fault_page to the same protocol, bounding
   alias memory by live mappings instead of forever).
2. **Dirty tracking, conservative v1**: mark the CachedPage dirty at
   MAP-install time when the mapping is W (a writable shared mapping
   may write; flushing an unwritten page is harmless I/O). Add
   `CachedPage.dirty: AtomicBool`. Precise write-fault-time dirtying
   (write-protect + dirty-on-fault, Linux page_mkwrite shape) is the
   v2 optimization.
3. **Writeback paths (v1)**:
   - msync(MS_SYNC) over a shared file VMA: flush dirty entries of the
     inode in [pgoff, pgoff+len) — pin the entry (ref++), drop the
     inodes lock (NEVER hold it across block I/O — R-WRITESTORM-1),
     write the frame's PAGE bytes through the ext4 write machinery at
     the page's offset, re-take, clear dirty only if not re-dirtied.
   - fsync(fd): flush all dirty entries of the inode.
   - eviction: dirty entries skip eviction (like pinned) — bounded by
     live shared mappings + unflushed dirty pages; a reclaim-time
     writeback is v2 (needs a sleepable flush context).
   - unmap: leave the entry dirty in cache (POSIX allows deferred
     writeback); the pin drops normally.
4. **write(2) coherence**: ext4_file_write detects pinned in-range
   cache entries (under the inodes lock): write-through INTO the frame
   (update the cache page bytes), mark dirty, and DO NOT invalidate
   pinned entries (invalidating a pinned entry would make later faults
   allocate a divergent second frame — breaking mapper coherence).
   Unpinned entries invalidate as today. Page-granular last-writer-wins
   matches Linux for non-concurrent workloads.
5. **The flush primitive**: an ext4 write variant taking the source as
   a cache FRAME (linear-map read, no bounce), reusing block
   allocation for appends/holes; must not invalidate its own range.
6. **Hazards documented**: truncate vs dirty mapped pages (must
   flush/discard in-range dirty entries — today's invalidate_inode
   leaves pinned entries alive and diverging); concurrent msync vs
   store races (last-wins + re-dirty recheck); close(2) does not flush
   (deferred, documented divergence).
7. **Validation plan**: FMAP-shared legs (two mappers see stores
   immediately; write(2)-then-read-back through a second mapping;
   msync persists — verify by reading the file in a SECOND boot of the
   same image; fork children coherent); LTP execl01..execveat02 family
   (the riscv alias's original motivation); OH/GNOME soak for pin
   accounting.

### Session 24 close-out

- Commits: 6e76430b (the port + FMAP/filedrain probes + drivers),
  this archive (+ chain artifacts). Pushed via ssh.github.com:443.
- Chain: r41 FULL budget clean (77659 lines, 0 KERNPANIC/ALLOCTHROW,
  0 WW-HIT, 0 heals), driver self-advanced to r42 at 04:09 (WW-ARMED
  verified in run/ww-chain-s23.log). **Cumulative: 0 WW-HIT across
  r1-r41**, 0 META-SMASH.
- Handoff: (1) the GNOME-image fake-OOM verdict still wants a real
  GNOME boot once the image is rebuilt/restored — the filedrain A/B is
  the mechanism-level evidence (23x headroom); relay the dead-asset
  finding (gnome-img/ + fire6.py gone; gnome-riscv.img and
  ubuntu-gui.img are NOT it) to the main session. (2) The MAP_SHARED
  design above is the next mm implementation item, x86_64 first, then
  the riscv alias migration onto PCACHE. (3) The dfx MEMLEDGER series
  interleaving/garbling note from session 23 reproduces on riscv64
  serials (only 3 MEMLEDGER lines per boot survived legible in the
  control run) — the dfx line's item.

## Session 25 (reconstructed from snapshot 46696158 — quota death at
## 07:07 Oct 23, no archive written at the time)

The predecessor implemented the session-24 MAP_SHARED design end to end
across BOTH arches + the fs layer, dying before any review or
validation. What the snapshot carried (all unreviewed at death):

- page_cache.rs: `CachedPage.dirty`, mark_dirty,
  writeback_update_range (write(2) write-through into pinned frames,
  replacing the write path's whole-inode invalidate), collect/flush_done
  (chunked dirty flush, inodes lock never across I/O), dirty-skip in
  both eviction arms.
- ext4/file.rs: ext4_write_at_core (explicit-offset core shared by
  write/pwrite/flush, u64::MAX O_APPEND sentinel), ext4_flush_dirty_inode,
  write-through replacing invalidate; ext4 native write_at (kills the
  set_pos/write/restore race in generic File::write_at); setattr
  flush-before-cut on shrink; sys_fsync flushes dirty frames before the
  data+journal sync; sys_msync flushes the span's dirty entries (with
  the clamped-span offset-base bug already noted fixed) and skips
  PCACHE leaves in the legacy PTE walk.
- mm both arches: PCACHE protocol for SHARED ext4 faults (W in place,
  PTE-owned pin — the riscv permanent pin retired), hole-as-zero cache
  pages (mapper-coherent zeros, block allocated at flush), PCACHE_SHARED
  carried through mprotect/marker rewrites; riscv alias migrated off
  shared_file_fault_page for ext4; no rmap on PCACHE leaves (retires
  the session-24 page_index-0 hazard).
- test/filemap.c FS1-FS8 + FSP legs and the two-boot persistence pass.

## Session 26 (continuation agent — snapshot reviewed, two critical bugs
## fixed, MAP_SHARED validated on both platforms)

### Inheritance reconstruction

- Chain tree DEAD at claim (r42 killed 15 min in at 04:24 with the
  predecessor's window — clean partial, 0 WW-HIT). Relaunched from r42
  as this session's background task at 07:08.
- Working tree = snapshot 46696158, nothing running otherwise. Audited
  the full diff against the session-24 design before any build.

### Audit verdict: design faithfully implemented; two critical bugs

1. **riscv64 `PCACHE_SHARED = 1 << 10` corrupts the PPN.** Sv39 PTEs
   are `(ppn << 10) | flags` — bit 10 is PPN[0], not an RSW bit (Sv39
   has exactly two: 8=COW, 9=PCACHE, both taken). Setting it adds 4 KiB
   to every shared mapping's physical address: each mapper would store
   into a frame one page away from the cache page — silent neighbor-page
   corruption, the exact engine class this hunt exists to catch. FIX:
   the bit is gone; a SHARED cache leaf is identified by PREDICATE —
   private cache installs now ALWAYS carry COW (read-only installs too),
   so `PCACHE && !COW` (cow_flags::pte_is_shared_cache) holds exactly
   for shared leaves at every consumer (mprotect walk, marker
   re-present). x86_64 keeps its dedicated bit (AVL 11 — three software
   bits exist there).
2. **ext4_flush_dirty_inode could loop forever.** flush_done clears a
   page's dirty bit only when its pin count reaches 0 — with a live
   writable mapping (pin held) the page STAYS dirty by design, and
   collect_dirty_range always returned the same leading chunk: the first
   msync of any file with >16 live-mapped dirty pages (any writable
   shared mapping >= 68 KiB) would rewrite the same 16 pages under the
   ext4 big lock forever. FIX: resume cursor — each round collects from
   the last collected index + 1 (progress rule documented on
   collect_dirty_range).

Smaller fixes in the same pass:

- **Self-copy UB on the flush path**: ext4_flush_dirty_inode writes a
  dirty frame's own bytes through ext4_write_at_core, whose
  writeback_update_range would copy the frame onto itself
  (copy_nonoverlapping with identical src/dst). writeback_update_range
  gained `skip_pfn`; the flush passes its source frame.
- **mprotect W-upgrade dirty gap** (both arches): a shared leaf faulted
  under an RO VMA and later mprotect'ed RW gains W in place but was
  never marked dirty — its stores would never flush. Both mprotect
  sites (x86 mprotect_pte_range via the PCACHE_SHARED bit, riscv walker
  via the predicate) now mark the page dirty at the W-grant
  (page_cache::mark_dirty_pfn — descriptor mapping/index resolution).
- **flush_done clears dirty on FAILED writes**: an I/O error or short
  write left the page's dirty bit cleared at pin-0 anyway — for a page
  whose mapping already unmapped (the frame is the only surviving copy)
  clean-marking made it evictable and its data silently losable.
  flush_done now takes per-entry (idx, pfn, write_ok) and requires ok.
- flush_done empty-key cleanup (put_by_key parity); cosmetic
  re-indentation of the x86 fault-around fill loop the snapshot had
  mangled.

Audit checks that PASSED as-is (documented for the next reader): pin
accounting is mask-based (`& PCACHE != 0`) at every unmap/teardown/fork
site, so the shared bit rides along; the ZERO_PAGE hole arm is
duplicate-insert safe (insert() no-ops on a fresh existing entry, the
following get_phys pins); arm ordering on x86 (SIGBUS gate → devfs
SHARED → ext4 SHARED → fault-around excludes SHARED → private arms) is
correct; the remaining invalidate_inode callers (fallocate, setattr
post-cut, free_inode) are destructive-metadata paths where
invalidate-after-flush is the right semantics; lock order pte → inodes
and EXT4_BIG_LOCK → inodes holds everywhere, no inodes → BIG nesting
(R-WRITESTORM-1 discipline intact); the riscv exclusive-COW fast path
can never see a shared leaf (no COW bit → handle_cow_fault not
reachable).

### Validation (final code, all legs green)

- Builds: x86_64 + riscv64 clean ("Compiling rux" fresh builds;
  config.rs regenerated per platform, restored to the committed x86_64
  flavor at commit — the generated-artifact quirk).
- **FMAP-PASS x (FS1-FS8 + FSP) + FMAP-PERSIST-PASS on BOTH
  platforms** (two boots of the same image, cold cache: the persist
  pattern read back from disk proves msync/fsync writeback reached the
  FILE). This validates: two-mapper immediate visibility (FS1),
  write(2)/pwrite write-through into live mappings (FS2), mapping-store
  → pread coherence (FS3), fork coherence (FS4), msync+fsync flush
  (FS5), truncate cut + past-EOF SIGBUS (FS6), munmap pin release +
  fresh-map retention (FS7), hole zero-fill + store+msync persisting
  into an unallocated block (FS8).
- Regression trio on x86_64: MMF-PASS (x14 legs), FAW-PASS (x6),
  LVMAP-PASS — mprotect/fault/loader paths unregressed.
- Probe-script fix along the way: boot_once now re-reads the serial
  after guest exit (the probe powers down immediately after its verdict;
  the poll loop kept losing the race — the known filedrain class).
- OH 45-min 0-panic soak (ww-fms1, dfx=taskdump,watchdog,periodic,
  memwatch,ledger): see close-out below.
- riscv64 in-kernel unit tests: the first run (early code) was killed
  by the 08:08 infra sweep with its pipe-tail buffer lost; rerun on the
  final code with direct file capture — see close-out.

### Ops events this session

- ~08:08 infra sweep (the s10/s13/s14 class) took BOTH this session's
  background tasks: the chain tree (r43 at 15/45, clean partial: 0
  WW-HIT, 0 panics, healthy to the last line) and the make-test QEMU
  (42 min in, output lost — `| tail > file` buffers everything on
  SIGKILL; capture with plain redirection next time). Chain relaunched
  from r43 at 08:14.

## Session 27 (continuation agent — MAP_SHARED verification close-out)

### Inheritance reconstruction (session 26's close-out was never written)

- Session 26's window died at 08:40 with the user pause, mid-close-out.
  Artifact verdicts recovered at claim:
  - **ww-fms1 (the 45-min OH 0-panic gate) was a ~21-MIN PARTIAL, not
    the gate**: serial born 08:20, last write 08:40:57. Healthy storm
    to the last line (wakes 1.88M), 0 KERNPANIC / 0 ALLOCTHROW / 0 OOM,
    memfree_pages=428556 (1.67 GiB free at death), MM-DROP-CTX 0,
    META-SMASH 0, USEGV-FOR 15 hits all at the documented
    rip=0x7ff082996bac cr2=0 errno-as-pointer service-side family,
    PREEMPT-LEAK 61 (wfi idle noise). Gate INCOMPLETE — re-run below.
  - **riscv make-test rerun: ww-s26-rv-test.log is 0 bytes** — the leg
    never completed. Re-run below.
  - r42 (chain): FULL budget clean (~07:09-07:53, 0 WW-HIT, 0 panics).
    r43 attempt 2 (08:14-08:40): killed at ~26/45 by the session-26
    window death — clean partial (0 WW-HIT, 0 panics, storming to the
    last line). Cumulative 0 WW-HIT across r1-r43.
- Worktree clean at 6b49140b; the riscv alias migration item from the
  handoff was already landed by the session-25/26 snapshot (verified in
  code: ext4 MAP_SHARED faults go through filemap_pin_cache_page with
  shared=true + dirty marking; only non-ext4 files keep the legacy
  shared_file_fault_page permanent pin — page_fault.rs:660-705).

### Task-1 legs re-run at HEAD (all green)

- x86_64 build: PLATFORM=x86_64 make build is a cargo no-op reproducing
  the byte-identical bzImage (md5 1fd06f7b...) — the binary session 26
  validated is the committed code.
- FMAP probe on BOTH platforms at HEAD (fresh boots this session):
  **FMAP-PASS (FS1-FS8 + FSP) + FMAP-PERSIST-PASS on x86_64 AND
  riscv64** — the handoff's probe-extension item (SHARED write
  visibility = FS1/FS2/FS3, concurrent-mapping consistency = FS1/FS3/
  FS4, msync persistence = FS5/FSP two-boot, truncate = FS6) was
  implemented and validated by session 26; re-confirmed here on the
  byte-identical binaries.
- riscv64 in-kernel unit tests (plain file capture, run/
  ww-s27-rv-test.log): **1085/1088** — byte-identical failure set to
  the s6/7/9/12/13/15/18/24 baseline (the 3 pre-existing config_test.rs
  size asserts). No regression from the MAP_SHARED writeback stack.
- ww-fms2 (the 45-min OH 0-panic gate re-run): see below.

## Session 27 close-out

- **ww-fms2 (the 45-min OH 0-panic gate re-run): PASSED.** Full budget
  (timeout kill as designed, 11:21-12:06), 94948 serial lines, storm
  active to the last line, **0 KERNPANIC / 0 ALLOCTHROW / 0 OOM
  events / 0 MM-DROP-CTX / 0 META-SMASH**, memfree_pages=422361
  (1.65 GiB free at boot-end — no drain; the filemap fix holds with
  the MAP_SHARED writeback stack riding). All 20 USEGV-FOR lines are
  ONE victim: pid 546 render_service at rip=0x3fbf08ad30,
  cr2=0x7ff00069a000, rdx=0x3a8 — the EXACT session-18 decoded
  thread-creation geometry (struct-pthread context copy into the
  2-page remnant above a worker-stack carve), service-side and
  documented; capture recipe = 2f54454b prints if it needs a new
  decode.
- **MAP_SHARED writeback verification is COMPLETE** (the handoff's
  task 1, all legs green at 6b49140b): FMAP x both platforms + two-boot
  persistence, riscv 1085/1088, x86_64 byte-identical build, 45-min OH
  0-panic. The "riscv alias migration" follow-on was already landed by
  the session-25/26 snapshot — verified in code this session (ext4
  SHARED faults pin via filemap_pin_cache_page with the conservative
  dirty mark; the legacy shared_file_fault_page permanent pin survives
  only for non-ext4 files, page_fault.rs:660-705). Code anchors for the
  two critical session-26 fixes confirmed: riscv predicate
  pte_is_shared_cache (mm_ops.rs:1838, the removed 1<<10 PPN-corrupting
  bit documented at :1822), flush resume cursor
  (page_cache.rs:804-808), dirty mark family (page_cache.rs:678-700).
  6b49140b is MERGE-READY for main.
- Chain: relaunched from r44 at 11:09 as this session's background
  task (run/ww-chain-s27.log, 15-point set, WW-ARMED verified). r44
  FULL budget clean (77489 lines, wakes 12.71M, strand_max 9.1s
  healthy class, 0 WW-HIT / 0 heals / 0 panics); driver self-advanced
  to r45 at 11:54 (armed verified). Cumulative: **0 WW-HIT across
  r1-r44** partials, 0 META-SMASH. The make-test QEMU reaped after
  results were file-captured (post-summary panic class, own PID).
- Ops: serial-log grep discipline note — the rs-image OH boots print
  no "fork: parent" lines in their storm phase (storm health reads
  from appspawn/APPMAPSRC counts instead; the ww-boot chain image
  prints both).
- Late-session ops: the chain background task was stopped externally
  at ~12:10 mid-r45 (serial frozen 12:09:23 mid-storm, whole tree gone
  — the s10/s13/s14 sweep class; the samgr-callback line started its
  own oh-sc172 boot in the same window). r45 verdict: clean partial at
  ~15/45 (53677 lines, storm-healthy tail, 0 WW-HIT, 0 panics).
  Relaunched from r46 at 12:20 (run/ww-chain-s27b.log, WW-ARMED 15
  verified, process tree re-recorded). Cumulative: **0 WW-HIT across
  r1-r45** partials.
- Next for the line: (1) chain continues toward the capture (the
  machinery is proven; the engine's rate under this load remains the
  variable); (2) 6b49140b rides to main at the next merge window; (3)
  unclaimed follow-ups from the archive: reclaim-time writeback (v2),
  close(2) deferred-flush divergence note, watchset room beyond 15.

## Session 28 (continuation agent — watchset to 19, reclaim-time dirty writeback v2)

### Inheritance reconstruction

- Base: fast-forward merge of main (4496f7fb — main took the session-27
  close-out via merge; our HEAD was already its parent). Clean tree.
- The handoff's "r46 died with session 27" was STALE for the SIXTH time:
  the whole tree was alive at claim (wrapper 2628474 → chain 2628485 →
  hunt 2628490 → qemu 2628491), r46 armed with the 15-point set,
  storming 5 min into its budget. Claimed as-is; no restart.

### Watchset 15 → 19: the full timekeeper neighborhood (commit 5b1995f0)

- Writer analysis (source-grounded): TSC_ACTIVE (:212/:381),
  TSC_CONTINUITY_OFFSET (:211/:380), TSC_BASE (:210/:379) store only
  inside the SAME HPET-gated calibration blocks that store TSC_MULT
  (:209/:378) — dead code on these boots (hpet::ready() since second 1;
  TSC_MULT has zero legit hits across r19-r45). JIFFIES_BASE is written
  once by the first-tick compare_exchange (x86_64.rs:433-436), long
  before the +20s gdb attach. The hot JIFFIES qword (0x8f98, CAS/fetch
  per tick) stays excluded — 8-byte watchpoint granularity separates it
  cleanly.
- ww-cap3 (bare boot, port 12403, own PIDs): 20 SIMULTANEOUS hw
  watchpoints arm with 20/20 confirmations and no insertion errors;
  position 20 (JIFFIES) FIRED 3629x in 40s with the writer symbolized
  (atomic_compare_exchange_weak = the increment_jiffies CAS loop) —
  capacity >= 20 with firing at the deepest position, retroactively
  validating the 15-point set's positions too. 0 hits on the 19 quiet
  candidates.
- Deployed from r47: the full 0xffffffff80be8f80-8fb0 stretch
  (TSC_ACTIVE, JIFFIES_BASE, TSC_CONT_OFF, JIFFIES, TSC_BASE, TSC_MULT)
  is now tiled except the hot JIFFIES qword — every confirmed victim
  geometry (b77/wt1 wide-smash fingerprint included) sits under
  watchpoints. r47 verified WW-ARMED 19, GUARD unchanged (frozen pair
  untouched).

### Chain stewardship

- r46 verdict: clean run — 66816 serial lines, wakes 8.32M,
  strand_max 10308ms (the healthy 9.5-10.5s class), ticks [100,100],
  0 KERNPANIC/ALLOCTHROW, 0 WW-HIT, 0 heals. Ended at ~32/45 min by an
  external timeout kill (rc=124 mid-storm, no teardown lines — the
  sweep class; the driver self-advanced). Cumulative: **0 WW-HIT across
  r1-r46**, 0 META-SMASH.
- r47+ rides the 19-point set (verified armed).
- Ops: the ~12:52 user pause produced the automatic wip snapshot commit
  (24d43e53) of this session's in-flight v2 work — the established
  pause-snapshot mechanism; work continued on top.

### Reclaim-time dirty writeback v2 LANDED (the session-24/27 follow-up)

The abandonment shape: a writable MAP_SHARED mapping stored, munmap'ed,
never msynced leaves every page a DIRTY, unreferenced cache entry that
evict_one must skip (evicting loses the only copy) — permanently
unevictable, invisible to reclaim, bounded only by "a later open+fsync
on the same file". The close(2)-deferred-flush divergence item folds
into this one: POSIX explicitly permits deferred writeback past close
(Linux does not flush on close either); the REAL gap was that no path
ever flushed without an explicit sync. Assessment: document, don't bolt
blocking I/O onto close(2) (that would diverge from Linux in the other
direction); fix the gap where Linux fixes it — in reclaim.

Implementation (page_cache.rs + vmscan.rs, arch-neutral):

- `InodePageCache` records its raw `(fs_id, ino)` (cache_key is a
  non-reversible hash; fs_id IS the Ext4FileSystem pointer, so the raw
  pair resolves everything).
- `PageCache::shrink_reclaim(nr)`: clean pass first (plain shrink), then
  on any SHORTFALL — deliberately not only at the exhausted wall, so the
  flush's own ext4/bio allocations still have memory to allocate (the
  Linux dirty-threshold insight; flushing only at the wall would make
  every journal/buffer alloc fail and the pages stay dirty forever) —
  collect up to 2 inode windows of 16 dirty unreferenced pages under the
  inodes lock, DROP the lock, resolve a live VFS Inode via
  `icache_lookup(ino, fs_id)` (Inode.fs_id == the fs pointer for ext4 —
  the same liveness contract sys_fsync uses), call the EXISTING
  ext4_flush_dirty_inode on the window, loop (the next clean pass evicts
  what the flush cleaned). Bounded: 2 inodes x 16 pages x 4 rounds per
  call; a WRITEBACK_GUARD AtomicBool degrades nested re-entry (reclaim
  inside the ext4 write path's own allocations) to clean-only.
- vmscan shrink_inactive_list switches to shrink_reclaim — direct
  reclaim only. That path is task context, spinlock-free, globally
  serialized by IN_DIRECT_RECLAIM, and ALREADY performs blocking swap
  I/O (reclaim_anonymous_pages), so blocking flush I/O changes no
  context contract. The mmu_init PT-alloc retry (per-mm PteLock held)
  KEEPS plain shrink() — the session-19 deadlock argument.
- An inode missing from icache is skipped (stuck as before, bounded; a
  future open() re-caches it). A mapper racing the flush keeps the page
  dirty (flush_done pin rule) — rewritten next round, same POSIX
  last-writer-wins latitude as concurrent msync.

### reclaimwb A/B (the v2 validation, x86_64, q35/TCG/smp2/2G)

New probe test/reclaimwb.c + reclaimwb_boot.py: ftruncate a 256MB file,
mmap MAP_SHARED RW, store a pattern every 64 words, munmap, close —
NO msync/fsync anywhere — then a sacrificial child touches anonymous
memory until it dies at the wall (survivable pagefault-OOM path),
reporting progress every 64MB. Discriminator = how far the child got +
the OOM-moment census (no knife-edge RAM arithmetic).

| kernel | child reached | census at OOM | panics |
|---|---|---|---|
| control 1fd06f7b (pre-v2) | 640MB | pagecache_pages=65536 (= the 256MB, dirty-stuck), anon=178755 | 0 |
| v2 e5038877 | 896MB | pagecache_pages=0, lru_file=0, dirty=0, anon=244164 | 0 |

The sums reconcile exactly: control anon 178755 + pagecache 65536 =
244291 ≈ v2 anon 244164 = the zone's true ceiling (~959MB managed; 2G
guest minus the 1GiB kernel-heap reservation). v2 converted the entire
abandoned dirty region into reclaimable memory; both worlds survived
the OOM-kill cleanly (0 panics; the v1 shape is the pre-existing
documented behavior, not a regression). Serial diagnostics during the
pressure grind (TIMER-SOFTIRQ-STALL ~1-4s, IRQ-OFF-WINDOW) symbolize to
the known idle/softirq classes (cpu::wfi, enable_irq) — no new
irq-off pathology from the flush path.

### Regression legs (all green)

- x86_64 (e5038877): FMAP PASS both passes (FS1-FS8 + FSP two-boot
  persistence through the new shrink_reclaim path).
- riscv64 (50fc6af5): in-kernel unit tests **1085/1088** (run/
  ww-s28-rv-test.log — byte-identical failure set: the 3 pre-existing
  config size asserts); FMAP PASS both passes; MMF-PASS (serial-verified;
  the driver poller missed the poweroff race, serial has 0 FAILs);
  reclaimwb PASS — the 128MB dirty region flushed, the 600MB child
  target COMPLETED (1G guest had room once the dirty pages returned).
- make-test QEMU reaped after results were file-captured (post-summary
  panic class, own PID after cwd verification); chain r47 untouched.

### Session 28 close-out

- **ww-v2soak (the 45-min OH 0-panic gate on the v2 kernel): killed at
  20/45 min by the user pause (below) — a HEALTHY PARTIAL, not the
  completed gate.** 72967 lines, full storm (864 forks, wakes 1.63M),
  0 KERNPANIC / 0 ALLOCTHROW / 0 OOM events, memfree_pages=454461
  (1.77 GiB free at death — no drain; the ww-fms2 45-min baseline was
  422361), strand_max 29s (the >=3-QEMU host-contention class: chain
  r47 + this soak + the samgr line ran concurrently). The completed
  45-min 0-panic boot is the ONE remaining merge gate for 4fb5d480 —
  everything else (A/B, both-arch probes, unit tests) is green.
- **Ops finding — the mid-session kills were USER PAUSES, not a rogue
  reaper:** QEMU trees died at 12:52:17 and 13:27:40, each minutes
  BEFORE the real 2700s budget, each with the `timeout`-shaped rc=124
  mid-storm end. run/PAUSE-NOTE-2026-10-23.md (written by the pause
  mechanism) settles it: "Killed at pause" — the 12:51:56 wip snapshot
  (24d43e53) and the 13:28 note are the same two events. Victims: r46
  (12:52), then r47 + ww-v2soak simultaneously (13:27:40) and the chain
  wrapper + r48 ~40 s into its run (~13:28). The chain's session-27
  ownership did NOT matter — pauses reap QEMU tasks wholesale. Chain
  relaunched from r49 at 13:46:52 as this session's own background task
  (the pause note's "resume from r48" was one run stale — r48 had
  already started and died with the wrapper).
- r47 verdict (collected before the reaper took it): FULL 2700 s clean —
  71411 lines, wakes 9.91M, strand_max 9648ms (healthy class), 0
  WW-HIT / 0 heals / 0 panics. **First complete run on the 19-point
  set.** r48: killed at ~40 s by the reaper (trivial clean partial,
  0 hits). Cumulative: **0 WW-HIT across r1-r48**, 0 META-SMASH.
- Commits: 5b1995f0 (watchset 19 + cap3), 24d43e53 (pause snapshot of
  the v2 WIP, pushed by the snapshot mechanism), 4fb5d480 (v2 landing +
  probes + this archive). All pushed via ssh.github.com:443.
- Next for the line: (1) chain continues at r49 under live-session
  ownership — any WW-HIT on the 19-point set is the capture; (2) the
  45-min OH 0-panic soak re-run for 4fb5d480 (or it rides the next
  merge window's gate); (3) MERGE-READY with that one gate named:
  reclaim-writeback v2 (A/B-proven: stuck-640MB vs flushed-896MB with
  census reconciliation), FMAP/MMF/make-test green on both arches.

## Session 29 (continuation agent — post-reboot claim: r49/r50 verdicts, chain from r51, v2soak gate re-run)

### Inheritance reconstruction (handoff "resume from r48" was two runs stale)

- The handoff note predated session 28's close-out: session 28 had
  already relaunched the chain from r49 at 13:46:52 (after the 13:28
  pause). r49 ran its FULL budget clean (106265 serial lines, 0 WW-HIT,
  0 heals, driver self-advanced at 14:31:52). r50 was then killed
  externally at ~15 min (serial last write 14:46, whole tree gone —
  session-28 window death class).
- **The HOST was rebooted**: at claim (17:13) uptime was 4 minutes, so
  everything from before ~17:09 is gone regardless. No chain/QEMU
  processes survived; nothing else was running on the host at claim
  (44G free of 59G, no other line's QEMUs back yet).
- r50 verdict from the artifacts: CLEAN PARTIAL — 54022 lines, full
  storm to the last line (wakes 3.56M, strand_max 8764ms = healthy
  class), 0 WW-HIT in the gdb log, 0 KERNPANIC/ALLOCTHROW. Cumulative:
  **0 WW-HIT across r1-r50**, 0 META-SMASH.

### Chain relaunched from r51 (primary task)

- `./run/ww-chain.sh ww-s2 100 12371 51` as this session's background
  task, 17:15. r51 verified armed with the full 19-point set
  (WW-ARMED 19, GUARD=0xffffffff80a7a000, frozen pair untouched) and
  storming within 4 min (36k serial lines, wakes 480k).
- Process tree recorded in run/ww-s29-proctree.txt: bash 11631
  (ww-chain.sh, THE driver root — kill first, then the QEMU) ->
  ww-hunt.py 11646 -> timeout 11647 -> qemu 11657.

### v2soak gate re-run (the one remaining merge gate for 4fb5d480)

- Worktree rebuild: `PLATFORM=x86_64 make build` is a cargo no-op that
  reproduces the byte-identical v2 kernel (md5 e5038877... = the
  session-28 A/B "v2" binary; build/ helper dir was wiped by the
  reboot, target/ survived).
- ww-v2soak2 (run/ww-v2soak2.sh): TAG=ww-v2soak2, worktree bzImage,
  dfx=taskdump,watchdog,periodic,memwatch,ledger (same flags as the
  ww-fms2 45-min baseline so memfree_pages compares), SECS=2700,
  own port 12375, launched 17:16 as a script-file background task.

### Session 29 close-out (reconstructed by session 30 from artifacts)

- The session-29 window died ~17:27 (serial last write 17:27:13) — it
  took BOTH background tasks at once, ~11 min into each:
  - r51 (chain, launched 17:15): CLEAN PARTIAL — 50264 lines, full
    storm (wakes 2.53M, strand_max 3241 ms healthy class), 0 WW-HIT,
    0 KERNPANIC/ALLOCTHROW. Cumulative: **0 WW-HIT across r1-r51**,
    0 META-SMASH.
  - ww-v2soak2 (gate re-run, launched 17:16): HEALTHY PARTIAL at
    11/45 min — 55086 lines, 702 forks, wakes 652k, memfree_pages
    ~455k (1.77 GiB free — no drain; v2soak's 20-min partial read
    454461), **0 KERNPANIC / 0 ALLOCTHROW / 0 OOM events**, 0
    MM-DROP-CTX / META-SMASH. The completed 45-min gate remains open.
- Session 30 verified the push state: origin/feature/wildwrite-hunt
  already sat at 47e5ab80 (session 29 pushed; the local tracking ref
  was stale because https fetch times out — check the remote directly
  before concluding "unpushed").

## Session 30 (continuation agent — claim after host reboot + window death; chain from r52, v2soak3)

### Inheritance reconstruction

- Handoff said "resume from r48" — STALE BY FOUR ROUNDS (the recurring
  pattern; archive sessions 28-29 are authoritative over pause notes):
  session 28 relaunched from r49 (13:46), session 29 from r51 (17:15).
  True state at claim (20:05): NOTHING running since 17:27; host
  rebooted 17:09 (uptime 2:57 at 20:06); latest artifacts r51 + 
  v2soak2, verdicts above.
- Frozen hunt pair intact (Oct 21 15:05, GUARD=0xffffffff80a7a000
  matches the r51 arm). Worktree v2 bzImage md5 e5038877... =
  byte-identical to session-28's validated v2 binary.

### Stewardship

- Chain relaunched from r52 at 20:07 (run/ww-chain-s30.log):
  WW-ARMED 19 verified, storming within 3 min. Driver root = the
  ww-chain.sh shell (PID tree recorded in run/ww-s30-proctree.txt:
  bash → ww-chain.sh 22556 → ww-hunt.py 22561 → timeout 22562 →
  qemu 22572). Chain-discipline reminder honored: kill the driver
  root FIRST, then the round's QEMU.
- ww-v2soak3 (run/ww-v2soak3.sh): the completed-gate re-run for
  4fb5d480, own port 12376, same flags/kernel as v2soak2. Verdict
  below.
- Foreign-QEMU watch at claim: samgr-callback line's KVM q35 boot
  (PID 22503, cwd /home/william/rux-agents/samgr-callback) started in
  the same minute — NOT touched (launcher-adjacent line, host-load
  discipline: my 2 QEMUs + theirs = 3 total, the known contention
  class for si_busy/strand readings).

### r52 + the wildwatch landing

- r52 (last round of the old flagless chain): FULL 2700 s clean —
  71684 serial lines, 0 WW-HIT, 0 heals, 0 panics; the old driver
  exited cleanly on the stop file (placed to avoid an in-place edit of
  a script a live driver was executing — bash reads scripts
  incrementally). Cumulative: **0 WW-HIT across r1-r52**, 0 META-SMASH.

- **dfx=wildwatch now rides every hunt boot (r53+).** Discovery while
  auditing the 19-point coverage: the 2-page hpet guard cluster (8 KB)
  had 8 of 1024 qwords hardware-watched and NO kernel-side tripwire —
  the dfx flag existed in the frozen kernel (42eb2006, pre-refreeze)
  but was never added to EXTRA_APPEND, and the gdbscript doc even said
  "do NOT combine" (written when the cluster points WERE the watchset).
  With the tripwire armed the whole cluster is RO in every alias: any
  CPU store there page-faults at the writer's RIP with the full
  KERNPANIC attribution dump (addr2line on the frozen ELF), and the 1 s
  canary cadence covers PTE-invisible/DMA-class writers — a ~128x
  capture-surface increase on the confirmed-victim neighborhood. The
  11 non-cluster watchpoints stay gdb's exclusive rip-attributing
  surface; the 8 cluster points ride as an arm-failure backstop (the
  heal paths are guard_armed()-gated — verified zero legit post-arm
  writers before landing).
- ww-chain2.sh (successor driver): stop check now greps BOTH logs —
  WW-HIT (gdb), WILDWATCH-CANARY / scribble-heals ("hpet: clock
  base|div|offset healed") / KERNPANIC (serial) — plus a per-round
  anomaly note when the armed line is missing. "clock corrected
  backward" (jitter clamp) deliberately not a stop marker.
- r53 (first wildwatch round, launched 20:54 under ww-chain2.sh):
  tripwire armed on try 0 ("2 guard pages RO (0 failed), 4 alias PTEs
  guarded"), 19-point set armed, full storm — the boot is healthy with
  the cluster RO, validating coexistence live.

### v2soak3: THE 45-MIN GATE PASSED — 4fb5d480 fully merge-ready

- Full budget (timeout kill as designed, 20:07-20:52), 141111 serial
  lines, 1197 forks, wakes 5.96M to the end, **0 KERNPANIC / 0
  ALLOCTHROW / 0 oom-killer / 0 OOMFX / 0 MM-DROP-CTX / 0 META-SMASH**,
  memfree_pages=429832 (1.68 GiB free at boot-end — no drain; ww-fms2
  baseline 422361). Every gate for the reclaim-time dirty writeback v2
  is now green: the A/B (stuck-640MB control vs flushed-896MB with
  census reconciliation), FMAP/MMF both arches, make-test 1085/1088,
  and this 45-min 0-panic soak. MERGE-READY for the next main window.

### qs2 (watchset-repurpose probe) — in flight

With the cluster under the kernel trap, the 8 cluster watchpoint slots
free up for outside-cluster statics. Eight candidates identified by
source analysis (HEAL_LAST at fc88b0 — the hpet::BASE wide-smash
neighborhood; KERNEL_MAP; VMEMMAP_INIT+INIT_DONE; the console ISIG pair
  and the three medium-risk console statics TX_LAST_NOTICE /
  DUMP_MAGIC_POS / LEDGER_MAGIC_POS). run/ww-qs2.sh boots a 15.5-min full
  OH storm with the 8 armed + wildwatch ON (coexistence validated in the
  same boot — armed try 0, storm healthy).

**qs2 VERDICT: ALL 8 CANDIDATES STORM-QUIET — zero hits** through a
56529-line full storm (wakes 2.89M, 0 panics, tripwire armed try 0).
Even the three medium-risk console statics stayed silent (the
TX-recovery notice path and the per-RX-char magic scanners never fire
on headless storm boots). The watchset repurpose is DEPLOYED in
ww-gdbscript.py (rides automatically from r54): the 8 redundant
GUARD-cluster points replaced by the 8 proven-quiet candidates —
19 points total, now ALL outside the cluster, tiling the hpet::BASE,
timekeeper, console-UART, and PHYS_RAM_BASE neighborhoods, with the
whole guard cluster under the kernel RO trap beneath them.

### Session 30 close-out

- r53 (first full wildwatch round): FULL 2700 s clean — 78978 lines,
  0 WW-HIT, 0 heals, 0 panics, tripwire armed the whole boot. r54
  verified armed with the REPURPOSED 19-point set (points 1-8 = the
  qs2 candidates) + wildwatch try 0 — the chain now runs at maximum
  capture configuration: kernel RO trap over the whole 8 KB guard
  cluster (rip-attributed KERNPANIC on any CPU store), 1 s canary
  cadence (DMA-class detection), 19 hardware watchpoints on every
  outside-cluster victim neighborhood, and the two-log stop check.
- Cumulative: **0 WW-HIT across r1-r53**, 0 META-SMASH. The engine's
  rate remains below one fire per 51+ armed boot-hours; the capture
  surface is now ~128x larger on the cluster + 8 new statics, so the
  per-boot-hour hit probability rises accordingly from r54 on.
- Archive follow-ups from the handoff all closed this session: reclaim
  writeback v2 (gate passed — merge-ready), close deferred flush
  (folded into v2's reclaim design, documented s28), watchset room
  (wildwatch + repurpose, above).
- Chain ownership: driver root = ww-chain2.sh shell (PID tree in
  run/ww-s30-proctree2.txt; kill the driver root FIRST, then the round
  QEMU). Ops notes: bash reads scripts incrementally — never edit a
  script a live driver executes (use the stop-file handoff + successor
  script); the qs1-style inline probe drivers were saved as durable
  scripts (ww-qs2.sh) this session.
- Commits this session (all pushed via ssh.github.com:443): 9004ebc1
  (wildwatch on chain + serial capture markers), 4097d56f (stop-file
  fix), 7f8b2a6a (qs2 probe), 3415f54c (r52/v2soak3/r53 archive),
  f957865f (watchset repurpose deployment), + this close-out.
- Next for the line: (1) chain continues — any WW-HIT /
  WILDWATCH-CANARY / scribble-heal / KERNPANIC marker auto-stops it
  with the evidence in place; (2) 4fb5d480 rides to main at the next
  merge window (all gates green); (3) if long stretches stay silent,
  the remaining escalation is load-profile diversity (fire6-style sdc
  cycles) rather than more statics — every confirmed victim geometry
  is now instrumented.


## Session 31 (continuation agent — chain claim, load-diversification
## surface deployed: the sdc-cycle hunt chain)

### Inheritance reconstruction

- Chain ALIVE at claim (21:50): session-30's tree (wrapper 29743 →
  ww-chain2.sh 29754 → r53 qemu) still storming, r53 already FULL-clean
  per the chain log (78978 lines, 0 WW-HIT / 0 heals / 0 panics — the
  s30 close-out verdicts hold). r54 in flight, verified armed with the
  repurposed 19-point set + wildwatch try 0. Proctree recorded
  (run/ww-s31-proctree.txt).
- 21:54: session-30's zcode window died and reaped its background tree —
  the chain died MID-r54 (~15/45 min). r54 verdict from artifacts:
  CLEAN PARTIAL (56823 lines, storm-healthy, 0 WW-HIT / 0 KERNPANIC /
  0 canary / 0 heals, armed to the last line). Cumulative: **0 WW-HIT
  across r1-r54** (r54 counted once), 0 META-SMASH.
- Chain relaunched from r54 at 22:18 as THIS session's background task
  (run/ww-chain2-s31.log; note: the relaunch overwrites the killed
  round's same-tag logs — its verdict lives here and in git history).
- 4fb5d480 (reclaim writeback v2) confirmed in main f91ec1cc per the
  handoff; nothing to redo there.

### Load-profile diversification deployed (the archive's named escalation)

Rationale: 0 fires in 51+ armed boot-hours under the ONE load profile
the chain generates (standard OH storm). The two historical trigger
contexts — b77's sdc fork/exec boundary and wt1's deep storm — are both
instrumented; the variable left is load diversity. sdc cycles were the
b77-era context, so the s30 close-out's escalation is exactly this.

Surface (private image, shared images untouched):

- run/ww31-system.img: private 3 GB copy of the OH system.img carrying
  ONE change — /system/etc/init.cfg's import array additionally imports
  /etc/init.wwload.cfg, which adds a `start wwload` cmd to the
  post-fs-data job and defines the service: /system/bin/sh
  /system/bin/wwload.sh, once, uid root. wwload.sh is an endless paced
  loop: `echo WWLOAD-BEAT; timeout 2 /system/bin/sdc` — ~2-3 s per
  cycle, i.e. hundreds of full fork+execve+interpreter+COW+teardown
  lifetimes of the 182 KB StorageDaemon binary per round, on top of the
  natural storm. Boot stays -snapshot + file.locking=off (all guest
  writes discarded).
- Injection method note (for successors): debugfs `write`/`rm` on this
  image CORRUPTED silently (orphan + bad-type inodes; e2fsck then
  deleted /system/etc/init.cfg). The working method: fresh copy → sudo
  loop-mount → install files → clean umount → e2fsck -fn green. Do not
  use debugfs writes on this image.
- run/ww-loadboot.sh (ww-boot.sh clone pointing at the private image;
  ww-boot.sh itself untouched — a live driver executes it),
  run/ww-loadprobe.sh (ww-hunt.py-equivalent round driver: frozen kernel
  + dfx=wildwatch + FULL 19-point ww-gdbscript.py set, own port 12381),
  run/ww-loadchain.sh (chain wrapper, same two-log capture-marker stop
  check as ww-chain2.sh plus a per-round sdc-cycle sanity count).

### wwload-p1 probe verdict (930 s): ALL GREEN — surface validated

- Import + service start confirmed in serial ("Import /etc/init.wwload.cfg",
  "ServiceStart starting:wwload"; the "No secon item in wwload" SELinux
  error is non-fatal — the service runs).
- Load measured: 285 sdc execs in the window vs 1 in baseline rounds
  (r53/r54). WWLOAD-BEAT echoes are invisible (service stdout is not
  the console) — the kernel `exec: path=/system/bin/sdc` lines are the
  load heartbeat.
- Capture stack fully armed (WW-ARMED 19, wildwatch try 0) and coexisting
  with the load; storm healthy (1445 forks, normal strand classes).
- 0 WW-HIT / 0 KERNPANIC / 0 canary / 0 heals — quiet window, zero
  false positives from the load.

### Both chains live at close of work (QEMU budget: chain 1 + experiment 1)

- MAIN chain: ww-chain2.sh driver (relaunched from r54, log
  run/ww-chain2-s31.log, port 12371) — standard-storm profile.
- LOAD chain: ww-loadchain.sh driver tag wwload from r1 (log
  run/ww-loadchain-s31.log, port 12381, rounds 2700 s) — sdc-cycle
  profile. Both at maximum capture configuration (19-point set + RO
  tripwire + canary).
- Driver roots recorded in run/ww-s31-proctree.txt; kill the DRIVER
  ROOT first, then the round QEMU (chain discipline). Foreign QEMUs at
  close: samgr-callback 43030, main-workspace 43497 — untouched.
- Host load with both chains + foreign QEMUs reached ~5.7 (the known
  >=3-QEMU contention class): strand_max readings from these windows
  are inflated; capture markers (exact watchpoints/trap) are not
  affected.

### Next for the line

1. Both chains hunt toward r100 or a capture; any WW-HIT /
   WILDWATCH-CANARY / scribble-heal / KERNPANIC auto-stops THAT chain
   with evidence in place (gdb log for watchpoint rip-attribution;
   serial KERNPANIC dump for the tripwire; addr2line on the frozen ELF).
2. If the load chain stays silent over several rounds, next diversity
   step is MORE load flavors on the same image (e.g. multi-worker sdc,
   bootanimation cycles) — not more statics.
3. Successor claiming a dead load chain: relaunch with
   `./run/ww-loadchain.sh wwload 100 12381 <next-round>`.

### Session 31 first-round results (both chains, full budgets)

- MAIN r54 (completed under session-31 ownership): FULL 2700 s clean —
  77656 lines, 0 WW-HIT / 0 heals / 0 panics. Cumulative: **0 WW-HIT
  across r1-r54 complete rounds** (plus partials), 0 META-SMASH. Chain
  auto-advanced to r55.
- LOAD r1 (first full sdc-cycle round): FULL 2700 s — 827 sdc exec
  cycles (vs 1 in a baseline round; ~19/h under dual-chain host
  contention), storm healthy to the end (wakes 11.27M, forks 2685),
  strand_max 52.5 s (the documented >=3-QEMU host-contention class —
  both my chains + foreign QEMUs share the host; capture markers are
  exact, unaffected), 0 capture markers of any kind, tripwire armed
  try 0. Chain auto-advanced to r2.
- Ops: a transient foreign QEMU (PID 46915) briefly appeared in THIS
  worktree's cwd at 22:43 and exited on its own within a minute — not
  started by this session's scripts (both chain QEMUs were continuous);
  no action needed, noted for the record.

### Session 31 close-out

- **Window-death kill at ~23:18**: both chain background tasks died
  mid-round (r55 at ~25/45, load r2 at ~23/45) with the session window —
  the known class, NOT a user pause (no new PAUSE-NOTE; the samgr
  line's KVM QEMU survived). Both partials clean (0 markers, 0 hits:
  r55 57483 lines, load-r2 82717 lines/255 cycles). Both chains
  relaunched within 10 min (main from r55 at 23:28:23, log
  run/ww-chain2-s31b.log; load from r2 at 23:28:29, log
  run/ww-loadchain-s31b.log), verified armed (19 + wildwatch try 0).
- **r55 (full, relaunched): CLEAN** — 79694 lines, 0 WW-HIT / 0 heals /
  0 panics. Cumulative: **0 WW-HIT across r1-r55 complete rounds**,
  0 META-SMASH. Chain at r56.
- **load r2 (full, relaunched): CLEAN** — 168085 lines, 782 sdc cycles,
  0 capture markers of any kind. Load chain at r3.
- Ops note for successors — the "ghost QEMU" pattern: `pgrep -f
  qemu-system` in a watch command matches the WATCH COMMAND'S OWN bash
  -c wrapper (the pattern string is in its cmdline; cwd = the worktree).
  The transient PIDs seen this session (46915, 50251, 50829, 52469)
  were all self-matches, gone in seconds. Real QEMU census must use
  `pgrep -f 'qemu-system-x86_64 -M'` from a DIFFERENT shell than the
  pattern-bearing one, or check /proc/<pid>/cmdline directly.
- Session totals: main-chain rounds r54-r55 completed clean under s31
  ownership + 3 clean partials (r54-first-attempt, r55-first-attempt,
  load-r2-first-attempt); load surface deployed and validated (probe
  p1: 285 cycles/930 s; rounds r1: 827 cycles, r2: 782 cycles per
  2700 s — ~19/h under 2-chain host contention vs ~33/h solo); zero
  capture markers anywhere. The engine remains silent under both load
  profiles; the doubled capture surface hunts on.
- Standing state for the successor (if both chains died with this
  window):
    ./run/ww-chain2.sh  ww-s2  100 12371 <next-main-round>
    ./run/ww-loadchain.sh wwload 100 12381 <next-load-round>
  Stop: touch run/ww-s2.stop / run/wwload.stop; kill the DRIVER ROOT
  (ww-chain2.sh / ww-loadchain.sh shell) first, then the round QEMU —
  proctrees in run/ww-s31-proctree.txt.

### Session 31 addendum — second sweep, chains relaunched again

- Second window-death sweep at ~00:28 (70 min after the first): both
  chains killed mid-round again (r56 / load r3, each ~15/45 min in,
  clean partials: r56 54134 lines, load-r3 79261 lines, 0 markers,
  0 hits, armed to the last line). Relaunched immediately (main r56 at
  00:29:07 run/ww-chain2-s31c.log; load r3 at 00:29:11
  run/ww-loadchain-s31c.log; both verified armed).
- Sweep cadence observation for the successor: ~70 min between sweeps
  this session (23:18, 00:28); each window completes at least one full
  round per chain (r54+r55 full / load r1+r2 full in the first two
  windows). If a future session sees both chains dead again with no new
  PAUSE-NOTE, relaunch per the close-out recipe — the standing state
  and verdict ledger live in this file.

### Session 31 final ledger

- r56 (full, second relaunch): CLEAN — 81784 lines, 0 WW-HIT / 0 heals /
  0 panics. Cumulative: **0 WW-HIT across r1-r56 complete rounds**.
  Chain at r57 (01:14:07).
- load r3 (full, second relaunch): CLEAN — 165380 lines, 785 sdc cycles,
  0 capture markers. Load chain at r4 (01:14:14). Load-surface totals:
  p1 + r1 + r2 + r3 = 2679 sdc exec cycles over ~4 armed load-hours,
  zero fires, zero false positives.
- Both chains hunting at max capture config (19-point set + RO tripwire
  + canary) on two load profiles (standard storm; sdc-cycle storm).

## Session 32 (reconstructed from artifacts — quota death at ~02:39, no
## archive written at the time)

- Claimed 01:28 with BOTH chains alive (inherited from the s31 relaunch:
  r57 / load-r4 in flight, proctree in run/ww-s32-proctree.txt).
- ~01:38 sweep (session-31 window death) killed both rounds at ~14 min —
  clean partials, 0 markers. Relaunched immediately at 01:38:55 (main
  from r57, load from r4; logs run/ww-chain2-s32.log / ww-loadchain-s32.log).
- **MAIN r57 (full): CLEAN** — 78348 lines, 0 WW-HIT / 0 heals /
  0 panics, wildwatch try 0. Driver advanced to r58 (02:23:56).
- **LOAD r4 (full): CLEAN** — 773 sdc exec cycles, 0 gdb hits, 0 capture
  markers, storm healthy (wakes 10.42M, avg_run 3ms, strand_max 43.5s =
  the >=3-QEMU contention class), 166490 serial lines. Driver advanced
  to r5 (02:23:58).
- r58 + load-r5 started 02:23:56/58; the quota death killed the s32
  window at 02:38:55, ~15/45 into both — clean partials armed to the
  last line: r58 53664 lines (wakes 3.54M, strand_max 8.8s healthy
  class, 0 markers), load-r5 83634 lines (wakes 2.78M, 248 sdc execs,
  0 markers). The load-r5 "signal 15 from (timeout)" line is the
  timeout process RELAYING the sweep's SIGTERM to its QEMU, not a
  budget expiry.
- No commits from s32 (HEAD stayed 3b8bf325; untracked round artifacts
  only). Cumulative at s32 close: **0 WW-HIT across r1-r57 complete
  rounds** (r58 partial x2), 0 META-SMASH; load surface at 5 full armed
  load-hours (p1 + r1-r4 = ~3452 sdc exec cycles), zero fires, zero
  false positives.

## Session 33 (continuation agent — both chains reclaimed dead, relaunched r58 / load-r5)

- Claim 03:29: BOTH chains dead (the s32 quota-death sweep), no QEMUs of
  any line except a later-starting perf-audit KVM boot (foreign,
  untouched). Host up since 17:09 (no reboot), load 0.37, 49G free —
  capacity fine for the two chain QEMUs.
- Frozen hunt pair verified intact (bzImage md5 bf779126...; GUARD=
  0xffffffff80a7a000 matches every arm line). Stop files absent.
- Relaunched at 03:30:26 as this session's background tasks:
  main `./run/ww-chain2.sh ww-s2 100 12371 58` (log
  run/ww-chain2-s33.log), load `./run/ww-loadchain.sh wwload 100
  12381 5` (log run/ww-loadchain-s33.log). Both verified armed
  (WW-ARMED 19, GUARD matches) and storming within 3 min. Process trees
  in run/ww-s33-proctree.txt — kill the DRIVER ROOT
  (ww-chain2.sh / ww-loadchain.sh shell) first, then the round QEMU.
- Standing state for the successor (if both chains died with this
  window): same recipe, next round numbers from the chain logs' last
  "run N/100 starting" lines.

## Session 34 (continuation agent — chains claimed ALIVE, r58/load-r5 full-clean)

- Claim 03:35: s33's chains were STILL ALIVE (the handoff's "may have
  died with its window" was stale — the s33 zcode-cli ancestor 9865 was
  alive, wrappers not yet reaped; the s10 lesson held again: ps before
  trusting a dead-chain assumption). Rounds r58/load-r5 were 5 min in —
  claimed as-is, no restart. Both WW-ARMED 19 (GUARD=0xffffffff80a7a000
  = the frozen pair), wildwatch try 0, storming. Proctree in
  run/ww-s34-proctree.txt; foreign QEMU: perf-audit KVM 105707,
  untouched. Host in the >=3-QEMU contention class (2 chain TCG + 1
  foreign KVM) — strand readings inflated, capture markers exact.
- Watch cadence ~10 min (driver roots 104867/104897, WW-HIT in both gdb
  logs, markers in both serials, line-count heartbeat). The chain
  scripts' marker check runs between rounds; the manual polls cover
  mid-round.
- **MAIN r58 (FULL 2700 s): CLEAN** — 76639 serial lines, wakes 12.85M,
  ticks [100,100], avg_run 2ms, 994 forks, strand_max 9782ms (healthy
  class), 0 WW-HIT / 0 heals / 0 KERNPANIC/ALLOCTHROW, wildwatch armed
  try 0. Driver self-advanced to r59 (04:15:27).
- **LOAD r5 (FULL 2700 s): CLEAN** — 165628 lines, wakes 11.17M, 2642
  forks, **813 sdc exec cycles**, strand_max 40.1s (the documented
  >=3-QEMU contention class), 0 hits / 0 markers, armed try 0. Driver
  self-advanced to load-r6 (04:15:29).
- Cumulative: **0 WW-HIT across r1-r58 complete rounds** (plus partials),
  0 META-SMASH. Load surface: p1 + r1-r5 = **4265 sdc exec cycles over
  6 full armed load-hours**, zero fires, zero false positives.
- Chains continue at r59/load-r6 (qemu 116201/116256, armed; budget ends
  ~05:00:27/29).

### Session 34 continued — 04:30 sweep, relaunch, r59/load-r6 full-clean

- The s33 window-death sweep arrived 04:30:26: both trees (driver
  roots 104867/104897 + both round QEMUs) reaped mid-round at 15/45.
  Both partials clean: r59-first-attempt 48008 lines, load-r6-first
  77644 lines, 0 hits / 0 markers / armed to the last line. (Foreign
  perf-audit KVM also gone with the same sweep's window; a samgr
  foreign QEMU appeared after — untouched.)
- Relaunched 04:35:36 as THIS session's background tasks, main from r59
  (log run/ww-chain2-s34.log), load from r6 (run/ww-loadchain-s34.log);
  new driver roots 120674/120704, proctree updated in
  run/ww-s34-proctree.txt. Both verified armed (WW-ARMED 19, GUARD
  matches) and storming within 3 min.
- **MAIN r59 (FULL 2700 s, relaunched): CLEAN** — 76208 lines, wakes
  12.98M, 959 forks, avg_run 2ms, strand_max 4035ms (healthiest class),
  0 WW-HIT / 0 heals / 0 panics, wildwatch try 0. Driver self-advanced
  to r60 (05:20:36).
- **LOAD r6 (FULL 2700 s, relaunched): CLEAN** — 169172 lines, wakes
  10.85M, 2690 forks, **829 sdc exec cycles**, strand_max 37.8s
  (contention class), 0 hits / 0 markers, armed try 0. Driver
  self-advanced to load-r7 (05:20:39).
- Cumulative: **0 WW-HIT across r1-r59 complete rounds** (plus partials),
  0 META-SMASH. Load surface: p1 + r1-r6 = **5094 sdc exec cycles over
  7 full armed load-hours**, zero fires, zero false positives.
- Chains at r60/load-r7 (qemu 130997/131044, armed; budget ends ~06:05).

### Session 34 incident response — pa15 probe crash (coordinator relay): the WW IRQ-OFF-WINDOW walk must never touch a user rsp

Relay: the perf line's pa15 boot KERNPANIC'd at t=418s on cpu1 —
badaddr=0x7ff090411000 (USER page), rip symbolizing into
handle_timer_tick, right after a "WW: IRQ-OFF-WINDOW ... rip=0x7ff0007c7a69
rsp=0x7ff090410fc8" line (a USER frame). Confirmed MY lane's probe
(aa742f18, UART perf gate round 2):

- Bug: the probe walks 12 stack words at regs.rsp guarded ONLY by
  kernel_ptr_canonical() — and user addresses are canonical on x86_64.
  A recovery tick interrupting a user frame walked the USER stack; the
  8th word (rsp+0x38) crossed into the stack's first unmapped page →
  kernel #PF → KERNPANIC. The probe itself was the panic producer on a
  healthy boot (perf line's log: /home/william/rux-agents/perf-audit/
  run/pa-pa15-serial.log line 23293).
- Fix (branch, both arches compile clean): (1) the walk now requires
  !regs.user_mode() && rsp >= 0xffff_8000_0000_0000 && canonical —
  user frames print " user-frame" and skip the walk (they carry no
  kernel return addresses); (2) the same hole hardened in
  pt_regs_rip_for_cpu (x86_64 + riscv64 twins: high-half bound before
  deref of the possibly-torn CURRENT_PT_REGS slot). TIMER-SOFTIRQ-STALL
  (timer.rs) prints values only — no deref, untouched.
- Hunt impact assessment: the FROZEN kernel carries the unfixed probe,
  and the probe fires regularly on chain boots (39 of ~67 round logs,
  hundreds of firings — r56: 28, load-r3: 40) with ZERO crashes: the
  crash needs the user-frame recovery tick AND rsp within 0x38 of an
  unmapped user page. No mid-hunt refreeze; the fix rides the next
  scheduled one.
- **False-capture triage signature (for any chain KERNPANIC from here
  on)**: badaddr in the USER half + rip inside handle_timer_tick + a
  preceding WW: IRQ-OFF-WINDOW line with user rip/rsp on that cpu =
  the probe bug, NOT a wild write. A REAL RO-trap capture has badaddr
  inside the guard cluster (0xffffffff80a7a000-0xffffffff80a7bfff) and
  rip at the wild writer's instruction. Triage before declaring a
  capture; a probe-crash round is a clean-restart, not a milestone.
- Smoke validation: ww-pafix1 (330 s OH boot, worktree bzImage at
  HEAD+fix, dfx=wildwatch, port 12377): full budget, 45933 lines, 713
  forks, 0 panics/canary/heals — and 456 IRQ-OFF-WINDOW firings with
  ZERO crashes: 50 through the new user-frame skip (rsp values in the
  pa15-geometry user stack-top region), 408 kernel walks. Both guard
  branches exercised live.

### Session 34 second relay (pa16): WW age=1000ms artifact + small_alloc lock queue item

- pa16 relay (perf line, merged main 0634bc8f): (a) their EFLAGS-sampling
  probe measured max closed-cli 4 ms for a whole boot while WW printed
  359 age=1000ms lines; (b) mm/small_alloc.rs allocator cache lock =
  12.6% of the whole machine (732M sections/boot at ~250 ns; the single
  biggest IF=0 contributor) — fix direction: per-CPU freelist (my mm
  lane, queued below).
- **age artifact root cause: method, not arithmetic.** ww_tick_age_ms
  (timer.rs) measures the WALL-CLOCK gap between timer-IRQ arrivals
  (WW_TICK_LAST stamps at timer_interrupt_handler entry). A gap conflates
  three causes no in-guest clock can separate: real IF=0 windows, host
  vCPU steal (the CPU not scheduled — dominant under multi-QEMU host
  contention; both pa16-KVM and my TCG smokes show it), and
  tickless-style idle. Fix (2e41b614, trap.rs probe — the timer.rs side
  is untouched territory): when the recovery tick lands on the IDLE
  task the gap was idle/steal by construction — print " idle-wake" and
  skip the culprit walk. Real-task frames keep the walk as a TRIAGE
  HINT only: corroborate with an EFLAGS sampler (perf line's probe)
  before reading age as IF=0. Both arches compile clean.
- **Hunt-probe reading discipline going forward**: WW IRQ-OFF-WINDOW
  lines under >=3-QEMU host contention are contention noise unless
  corroborated; the wakewatch TIMER-SOFTIRQ-STALL report has the same
  caveat for its tick_age field.

### Session 34 chain ledger (post-relaunch 2)

- r60 (FULL, relaunched): CLEAN — 97675 lines, wakes 12.43M, 1109
  forks, strand_max 39.3s (contention class), 0 WW-HIT / 0 heals /
  0 panics, armed try 0. Driver self-advanced to r61 (06:29:03).
- load-r7 (FULL, relaunched): CLEAN — 167811 lines, wakes 9.46M, 2587
  forks, **871 sdc exec cycles**, strand_max 8.7s (healthy class),
  0 markers, armed try 0. Driver self-advanced to load-r8 (06:29:06).
- Cumulative: **0 WW-HIT across r1-r60 complete rounds** (plus partials),
  0 META-SMASH. Load surface: p1 + r1-r7 = **5965 sdc exec cycles over
  8 full armed load-hours**, zero fires, zero false positives.

## Session 35 (continuation agent — s34 quota-death successor: allocator
## magazine implemented + validated; chains at r63/load-r10)

### Inheritance reconstruction (claim 08:34)

- s34 died at ~08:33 mid-analysis (snapshot 2b932302). Artifact verdicts:
  r61 FULL-clean (110841 lines), load-r8 FULL-clean (165897 lines);
  r62/load-r9 were quota-death partials at 15/45, both clean (0 markers,
  armed to the last line). Both chains DEAD at claim (no QEMUs); host
  load 0.34, capacity fine.
- Relaunched 08:35 as this session's tasks: main r62 (log
  run/ww-chain2-s35.log), load r9 (run/ww-loadchain-s35.log); both
  verified armed (WW-ARMED 19, GUARD=0xffffffff80a7a000, wildwatch
  try 0). Proctree: run/ww-s35-proctree.txt.

### Allocator per-CPU magazine (pa16 12.6% item) — implemented, s34
### snapshot rewritten after audit found five defects

- Audit of the in-flight snapshot (2b932302) — it compiled but was NOT
  correct: (1) cpu_id() read outside the irq-off windows (a preempted
  task can migrate between read and row access -> two CPUs on one row);
  (2) the flush loop held a &mut to a magazine row ACROSS irq-off
  windows (same migration hazard); (3) flush took the class lock once
  PER ENTRY — as lock-heavy as pre-magazine, contradicting its own
  comment; (4) obj_allocs/obj_frees double counting (refill+pop,
  push+flush); (5) WORST: the fast push skipped the locked validation
  entirely, so a misaligned/foreign pointer could park in a magazine
  and be handed out by the next alloc pop on that CPU — defeating the
  consume-and-taint hardening this hunt line's allocator contract
  depends on.
- Final design (cb51db03): fast pop/push under LOCAL irq-off with
  cpu_id() read INSIDE every window; row-full flush snapshots the whole
  row out under ONE window then batch-frees under ONE class-lock hold
  (reclaimed pages return to the buddy after the unlock, as before);
  push-time screening (fresh magic/cls read, alignment, free_count
  head room inside the window) routes anything doubtful to the locked
  body, so a bad pointer never parks; counters bump only in the locked
  bodies. Deferred-free reclaim safety is by construction (a magazine
  entry holds its page's slot un-accounted, so free_count can never
  reach objs_per_page while the entry exists; holds inside batches
  too). MAG_DEPTH=16, 4.3 KB of .bss.
- Follow-up (this session, second commit): lock_holds counter per
  class + census_line extended with locks=, and the periodic taskdump
  (dfx=taskdump) now carries the SMALLALLOC census — locks vs
  obj_allocs+obj_frees on the same line is the live traffic-ratio
  readout (pre-magazine, every op took the lock).

### Validation ladder (all green)

- Unit tests: kernel/src/tests/small_alloc.rs, 7 cases (per-class
  round-trip, 80/class churn across flush+refill boundaries, hot loop
  x200, misaligned free consumed+tainted never circulated, 600-object
  span with strided partial free + realloc, double-free smoke last on
  purpose, census). riscv64 SMP4 unit boot: 7/7 PASS; 1093/1096 total —
  the 3 fails are pre-existing config expectations (KERNEL_HEAP_SIZE
  512MB vs the riscv64 128MiB clamp, FUTEX_WAITER_POOL_SIZE,
  FUTEX_HASH_SIZE), untouched by this change.
- Both arches compile clean (make build, make build-x86).
- OH x86_64 soak, full 2700 s (run/ww-magval.sh, private serial
  run/magval1-serial.log, dfx=wildwatch, -smp 2, same images/geometry
  as the chain boots, worktree bzImage 08:51 = magazine without the
  counter): **80458 lines, 0 KERNPANIC, 0 ALLOCTHROW, 0 heals/canary,
  1454 forks, wildwatch armed to the end** — the 45-min 0-panic gate
  PASSES. (WAKE-STRANDED 1001ms as last line = the documented
  >=3-QEMU contention class; not a capture marker.)
- A/B lock-traffic boot (dfx=wildwatch,taskdump, 900 s): see the
  verdict block appended below.

### Chain ledger (s35 ownership)

- MAIN r62 (FULL 2700 s): CLEAN — 77561 lines, 0 WW-HIT / 0 heals, no
  panics. Advanced to r63 (09:20:09).
- LOAD r9 (FULL 2700 s): CLEAN — 165285 lines, 769 sdc cycles, storm
  healthy, 0 markers. Advanced to load-r10 (09:20:15).
- Cumulative: **0 WW-HIT across r1-r62 complete rounds** (plus
  partials), 0 META-SMASH. Load surface: p1 + r1-r9 = **~6734 sdc
  exec cycles over 10 full armed load-hours**, zero fires, zero false
  positives.
- 09:40 sweep: the session's background-task WRAPPERS were reaped (the
  known periodic reaper class), but the DRIVER ROOTS survived
  (176512 ww-chain2.sh / 176547 ww-loadchain.sh, reparented): r63 and
  load-r10 kept storming through it — only the task handles died.
  Watching continues via the serial logs; if a later sweep takes the
  roots, relaunch recipe: ./run/ww-chain2.sh ww-s2 100 12371
  <next-main-round> / ./run/ww-loadchain.sh wwload 100 12381
  <next-load-round>.
- Ops: my leftover riscv64 unit-test QEMU (188132, orphaned when the
  run.sh timeout wrapper died before the child) was killed by exact
  PID (own process, verified cmdline). Foreign samgr-callback QEMU
  untouched.

### A/B lock-traffic verdict (magval-ab, 900 s, dfx=wildwatch,periodic)

- First attempt used the WRONG flag (dfx=...,taskdump = the keyboard
  dump key; the periodic dump switch is `periodic` — 3000 jiffies/30 s
  per timer.rs sc115). Killed by exact PID and relaunched with
  dfx=wildwatch,periodic. Boot itself clean both times (0 panics).
- Final cumulative census at 900 s (29 blocks): total obj ops
  59,391,505 vs class-lock acquisitions 3,601,522 = **16.49x** — per
  class 16.5-16.6x across all eight, i.e. the MAG_DEPTH=16 (+1 first
  object) amortization realized almost exactly. Pre-magazine every op
  took the lock, so a pa16-scale boot (732M sections) now takes
  ~44M class-lock irqsave sections — the 12.6% item's lock traffic is
  down ~94% (the per-op local irq-off windows that replace it are
  O(tens of ns) and uncontended; the perf line's EFLAGS sampler can
  confirm the wall-clock share independently).
- The 900 s A/B boot doubles as a mini-soak of the final committed
  image (counter + census hook): 75884 lines, 0 KERNPANIC, 0
  ALLOCTHROW, storm healthy.

### Session 35 close-out state

- Chains at r64/load-r11 (started 10:05, driver roots 176512/176547,
  reparented but alive; budget ends ~10:50). Watch via serial logs;
  relaunch recipe in the ledger above if a sweep takes the roots.
- Commits: cb51db03 (magazine rewrite + tests, pushed) + this
  close-out (lock counter, census hook, A/B scripts, doc).
- For the perf line (pa16): the magazine is on feature/wildwrite-hunt,
  validated per the ladder above; merge-window ready.

### Session 35 final ledger

- MAIN r63 (FULL): CLEAN — 69205 lines, 0 WW-HIT / 0 heals / 0 panics.
- LOAD r10 (FULL): CLEAN — 164395 lines, 740 sdc cycles, 0 markers.
- MAIN r64 (FULL): CLEAN — 0 WW-HIT / 0 heals / 0 panics, armed try 0.
- LOAD r11 (FULL): CLEAN — 164821 lines, 793 sdc cycles, 0 markers.
- Cumulative: **0 WW-HIT across r1-r64 complete rounds** (plus
  partials), 0 META-SMASH. Load surface: p1 + r1-r11 = **~8267 sdc
  exec cycles over 12 full armed load-hours**, zero fires, zero false
  positives.
- Standing state for the successor: chains at r65/load-r12 (started
  10:50:10/10:50:21, budget ends ~11:35), driver roots 176512/176547
  (reparented orphans — check liveness FIRST with ps before assuming
  death; the s10/s34 lesson). If dead, relaunch:
    ./run/ww-chain2.sh  ww-s2  100 12371 <next-main-round>
    ./run/ww-loadchain.sh wwload 100 12381 <next-load-round>
  and record new driver roots in run/ww-s35-proctree.txt's successor.
  False-capture triage signature unchanged (badaddr user half + rip in
  handle_timer_tick after a user-frame WW IRQ-OFF-WINDOW line = probe
  bug; real capture = badaddr inside the guard cluster
  0xffffffff80a7a000-0xffffffff80a7bfff with rip at the writer).
- The allocator magazine (cb51db03 + 64156dcb) is merge-window ready;
  the perf line's EFLAGS sampler can independently confirm the
  wall-clock share now that the lock traffic is down ~94%.

## Session 36 (continuation agent — pure watch session: chains claimed
## alive at r65/load-r12, main merged)

### Inheritance reconstruction (claim 11:04 Oct 24)

- The handoff's "driver roots 176512/176547 may have survived" was
  CORRECT for once — both chains ALIVE at claim (ps before assumption,
  the s10/s34 lesson): driver roots 176512 (main, r65 in flight since
  10:50:10) / 176547 (load, load-r12 since 10:50:21), rounds WW-ARMED 19
  (GUARD=0xffffffff80a7a000, frozen pair md5-verified bf779126...),
  wildwatch try 0, storming mid-round with 0 markers.
- Merged origin/main 988e10c1 (fast-forward — s35's 39dc92b2 was already
  its parent; branch == main now). Main brought the perf-audit line's
  dfx/syscost.rs (+2041), trap.rs probe extensions, and mm/oom_kill /
  arch page_fault additions; no conflicts, no action needed. The frozen
  hunt pair is by design untouched by any of this.
- Foreign QEMUs at claim: samgr-callback (212223), perf-audit (214304),
  main-workspace (214924+) — untouched. Host in the >=3-QEMU heavy
  contention class (load 8.15): strand readings inflated, capture
  markers exact. No experiment QEMU launched this session on purpose.

### Chain ledger (s36 ownership)

- MAIN r65 (FULL 2700 s): CLEAN — 78752 lines, 0 WW-HIT / 0 heals, no
  panics, armed try 0. Driver self-advanced to r66 (11:35:10).
- LOAD r12 (FULL 2700 s): CLEAN — 167747 lines, 782 sdc exec cycles,
  0 gdb hits / 0 capture markers, armed try 0. Driver self-advanced to
  load-r13 (11:35:24).
- Cumulative: **0 WW-HIT across r1-r65 complete rounds** (plus
  partials), 0 META-SMASH. Load surface: p1 + r1-r12 = **~9049 sdc
  exec cycles over 13 full armed load-hours**, zero fires, zero false
  positives.
- r66 / load-r13 in flight (started 11:35:10/11:35:24, budget ends
  ~12:20; qemu 222524 / 222584, gdb 222608 / 222655, both armed).
- Commits: 2617b3d2 (s36 claim record), f1f1ffec (r65/load-r12 round
  artifacts).

## Session 37 (continuation agent — pure watch session: chains adopted
## alive mid-round at r66/load-r13)

### Inheritance reconstruction (claim 11:52 Oct 24)

- Both chains ALIVE at claim (ps before assumption, the s10/s34 lesson):
  driver roots 176512 (main, ww-chain2.sh — r66 in flight since 11:35:10)
  / 176547 (load, ww-loadchain.sh — load-r13 since 11:35:24), round
  QEMUs 222524/222534 and 222584/222595, gdb 222608/222655 on
  12371/12381. Rounds WW-ARMED 19 points (GUARD=0xffffffff80a7a000),
  wildwatch try 0. The s36 handoff PIDs matched exactly — first clean
  PID handoff in a while.
- Mid-round sweep at claim (11:52): r66 56158 lines storming
  (WW: wakes=4.15M), load-r13 87163 lines with normal execve/cow
  traffic. 0 WW-HIT / CANARY / META-SMASH / KERNPANIC in both.
- Main repo drift check: b8f686a6 vs the merged base 988e10c1 — zero
  diff under kernel/ (the new commits are docs + a chore dropping the
  perf-audit line's two 11MB probe bzImages). The frozen hunt pair is
  unaffected; chains NOT rebuilt, per the no-rebuild rule for
  docs-only main movement.
- Foreign QEMUs at claim: samgr-callback (225991, KVM). Untouched.

### Chain ledger (s37 ownership)

- MAIN r66 (FULL 2700 s): CLEAN — 76772 lines, 0 WW-HIT / 0 heals / 0
  watchpoint hits, no panics, armed try 0. Driver self-advanced to r67
  (12:20:10).
- LOAD r13 (FULL 2700 s): CLEAN — 164915 lines, 877 sdc exec cycles,
  storm healthy (WW: wakes=9.49M, one strand_max 3949ms — host
  contention class, no markers), 0 gdb hits / 0 capture markers, armed
  try 0. Driver self-advanced to load-r14 (12:20:27).
- Cumulative: **0 WW-HIT across r1-r66 complete rounds** (plus
  partials), 0 META-SMASH. Load surface: p1 + r1-r13 = **~9926 sdc
  exec cycles over 14 full armed load-hours**, zero fires, zero false
  positives.
- r67 / load-r14 in flight (started 12:20:10/12:20:27, budget ends
  ~13:05; qemu 235499 / 235605, gdb 235637 / 235727, both WW-ARMED 19
  with GUARD=0xffffffff80a7a000, confirmed via gdb logs).
- Commits: 0d200333 (s37 claim record + r66/load-r13 gdb cmds).

## Session 38 (continuation agent — pure watch session: chains adopted
## alive mid-round at r68/load-r15 after the ~12:33 quota death)

### Inheritance reconstruction (claim 13:35 Oct 24)

- Both chains ALIVE at claim (ps before assumption, the s10/s34 lesson):
  driver roots 176512 (main, ww-chain2.sh — r68 in flight since
  13:05:11) / 176547 (load, ww-loadchain.sh — load-r15 since 13:05:30),
  the same PIDs the s36/s37 handoffs carried — third consecutive clean
  PID handoff. Round QEMUs 242828/242838 and 242918/242929, gdb 242936 /
  242986 on 12371/12381. Rounds WW-ARMED 19 (GUARD=0xffffffff80a7a000,
  confirmed in both gdb logs), wildwatch try 0, storming mid-round with
  0 markers.
- The ~12:33 agent quota death did NOT take the drivers (detached by
  design): across the 08:35-launched stint the chains ran autonomously,
  main r62→r68 and load r9→r15, every intervening round full-budget
  clean per the driver verdict blocks (chain2-s35 / loadchain-s35 logs).
- Main repo drift check: 78460742 vs merged base 988e10c1 — ZERO commits
  under kernel/ (docs + chore dropping two 11MB probe bzImages; the
  perf-audit leg-13 merge adjudicates the IRQ-OFF age=1000ms family as
  x86_64 timer 1Hz cadence). Frozen hunt pair unaffected; chains NOT
  rebuilt, per the no-rebuild rule for docs-only main movement.
- Handoff context (accepted, recorded): the age=1000ms IRQ-OFF family is
  the TSC timekeeper 1s quantization bug, fix landing on the A-line
  branch. This session's rounds show ZERO age=1000ms lines (r67: 1,
  load-r14: 2, r68: 0, load-r15: 1 IRQ-OFF-WINDOW lines, all the benign
  short class age 108-139 ms, same rip ffffffff80573d96 as always) —
  consistent with cadence/jitter, not long cli, and no behavior change
  expected on the frozen kernel. Real capture surfaces remain WW-HIT /
  WILDWATCH-CANARY / META-SMASH / KERNPANIC / protection-cluster badaddr.
- Foreign QEMUs at claim: none besides the two chain pairs (host load
  lighter than s36/s37 claims).

### Chain ledger (s38 ownership)

- MAIN r67 (FULL 2700 s): CLEAN — 87395 lines, "budget spent" natural
  end, 0 WW-HIT / 0 watchpoint hits / 0 heals, no panics, armed try 0.
  Driver self-advanced to r68 (13:05:11).
- LOAD r14 (FULL 2700 s): CLEAN — 169300 lines, 834 sdc exec cycles,
  0 gdb hits / 0 capture markers, armed try 0. Driver self-advanced to
  load-r15 (13:05:30).
- MAIN r68 (FULL 2700 s): CLEAN — 76378 lines, "budget spent" natural
  end, 0 WW-HIT / 0 watchpoint hits / 0 heals, no panics, armed try 0;
  6 IRQ-OFF-WINDOW lines, all benign short class (age 103-241 ms, same
  rip ffffffff80573d96). Driver self-advanced to r69 (13:50:11).
- LOAD r15 (FULL 2700 s): CLEAN — 170424 lines, 848 sdc exec cycles,
  0 gdb hits / 0 capture markers, armed try 0; 10 IRQ-OFF-WINDOW lines,
  all benign short class (age 108-546 ms). Driver self-advanced to
  load-r16 (13:50:32).
- MAIN r69 (FULL 2700 s): CLEAN — 81206 lines, "budget spent" natural
  end, 0 WW-HIT / 0 watchpoint hits / 0 heals, no panics, armed try 0;
  3 IRQ-OFF-WINDOW lines, benign short class (age 103-460 ms). Driver
  self-advanced to r70 (14:35:11).
- LOAD r16 (FULL 2700 s): CLEAN — 167297 lines, 781 sdc exec cycles,
  0 gdb hits / 0 capture markers, armed try 0. Driver self-advanced to
  load-r17 (14:35:35).
- Cumulative: **0 WW-HIT across r1-r69 complete rounds** (plus
  partials), 0 META-SMASH. Load surface: p1 + r1-r16 = **~12389 sdc
  exec cycles over 17 full armed load-hours**, zero fires, zero false
  positives.
- r70 / load-r17 in flight (started 14:35:11/14:35:35, budget ends
  ~15:20; qemu 258402 / 258532, gdb 258477 / 258600, both WW-ARMED 19
  with GUARD=0xffffffff80a7a000, wildwatch try 0, confirmed via gdb
  logs).
- Commits: d79636ea (claim record + r68/load-r15 gdb cmds), a869968d
  (r67-r68/load-r14-r15 ledger + r69/load-r16 gdb cmds).
