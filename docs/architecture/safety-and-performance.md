# Safety and speed, together — the evidence ledger

Thesis (user directive 2026-11-03): a reader's first objection to a
fast new kernel is "they must have skipped the safety machinery to get
the speed." This ledger exists to answer that objection with evidence,
continuously: **Rux is safer by construction AND at or ahead on
measured performance — because the same architecture delivers both.**
Every entry cites a verifiable artifact; the ledger grows with every
merged round (AGENTS.md rule). This is the argumentative companion to
the technical register (rust-native-register.md).

## 1. The core claim, in one architecture
In a C kernel, safety and speed trade off: runtime checking, safer
idioms, and the debugging toolchain (KASAN/lockdep-class builds) cost
real cycles, and the language cannot keep memory-safety bugs out of
the fast path at all. In Rux, the safety checks that matter run at
COMPILE TIME and vanish from the fast path — so the fast design and
the safe design are the SAME design. The performance entries in §3
are each annotated with the safety property their design carries.

## 2. Safety, measured

### 2.0 Safe-by-construction surface
All non-arch kernel paths are `no_std` safe Rust; the unsafe surface
is enumerated and contracted — 3,158 unsafe tokens in service code vs
1,340 in the audited Framework layer, 47 service files enforce
`forbid(unsafe_code)` at the compiler, 56 more are already unsafe-free;
SAFETY-comment coverage 3,207/5,009 (64%); 157 Kani harnesses, 1,104
proptests, 4 SPIN models, Miri on the verify crate (generated ledger:
docs/development/tcb-ledger.md; security-design.md §0).

### 2.1 The bug taxonomy v1 (round-3 sweep, 2026-11-03)

Method. The sweep covered the entire bug history available to this
repository: all 3,358 deduplicated non-merge commit subjects reachable
from main (cherry-picks across lanes deduplicated by subject), of which
2,702 are kernel-code subjects after excluding docs, session logs, LTP
scan results, test drivers, and instrumentation-only commits; the lane
hunt ledgers (docs/development/wildwrite-hunt.md,
docs/archive/r74-scribble-hunt.md, docs/development/review-coverage.md,
docs/development/security-posture.md); and the fix diffs themselves.
Every subject matching a corruption/race/panic vocabulary (169
candidates) was individually adjudicated, and each memory-corruption
row below had its fix diff inspected to establish WHERE the defect
lived: inside an explicit `unsafe` block / arch assembly / an unsafe
interior (raw-pointer or `UnsafeCell` metadata structures), or in safe
code. Containment-only commits (heals, guards, tripwires) are not
counted as bug rows; revert-and-reland pairs count once. Known limits:
a corruption bug whose commit subject carries none of the vocabulary
could escape the candidate net — the hunt ledgers were cross-checked
to bound this, and any later find appends a row here.

Classification: (a) memory-corruption class — the kernel itself read
or wrote memory it should not have (overrun, UAF, double-free,
race-on-memory); these are the CVE classes in C-kernel history.
(b) logic/semantic — wrong behavior with in-bounds, correctly-owned
memory (syscall parity, protocol semantics, lock discipline without
corruption, accounting drift). (c) safe-language panic surface — a
defect that manifested as a caught, loud panic instead of silent
corruption.

**The decisive table — every (a)-class bug found in the campaign
history, with residency:**

*Defect inside an explicit unsafe block, arch assembly, or unsafe
interior — 23 rows:*

