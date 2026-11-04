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
| schedtrace, schb, periodic, taskdump, parkdump, sleepsnap | sched:sched_switch / sched_waking / sched_wakeup / sched_wakeup_new / sched_process_fork / sched_process_exit / sched_process_wait / sched_process_free / sched_migrate_task (all present; waking/wakeup SITES added round 2 — round 1 armed the events but nothing recorded them) | **ported (round 2, alias)** |
| syscalltrace, sysforensic, sysring | raw_syscalls:sys_enter / sys_exit (present; sysring forensic ring is subsumed long-term per charter) | **ported (round 2, alias)** |
| sigtrace | signal:signal_generate (present) | **ported (round 2, alias)** |
| bindertrace, dbtrace, txnhunt | binder:binder_transaction / _received / _alloc_buf / _buffer_release / wait_for_work (present) | **ported (round 2, alias)** |
| memwatch | oom:mark_victim (present) + mm_page_alloc/free (landed round 2) | **ported (round 2, alias)** |
| allocwatch, leakhunt, mmforensics, vmap, hole, poolcensus, monowatch | kmem:kmalloc/kfree (reconciled from feature/dfx-align 486cb916, ids 27/28) + kmem:mm_page_alloc/free (new, ids 29/30, from the reference kmem.h with the live-format substitution rules) | **ported (round 2, alias)** |
| syncguard, syncwatch, thpark, futextrace | futextrace/fuxstuck/thpark ported round 2 as reference-shaped visibility: the reference kernel carries NO futex trace events (verified against master — no futex.h in include/trace/events/), so their aliases arm raw_syscalls + the sched wake pair — the reference's own futex visibility. syncguard/syncwatch wait on lock:lock_acquire/lock_release (a later round) | partial (round 2) |
| sfddbg, exthunt, filerd | ext4:ext4_sync_file_enter/exit, ext4_ext_map_blocks_enter/exit, ext4_da_write_begin/end, ext4_free_blocks (ids 38-44) + jbd2:jbd2_start_commit/end_commit (45/46); the round-3 alias arm | **ported (round 3, alias)** |
| epollscan, epolltrace | no reference analog; rux:epoll_ctl_insert/remove, rux:epoll_wait_scan/exit, rux:epoll_delivery (ids 47-51) — the documented-divergence rux: group | **ported (round 3, alias)** |
| tcptrace, socksnap | reference net event family (tcp:*) | round 2+ |
| inputtrace | reference input event family | round 2+ |
| writetrace | exceptions:page_fault_kernel (present) + block:block_rq_issue/complete (present) | **ported (round 2, alias)** |
| execcost, ohhunt, treehunt, driftwatch | sched:* + raw_syscalls:* timelines answer the same questions (present) | **ported (round 2, alias; driftwatch left un-aliased — debug tap row below wins)** |
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

## Round 2 verification record (2026-11-03, x86_64+KVM + riscv64 TCG, this host)

- **Disabled cost**: VERIFIED IN RELEASE ASM for every round-2 site —
  `movzbl <event>.enabled` + `test` + branch: sched_waking/sched_wakeup
  in wake_up_enqueue_opt, mm_page_alloc in alloc_pages (4 inlined
  copies across the wrapper arms), mm_page_free in free_pages (2
  copies), and the kmem pair's armed checks inlined at every
  GlobalAlloc site (drop glues, BTreeMap/Arc paths — each carries the
  same one-load shape).
- **Enabled overhead** (guest differentials, KVM, min-of-3, events-off
  baseline; run/tr2-kvm-proof-serial.log):
  - raw_syscalls armed: +36 ns/read over 4000x8K zero-reads (round-1
    class: 37).
  - sched family armed: -149 ns per 150-fork loop (noise; no
    measurable cost).
  - kmem family armed (all four events, frame-walk origins): +11.8 µs
    per fork (~120-240 ns per kmalloc+kfree record pair — the
    alloc_call_site walk dominates; events stay opt-in).
- **Image delta** (x86_64 release bzImage, both sides built with the
  debug-then-release sequence so ksyms embed): origin/main d2b2672a =
  13,545,472; this branch = 13,066,232 → **-479,240 bytes**. The
  zero-init ring fix (813b040c) moved BOTH ring sets (live + snapshot
  spare) from .data to .bss (-1 MiB stored); the round-2 events add
  ~+33K (.rodata print-fmt blobs, gfp table, origin walk).
