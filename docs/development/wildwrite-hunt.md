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
- ww-r4 (2700 s, watchpoints, GUARD-relocated build): see below.