| # | Bug (root cause) | Residency | Citation |
|---|---|---|---|
| 1 | kthread pt_regs zeroing: `write_bytes` element-count overrun (168×168 B on x86_64, 28,224 B past the frame; 28 KB-102 KB on the three arches; zeroed live buffer-cache/inode blocks — the defect behind the WalkClaim revert) | explicit `unsafe {}`, kernel/src/process/kthread.rs | d047afad |
| 2 | riscv64 trap.S `.Lfrom_user` stored live sp into ti_user_sp before the SPP origin check (thread_info smear producer #1) | arch asm, trap.S | c1539f6e (hunt: r74-scribble-hunt.md) |
| 3 | riscv64 trap.S sp→ti_kernel_sp propagation unvalidated — corrupted sp written verbatim, every consumer propagates it (smear producer #2; R85 anchors ksp at stack top on every touch) | arch asm, trap.S | 8b8bdd16 |
| 4 | ret_from_fork_user set ti_kernel_sp to the heap-allocated PtRegs — later trap frames pushed onto heap objects (FsStruct cwd Vec corrupted) | arch asm, trap.S | 08cdc1bcba76 |
| 5 | x86_64 entry left EFLAGS.DF dirty — user-set DF reversed `rep movsb`, kernel copies wrote N−1 bytes below destination (the x86 scribble-engine root: smashed saved registers, return addresses, lock words) | arch entry asm | e70702240725 |
| 6 | aarch64 trap.S restore-tail paired `ldp` at the wrong slot offsets (every restored x23..x30 from its neighbour) + SPSR.M tested on a clobbered register (EL1 traps returned corrupt) | arch asm, trap.S | 92799345 |
| 7 | aarch64 boot.S stored the vector-table base into dtb_pointer (latent; repaired later in boot; review-caught) | arch asm, boot.S | 6e808eea |
| 8 | x86_64 copy_kernel_mappings cloned the kernel root's low half into every user mm — fork/teardown demoted and freed device-window tables another tree still linked; buddy reused the frame and later walks read/wrote through recycled file data (self-propagating .text scribbler; closed by the kernel-half device alias + fixmap shape) | unsafe PTE walks, arch/x86_64/mm/mmu_init.rs | 963832aa0d5c (+27559fe4 guard, 7350fc30/d58d0221 stamps) |
| 9 | riscv64 copy_kernel_mappings copied kernel L1 entries that also pointed at user L0 tables — child execve overwrote parent PTEs; child exit freed the parent's L0 through the stale link | `unsafe fn copy_kernel_mappings` | 7a497838615b |
| 10 | address-space teardown ran put_page/rmap/free_pages on framebuffer DEVICE frames — page_desc accounting of unrelated descriptors corrupted | unsafe leaf walks, mmu_init.rs | 5332954b02c2 |
| 11 | early-boot page-table frames (memblock-allocated, not reachable from the kernel root walk) never stamped boot-permanent — teardown freed them twice in one walk (aliased links; the recycled-frame family) | unsafe tree walk/stamp, mmu_init.rs | d58d0221fb10 (+7350fc30) |
| 12 | buddy split left the final block unmarked as allocated — same memory handed out multiple times (FdTable 8 KB Vec corrupted; fork hangs) | allocator interior (UnsafeCell metadata arrays) | 016b3fa192f3 |
| 13 | review Wave-2 trio: remap_page rebuilt leaf PTE with a page number where a vaddr belonged; swap-out freed pages under live PTEs; buddy descriptor transitions outside the zone lock | unsafe PTE/buddy paths, review-found pre-merge | be73c63873f7 |
| 14 | virtio desc allocator reset reclaimed descriptors of in-flight requests + one shared used.idx for all cores (per-core completion confusion, stale data returns) | unsafe MMIO/shared rings | 9810e1313aea |
| 15 | virtio net TX DMA programmed with virtual addresses; MMIO register offsets off-spec; global-counter completion race (probabilistic stale reads) | unsafe MMIO writes | d5144ab7df81 |
| 16 | do_exit woke the parent before schedule() — parent reaped (freed) the still-running child; the freed Task's ti_cpu (−1) sign-extended through trap.S `lw` into per-CPU index corruption | raw `*mut Task` protocol + trap.S | 3b39b73270c6 |
| 17 | secondary CPU enabled timer IRQs before tp held a valid Task; the trap WFI-skip dereferenced garbage sepc (unmapped phys) — nested fault, loud panic (no silent read: the MMU caught it) | unchecked deref in trap path | 33b954a70b5a |
| 18 | devfs stored `Arc::as_ptr` without an owning clone — dangles when the map entry drops (review-found) | safe storage → unsafe deref boundary | 69f456923298 (F7-02) |
| 19 | KILLDUMP walked unpinned raw task pointers — freed mid-dump, poisoned tid printed, deepest boot page-faulted | dfx unsafe raw pointers | d14d9403bec7 |
| 20 | scheduler tick dereferenced a garbage cgroup_ptr (deterministic 0x652a... value; producer = the page-table teardown family of rows 8-11) | `unsafe { &*ptr }`, sched/cgroup.rs | 5393d3aa8a84 |
| 21 | io_uring SQE index read from the user-shared ring used unchecked in `kvirt.add(idx * 64)` raw-pointer read (review-found) | unsafe raw-pointer arithmetic | 5465735f8999 (F15-12) |
| 22 | ext4 write_group_descriptor `from_raw_parts` length taken from on-disk desc_size over a 32-byte struct — 64-byte read/write, on-disk descriptor fields corrupted | unsafe block, ext4 | 8bbb94fd534d |
| 23 | CR0.WP never set — supervisor stores ignored page-table RW bits; kernel-mode uaccess wrote through COW/share protections (cross-mapper content bleed with no trace) | boot asm config omission | 12f82297e85b |

*Defect in SAFE code, corruption executed through a trusted unsafe
boundary — 10 rows:*

| # | Bug (root cause) | Residency | Citation |
|---|---|---|---|
| 24 | vmscan freed when `refcount <= 0` — the underflow arm freed a page another CPU had already freed (the zone double-free enabler; "same PFN to two owners") | safe comparison in mm/vmscan.rs driving a safe-signature free | f4b67f669f0d (F10) |
| 25 | page_cache stale puts: two failed decrements of one invalidated page each released the same frame (the convicted zone double-free stream) | safe release logic, fs/page_cache.rs | f4b67f669f0d (R14-16) |
| 26 | exec init early-return path dropped the RAII guard — PID 1's freshly built page tables freed to the zone while init kept executing on them | safe code omitted `mem::forget`, process/exec.rs | 7350fc304fe8 (pt 1) |
| 27 | Task::address_space `Option<Arc<MmStruct>>` swapped with no synchronization against foreign-task readers cloning the Arc — a racing clone resurrected freed memory and the resurrected Arc's drop ran MmStruct teardown twice on one pgd | safe lock-protocol omission (both accessors are safe fns) | 33960134670b |
| 28 | pty construction chain ran ~71 KB deep in the 64 KB kernel stack — spill zeroed the adjacent heap page (GDB watchpoint convicted `Spinlock<TtyInput>::new` memcpy below stack bottom) | safe-code stack depth; no guard page at the time | 7eeb5cb149ae |
| 29 | 80 KB packed kstack slot overrun on riscv64 (test kernel nests deeper; adjacent heap data corrupted — getpriority read nice=-120) | sizing decision in safe config; canary added only at re-land | f793c63f → fee7774fc468 → gated re-land 488955a38fa3 |
| 30 | ti_cpu steering TOCTOU (wake-affinity + deferred-exit-notify steers gated on an unlocked on_cpu read) — task ran on CPU A with ti_cpu=B; every subsequent schedule booked against the wrong task (upstream of the scribble/double-run engines) | safe unlocked check-then-write | ed236fffee63 |
| 31 | GRQ keyed-lock swap discarded the returned guard — the lock silently released at end-of-statement, every critical section ran unexcluded (CFS-timeline btree corruption panic family; convicted by 13-boot paired bisection) | safe API misuse; guards are `#[must_use]` since | 2b092c49, convicted b1fe066f/623c4736; mechanism corrected 75dbf637 |
| 32 | recvmsg family re-fetched the user iovec array after validating it — a racing thread redirected scatter copies to unvalidated addresses (review-found) | safe TOCTOU protocol | 32fd359f |
| 33 | PCACHE_SHARED defined as bit 10 — bit 10 is PPN[0] in Sv39, every shared mapping aliased a frame 4 KiB off the cache page (silent neighbour-page corruption; caught in snapshot review before any boot ran it) | safe constant at the unsafe PTE boundary | 6b49140ba1c6 |

**Boundary rows — corruption-class consequences without a kernel
memory-safety violation** (counted separately, not in the 33): wrong
content or wrong bookkeeping delivered to the right addresses, the
corruption landed in user memory or on disk via legal stores:
mremap in-place expand over merged-VMA neighbours (gnome-shell heap
corruption, canary clobber at +0x21000; 56fc7e8b760e); the R71
file-position race on a shared `Arc<File>` (pages filled from the
wrong offsets — .text containing .rodata; c17f6ec07d63); VMA surgery
atomicity (MAP_FIXED piece drop, munmap zap window — thread stacks
lost VMAs while PTEs stayed live; 56b2baebf6cf); ext4 stale extents
aliasing reallocated blocks + symlink inline string freed as bitmap
bits + inode alloc off-by-one (on-disk corruption; 2f9763fc9e10,
f4b67f669f0d F1).

**The statistic, corrected from the draft claim.** The draft said
"every memory-corruption-class bug found lived inside an
explicitly-audited unsafe block; zero in safe code." The sweep
falsifies the strict form and replaces it with the residency split:
**33 memory-corruption-class bugs; 23 had the defect inside an
explicit unsafe block, arch assembly, or an unsafe interior (raw
`*mut Task`/cgroup protocols, `UnsafeCell` allocator metadata); 10 had
the defect in safe code — and every one of those ten is an
ownership/refcount/lock-protocol/stack-depth decision that safe Rust
does not type-check today, executed through a trusted unsafe boundary
or an unguarded stack.** Zero pointer-expression defects (indexing
overrun, iterator invalidation, format-string, wild pointer
arithmetic) ever appeared in safe code: the classic C expression-level
classes are compile errors here. The one safe-code would-be
out-of-bounds WRITE (the pcp refill race, below) was stopped by the
bounds check and surfaced as a loud panic. Six of the 33 were caught
by code review before any boot ran them (rows 7, 13, 18, 21, 32, 33).
The honest reading: the language removed the C-class pointer bugs
from 100% of the safe-code surface; the remaining safe-code risk is
concentrated exactly where ownership is manual (physical frames,
page tables, cross-task lifetimes) — which is what the §5 gaps
register and the typed-guard/typed-key work attack.

Honesty notes from the same hunts: instruments get convicted too —
PTGUARD/FREEGUARD were misdiagnoses (mapcount=−1 was the
PAGE_MAPCOUNT_BIAS sentinel; e61f7a3a36e2), and the first PT_LEDGER
hash design produced false DOUBLE-FREE reports plus real frame leaks
(626658991200); the 2b092c49 "latent GRQ race" theory was wrong (the
discarded-guard mechanism above replaced it). Negative results with
receipts stay in the record.

**Bug classes structurally absent from safe code**: buffer overruns
(bounds-checked slices), use-after-free/double-free of Arc-owned
objects (ownership; row 18 shows the raw-pointer-storage boundary),
data races on typed shared state (Send/Sync), iterator invalidation
(borrowck), format-string injection (typed formatting). The classes
that dominate C-kernel CVE history cannot be expressed here outside
audited unsafe.

### 2.2 Panic surfaces enumerated, not latent
The kernel's panic paths are a designed surface (checked UTF-8 on
userspace-writable strings, bounded slot namespaces, stack-slot
recycling guards) — each found instance eliminated with a regression
test, not filed as a CVE class. Receipts (class (c) from the sweep —
caught loudly, each row a fix): the pcp refill race whose racing
store hit a bounds-checked index and panicked instead of writing past
the row ("index out of bounds: the len is 32 but the index is 32",
165ee5fcebc0); the UTF-8 printer family on userspace comms
(f0c57182 / 3ad32b9e); the stale negative-memo null-deref panic
(ea26d923); the mmprof scalar-slot namespace indexing panic
(276c2e70); the dfx switch-table array panic (363401cd); the memwatch
PFN→site bound panic on >2G RAM (cbd41b40); R81 garbage-pid print
hardening (2c044bb5); the R73 KERNPANIC stack-walk bound and dump
latch (9777f195); the taskdump lockless-timeline scan panic
(bf995f2a); the ext4 empty-index-root `idxs[0]` panic (b70a5deb);
the aarch64 trap-report regs[31] out-of-bounds (363ac70b); the binder
dead-proc `expect()` replaced by a bounded warning (619bf4b6); the
  timer same-CPU IRQ-reentrancy family (186a00b6); the Wave-1
  one-step-kill/panic class, 11 items (83d0b691), and the get_cwd UB
  + UTF-8 cmdline panic fix (830e3f8c). Test-harness-only panics
  (unit-test Vec drop) are excluded by method.

## 3. Performance, measured (each fast path IS a safe path)
- lmbench, same host, same accel, both kernels under one q35+KVM rig
  (kernel e264a64b, standing table 2026-11-03): 4 rows ahead of the
  reference — lat_ctx −s64 2: 1.98 vs 6.71 µs (3.4x), lat_pipe 5.11
  vs 13.30 µs (2.6x), lat_proc exec 62.9 vs 87.43 µs (1.39x),
  lat_syscall fstat 0.099 vs 0.118 µs (1.19x); the rest of the swept
  rows in the 1.17-2.28x band, both directions recorded
  [docs/test/lmbench-report.md]. Safety pairing: the measured switch
  path carries typed per-CPU storage whose guard lifetime IS the
  interrupt-disabled window (D-7) and keyed locks whose nestings are
  compile facts (D-11) — the fast rows run on designs whose IRQ/lock
  discipline the compiler checks, not on removed checking.
- Typed timer wheel (D-19): timer lock holds per op 924→697 ns class
  at matched volumes, nanosleep 772.6→662.7 ms — AND six timer-misuse
  classes (mixed-clock compare, periodic-Wake, zero interval,
  one-shot+interval, spent-guard misuse, double-cancel) became
  compile errors, verified by planted rustc catches on the real tree.
- WalkClaim lock-free dcache reads: stat 0.64→0.42 µs, open
  1.128→0.889 µs — AND reader progress became a type property (the
  SeqCst bank claim is the only currency the lock-free reads accept;
  no RCU protocol discipline to get wrong).
- vm-op fast path: mmap 72→30 µs under load, 749 ms tail → 2-3 ms —
  via an allocation-free leaf walk; the safety pairing is the
  ownership shape itself: the walk that cannot allocate cannot
  OOM halfway, and the translation artifact it removed (a
  cond_resched per chunk) was our own bug, not a checked cost.
- x86_64 context-switch levers: lat_ctx −s64 2 4.97→2.07 µs (−58%),
  −s64 8 4.68→1.94 µs (med-of-3 interleaved A/B; FSGSBASE + lazy FPU,
  PCID armed where hardware reports it) — pairing: the lazy-FPU move
  was verified against an FP-free kernel binary (no compiler-emitted
  FP to trap on), and the PCID arm-on-every-CPU class that would have
  #GP'd was itself fixed and boot-verified (af464fb9).
- p5 IPC family re-banks (perf-gate-baseline.md, 2026-11-01 board,
  main@afc41b8b): p5b_dgram 318.6→23.1 µs (−93%), p5c stream
  276.5→24.9-26.8 µs (−90-91%) — pairing: the wakeup-preemption gate
  and idle-CPU steering that produced the win sit behind keyed lock
  nestings (futex bucket→GRQ), and the cost side (tick-quantized
  wake→run ~10 ms behind an incumbent, both hog regimes) is banked in
  the same table — the ledger keeps the cost next to the win.
- fs metadata family (same board): stat 200.9→71.1 µs, openclose
  512.9→156.3 µs, append 657.2→77.4 µs, mkdrmdir 22,814→2,757 µs —
  the D-8/D-12/D-13 write-through jbd2 + batched-publication stack;
  pairing: the same batching removed the sync-write cost class while
  keeping crash-consistency write-through (fsw counters show no new
  sync-write class outside provisioning).
- Geekbench multicore class: parity-to-ahead of the reference on the
  same host (7139 vs 7027, 4-CPU pinned class) — pairing: it runs on
  the D-1 global run queue, the single structure the wake/pick census
  adjudicates on every accepted quiet boot (TERM-DESCHED 8±2 /
  WAKE-REFUSED 32±4, exact across the board since 2026-10-25); the
  design that makes the workload fast is the design the safety
  instruments reason over.
- Trace records sequence per CPU, merge by timestamp (oh-tracepc
  round, 2026-11-06): the ONE global fetch_add every armed trace
  record paid (both CPUs, sys_enter+sys_exit+kmem class) became a
  per-ring counter + read-time timestamp merge — the reference's
  local_t/__find_next_entry shape, so cross-CPU recording no longer
  shares a cache line. Same-rig A/B (q35 KVM SMP2, debug kernels,
  events armed via the reference-parity trace_event= boot param):
  pbench p1_null median unchanged (0.701 µs both) while the tail
  recovered toward the events-off floor — p99 1.403→0.741 µs (−47%),
  avg −2.6%; p2_ctxsw median −3.4%; armed OH boot kernel ticks in the
  early window −1.4% at the same SA ladder (51/51 inserts, ceiling
  8301 both). Events-off census A/B identical (TERM 0/0, WAKE-REF
  0/0, zero panics) — the disabled path is byte-identical
  (p1_null 0.290/0.290 µs medians). Pairing: per-CPU sequencing
  removes the shared mutable state rather than narrowing a race
  window around it; the merge order is now a timestamp property the
  renderer asserts, not a global counter's arrival order. Artifacts:
  oh-tracepc TASK.md §5-7, run/ab-analyze.sh outputs
  (run/oh-abp1[ab]-*, run/oh-abe1[ab]-*, work/pbench-pb[oa]-[ab]-*).

## 4. Nothing traded away
The compatibility checklist stands while both ledgers hold: 345+
syscalls with reference-matched semantics; LTP 1,059/1,869 and
climbing (35→524→801→945→1,005→1,059); unmodified Ubuntu 22.04 to a
GNOME Shell desktop; unmodified OpenHarmony with 135+ services; three
architectures from one tree. Speed did not cost compatibility; safety
did not cost speed.

## 5. Honest gaps register v1 (what the reference kernel carries that Rux does not YET)
Credibility ordering: by-construction answers first (where the
reference needs tooling because the language cannot provide the
property), then what Rux has with artifacts, then partials, then
absents stated plainly. Each absent names what closing looks like.

**By construction (the property is the language, not a build option):**
- Memory safety of the non-unsafe bulk — the reason KASAN-class tools
  exist in C. Evidence: 12.9% unsafe lines whole-kernel
  (docs/development/tcb-ledger.md), 47 `forbid(unsafe_code)` service
  files, §2.1's zero pointer-expression defects in safe code.
- Bounds-checked indexing everywhere in safe code (FORTIFY-class).
  Receipt: §2.2's pcp refill race — the would-be OOB write panicked
  at the check (165ee5fcebc0).
- Data-race freedom for typed shared state (KCSAN's heap-object
  target class). The safe-code races found (§2.1 rows 27, 30-32) were
  lock-protocol omissions on manually-owned lifetimes — not
  type-expressible races.
- Iterator/borrow invalidation, format-string injection — compile
  errors in safe code.

**Have, with artifacts:**
- SMAP on x86_64 with EFLAGS.AC brackets (~55 ns/bracket,
  security-design.md L2); capability bit read from CPUID leaf 7
  EBX[20] (the G1 register-read bug is fixed, 763e9f3c; boot-log
  assertion from live CR4+CPUID, bec6c79b); zero SMAP-VIOL beacons
  across the suite (D-18).
- CR0.WP from earliest boot (12f82297; §2.1 row 23).
- No identity/physical-memory window: MMIO behind kernel-half alias
  windows, bootstrap identity map unlinked after boot (D-17); a bad
  user pointer faults instead of returning kernel bytes (x86_64 suite
  1194/1194 with the window gone).
- User-space W^X: NX on x86_64 user mappings, PT_GNU_STACK consulted,
  READ_IMPLIES_EXEC honored (1de56837).
- seccomp STRICT/FILTER engine gated before dispatch +
  PR_SET_NO_NEW_PRIVS (security-posture.md; LTP prctl02/04).
- Capability model: 41 CAP_* constants (CAP 0..40,
  security/capability.rs) with UAPI numbering, exec transitions with
  rollback, no_new_privs; enforcement breadth partial (below).
- Kernel-stack guard rails: per-slot canary (R18-2), KSTACK-HW depth
  probe, freed-stack ring with 0xC3 free-paint (dfx-gated) — the
  stack-protector + overflow-detection analog for kernel stacks.
- Deferred-free quarantine for teardown-raced objects
  (mm/deferred_free.rs; the call_rcu idea, wall-clock so preempted
  readers are covered).
- Oops/panic discipline: oops path with the reference report shape,
  panic_on_oops sysctl, panic= reboot, pstore console capture.
- Unified tracepoints (tracefs, D-15): disabled site compiles to one
  load + test + branch (verified in release disasm), +18-19 ns per
  enabled record, +0.12% image — ftrace-class observability.
- Verification rig with no reference-kernel analog: 157 Kani
  harnesses, 1104 proptests, 4 SPIN models, Miri, loom on the sync
  mirror (S-1).

**Partial:**
- lockdep-class lock debugging: typed lock keys make the named
  nestings compile facts (D-11) and the preempt-count census/heal
  adjudicates phantom-bracket classes (model-vs-reality MISS = 0
  banked); a full runtime held-lock graph over the whole tree is
  absent. Closing: extend the keyed coverage to the remaining
  nestings; a debug-build held-lock bitmap is the drop-in shape.
- Refcount hardening (refcount_t-class saturation/leak semantics):
  page/frame refcounts are plain atomics. Two of §2.1's ten safe-code
  corruption bugs (rows 24-25) are exactly the class a checked
  Refcount newtype catches. Closing: typed Refcount with
  decrement-must-hit-zero-to-free as the only free path.
- Arithmetic hardening in release builds: overflow-checks are off in
  the release profile (Cargo.toml), on in dev. Closing: one line plus
  a measured boot/perf cost through the merge gate.
- KASAN-style allocator poisoning breadth: today's poisoning is
  task-page paint + header audits + the buddy-meta shadow, all
  dfx-gated (metawatch, R83); no redzones on small_alloc. Closing:
  build-flag redzones and poison-on-free in small_alloc/slab.
- LSM surface: a Linux-like hook point exists; selinuxfs is a
  label-carrying compat mount, permissive by construction
  (security-posture.md). Closing: policy parse + AVC decisions.
- Namespaces: PID and IPC namespaces real (1bde7490); net/user/mount
  are stubs.
- Capability enforcement breadth: 41 constants, ~18 enforced gate
  sites (the posture row's set plus sec-r1's six) vs the reference's
  ~120.

**Absent (plainly):**
- KASLR: the offset field and plumbing exist; no entropy source is
  wired at boot (posture row). Closing: RDRAND/seed at entry, boot
  print of a varying offset.
- Kernel text/rodata W^X tightening: the image is mapped RWX
  (riscv64) / RW without NX (x86_64); no mark_rodata_ro pass exists
  (posture row). Closing: post-init RO flip + a dfx page-table walk
  tripwire.
- Compiler stack-protector / shadow-call-stack instrumentation: none
  today (the kernel-stack canary above is kernel-side, not
  compiler-generated). Closing: rustc stack-protector attribute +
  build flag, cost measured through the gate.
- Hardened usercopy (allocation-region whitelists): access_ok bounds
  checks only. Closing: a per-object usercopy-region check at the
  uaccess capsules.
- Module signing: moot today — the module surface is registration
  only (no runtime code loading, kernel/src/module/mod.rs); when a
  real loader lands, signature verification must ship with it.
- KCSAN-class watchpoint race checker: absent; §2.1 shows the
  residual safe-code race classes are protocol-level. Closing: a
  watchpoint checker is a project, the loom/Kani mirror covers the
  primitives it can.
- eBPF-class runtime mitigation channel (security-design L6): absent.
- kptr_restrict / kallsyms masking: /proc/kallsyms shows real
  addresses to every reader (posture quick row).

## 6. Process (how this grows)
- Every merged round that lands a safety property, a perf number, or a
  bug-class elimination appends its evidence here (AGENTS.md rule,
  2026-11-03) — same change, not a follow-up.
- The arch lane re-mines lane histories each round for §2 taxonomy
  rows and §5 gap closures.
- No entry without an artifact pointer; numbers from same-rig A/B.
- Round-3 (2026-11-03): §2.1 taxonomy v1 landed (33 adjudicated
  memory-corruption rows, residency split 23/10, corrected headline
  claim); §5 gaps register v1; §3 pairing sentences completed.
- Observability wave 3 (2026-11-04, dfx-trace3 merge): event catalog
  37 → 51 (ext4:/jbd2: reference families + rux:epoll); disabled-site
  cost discipline held (39 kernel sites one-load, ASM-verified,
  addresses in the lane record); enabled cost opt-in at 13-35
  ns/record; events-off census A/B identical (TERM-DESCHED 8/8, 0
  panics both sides). Real bug fixed in the same change: the round-2
  alias CONSUMER (boot arming loop in DfxSwitch::set) was silently
  dropped by a merge while its table survived — dfx= hunt boots had
  armed nothing until restored. Lesson: table+consumer pairs
  reconcile as one unit; keeping the table and losing the call site
  is a silent observability outage, not a clean merge.
  Source: dfx-trace3 TASK.md round-3 record (run/tr3-base-check.log,
  census A/B logs in the lane worktree).
- Trace per-CPU sequencing round (2026-11-06, oh-tracepc): GLOBAL_SEQ
  (one atomic for every record on every CPU) replaced by per-ring
  counters + read-time timestamp merge — the reference's
  ring_buffer.c/__find_next_entry contract, verified on 4-CPU real
  data (500-record merged window globally ts-sorted). The reference's
  trace_event= boot parameter landed with it (deterministic boot-time
  arming for any lane). Evidence: §3 bullet above; oh-tracepc TASK.md
  (functional + gate record).