- **Serial proof** (run/tr2-kvm-proof.sh + run/tr2-proof-init.c on
  run/tr2-rootfs.img): P0 boot state empty/0; snapshot file present;
  kmem group set_event arming; KMEMDBG alloc=117 free=94 page_alloc=53
  page_free=87 origin=180 (symbolized call_sites); WAKEDBG waking=8
  wakeup=8 wakeup_new=6 (both round-2 sites record, waking >= wakeup —
  the refused-wake census discrimination); SNAPDBG saved=131 lines,
  live trace restarted to header-only after write-1, after_clear=0.
- **Alias proof** (dfx=schedtrace boots): tracing_on=1 and the whole
  sched:* family pre-armed at boot (set_event lists all nine). Boot B1
  crashed (init SIGSEGV at first exec, rip in early libc text, NULL
  read) WHILE a 4-core TCG make-test ran; B2 and B3 clean in isolation
  (full flow to PROOF-INIT-DONE) — adjudicated a load flake per the
  repo's known-flake rule; forensics in
  run/tr2-kvm-proof-alias-serial.log.
- **kmem smoke** (test/trace-kmem-smoke.sh, riscv64 TCG): 20/20 —
  formats, group 1/0 switch, live lines in the reference geometry
  (gfp_flags renders GFP_KERNEL/GFP_USER per the mask actually passed
  — Rux GfpFlags translated onto the reference values at the site),
  symbolized origins (origin_count=48). NOTE: the smoke kernel must be
  built with `make build` (gen-ksyms embeds the per-arch table; a bare
  cargo build after an other-arch build leaves stale symbols → raw-hex
  call_sites).
