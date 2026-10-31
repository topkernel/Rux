# Review findings — cr3 session (code-review lane, 2026-11-01)

Scope (coordinator assignment): the perf-sprint delta merged to main
since the lane's last reviewed point (d608a9b7..5bf31528, 51 commits).
The NEW shared surface, reviewed line-by-line against reference
semantics: the ext4 perf family, the timer LAPIC one-shot rework, the
console batched TX paths, the syscall hot-path unloading (dfx=
sysforensic), the WAKE-HEAL/CLAIM-SWEEP scheduler heals (three-gated
against the sc265/sc267 convictions), the SSIE riscv64 fix, the binder
death-notification delta, and the fstat-socket + SA_RESTORER + arm64
open-flag fixes, plus the x86_64 bootstrap-identity-map drop (this
lane's own sibling branch).

Standing rule honored: a perf fix that changes semantics is a
regression. Verdict for the batch: NO functional regression found.
One cosmetic defect fixed, two register entries added, one process gap
closed.

## Fixed this session

1. **uaccess: duplicate shadowed `let limit` pairs in the chunked
   strncpy_from_user** (arch/riscv64/uaccess.rs, arch/x86_64/uaccess.rs;
   remnant of 8c2da38b's rewrite — the new chunked body re-declared the
   two `min` lines the function head already computed). Dead code, same
   values, no behavior change; removed on both arches.

2. **design-divergences.md register gap** — the sprint's two
   semantics-adjacent internal divergences reached main without
   register entries (AGENTS.md core principle requires recording every
   advantageous divergence). Added:
   - **D-13: Batched metadata publication + discard-on-free** (the
     PUB_BATCH=64 journal-stop drain + the freed-block forget +
     getblk_overwrite skip-read), with the durability-contract
     argument (POSIX never promised per-op dir durability), the
     reference-FORGET alignment, the known costs (64 records in
     flight, pinned-buffer late write, no TRIM), and the guard
     (fsw pb2/pm, FSW3 db=, pbench fs rows).
   - **D-14: Measured-wait wakeup-preemption gate** (preempt_check_
     wake), the reference check_preempt_curr divergence, the
     starvation argument (>=1 latency window keeps immediate preempt),
     and the guard (P2/P5 rows, census bands).

## Three-gate verdicts (reviewed, nothing to fix)

- **WAKE-HEAL (wake_up_enqueue_opt)** — PASS, sound under all three
  gates. The invariant backing the zero is structural, not statistical:
  (a) __schedule's preempt-discipline check refuses to park ANY task
  with preempt_count != 0 (restores RUNNING and returns), (b) the GRQ
  guard is released (unlock_irqretain) BEFORE context_switch, so a
  mid-switch task carries pc==0, (c) the current-slot publish and the
  SWITCHING_OUT marker are both written under the same GRQ hold the
  heal's curr_on scan takes — the scan cannot interleave a pick. So a
  task that is SLEEPING/STOPPED (the wake path's state filter), owned
  by no slot, and not mid-switch can never legally hold a count; the
  sc265 conviction (zero bookings ever made on a no-CPU task) is the
  empirical twin of this structural argument. sc267 is correctly
  honored too: the never-schedule incumbents are covered by the
  tick-side PREEMPT-HEAL (+need_resched), not by the wake heal. The
  stale-marker time-confirmation (>=50 jiffies across two wake
  attempts) only matters when pc != 0, which a genuinely mid-switch
  task cannot have — belt-and-braces, not load-bearing. Full-zero
  (base + stranded hardirq) is correct for a task owned by no CPU
  (fleet-session-3 argument: no in-flight nesting by definition).
  Print budgets 300/60, direct-console under GRQ (R34 discipline).

- **CLAIM-SWEEP (scheduler_tick -> fair::claim_sweep)** — PASS. It
  zeroes OWNERSHIP marks, not preempt counts (the coordinator brief
  conflated the two heals on this point). Leg 1 (on_cpu=1) requires
  pick_owner_live == false AND the 50-jiffy exec-frozen orphan-watch
  confirm; leg 2 (on_cpu=0, claim != -1) heals only claims backed by no
  slot and no in-flight switch; lying-slot shapes (owned_claim) are
  census-only, NOT healed. The full-qword release matches
  __switch_to's own store. Once per second, whole-timeline walk under
  GRQ — microseconds at the OH task population.

- **preempt_check_wake / claim_idle_cpu_for (977adb51)** — PASS.
  Policy divergence (now D-14). The idle claim is advisory: the GRQ is
  global so any scheduling CPU picks the wakee; a lost claim cannot
  strand a task (worst case: latency to slice end). The measured-wait
  gate keeps immediate preemption for >= 6ms waits — starvation-free.
  exec_start==0 (never ran) falls through to the preempt check, so new
  tasks preempt.

- **ext4 perf family (2f219478, cd1a2347, 39975867, 016bdbaf,
  1debc2b1)** — PASS. Publication batching: POSIX durability contract
  intact (fsync/sync/eviction/commit unchanged; H1/H2 constraint
  guards unchanged); the drain runs at a leaf-lock level (no inversion
  against ENTRY_MUTEX/BIG_LOCK). getblk_overwrite: full-4096 copy on
  both miss (zeroed+dirty, no I/O) and hit (uptodate-gated); partial
  writes still take the read path — correct for sub-4K filesystem
  blocks sharing a cache block. Directory scans: in-place scan under
  the bread pin, tail-first add / bidirectional find are speed-only
  (names unique per directory; any free slot is a legal host); only
  the hosting block is cloned; fresh dir blocks are fully initialized
  in memory (zeroed Vec + entry + csum tail) so the skip-read can
  never leak prior-life bytes. Discard-on-free: dirty-unpinned-only
  drop with find+verify+unhash under one bucket hold (R7-C1 shape);
  jbd2 forget removes the CURRENT handle's registration (a prior
  in-flight commit may still write dead data to a free block —
  harmless, the bitmap owns the truth); revoke records still written
  before the free; mirrors the reference EXT4_FREE_BLOCKS_FORGET.
  Residuals ledgered in D-13 (pinned-buffer late write; no TRIM).

- **Timer LAPIC one-shot (6394e8ee)** — PASS. u128 count conversion
  clamped [1, u32::MAX]; past-deadline clamps to 1 count (fires now,
  softirq sweep delivers); !READY or freq==0 refuses arming (periodic
  keeps the grid on calibration-failed boots); the tick handler
  re-arms BEFORE any in-handler switch (a missed re-arm on a one-shot
  LAPIC would tick-less the CPU); PIT-phase double-tick is
  structurally impossible (jiffies derive from the timebase is
  idempotent when precise; set_next_trigger is skipped when not).

- **Console batched TX (50043253)** — PASS. Byte-identical output
  (\r\n expansion preserved in all three staged writers); ConsoleWriter
  flushes at Drop; /dev/console writes chunk at 32 bytes with
  cond_resched. ONE NOTE (ledgered here, guard-relevant): puts_bytes
  holds the irqsave lock across the whole slice — at boot print volumes
  the closed-cli segments stay in the 3-7 ms gate class (2048-byte
  burst measured ~2.05 ms), but a future caller passing much larger
  slices would lengthen the window; keep printk emits line-sized.

- **syscall hot-path unloading (1eed885b) + perf-syscall family
  (071afe42, c358c711, 2e595ed3, 8c2da38b, 542bbc6d)** — PASS. The
  gated hooks are diagnostics-only; the pre-translate LASTSYS latch
  taskdump depends on stays always-on. ISIG/CAD load-first cannot lose
  a latch (the store stays set; only a nonzero load proceeds to the
  swap). strncpy_from_user chunking preserves the Ok/EFAULT shapes
  exactly (mid-chunk fault zero-fills the tail; a NUL found in the
  readable prefix succeeds exactly where the byte loop did; NUL-at-0
  still succeeds). path_lookup fast path: the normal-form probe is
  equivalent to normalize_with_root("/") for the shapes it accepts;
  chrooted/non-normal paths keep the allocating arms; the single
  rooted read is MORE consistent than the old code's two independent
  get_process_root() reads. The symlink hint passes None after a mount
  hop (both arms) — the one shape where the hint would describe the
  wrong dentry.

- **SSIE fix (94c8dcec)** — PASS, and the standout correctness fix of
  the batch: secondary harts ran with software interrupts masked
  forever (resched/call-function/TLB-flush IPIs pending-but-disabled),
  which both quantized cross-hart wakes to the 10 ms grid AND left
  TLB-flush IPIs silently unexecuted — a standing TLB-staleness
  hazard, now removed. csrsi sie,2 at secondary bring-up, after
  external-interrupt enable, before the idle loop.

- **binder death-notification delta (3ad7c3e7, ff273421, a7189470,
  773f701e + the ams46/SACB taps)** — PASS. The delta is
  instrumentation only: sendfail_why is ams46-gated, the SENDFAIL WARN
  is ams46-gated with a 400 budget, SACB is samgr-pid-latched with a
  400 budget (zero lines on non-OH boots), the ripple window is 120 s
  with a 640 budget. The BR_DEAD_REPLY / BR_FAILED_REPLY mapping per
  failure case (requester-dead, child-node-dead, owner-proc-dead,
  no-ctxmgr, target-died-pre-copy, protocol/alloc errors) is unchanged
  from the previously-reviewed state and matches the reference binder
  protocol.

- **fstat-socket (be5b2709)** — PASS. fstat on socket fds must report
  S_IFSOCK|0777 with a per-description st_ino (reference sockfs
  behavior); the old EBADF was simply wrong and broke sd-bus unix-fd
  negotiation. Ops-ptr dispatch covers all four socket families;
  non-socket Files fall through.

- **aarch64 SA_RESTORER (7d605228)** — PASS. sa_restorer honored only
  under SA_RESTORER; restorer-less actions return via the kernel
  sigtramp page — matches the reference arm64 setup_return contract
  (glibc aarch64 leaves the sa_restorer slot uninitialized stack
  garbage otherwise; the 34-test garbage-lr SIGSEGV class).

- **arm64 open-flag bits (2833a267)** — PASS. O_DIRECTORY 040000 /
  O_NOFOLLOW 0100000 / O_DIRECT 0200000 / O_LARGEFILE 0400000 on
  aarch64 vs the asm-generic set elsewhere — verified against both
  reference uapi tables; the two raw literals now use the shared
  constant.

- **x86_64 bootstrap identity-map drop (e22f72a8, via this lane's
  code-review-perf sibling)** — PASS. Closes the standing uaccess
  hole (copy_from_user at low user addresses read physical memory
  through PML4[0] instead of faulting); low-MMIO consumers ioremapped
  into the kernel-half dynamic alias arena.

## Residual risks named (no action this session)

- discard-on-free leaves a PINNED dirty freed buffer in the cache; if
  the block is reallocated before the pin drops and the holder writes
  it, dead bytes land on the new owner's block. Requires a concurrent
  pin on a block being freed — excluded by ENTRY_MUTEX/BIG_LOCK for
  metadata, not formally excluded for file data (the pre-existing
  page-cache-vs-free race is the parent class; free_inode invalidates
  pages but the ordering vs free_inode_blocks is not serialized).
  Pre-existing, not introduced by this delta; noted for the fs lane.
- puts_bytes irqsave span (see console verdict).
- The heals (WAKE-HEAL/PREEMPT-HEAL/CLAIM-SWEEP) remain mitigations
  for the wild-write producer that sc266 convicted as landing outside
  every instrumented channel; the producer hunt (r83hunt/wildwrite
  lane) stays the root-cause path.

## Verification
x3 builds rc=0 (run/cr3-build-{x64,r64,aa64}.log, kernel crate
recompiled in each AFTER the uaccess edits); make verify 1104/1104
(run/cr3-verify.log); riscv make test ALL TESTS PASSED, suite verdict
green (run/cr3-maketest.log; wrapper rc=124 is the expected post-halt
kill); coverage --check green.

## Successor closeout (same session, quota-cap succession)
The cr3 reviewer died before committing; the successor (this entry)
independently re-verified the load-bearing verdicts against the code
before committing them:

- WAKE-HEAL structural argument re-derived from source: (a) the
  preempt-discipline refusal at __schedule head (sched.rs ~2417:
  `!prev_state.is_dead() && preempt_count != 0` restores RUNNING and
  returns — nothing can park a live count), (b) SWITCHING_OUT is
  published while the GRQ is still held and the lock is released
  before context_switch (~2560-2643), so the heal's three skips cover
  every legal-count window, (c) preempt_count_add/sub operate only on
  `current()` (preempt.rs) — a task owned by no CPU cannot legally
  GAIN a count. Confirmed PASS.
- CLAIM-SWEEP re-read: heals ownership marks only (full-qword release
  via pick_owner_live + 50-jiffy stale_mark_confirm on the on_cpu leg;
  claim store -1 on the claim leg); lying-slot shapes census-only;
  once/100 jiffies under GRQ from scheduler_tick. Confirmed PASS.
- preempt_check_wake: exec_start==0 falls through to the preempt check
  (new tasks preempt); measured-wait gate exactly as registered in
  D-14. Confirmed PASS.
- ext4 family spot re-checks: get_overwrite hit path is
  uptodate-gated (failed-async residue treated as miss, same
  discipline as the pre-existing get()); write_block_from_vec's
  full-block check (>=4096) matches the fixed 4 KiB cache block
  (BlockCache::new(2048, 8192, 4096)); drop_block is
  dirty+unpinned-only with find+verify+unhash under one bucket hold;
  sync_buffers/sync_device_buffers_excluding still flush publications
  first (durability points intact). Confirmed PASS.

Delta 5bf31528..main (3 non-merge commits, reviewed this session):
- 88dd4b82 (leakorigin outermost-acquire tap): diagnostics-only;
  note_acquire callers all sit behind the ENABLED check in
  preempt_count_add, note_release/check_user_return/report_* gate
  internally; budgets 400/200/40; report_outermost uses direct
  console (correct under the GRQ-held WAKE-HEAL call site); the
  task.rs hooks add two compares on the sub path when unarmed. PASS.
- e66d7830 + 0d4a5050 (gdm image repack scripts + probes tarball):
  image-side forensics only, no kernel surface. PASS.
- leakorigin.rs itself was a "todo" gap in the cr3 coverage ledger
  (created by e17fb73b inside the window); read end to end this
  session — ledger updated to done.

Final verdict stands: NO functional regression in the
d608a9b7..1134d002 perf-sprint surface.

## Coverage delta
cr3 annotations on sched.rs / fair.rs / bio.rs / ext4/namei.rs /
binder.rs / both uaccess twins / apic.rs / timer/x86_64.rs /
net/socket.rs / signal.rs / fs/file.rs / console.rs + the new
dfx/scmtap.rs entry; register D-13/D-14 appended.