- **x3 builds** rc=0 (riscv64 make build, x86_64 make build, aarch64
  make build — dtb_blob.S restored after the aarch64 leg); make test
  **1278/1278 green** (count moved with origin/main's 26 commits; round
  1's 1297 predates them).
- **Census A/B (events off)**: run/tr2-census.sh — same driver
  (q35 KVM smp2/4G, syscost=1 dfx=wwcensus, 480 s, shared rootfs) on
  this branch vs origin/main debug bzImages; census classes compared
  side by side (the pa-boot 780 s tok109 vehicle stays the merge
  gate's). RESULT: IDENTICAL — TERM-DESCHED 8 vs 8 (the pa-band center
  even on this driver), WAKE-REFUSED 0 vs 0, WAKE-STRANDED 0 vs 0,
  panics/OOM 0 vs 0, the same idle-task PREEMPT-LEAK print class on
  both (the wwcensus vehicle's own artifact; per-build rip). No
  sched/mm guarded-domain movement with events off
  (run/tr2-census-{a,b}.log).
- Known divergences added this round (all noted, none silent):
  - mm_page_alloc/mm_page_free print-fmt text constructed from the
    reference kmem.h TP_printk with the same substitutions the two
    verbatim round-1 captures show (REC-> fields, show_gfp_flags →
    the __print_flags blob, pfn_to_page kept as a call); gfp_flags
    carries the REFERENCE mask translated from Rux's private GfpFlags;
    `page=` renders the real struct-page address (no %p hashing).
  - mm_page_free fires for every order at Rux's single free funnel;
    the reference's order-0 PCP frees carry mm_page_free_batched (not
    in this table — the ledger's named pair only).
  - kmalloc/kfree gfp flags are the honest analog (GFP_KERNEL 0xcc0,
    node -1, accounted false) — Rux's allocator takes no gfp mask.
  - The snapshot save is a copy-out + live restart, not the
    reference's O(1) buffer-pointer swap (statically allocated rings);
    records racing the save can drop out of the saved copy (same
    spool-and-reset trade the pipe reader documents).

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

### 2026-11-03, round 2 (this session) — the ported wave, the kmem reconciliation, snapshot
- Resumed off a quota-killed predecessor: the WIP (trace_alias table +
  sched_waking/wakeup sites) was completed rather than redone — the
  sites now use the generated typed calls; the missing semicolon and
  the kmem-event references that didn't exist yet were the exact
  interruption points.
- Synced origin/main d2b2672a (26 commits: vm-op fast paths, negmemo,
  mmprof) — merge clean; commits 5e673a97, 160bd784, 813b040c,
  b3766e3e on top.
- The kmem wave reconciled from feature/dfx-align 486cb916 (that branch
  stays parked/entangled; only its event series moved): kmalloc/kfree
  ids renumbered 27/28 (dfx-align predated round-1's irq/migrate/
  console 23-26), gfp_flags_str's shortcut table replaced by the exact
  reference __print_flags walk. mm_page_alloc/free (29/30) NEW, from
  the reference kmem.h (fetched from master; no futex.h exists there —
  the futex-family alias is syscalls + the wake pair, documented).
- tracefs gaps: buffer_size_kb + per_cpu stats were round-1 work;
  snapshot landed this round (spare ring set, copy-out save semantics).
- Perf surprise found and fixed: TraceRing's '.' flag init kept BOTH
  ring sets in .data as stored image bytes; zero-init moved them to
  .bss (-1 MiB image) — 813b040c. Net image delta vs origin/main:
  -479,240 bytes.
- TASK.md note for the coordinator: origin/main's tracked TASK.md now
  carries the stress-port lane's charter (same path, both lanes write
  it); this file is restored to the dfx-trace charter + records —
  reconcile at merge time.
- Round-2 verification battery above; all green. Census A/B identical.
- Artifacts: run/tr2-proof-init.c, run/tr2-kvm-proof.sh (+ -serial/
  -alias-serial logs), run/tr2-census.sh (+ -a/-b logs), run/
  tr2-rootfs.img (mkfs without metadata_csum_seed/orphan_file),
  run/tr2-bzImage-mine (the exact release kernel measured),
  run/trace-kmem-smoke-run2.log (20/20).

## Round 3 verification record (2026-11-04, x86_64+KVM + riscv64 TCG, this host)

Branch dfx-trace3 (fresh off origin/main 53fba4f6 after the old
dfx-trace worktree was deleted on the host; the TASK.md charter +
run/ artifacts were restored from the lane's saved copy — nothing
uncommitted was lost).

- **The wave** (14 events, ids 38-51, all in the tracepoints! table):
  - ext4 (reference include/trace/events/ext4.h shapes; field rows
    and print fmts verbatim with the MAJOR/MINOR and show_* expansions,
    flag values as TRACE_DEFINE_ENUM resolves them): ext4_sync_file_
    enter/exit (38/39, at ext4_sync_file's bracket; datasync threaded
    from the fsync/fdatasync VFS entry via do_fsync), ext4_ext_map_
    blocks_enter/exit (40/41, at the extent-mapping funnel
    ext4_ext_get_block_ex with ino threaded through both callers),
    ext4_da_write_begin/end (42/43, inside the delayed-allocation
    buffered write, fired after O_APPEND offset resolution), ext4_
    free_blocks (44, at the namei free funnel — 6 guarded sites via
    one helper, ino/mode threaded).
  - jbd2 (reference jbd2.h shapes): jbd2_start_commit/end_commit
    (45/46) at Rux's single commit funnel (the jbd2_journal_stop
    commit call — every commit passes there).
  - rux (documented divergence — the reference has NO epoll trace
    events): epoll_ctl_insert/remove (47/48), epoll_wait_scan (49),
    epoll_wait_exit (50, at a wrapper over the renamed inner body),
    epoll_delivery (51, ops rendered %pS through the kernel symbol
    table).
  - Shared renderers: print_flags_walk (the reference's
    fs/trace_output.c flags walk, consuming matched masks, hex
    leftover) + the map/mflags/free flag tables; gfp_flags_text kept
    its round-2 shape.
- **Alias restoration (found broken)**: origin/main carried round 2's
  trace_alias TABLE but the merge DROPPED its consumer — the boot-time
  arming loop in DfxSwitch::set. dfx=exthunt booted with tracing_on=0
  and nothing armed. Restored verbatim (commit 8baccd72); verified on
  both new arms.
- **x3 builds** rc=0 (riscv64 make build, x86_64 make build,
  aarch64 make build — dtb_blob.S restored after the aarch64 leg);
  **make test green** on riscv64 (ALL TESTS PASSED verdict, wrapper
  rc=0; the success path deletes the wrapper log, and the numeric
  count was not captured this round — the count moved with main's 26+
  commits since round 2's 1278 and is not comparable anyway).
- **Disabled cost**: VERIFIED IN RELEASE ASM on the final binary
  (run/tr3-release-asm.txt) — every kernel call site compiles to
  `movzbl <event>.enabled(+0x4a)` + `test` + `jcc`, one relaxed load
  on the disabled path (an occasional instruction interleaved between
  load and test; three sites double-load only on the ARMED path — the
  inlined trace_ call's own check). First-load addresses:
  sync_file_enter/exit ffffffff804cc477/804cc504 (do_fsync — the
  wrapper inlines), map_enter/exit ffffffff805359b0/80535a51
  (ext4_ext_get_block_ex), da_write_begin ffffffff80521d71 (+2 dup
  regions), da_write_end ffffffff80522502/8052267e (ext4_buffered_
  write), free_blocks ffffffff80530f0b/80531124 (free_inode_blocks) +
  ffffffff80531812/805318a2 (free_indirect_block), jbd2_start/end
  ffffffff80536cb8/80536cc3 (ext4_journal_stop — jbd2_journal_stop
  inlines), epoll_ctl_insert/remove ffffffff804d1398/804d10d1
  (sys_epoll_ctl), epoll_wait_scan ffffffff804d2927, wait_exit
  ffffffff804d211b, delivery ffffffff804d25f5 (sys_epoll_wait).
- **Enabled overhead** (guest differentials, KVM, min-of-3, events-off
  baseline; run/tr3-kvm-proof-serial.log):
  - pure buffered pwrite loop (da pair, 2 records/op): +26..38 ns/op
    → **~13-19 ns per record**.
  - epoll ping-pong (scan+exit+delivery, 3 records/op): +84..106
    ns/op → **~28-35 ns per record** (the round-1 37 ns/read class).
  - fsync loop (8 records/op): the armed rep's real 205-224 µs/op sits
    INSIDE the unarmed run-to-run spread of this rig (195-226 µs/op
    across the four runs, host loaded with other lanes' QEMUs) — the
    delta is below rig noise; bounded by the two clean differentials
    above. See the found-issue note for why the fsync A/B is noisy.
- **FOUND ISSUE (preexisting on origin/main, NOT round-3 causal — for
  the fs lane)**: a create+fsync-loop+unlink workload eventually puts
  the tr2-rootfs image class into a state where open(O_CREAT) returns
  EIO and fsync becomes a fast no-op; the number of fsyncs before it
  varies run to run (160-600+; timing-dependent). The BASE kernel
  (origin/main 53fba4f6, run/tr3-base-check.log) fails IDENTICALLY —
  5x EIO at the same bench positions. Suspect: the journal write-
  through commit path's log-space recycling under forced commits
  (jbd2 tid stays 1; commits keep advancing the head). Repro: the P6
  bench in run/tr3-proof-init.c on run/tr3-rootfs.img; evidence in
  run/tr3-kvm-proof-serial.log + run/tr3-base-check.log.
- **Image delta** (x86_64 release bzImage, both sides built with the
  debug-then-release sequence so ksyms embed): base 53fba4f6 =
  13,484,032; this branch = 13,508,608 → **+24,576 bytes (+0.182%)**
  for 14 events + sites + the alias consumer. Against CURRENT
  origin/main (602337ff, 115 commits of other lanes' work landed
  mid-round): 14,303,232 → -794,624 (that delta belongs to the other
  lanes, not round 3; the merge-gate A/B should rebase first).
- **Serial proof** (run/tr3-kvm-proof.sh + run/tr3-proof-init.c on
  run/tr3-rootfs.img, final kernel): P0 AVAIL14 14/14 in
  available_events; P1 formats + "ext4" group arming + per-event
  enable (free_blocks alone on, sync_file_enter stays off); P2
  EXTDBG sync_enter/exit=1 map_enter/exit=6 da_begin/end=2
  free_blocks=1 jbd2_start/end=1 with live lines in the reference
  geometry (dev 8,0; mflags M; pblk; the trailing-space datasync
  printk); P3 EPDBG insert/remove=1 scan/exit=2 delivery=1; P4
  trace_pipe drained 3252 bytes, live begin-left=0; P5 SNAPDBG
  before=3 saved=3, live cleared after save, saved cleared after 0.
  Free_blocks fires at the DEFERRED reap (POSIX last-close rule) —
  the proof drives it with a follow-up create; a plain `sync(2)` does
  NOT hit ext4_sync_file (per-fd fsync does: dd conv=fsync).
- **Alias proof**: dfx=exthunt boots with set_event listing the nine
  ext4:*/jbd2:* events, ext4/enable=1 jbd2/enable=1 rux/enable=0,
  tracing_on=1 (run/tr3-kvm-proof-alias-serial.log); dfx=epolltrace
  boots with the five rux:epoll events armed and the legacy EPTRC
  prints still flowing alongside (…-alias2-serial.log).
- **riscv64 smoke** (test/trace-ext4-smoke.sh): 28/28 — formats
  (field rows, the three flag blobs, %pS ops print), live fs lines in
  the reference geometry (free_blocks mode 0100644, jbd2 head), group
  on/off, rux arming (no epoll user on the minimal rootfs — the KVM
  proof init carries the live epoll workload).
- **Census A/B (events off)**: run/tr3-census.sh — same driver as
  round 2 (q35 KVM smp2/4G, syscost=1 dfx=wwcensus, 480 s, shared
  rootfs) on this branch vs base 53fba4f6 debug bzImages. RESULT:
  classes identical — TERM-DESCHED 8 vs 8, WAKE-REFUSED 3 vs 2 (both
  effectively zero on this idle driver; the 32±4 band belongs to the
  tok109 vehicle), WAKE-STRANDED 0 vs 0, panics/OOM 0 vs 0, the same
  idle-task PREEMPT-LEAK print class on both (the wwcensus vehicle's
  own artifact; per-build rip), ticks [100,100] both, syscost volume
  535 vs 552 lines. No sched/mm/fs guarded-domain movement with events
  off (run/tr3-census-a.log / -b.log).
- Known divergences added this round (all noted at the event docs,
  none silent):
  - ext4_sync_file_enter's `parent` reports 0 (Rux's sync core works
    from the inode; no dentry parent at hand) — the round-1 ioprio
    precedent: keep the reference field, report the honest constant.
  - map_blocks len is always 1 and flags always 0 (Rux's funnel maps
    one block per call and passes no EXT4_GET_BLOCKS_* mask); a hole
    reports pblk 0 len 0 ret 0 (the reference's miss shape); unwritten
    extents map M|U with the real pblk.
  - da_write_end fires only for a successful copy (copied = the
    returned count, partial included); a failed write leaves a begin
    with no end.
  - free_blocks flags always 0 (no EXT4_FREE_BLOCKS_* mask); count is
    the run length (whole contiguous extents batch, the reference's
    own granularity).
  - jbd2 sync_commit carries the stopping handle's h_sync (Rux's
    Transaction has no t_synchronous_commit); the events sit at the
    jbd2_journal_stop call site, Rux's single commit funnel, not
    inside the reference's commit-phase points.
  - ext4_ext_get_block (the raw variant used at journal setup) is not
    instrumented (no ino at hand, one call per mount).
  - The rux: group itself is the divergence (reference has no epoll
    events); formats follow the house key=value style of the round-2
    net: events.

### 2026-11-04, round 3 (this session)
- Worktree deleted on the host mid-round (external cleanup); rebuilt
  from the branch: /home/william/rux-agents/dfx-trace3 on dfx-trace3
  off origin/main 53fba4f6, TASK.md + run/ artifacts restored, base
  worktree /home/william/rux-agents/dfx-trace3-base for the A/B
  kernels.
- origin/main moved 115 commits during the round (ltp-r21 + stress-port
  merges, 53fba4f6 → 602337ff); per the coordinator the branch stays
  on its base — all A/B numbers above are same-base comparisons.
- Artifacts: run/tr3-proof-init.c + tr3-init, run/tr3-kvm-proof.sh,
  run/tr3-rootfs.img, run/tr3-bzImage-mine / -base53 / -base,
  run/tr3-release-asm.txt, run/tr3-census.sh (+ -a/-b logs),
  run/tr3-base-check.log (the preexisting-EIO evidence),
  run/trace-ext4-serial.log (the 28/28 smoke).
