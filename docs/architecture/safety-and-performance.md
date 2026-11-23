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

**Crate-flip rung 2 (net service crate, 2026-11-22):** the network
stack's 16 files compile solely in kernel/rux-net under
`#![forbid(unsafe_code)]` (compiler-enforced, negative-probe verified)
— the second service column after rux-syscall. The move left ZERO
unsafe behind: the 24 substrate constructs (static-mut table refs,
Arc-refcount probes, POD header views) were extracted to the framework
edge module first (net/net_edges.rs, INV rows above), and the four
historical `unsafe impl Sync` socket markers proved VESTIGIAL — the
structs are auto-Sync (all-Spinlock/atomic fields; deletion verified
by green triple-arch build), a repeat of the "fear marker, not hazard"
class. One safety improvement rode the extraction: pod_view/pod_view_mut
CHECK ALIGNMENT (the old inline header casts never did — a misaligned
skb buffer was silent UB, now the packet drops). Unsafe census:
kernel/src net tree = the framework zone only (buffer + edges +
wrappers, all INV-ledgered); the service files carry none. Battery:
make test 886/886, riscv boot + /proc/net readback, fio 3-case verify
battery PASS (same rig as baseline).

**INV-UACCESS-1 (user-copy soundness, unsafe-slim round, 2026-11-19):**
user-memory access outside `arch/` routes through `arch::uaccess_safe`
(`copy_out`/`copy_in`/`read_user`/`write_user`/`clear_user`/
`strnlen_user`/`futex_cmpxchg_u32` and the typed `UserPtr<T>`/
`UserSlice` handles). Soundness contract, held inside that one module:
the kernel-side operand is proven by the type system (`&[u8]`/
`&mut [u8]`/`T: Copy` — no drop glue, no partial writes), the user side
is access_ok-checked and faults through the exception table, so ANY
user address value is sound to pass and a bad one reports uncopied
bytes / `None` / `false`. Handles are `!Send`/`!Sync` (they alias user
mappings valid only for the current syscall on the current CPU). Call
sites carry no `unsafe`.

**INV-KPTR-1..5 (owned kernel allocation, unsafe-slim round 3 lane A,
2026-11-19):** `safety::ptr::KPtr<T>` is the kernel's `Box` for
device-visible memory: INV-KPTR-1 construction is the single audited
`alloc` edge (null-checked, layout derived from the same `T` as the
eventual dealloc); INV-KPTR-2 uniqueness gives safe `Deref`/`DerefMut`
(exactly one owner, the `Box` aliasing contract); INV-KPTR-3 drop
deallocates with the same `Layout::new::<T>()` — the layout-drift
bug class is structurally closed; INV-KPTR-4 `as_ptr`/`phys`/
`into_raw`/`from_raw` are the DMA escape hatches (block must stay
alive while the device holds the address; re-adopt exactly once);
INV-KPTR-5 device-written memory is read via `read_volatile`, never
plain `Deref`.

**INV-SREF-1..3 (machine-lifetime shared references, round 3 lane
A):** `safety::ptr::StaticRef<T>` holds a shared view of storage that
outlives the machine (INV-SREF-1: static / boxed-once registry /
deliberate leak — each `from_parts` site names its storage), hands
out only `&T` (INV-SREF-2: mutation stays behind the storage's own
lock), and preserves provenance (INV-SREF-3: no offset arithmetic
inside the type; projections re-audit via `map`). The INV-BLKDEV-1
gen_disk hand-out generalized.

**INV-DMA-1..3 (device-shared carve, round 3 lane A):**
`safety::dma::DmaBlock` is one allocation carved into disjoint typed
slots for virtqueue-style rings: INV-DMA-1 single tracked allocation,
`Drop` deallocates with the stored layout; INV-DMA-2 `carve*` checks
alignment, bounds, and disjointness (append-only frontier) and the
owning block outlives its slots (both live in the same struct);
INV-DMA-3 device-written fields are read only through the
bounds-checked volatile paths, so accesses are neither torn nor
elided.

**INV-MMIO-1..3 (device-register window, round 3 lane A):**
`safety::mmio::MmioRegion` is a bounds-checked volatile window:
INV-MMIO-1 construction at probe carries the mapping proof;
INV-MMIO-2 every accessor checks `offset + width <= len` before the
access (an out-of-range offset panics loudly — a driver bug — instead
of walking foreign memory); INV-MMIO-3 all accesses are volatile.
Fence discipline (device ordering) stays with the driver.

**INV-SAFECALL-1..3 / INV-SAFECAST-1 (unsafe-fn-call discipline,
round 3 lane A):** an unsafe fn whose precondition discharges once
gets exactly ONE safe wrapper (INV-SAFECALL-1) whose signature makes
violations impossible or loud (INV-SAFECALL-2), and the unsafe
original is never routed back through the wrapper or itself at its
own call sites — the round-2 self-recursion class (INV-SAFECALL-3).
`safety_cast` is the transmute-shaped edge: size assert first, ONE
audited `transmute_copy` (INV-SAFECAST-1). The `safe_call!` macro
generates wrapper boilerplate with the unsafe call existing only in
the expansion.

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
f4b67f669f0d F1); the program break kept per-TASK instead of per-mm —
a thread's stale snapshot made its malloc_trim brk shrink munmap live
heap pages of the same process (x86 GNOME campaign boot-01: shell +
gdbus thread SIGSEGV on one heap page, one fault inside brk itself;
riscv face was pango/FcInit, c127e4bc1 — per-mm authority + per-mm
brk_lock, A/B 3/12 boots hit -> 0/12, f6e3b8837).

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

### 2.3 The `expect`/`unwrap` population is a fail-fast surface, not a defect class

A kernel-wide scan (2026-11-22) counts **286 `expect`/`unwrap` sites in
production code** (tests/ excluded): binder 76, vfs 20, bio 16,
sysv_sem 15, rootfs 11, and the rest in smaller clusters. They are NOT
a defect class, and the review proposal to convert them all to
`match`-plus-error-code would make the kernel worse. They split into
three kinds:

1. **Unreachable invariant assertions** — the majority. Examples:
   `w.as_ref().unwrap()` on the WORLD guard (WORLD is initialized
   before any binder call), `g.get(&k).expect("contains_key")` right
   after a `contains_key` on the same map under the same lock
   (`fs/page_cache.rs`, the map is `entry.lock`-guarded for the whole
   block), `group.iter().min().unwrap()` on a slice the caller just
   proved non-empty (`ipc/binder.rs`), `try_fdtable().unwrap()` right
   after `try_fdtable()` returned Some in the same expression chain.
   In a kernel, an unreachable invariant that stops to panic is the
   correct behaviour: it turns a silent corruption into a loud,
   attributable oops. Converting these to error returns would turn
   "the kernel state is broken" into "one syscall returns -EIO",
   which is strictly harder to diagnose and hides the corruption from
   the bug detector.
2. **Contract assertions on a caller-guaranteed precondition** — the
   same reasoning; the contract is documented at the call site.
3. **Genuinely optional lookups** — a small residue that must return
   an error rather than panic. These are the ones worth changing, and
   each change needs its own case-by-case reading (the map may be
   concurrently mutated, the lookup may be absent by design).

The TOCTOU subset — presence check followed by `get().expect()` with a
mutation window in between — was scanned mechanically across
kernel/src: **2 sites, both in `fs/page_cache.rs`, both inside the
`entry.lock` guard** (the map cannot be mutated concurrently there),
so neither can fire. The residue is therefore an unreachable set, not
a latent panic family.

Rule of thumb recorded here: a panic is a defect when its PRECONDITION
can be broken by user input or by a concurrent actor. It is a
detector when the precondition is an internal invariant. The review
sweep should be re-run against that distinction, not against the
syntactic presence of `expect`.

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
- TLB-shootdown lazy mm_cpumask (x86_64, OH boot lane 2026-11-12):
  every munmap/fork-COW/mprotect/COW-fault used to BROADCAST a
  full-TLB-flush IPI to all 8 CPUs — /proc/interrupts on a pristine
  OH boot counts 4,031,356 shootdown IPIs by t=200 s (~20k/s; each a
  KVM-intercepted LAPIC write plus a remote full flush that destroys
  the remote's working set). The per-mm cpu_mask (set at switch-in
  before the serializing CR3 write, cleared at switch-away where the
  plain CR3 write's full flush makes it sound; grow-only under PCID)
  drops the count to 10,209 — 395x fewer, 99.7% of the shootdown
  traffic gone — with flush semantics unchanged (remotes still
  full-flush; the mask only decides WHO can hold entries). Safety
  pairing: the mask's ordering argument is documented at the field
  (MmStruct::cpu_mask) and the lazy clear is PCID-gated so the
  NOFLUSH regime keeps the conservative grow-only mask.
- fs metadata family (same board): stat 200.9→71.1 µs, openclose
  512.9→156.3 µs, append 657.2→77.4 µs, mkdrmdir 22,814→2,757 µs —
  the D-8/D-12/D-13 write-through jbd2 + batched-publication stack;
  pairing: the same batching removed the sync-write cost class while
  keeping crash-consistency write-through (fsw counters show no new
  sync-write class outside provisioning).
- open/close registry empty-gates + per-CPU file ids (sysmm-2x lane
  2026-11-17, 9b7a9aef3): every regular-file open paid
  LEASE_TABLE.lock+scan (break_lease's conflict probe) and every
  File::drop paid FOUR more global lock pairs + scans (flock release,
  inode-open unregister's lease leg, oasync unregister) with all
  tables empty — the default boot state. Live-entry mirrors
  (FLOCK_COUNT/LEASE_COUNT/OASYNC_COUNT, the RECORD_LOCK_COUNT
  pattern) collapse each to one Acquire load. Safety pairing: the
  0-read vs racing-first-insert soundness argument is documented at
  each site — an insert runs inside flock/fcntl on a held File
  reference, and File::drop runs only after the last reference died,
  so the Arc refcount Release/Acquire chain orders the dropping CPU
  after the inserter's length store; file_id moved to per-CPU counters
  (cpu<<56|local) whose uniqueness argument (registries compare
  equality, never order; a racing same-CPU fetch_add still yields a
  distinct id) is at the field. Measured: lmbench open med 0.931→0.829
  µs (−11%, interleaved 3+3, SMP-8 KVM).
- VMA-range-bounded fork/teardown leaf scans (sysmm-2x lane
  2026-11-17, 1aa932fd4): munmap leaves emptied leaf tables behind
  (unmap_pages never frees them), so a fork-heavy parent carries 36
  leaf tables for 61 present PTEs and both whole-tree walks (fork COW
  copy, mm teardown) scanned all 512 PTEs per table — 18k streamed
  loads per fork+exit, the 20 µs mm_cow + 18 µs drop_ptfree phases of
  the SMP-8 fork decomposition. Both walks now scan only the PTE
  windows the VMA snapshot covers (8-window stack array; no-coverage
  tables and degenerate shapes keep the FULL scan so an orphan-PTE
  invariant break still frees what it finds — the defense is the
  fallback, not the assumption). Measured: forkprof-armed p4_fork_exit
  med 107.4→56.6 µs; the wall-clock row stays dominated by a bimodal
  51-69/90-107 µs child wake-to-run amplitude (sched/wakeup family,
  handed off with the forkprof evidence in run/SYSMM-2X.md).
  p3b_fault_cow unchanged (0.601-0.611 both arms) — the walk change
  does not touch the fault path.
- Emptied-leaf-table free in munmap behind a TLB-flush epoch + pcp
  table allocs (fork-frag lane 2026-11-17, 746678591): the SOURCE of
  the sysmm-2x fragmentation finding closed. munmap now unlinks a leaf
  table whose full 512 entries are raw-zero and frees the frame only
  after every CPU that ever loaded the mm completed a full TLB flush
  taken AFTER the PMD clear (per-CPU TLB_FLUSH_EPOCH incremented at
  the end of the shootdown IPI handler — the fire-and-forget flush
  finally has an ack side; drains at munmap entry/exit, fork and
  exit). The free never outruns the proof: a table frame reaches the
  allocator only with every stale paging-structure-cache holder
  flushed. Walker safety is by the same invariant the PTE lock always
  owned: the fork COW walk and teardown re-read the PMD entry INSIDE
  the per-table chunk (an unlinked table reads invalid, never a
  dangling dereference); ptrace/coredump walks take the target mm's
  PteLock (cold paths). Table allocations now use the per-CPU pageset
  fast path (the data-page discipline; the GFP slowpath stays out of
  the PteLock context). Measured (pbench SMP-8 KVM): p4_fork_exit
  68.6→39.0 µs vs reference 12.9 (5.31x→3.02x), p4_fork_exec
  77.2→44.3 vs 25.7 (3.00x→1.72x), quiet-host single boots; interleaved
  3+3 at host load 6-8: 89.7→49.1 / 66.5→46.6. forkprof: 36.0→7.0
  leaf tables per fork, mm_cow 22→5 µs, drop_ptfree 17→7 µs. Clean
  small-parent shape NOT regressed (lmbench lat_proc fork 46.3→45.2
  interleaved med); p3b_fault_cow 0.601→0.571-0.591. Lever C
  (deferred last-Arc mm drop via mmdropd kthread) measured
  NET-NEGATIVE (p4_fork_exec 44.3→76.0 µs — wake IPI + RCU-grace
  poll per queued mm on busy CPUs) and was reverted; record in
  run/FORK-FRAG.md. Full anatomy: run/FORK-FRAG.md.
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
- Unified-track vDSO (vdso-rearm lane, commit 4c0cabe85, 2026-11-15):
  clock_gettime syscalls per OH boot 8,259,773 → 0 (name-only
  same-rig A/B, tok-r4fix vehicle, KVM q35 SMP2 4G, 780 s;
  −29.4 CPU-s = −2.0% of the box, −26% total syscall volume),
  nanosleep avg and the GRQ lock-site avg flat, zero panics both arms
  — AND the speed came from a correctness construction: the kernel
  clock and the userspace vDSO now read ONE anchor under one seq
  protocol with identical arithmetic (bit-identical tracks — the
  property LTP clock_gettime04's strict adjacent-pair test enforces),
  where the previous fast attempt needed per-CPU correction spread
  and had to be disarmed. The seq word doubles as the writer lock
  (cmpxchg even→odd), eliminating the multi-writer publication tear
  as a class rather than narrowing it; the page is user-RO precisely
  because the kernel trusts its words. LTP clock_gettime04 6/6 incl.
  the vdso variants and both COARSE clocks. Artifacts: vdso-rearm lane
  run/VDSO-REARM.md, run/pa-vdsorearm-{a,b}-serial.log.

- close(2) grace-wait elimination (oc-rc, 2026-11-17): the
  unconditional-increment fd lookup made every close(2) wait out an
  RCU grace period (~250-330 ns) before its close decisions; the
  conditional-acquisition rework runs the decisions at the
  last-strong-reference drop (closer's own context — the strict
  userspace-observable timing is PRESERVED) and frees the ArcInner
  after an asynchronous grace via a parked Weak (call_rcu shape).
  p6_openclose 0.681 -> 0.591 µs, p6_read_cached 1.002 -> 0.912 µs
  (pbench KVM smp8, med-of-3; reference 0.371/0.551). Safety
  pairing: the once-only close op is decided by the refcount
  transition itself (exactly one drop-to-zero — the stale-flag and
  grace-observation hazard classes are structurally gone), the
  reader's failed CAS is EBADF (the reference fget race outcome),
  and the ArcInner layout the CAS depends on is boot-asserted, so a
  toolchain change fails loudly instead of corrupting. riscv64 make
  test 1328/1328. [run/OC-RC.md, lane worktree oc-rc]

- O_DIRECT fresh-write unwritten allocation + completion-time
  conversion, and delalloc reservations (fio-delalloc, 2026-11-18):
  the fresh 1 MiB direct write allocated written extents and
  breaded/zeroed/dirtied every fresh block — synchronous garbage
  reads, 256 dirty cache victims per op, and a cache-resident target
  set that forced the data off the direct path. The reference shape
  (unwritten extents at submit, bios over them, bulk state conversion
  at completion — ext4_map_blocks(PRE_IO) + iomap_dio_complete ->
  ext4_convert_unwritten_io_end_vec) took the fresh row from 3429 to
  4000-4800 MiB/s, EQUAL to the overwrite row (fio KVM smp8 med-of-3,
  same-window interleaved vs main). Safety pairing: conversion now
  runs AFTER the data is device-acked — data-before-metadata ordering,
  so the crash window between data and extent-state flip exposes
  unwritten extents (readers see zeros) instead of preallocation
  stale bytes (the strictly safer exposure; the old shape documented
  the stale-bytes window as the NJ stance). The delalloc reservation
  skeleton moves buffered-write ENOSPC to write(2)
  (ext4_da_reserve_space shape): concurrent writers on a nearly full
  fs can no longer race past the zero-free gate and strand
  unflushable dirty pages — the dirty-balloon bug class the
  falloc-wedge lane documented is refused at the reservation gate,
  with the file consistent (probe battery T9: partial land, size ==
  landed, read-back clean). Gate discipline caught the skeleton's own
  representation costs: pbench boards measured p6_append 4.9 -> 27+
  µs (O-files-ever-written total scan) and +24% (per-block BTreeSet)
  before merged runs + an exact counter brought it to +10.9%
  in-band — the boards gate what the functional battery cannot.
  37/37 probe battery, fio verify=md5 clean (direct + buffered),
  riscv make test 1333/1333. [lane worktree fio-delalloc
  run/FIO-DELALLOC.md]

## 4. Nothing traded away
The compatibility checklist stands while both ledgers hold: 345+
syscalls with reference-matched semantics; LTP 1,059/1,869 and
climbing (35→524→801→945→1,005→1,059); unmodified Ubuntu 22.04 to a
GNOME Shell desktop; unmodified OpenHarmony with 135+ services; three
architectures from one tree. Speed did not cost compatibility; safety
did not cost speed.


### Primitive INV index (the literal registry — gate-verified)

Every `INV:<id>` artifact row in the sidecar must resolve here. One
line per id; the contract lives in the owning module.

- **INV-BLKDEV-1** — GenDisk registry + driver ABI (drivers/blkdev, L5)
- **INV-CBYTES-1** — CopyBytes byte-image family (kernel/src/safety/bytes.rs)
- **INV-CBYTES-2** — CopyBytes byte-image family (kernel/src/safety/bytes.rs)
- **INV-CBYTES-4** — pinned-frame window (kernel/src/safety/bytes.rs)
- **INV-CBYTES-5** — advisory volatile store (kernel/src/safety/bytes.rs)
- **INV-CBYTES-6** — pinned-frame run window (kernel/src/safety/bytes.rs)
- **INV-DMA-1** — DmaBlock device-shared carve (kernel/src/safety/dma.rs)
- **INV-DMA-2** — DmaBlock device-shared carve (kernel/src/safety/dma.rs)
- **INV-DMA-3** — DmaBlock device-shared carve (kernel/src/safety/dma.rs)
- **INV-FOR-1** — hunt-era forensics edges (kernel/src/dfx/forensics_edges.rs, L5)
- **INV-FOR-2** — hunt-era forensics edges (kernel/src/dfx/forensics_edges.rs, L5)
- **INV-FOR-3** — hunt-era forensics edges (kernel/src/dfx/forensics_edges.rs, L5)
- **INV-KPTR-1** — KPtr owned allocation (kernel/src/safety/ptr.rs)
- **INV-KPTR-2** — KPtr owned allocation (kernel/src/safety/ptr.rs)
- **INV-KPTR-3** — KPtr owned allocation (kernel/src/safety/ptr.rs)
- **INV-KPTR-4** — KPtr owned allocation (kernel/src/safety/ptr.rs)
- **INV-KPTR-5** — KPtr owned allocation (kernel/src/safety/ptr.rs)
- **INV-LOCK-5** — primitive contract (see owning module)
- **INV-LOCK-7** — primitive contract (see owning module)
- **INV-MMIO-1** — MmioRegion register windows (kernel/src/safety/mmio.rs)
- **INV-MMIO-2** — MmioRegion register windows (kernel/src/safety/mmio.rs)
- **INV-MMIO-3** — MmioRegion register windows (kernel/src/safety/mmio.rs)
- **INV-NET-ARCREF-1** — net framework edges: Arc-refcount probe + audited deref (net/net_edges.rs, L1)
- **INV-NET-CONTAINEROF-1** — net framework edges: field-ptr container_of (net/net_edges.rs, L1)
- **INV-NET-HDRVIEW-1** — net framework edges: POD header views, alignment-checked (net/net_edges.rs, L1)
- **INV-NET-SOCKTAB-1** — net framework edges: static-mut table ref formation (net/net_edges.rs, L1)
- **INV-NETARC-1** — socket-file Arc recovery (net/mod.rs, L5)
- **INV-PCP-1** — primitive contract (see owning module)
- **INV-RING-1** — SpscRing lock-free ring (kernel/src/safety/ring.rs)
- **INV-RING-3** — SpscRing lock-free ring (kernel/src/safety/ring.rs)
- **INV-SAFECALL-1** — safe_call discipline (kernel/src/safety/call.rs)
- **INV-SAFECAST-1** — primitive contract (see owning module)
- **INV-SCE-1** — sched framework edges (kernel/src/sched/edges.rs, L1)
- **INV-SCE-2** — sched framework edges (kernel/src/sched/edges.rs, L1)
- **INV-SCE-3** — sched framework edges (kernel/src/sched/edges.rs, L1)
- **INV-SCE-4** — sched framework edges (kernel/src/sched/edges.rs, L1)
- **INV-SCE-5** — sched framework edges (kernel/src/sched/edges.rs, L1)
- **INV-SCE-6** — sched framework edges (kernel/src/sched/edges.rs, L1)
- **INV-SCE-7** — sched framework edges (kernel/src/sched/edges.rs, L1)
- **INV-SCE-8** — sched framework edges (kernel/src/sched/edges.rs, L1)
- **INV-SCE-9** — sched framework edges (kernel/src/sched/edges.rs, L1)
- **INV-SLOT-1** — OnceSlot publish-once singletons (kernel/src/safety/slot.rs)
- **INV-SREF-1** — StaticRef machine-lifetime storage (kernel/src/safety/ptr.rs)
- **INV-SY-1** — sync framework edges (kernel/src/sync/edges.rs, L1)
- **INV-SY-3** — sync framework edges (kernel/src/sync/edges.rs, L1)
- **INV-UACCESS-1** — UserPtr family (arch/uaccess_safe.rs)
- **INV-WINDOW-1** — KernelStackWindow stack reads (kernel/src/safety/frame.rs)

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
- Data-frame free vs shootdown completion (found by the fork-frag
  lane 2026-11-17 while building the stricter table-free protocol):
  unmap_pages frees DATA frames inside the PTE lock, BEFORE the
  fire-and-forget flush_tlb_others_mm IPIs complete — a remote CPU of
  a MULTI-THREADED mm holding a stale writable TLB entry can touch a
  reused frame until its flush lands. Single-threaded mms are sound
  (mask = self, local flush covers); the table-free protocol now
  carries the proof the data path lacks (TLB_FLUSH_EPOCH acks). The
  reference batches data frees behind completed shootdowns
  (mmu_gather). Closing shape: route order-0 data frees from
  same-mm shootdown paths through the epoch list (or wait-variant
  shootdowns) for CLONE_VM mms; owner: mm lane, evidence
  run/FORK-FRAG.md §gaps.
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

### 5.1 The irreducibility register (D-round ledger, round 5 — 2026-11-20)

The per-block proofs the <=5,000-line plan requires: for every unsafe
family that survived the round-5 conversion, what it reads, why no safe
shape exists, the reference kernel's counterpart, and the containment.
Owners are named files; the blocks live there and nowhere else.
Territory context: lane L5 converted drivers/dfx/interrupt/net from
5,036 dual lines to ~1,900; what follows is what could NOT convert and
why, family by family.

**F1. Stale-handle task forensics** — owner
`kernel/src/dfx/forensics_edges.rs` (INV-FOR-1/2/3) + the hunt modules'
walkers (scribble 192L, leakorigin 190L, r83hunt 107L, treehunt 78L,
wildwatch 190L, softirq's bracket-leak, preempt's orphan/pair scans).
*Reads:* `*mut Task` handles captured one tick ago by a census or IRQ
record and read NOW — the task may have exited and freed in between;
kernel-stack qwords via captured frame pointers; seed words at guarded
pages. *Why no safe shape exists:* the diagnostic exists BECAUSE a typed
invariant is suspected broken; its input is a handle whose liveness no
type can prove — a `TaskRef` acquisition would either resurrect the
liveness claim (unsound) or refuse (and the hunt reports nothing). The
reference's equivalents do the same: `sched_show_task` dereferences
tasks under `tasklist_lock` without refcounts, the hung-task checker's
`do_each_thread` reads, KASAN's quarantine walks. *Containment:* every
read is <=16 bytes, volatile, value-only (never re-dereferenced);
callers gated by their `rux:` cmdline flag / tracepoint, all default-OFF
in release; zero cost when disarmed (flag check only).

**F2. backtrace (139L)** — owner `kernel/src/dfx/backtrace.rs`. *Reads:*
the caller's own frame pointer / return address (asm: `mv s0`, `mov
rbp`, `ldr x29`), register save blocks, CSR dumps. *Why:* the frame
chain and register file have no Rust-level representation at all — this
IS the machine state, and the register-read asm is the only instruction
sequence that can capture it (the module documents the rbx-clobber
compiler hazard on the x86 side). *Reference:* `save_stack_trace` /
`unwind_frame` — the same asm. *Containment:* panic/oops paths and
explicit `dump_stack()` sites only; never on a measured hot path.

**F3. memwatch fp-chain walks (101L, 3 arch twins)** — owner
`kernel/src/dfx/memwatch.rs::walk_fp_chain`. *Reads:* frame-pointer
chains through ARBITRARY tasks' kernel stacks mid-execution. *Why:* the
chain is self-describing only in memory — each `fp` word names the next
frame; a wrong or racing value yields garbage the walk must tolerate,
not a type error. `KernelStackWindow` (round-4D) bounds each read to
the task's stack region — the remaining rawness is the chain-follow
itself. *Reference:* `save_stack_trace_tsk` under `try_to_unuse`-style
best-effort discipline. *Containment:* `rux:memwatch` gated, cold path.

**F4. pstore (60L)** — owner `kernel/src/dfx/pstore.rs`. *Reads/writes:*
the reserved-RAM record header at a fixed physical window, EARLY in boot
(before the allocator exists) and at panic time (when locks are
unusable). *Why:* both ends run outside every subsystem — pre-heap boot
and the panic path cannot take a typed handle that heap-allocates or
locks. *Reference:* ramoops' `persistent_ram_zone` postcard headers.
*Containment:* boot/panic-only, fixed window, header-validated.

**F5. asm register/MSR/IO-port edges** — owners
`kernel/src/drivers/intc/apic.rs` (rdmsr/wrmsr/inb/outb, KVM-pv hc),
`gicv3.rs` (icc_* system registers), `clint.rs` (rdtime, sbi ecall),
`taskdump.rs` (frozen-CPU gs: probe), `backtrace.rs` (F2). *Why:* the
instruction IS the interface (MSR/CSR/port access has no Rust
representation); the compiler must be told the exact register
constraints. *Reference:* the whole arch layer. *Containment:* each is
a named fn wrapping one instruction; call sites are safe.

**F6. intrusive tasklet list** — owner
`kernel/src/interrupt/tasklet.rs` (`splice_all` + `drain_tasklet_list`,
the two audited edges after round-5 deduped the twin handlers). *Reads:*
`TaskletStruct::list` nodes embedded in driver-owned structures,
linked through raw `ListHead`s; the walk recovers containers by
`offset_of!` (container_of). *Why:* the intrusive shape is the
interface (drivers embed the node; no allocator indirection on the BH
path); a safe iterator over raw-linked nodes cannot prove the
container liveness the links themselves encode. *Reference:*
`tasklet_action`'s `list_for_each` + `from_tasklet`. *Containment:*
per-CPU lists under irqsave locks; the splice+walk edges are the only
two blocks; the rest of the file is safe.

**F7. virtio ring/DMA bottoms** — owner `kernel/src/drivers/virtio/`
(the 3a-landed descriptor-window discipline; ~250L) +
`safety/dma.rs`/`safety/ring.rs`. *Reads:* descriptor tables, avail/used
rings the DEVICE writes concurrently (volatile, ordered by fences);
io_buf pool chunks whose first 8 bytes are reinterpreted as a freelist
link while parked. *Why:* the device is a concurrent writer the type
system cannot see; the pool link overlays the buffer by design (the
allocation is metadata-free). *Reference:* virtio-ring's `vring.h`
macros. *Containment:* INV-tagged per edge (3a's contract), lock-order
doc, per-slot locks.

**F8. net staged copies + whole-table views** — owners
`kernel/src/net/unix.rs` (write_user staged copy; park_audit = F1
class), `tcp.rs`/`udp.rs`/`route.rs` whole-table `&'static mut` views.
*Why (staged copies):* `set_len` on a pre-sized Vec filled by an
exception-table copy is the one-shape zero-copy path; the safe API
covers the common case, this is the high-throughput edge. *Why (table
views):* the `&'static mut` hand-out predates the round; threading
TCP_TABLE_LOCK guards through every caller is a hot-path change that
must ride the perf gate, not a mechanical conversion — REGISTERED AS
OWED (the witness pattern landed this round for the manager accessors;
the table accessors follow next round). *Reference:* `local_bh_disable`-
style discipline on the C side. *Containment:* documented contracts per
accessor.

**F9. GEM aperture + alloc edges (drm ~65L)** — owner
`kernel/src/drivers/drm/mod.rs`. *Reads:* composer GEM buffers via
`gem.kva` word walks (import verification sampling); two raw
alloc/dealloc edges. *Why:* the aperture is device-shared memory sized
by the host display protocol (not a Rust object); the alloc edge pairs
with a Drop that releases the host resource. *Reference:* GEM object
pages. *Containment:* DRM ioctl paths only; the walks are clamped.

**F10. oops-path fault probes** — owner `kernel/src/dfx/mod.rs`
(register_debugfs_knobs: read of a poisoned const, `csrr x0`, `ud2`).
*Why:* these edges exist to FAULT — deliberate fault-injection knobs
exercising the oops/trap path; "safety" here means the trap, not the
absence of UB at the probe. *Reference:* Linux's `BUG()`/`BUG_ON`
selftests. *Containment:* debugfs-gated, boot-diagnostic only.

**F11. buffer-cache intrusive protocol** — owner
`kernel/src/fs/bio.rs`. *Reads/writes:* the hash chains and LRU list
that link `CacheEntry` nodes by raw pointers (move/push/remove_lru,
evict_one's four-phase pick/unhash/unlink/free, get()'s insertion
relink). *Why no safe shape exists (this round):* the cache is ONE
allocator-lifetime structure — entries are Boxed once, linked into two
intrusive lists, and freed exactly when both unlinked and count==0; the
eviction protocol's correctness IS the ordering of those raw relinks
(R7-C1's orphaned-tail race, R11-1's presence-is-authoritative fix —
both landed as raw-link-order fixes with panic evidence). A typed
redesign (entry handles + link methods, tasklet's splice_all shape)
is the right end state but changes the hot metadata path of the
data-integrity domain: it must land WITH the fio --verify battery and
pbench fs rows in band, not inside a conversion lane. Registered as
the next measured step; the protocol blocks stay behind INV-BIO-ENTRY/
INV-BIO-BH/INV-BIO-PIN with their history comments.
*Containment:* all link ops run under the bucket/LRU locks; the
pinned-handle family (`bh_pinned`, one edge) fronts every caller-facing
deref; BLOCK_CACHE is an OnceSlot now (INV-SLOT-1/2).
*Reference:* `buffer_head`'s `b_hash`/`b_assoc_map` intrusive links —
the same C shape with no type discipline at all.

**F12. boot provenance (init.rs)** — owner `kernel/src/init.rs`.
*Reads/writes:* PID 1's task constructed IN a static storage slot
(INIT_TASK_STORAGE) before the allocator, scheduler, or any typed
lifecycle exists; the address-space switch into the fresh user root;
the handoff of the child's pt_regs frame. *Why no safe shape exists:*
there is nothing to borrow FROM yet — no fdtable, no pid hash, no
scheduler; every typed handle in the kernel presumes a state this code
is CREATING (the same reason main.rs's boot provenance is F-family).
*Reference:* `start_kernel`/`rest_init` — C has no shapes to violate.
*Containment:* runs once, BSP, IRQs off; the ELF parse itself rides
CopyBytes (from_bytes is safe since round-5 batch 9).

**F13. page-cache frame fills + intrusive hash chains** — owner
`kernel/src/fs/page_cache.rs`. Round-5 split: the nine copy-into-frame
sites (insert/insert_ext/insert_batch_ext/insert_dirty_user fills and
revives, writeback partial spans, the DIO user->frame copies) are now
SAFE calls over FOUR named edges — fill_page_from / copy_page_page /
copy_into_page / copy_user_into_page (INV-PC-PAGECOPY: the frame is
this call's private zone allocation and the span stays inside
PAGE_SIZE; the user variant rides the exception-table primitive).
What stays raw: (a) the bucket chains' `(*p).key` walks and the
Arc::from_raw revival in get/claim — the same intrusive-cache protocol
class as F11's bio (lock-scoped relinks; the Arc discipline IS the
table's ownership), (b) shared_file_fault's zero-fill + install (the
fault path fills a bounce frame then publishes under the table lock —
one coherent publish edge), (c) the &*p entry adoptions in the walk
helpers (INV-PC-CLAIM's claim protocol). *Reference:* the page cache's
radix/XArray slots + page->mapping — C's whole-tree equivalent.
*Containment:* all under the per-inode table locks; the copy edges are
the only writers before publication.

**F14. console TX asm stores + x86 port-IO** — owner
`kernel/src/console.rs`. *What stays raw:* the riscv `sb`/aarch64 `str`
asm stores in putc/puts_batch (and uart_data_ready's `ldr`) — deliberate
per-byte MMIO store instructions with `nostack, nomem`, kept because the
console row is its own measured fleet (the riscv per-byte batch comment);
the x86 8250 `outb`/`inb` register pair and the `rep outsb` string-IO
burst (port-IO has no MmioRegion shape — the instruction IS the only
access path). *Why no safe shape exists:* port-IO instructions have no
typed region primitive; the asm stores pre-date and deliberately bypass
the region API for the measured TX path. *Reference:* Linux 8250's
`outb`/`iowrite8` pairs. *Containment:* console-only, TX path, called
under the printk lock or early-boot single-CPU. Everything else in the
file rides MmioRegion (INV-MMIO-1/2/3): read_reg/write_reg (MMIO
arches), the PL011 FR/DR/ICR/IMSC reads+writes and getchar's pre-arm
DR poll (UR-L4b conversion).

**F15. intrusive ListHead primitive** — owner `kernel/src/list.rs`
(the Linux `list_head` port: task tree, runqueue, device lists — 10
users). *Shape:* the unsafe fns ARE the primitive's surface (add/del/
move/for_each over embedded raw links); each carries its per-method
Safety contract. *Why no safe shape exists:* the links live inside
arbitrary embedding types and the list protocol's invariants (unlink
ordering under the owner's lock) are exactly what the callers enforce
per use site — a container_of/typed-handle redesign is the same class
as F11/F13's intrusive caches (their registry rows explain why that
lands with its own measurement, not in a conversion lane).
*Reference:* Linux `include/linux/list.h` — the identical shape with
zero contracts. *Containment:* never exported past kernel/; callers
hold the embedding structure's lock.

**F16. SBI ecall + boot stack asm** — owners `kernel/src/sbi.rs`,
`kernel/src/main.rs`. *What stays raw:* the two SBI ecall asm blocks
(system-reset, send-IPI) — the `ecall` instruction is the ONLY path to
the RISC-V firmware ABI; and main.rs's x86_64 boot stack-switch `asm!`
(the context-entry primitive; see boot_edges.rs's module doc — F12's
sibling). *Why no safe shape exists:* firmware-call and stack-switch
instructions have no Rust-level abstraction in-tree; wrapping them in
a "safe" fn would only hide the edge, not type it.
*Reference:* Linux's `sbi_ecall` asm + `cpu_startup_entry` handoff.
*Containment:* sbi.rs riscv-only, reset/IPI only; main.rs runs once at
boot entry.

**F17. binder buffer block-views** — owner `kernel/src/ipc/binder.rs`.
*Shape:* r3c's consolidated view primitives — INV-BINDER-BLOCKVIEW
(the ONE raw window over the mmap'd binder buffer: bounds-checked
slice/word/POD views), INV-BINDER-STAGE-POD (bytewise view of Copy
values), INV-BINDER-HELD (token-checked accessors) — plus the narrow
call sites beneath them. Every access goes through the bounds-checked
view methods; no raw `.add()` escapes the primitive.
*Why no safe shape exists:* the binder buffer is one mmap'd arena
shared with user binder threads, sized by the protocol — the same
device-window relationship as MmioRegion, but with the r3c view API
already measured in place (SLOW-TXN families are gated on it).
*Reference:* Android's flat_binder_object walks over the same arena.
*Containment:* ipc/binder.rs only; world lock discipline per the
guarded-domain notes.
**F18. mm-forensics page-descriptor walkers** — owners
`kernel/src/dfx/mmforensics.rs` (note_ref, capture_frame), 
`kernel/src/dfx/ledger.rs` (the census walks), and
`kernel/src/dfx/pathstash.rs` (per-tid static slot table).
*Reads:* raw `*const Page` descriptors across the whole mapped vmemmap
span (refcount/flags/type/mapping/index — value-only, never
re-dereferenced), exactly like F1's stale-task reads but for PAGE
descriptors; pathstash's static SLOTS are written per-tid exclusively
(publication ordered via tid/len atomics — the slowtask-SLOTS
discipline twin). *Why no safe shape exists:* these instruments exist
to report on descriptors whose invariants are SUSPECTED broken (the
wild-write/fake-OOM hunt lines) — a typed handle would presume the
invariant under investigation. *Reference:* Linux's
`dump_page`/page-type census walkers (the same untyped descriptor
reads). *Containment:* gated by rux:mmforensics / the census flag,
default-off, value-only reads <= one descriptor at a time.

Also under this row's neighborhood (already-per-site-documented
residue): taskdump's %gs:PC_MAGIC read is F5-class (asm register
edge); its CURRENT_PT_REGS frame read is the one-frame-edge
discipline; slowtask's `audit` call is the single-auditor CAS
contract; oops.rs's putchar_no_lock is the panic-path console twin
(F4-adjacent).

# INV index — code-resident contracts (Round D step 1)


Every INV id below is DEFINED at its owner site in kernel/src (the
comment carries the full contract; this index is the gate-resolvable
registry so `INV:<id>` artifacts verify). Round-D retags cite these
ids; the full text lives at the owner line.


| INV id | owner file | contract (headline) |
|---|---|---|
| INV-AE-COPY-1 | `arch/aarch64/mm/unsafe_edges.rs` | Copy one page between two linear-map addresses (INV-AE-COPY-1): both |
| INV-AE-PT-1 | `arch/aarch64/mm/unsafe_edges.rs` | ! - **INV-AE-PT-1 (liveness)** — [`PtTable`] is only constructed from a |
| INV-AE-PT-2 | `arch/aarch64/mm/unsafe_edges.rs` | ! - **INV-AE-PT-2 (bounds)** — accessors take any `usize` index and |
| INV-AE-PT-3 | `arch/aarch64/mm/unsafe_edges.rs` | ! - **INV-AE-PT-3 (aliasing)** — tables are shared across CPUs; `get` |
| INV-AE-PT-4 | `arch/aarch64/mm/unsafe_edges.rs` | ! - **INV-AE-PT-4 (stage discipline)** — construction routes through |
| INV-BLKDEV-3 | `drivers/blkdev/mod.rs` | audited edge for the driver-private slot (INV-BLKDEV-3). |
| INV-BOOT-PANICBUF-1 | `boot_edges.rs` | INV-BOOT-PANICBUF-1: the single-panicking-CPU path — the UART line |
| INV-BOOT-RAWFRAMES-1 | `boot_edges.rs` | INV-BOOT-RAWFRAMES-1: diagnostic read-only walk of the current |
| INV-BOOT-REGIONS-1 | `main.rs` | module (INV-BOOT-REGIONS-1). |
| INV-BOOT-SETUP-1 | `boot_edges.rs` | INV-BOOT-SETUP-1: `rust_main` runs this exactly once, before any |
| INV-BOOT-SYM-1 | `boot_edges.rs` | INV-BOOT-SYM-1: only the symbol's ADDRESS is read — the linker |
| INV-BOOT-ZEROPAGE-1 | `main.rs` | (INV-BOOT-ZEROPAGE-1 in the framework module). |
| INV-BUDDY-1 | `mm/buddy_allocator.rs` | ! The same INV-BUDDY-1 through INV-BUDDY-5 invariants from `zone.rs` apply, |
| INV-BUDDY-2 | `mm/zone.rs` | ! - **INV-BUDDY-2**: A page's `private` order field matches the order of the |
| INV-BUDDY-3 | `mm/zone.rs` | ! - **INV-BUDDY-3**: Each order's free list forms a valid singly-linked list |
| INV-BUDDY-4 | `mm/zone.rs` | ! - **INV-BUDDY-4**: `free_count[order]` matches the actual list length for |
| INV-BUDDY-5 | `mm/buddy_allocator.rs` | ! The same INV-BUDDY-1 through INV-BUDDY-5 invariants from `zone.rs` apply, |
| INV-COW-1 | `arch/aarch64/mm/page_fault.rs` | deferred through COW per INV-COW-1); otherwise the access is a real |
| INV-COW-2 | `arch/riscv64/mm/mm_ops.rs` | ! - **INV-COW-2**: When the COW bit is set, the page's `_refcount >= 2` |
| INV-COW-3 | `arch/aarch64/mm/mm_ops.rs` | Handle copy-on-write page fault (INV-COW-3/4; twin contract): |
| INV-COW-4 | `arch/aarch64/mm/mm_ops.rs` | Shared: install the private copy (INV-COW-4). The page came from |
| INV-COW-5 | `arch/aarch64/mm/mm_ops.rs` | INV-COW-5: downgrade BOTH parent and |
| INV-CS-1 | `arch/riscv64/context.rs` | ! - **INV-CS-1**: `__switch_to` saves and restores all callee-saved registers |
| INV-CS-2 | `arch/riscv64/context.rs` | ! - **INV-CS-2**: FPU state (if `Fs != Off`) is saved/restored via |
| INV-CS-3 | `arch/riscv64/context.rs` | ! - **INV-CS-3**: The MMU (satp) is switched via `switch_mm()` **before** |
| INV-CS-4 | `arch/riscv64/context.rs` | ! - **INV-CS-4**: `__switch_to` does NOT enable SIE (interrupts). The caller |
| INV-CS-5 | `arch/riscv64/context.rs` | ! - **INV-CS-5**: After `__switch_to`, the previous task's stack pointer |
| INV-DFXWALK-1 | `dfx/mod.rs` | for the dfx family; INV-DFXWALK-1: `root` is the diagnosed task's own |
| INV-EXT4-SBRES-1 | `fs/ext4/file.rs` | The cached Ext4Inode box behind inode.sb (INV-EXT4-SBRES-1), |
| INV-FDT-1 | `cmdline.rs` | (INV-FDT-1): boot.S maps the bootloader-provided DTB blob at its |
| INV-FRAME-1 | `ipc/binder.rs` | INV-FRAME-1 typed descriptor (out-of-range -> None). |
| INV-FS-FILEPRIV-1 | `io_uring/mod.rs` | (the cell discipline's close-path take, INV-FS-FILEPRIV-1), then we |
| INV-FS-GLOBALSB-1 | `printk.rs` | GLOBAL) — resolved through the framework edge (INV-FS-GLOBALSB-1 |
| INV-FS-PRIVDATA-1 | `fs/unsafe_edges.rs` | INV-FS-PRIVDATA-1: `inode.private_data`, when `Some(p)`, is a raw |
| INV-HEAP-1 | `dfx/metawatch.rs` | ! cross-checks INV-HEAP-1/2/3. Pass 1 is lock-free (transient |
| INV-HEAP-2 | `mm/buddy_allocator.rs` | ! - **INV-HEAP-2**: `free_lists[order]` is either `EMPTY_LIST` or a valid |
| INV-HEAP-3 | `mm/buddy_allocator.rs` | ! - **INV-HEAP-3** (session 5, wedge fix): the per-page FREE bit lives in |
| INV-IOURING-RING-1 | `io_uring/mod.rs` | Write SQ ring header (INV-IOURING-RING-1: the region is in hand, |
| INV-LIST-1 | `list.rs` | ! INV-LIST-1 (F15-registered primitive, safety-and-performance.md): |
| INV-MEMFILE-1 | `fs/vfs.rs` | INV-MEMFILE-1 (unsafe-slim r3): open_mem_file stores an OWNED |
| INV-PCP-2 | `mm/mod.rs` | (INV-PCP-1), zone-counter parity (INV-PCP-2) and irq-off same-CPU |
| INV-PCP-3 | `mm/mod.rs` | exclusivity (INV-PCP-3); see that module's header for the invariants. |
| INV-PROC-ACTIVEMM-1 | `process/unsafe_edges.rs` | INV-PROC-ACTIVEMM-1: `mm_ptr` names an AddressSpace whose owning |
| INV-PROC-APPPARK-1 | `process/unsafe_edges.rs` | INV-PROC-APPPARK-1: `t` is the current, still-live dying task (the |
| INV-PROC-BORROW-1 | `process/ptrace.rs` | task is current (INV-PROC-BORROW-1's current-task arm). |
| INV-PROC-COREDUMP-1 | `process/unsafe_edges.rs` | ! * [`dump_read_page`] — INV-PROC-COREDUMP-1: page-table walk + one |
| INV-PROC-DIAG-1 | `process/unsafe_edges.rs` | INV-PROC-DIAG-1: bounded, read-only forensics of a possibly-racing |
| INV-PROC-EXEC-COPY-1 | `process/unsafe_edges.rs` | INV-PROC-EXEC-COPY-1: `root_ppn` is a valid user page-table root of |
| INV-PROC-EXEC-ELF-1 | `process/unsafe_edges.rs` | INV-PROC-EXEC-ELF-1: `ehdr`/`data` come from a validated ELF image |
| INV-PROC-EXEC-FREETREE-1 | `process/unsafe_edges.rs` | INV-PROC-EXEC-FREETREE-1: `root_ppn` was allocated by |
| INV-PROC-EXEC-MAP-1 | `process/unsafe_edges.rs` | INV-PROC-EXEC-MAP-1: `root` is a freshly allocated page-table root |
| INV-PROC-EXEC-MMNEW-1 | `process/unsafe_edges.rs` | INV-PROC-EXEC-MMNEW-1: `user_ppn` is a freshly allocated page-table |
| INV-PROC-EXEC-PROBE-1 | `process/unsafe_edges.rs` | INV-PROC-EXEC-PROBE-1: `root` is the loader's fresh tree and `va` a |
| INV-PROC-EXEC-PTE-1 | `process/unsafe_edges.rs` | INV-PROC-EXEC-PTE-1: the walk descends the fresh tree's own tables |
| INV-PROC-EXEC-SWITCH-1 | `process/unsafe_edges.rs` | INV-PROC-EXEC-SWITCH-1: `user_ppn` is the new image's page-table |
| INV-PROC-EXEC-TRAMP-1 | `process/unsafe_edges.rs` | INV-PROC-EXEC-TRAMP-1: `kva` is the linear-map address of a frame |
| INV-PROC-EXEC-VDSO-1 | `process/unsafe_edges.rs` | INV-PROC-EXEC-VDSO-1: `root` is the freshly created page-table root; |
| INV-PROC-FORK-COW-1 | `process/unsafe_edges.rs` | INV-PROC-FORK-COW-1: child_root is the child's fresh root ppn; va is |
| INV-PROC-FORK-FPU-1 | `process/unsafe_edges.rs` | INV-PROC-FORK-FPU-1: called on the live current task in syscall |
| INV-PROC-FORK-TIDWRITE-1 | `process/unsafe_edges.rs` | INV-PROC-FORK-TIDWRITE-1: `kva` is the linear-map address of the |
| INV-PROC-FORK-WALK-1 | `process/unsafe_edges.rs` | INV-PROC-FORK-WALK-1: `root` is the child's fresh root ppn and `va` |
| INV-PROC-FRAME-1 | `process/ptrace.rs` | (INV-PROC-FRAME-1's read arm). |
| INV-PROC-JOURNAL-1 | `process/unsafe_edges.rs` | INV-PROC-JOURNAL-1: the slot is written only by the task itself |
| INV-PROC-KSTACK-1 | `process/unsafe_edges.rs` | INV-PROC-KSTACK-1: `entry` is a dead stack bottom previously linked |
| INV-PROC-KSTACK-2 | `process/unsafe_edges.rs` | INV-PROC-KSTACK-2: `layout` is a valid nonzero layout the caller |
| INV-PROC-KSTACK-3 | `process/unsafe_edges.rs` | INV-PROC-KSTACK-3: `base + off` stays inside the caller's live |
| INV-PROC-LIST-1 | `process/unsafe_edges.rs` | INV-PROC-LIST-1: the caller holds PROCESS_TREE_LOCK (or owns both |
| INV-PROC-NSFILE-1 | `process/unsafe_edges.rs` | INV-PROC-NSFILE-1: the slot was populated by this file type's |
| INV-PROC-NSFILE-2 | `process/unsafe_edges.rs` | INV-PROC-NSFILE-2: the exactly-once reclaim edge for the close path |
| INV-PROC-PICKQWORD-1 | `process/unsafe_edges.rs` | INV-PROC-PICKQWORD-1: `addr` is the aligned address of the task's |
| INV-PROC-PIDHASH-1 | `process/unsafe_edges.rs` | INV-PROC-PIDHASH-1: the caller holds the bucket's irqsave lock (the |
| INV-PROC-PLACE-1 | `process/unsafe_edges.rs` | ! * [`place_task`] — INV-PROC-PLACE-1: placement-new of a whole `Task` |
| INV-PROC-PLACE-2 | `process/unsafe_edges.rs` | INV-PROC-PLACE-2 (self-reference rule, added by the slim4-fix lane |
| INV-PROC-PTRACE-GREG-1 | `process/ptrace.rs` | (INV-PROC-PTRACE-GREG-1/MEM-1). |
| INV-PROC-PTRACE-ICOH-1 | `process/unsafe_edges.rs` | INV-PROC-PTRACE-ICOH-1: the standard per-arch text-write coherence |
| INV-PROC-PTRACE-MEM-1 | `process/ptrace.rs` | (INV-PROC-PTRACE-MEM-1). |
| INV-PROC-PTRACE-WALK-1 | `process/unsafe_edges.rs` | INV-PROC-PTRACE-WALK-1: `mm` is the pinned target mm whose root the |
| INV-PROC-PTRD-1 | `process/unsafe_edges.rs` | INV-PROC-PTRD-1: `pr` is a shared read view of a live task's own |
| INV-PROC-PTREGS-FREE-1 | `process/unsafe_edges.rs` | INV-PROC-PTREGS-FREE-1: the pointer came from the fork path's |
| INV-PROC-ROOTSWITCH-1 | `process/unsafe_edges.rs` | INV-PROC-ROOTSWITCH-1: reads the boot-time root-table constant and, |
| INV-RE-COPY-1 | `arch/riscv64/mm/unsafe_edges.rs` | Copy one page between two linear-map addresses (INV-RE-COPY-1): both |
| INV-REF-1 | `mm/page_desc.rs` | ! - **INV-REF-1**: `_refcount` must never be negative. |
| INV-REF-2 | `mm/page_desc.rs` | ! - **INV-REF-2**: `_refcount == 0` ⟺ page is on a buddy free list (or otherwise free). |
| INV-REF-3 | `mm/page_desc.rs` | ! - **INV-REF-3**: `_refcount > 0` ⟺ page is in use by at least one owner. |
| INV-REF-4 | `mm/page_desc.rs` | ! - **INV-REF-4**: `_mapcount == -1` (PAGE_MAPCOUNT_BIAS) ⟺ page is not mapped |
| INV-REF-5 | `mm/page_desc.rs` | ! - **INV-REF-5**: `_mapcount > -1` ⟺ page is mapped in `(_mapcount + 1)` page tables. |
| INV-REF-6 | `mm/page_desc.rs` | ! - **INV-REF-6**: If `Cow` flag is set ⟹ `_refcount >= 2` AND the PTE `W` bit is clear. |
| INV-RE-FRAME-1 | `arch/riscv64/unsafe_edges.rs` | ! - **INV-RE-FRAME-1 (origin)** — [`SavedFrame`] is constructed only |
| INV-RE-FRAME-2 | `arch/riscv64/unsafe_edges.rs` | ! - **INV-RE-FRAME-2 (liveness)** — the frame lives on the interrupted |
| INV-RE-FRAME-3 | `arch/riscv64/unsafe_edges.rs` | ! - **INV-RE-FRAME-3 (aliasing)** — exactly one &mut frame handle |
| INV-RE-PT-1 | `arch/riscv64/mm/unsafe_edges.rs` | ! - **INV-RE-PT-1 (liveness)** — [`PtTable`] is only constructed from a |
| INV-RE-PT-2 | `arch/riscv64/mm/unsafe_edges.rs` | ! - **INV-RE-PT-2 (bounds)** — accessors take any `usize` index and |
| INV-RE-PT-3 | `arch/riscv64/mm/unsafe_edges.rs` | ! - **INV-RE-PT-3 (aliasing)** — tables are shared across harts; `get` |
| INV-RE-PT-4 | `arch/riscv64/mm/unsafe_edges.rs` | ! - **INV-RE-PT-4 (stage discipline)** — construction routes through |
| INV-SCE-10 | `sched/sched.rs` | PER_CPU moved into crate::sched::edges (INV-SCE-10): the per-CPU |
| INV-SCHED-EVRING-1 | `sched/side_edges.rs` | INV-SCHED-EVRING-1: `slot` is the modulo-bounded index of a unique |
| INV-SLOT-2 | `safety/slot.rs` | ! * **INV-SLOT-2 (barrier)** — publish stores with `Release`; `get` |
| INV-SLOT-3 | `safety/slot.rs` | ! * **INV-SLOT-3 (exclusive mut access)** — `get_mut_locked` requires |
| INV-SY-2 | `sync/edges.rs` | ! - INV-SY-2 `entry_frame_*` — the task's trap frame at the top of |
| INV-SYSC-1 | `syscall/misc.rs` | (INV-SYSC-1 via edges::list_*). |
| INV-SYSC-2 | `syscall/file.rs` | non-null (INV-SYSC-2 reachability arm). |
| INV-SYSC-3 | `syscall/edges.rs` | ! - INV-SYSC-3 (current-task park/unwind) — the repeated |
| INV-SYSC-4 | `syscall/edges.rs` | Arch reset edges (INV-SYSC-4: the reboot/panic ladder's port I/O and |
| INV-SYSC-5 | `syscall/edges.rs` | INV-SYSC-5: the caller has verified the File's ops identity |
| INV-SYSC-7 | `syscall/file.rs` | read/write lease on the inode — INV-SYSC-7 pre-publish arm. |
| INV-SYSC-8 | `syscall/edges.rs` | INV-SYSC-8: the mmap/mremap/msync/mprotect page-table family. The |
| INV-TASKREF-1 | `process/task_ref.rs` | INV-TASKREF-1 (pin exclusivity, added by the slim4-fix lane |
| INV-TL-1 | `sched/sched.rs` | Slot take (INV-TL-1): the ownership-take half of the task-page |
| INV-TL-2 | `sched/sched.rs` | Slot release (INV-TL-2): the terminal-release half of the |
| INV-TRACERING-1 | `trace/mod.rs` | interior mutability is behind the slot protocol (INV-TRACERING-1). |
| INV-VFSOPS-1 | `fs/vfs.rs` | INV-VFSOPS-1 dispatchers (best-effort clone |
| INV-VMA-1 | `mm/vma.rs` | ! - **INV-VMA-1**: No two VMAs in the same address space overlap |
| INV-VMA-2 | `mm/vma.rs` | ! - **INV-VMA-2**: VMAs are sorted by start address, enforced by the BTreeMap key. |
| INV-VMA-3 | `mm/vma.rs` | ! - **INV-VMA-3**: `max_end` (if maintained) equals `max(end)` across all VMAs |
| INV-VMA-4 | `arch/x86_64/mm/page_fault.rs` | Window bounds: VMA end (INV-VMA-4), then EOF — a |
| INV-XE-COPY-1 | `arch/x86_64/mm/unsafe_edges.rs` | Copy one page between two linear-map addresses (INV-XE-COPY-1): both |
| INV-XE-FRAME-1 | `arch/x86_64/unsafe_edges.rs` | ! - **INV-XE-FRAME-1 (origin)** — [`SavedFrame`] is constructed only |
| INV-XE-FRAME-2 | `arch/x86_64/unsafe_edges.rs` | ! - **INV-XE-FRAME-2 (liveness)** — the frame lives on the interrupted |
| INV-XE-FRAME-3 | `arch/x86_64/unsafe_edges.rs` | ! - **INV-XE-FRAME-3 (aliasing)** — exactly one &mut frame handle |
| INV-XE-PT-1 | `arch/x86_64/mm/unsafe_edges.rs` | ! - **INV-XE-PT-1 (liveness)** — [`PtTable`] is only constructed from a |
| INV-XE-PT-2 | `arch/x86_64/mm/unsafe_edges.rs` | ! - **INV-XE-PT-2 (bounds)** — accessors take any `usize` index and |
| INV-XE-PT-3 | `arch/x86_64/mm/unsafe_edges.rs` | ! - **INV-XE-PT-3 (aliasing)** — tables are shared across CPUs; `get` |
| INV-XE-PT-4 | `arch/x86_64/mm/unsafe_edges.rs` | ! - **INV-XE-PT-4 (stage discipline)** — construction routes through |

**F19. arch trap entry/exit + context switch + IPI asm** — owners
`kernel/src/arch/*/trap.rs`, `thread.rs`, `context.rs`, `process.rs`
(arch twins), `mm_ops.rs` (switch/TLB flush parts), `ipi.rs`,
`exception.rs` walkers, `asid.rs`, `fixmap.rs`. *What stays raw:* the
entry/exit stubs and `ret_from_fork` trampolines (asm by definition),
FPU/xstate save-restore spans, satp/CR3 switch + TLB ops, IPI
send/receive (the only cross-CPU doorbell), the frame-pointer walks
the trap printer runs on the interrupted frame. *Why no safe shape
exists:* these ARE the machine's context manipulation — the plan's
Round-D floor names them ("context switch, the interrupt entry — must
be hand-written in every kernel"); a safe wrapper would only relocate
the asm, not type it. *Reference:* Linux's `entry.S`/`__switch_to`/
`smp_call_function` — identical asm with zero contracts.
*Containment:* entry paths run with IRQs off on the taken CPU; the
IPI/TLB edges hold the sending CPU's ordering (fence/ipi discipline
per arch).

**F20. page-table walk/map protocol (boot init + fault path)** —
owners `kernel/src/arch/*/mm/mmu_init.rs` (boot-time table build —
before the typed PtTable wrappers exist), `page_fault.rs` (the
demand-fault protocol: PTE-locked walk + map_user_page install +
page-descriptor flag set on the exclusively-owned page),
`pagetable.rs` runtime edges. *Why no safe shape exists:* the walk/map
pair is the page-table protocol itself — the AE/RE/XE-PT INVs own the
table ACCESSOR discipline at their edges; these rows are the
lock-scoped INSTALL protocol on top (the R7-C3 race re-check, the
loser-frees rule sampled at the fault sites). *Reference:* Linux's
`handle_mm_fault`/`_pte_alloc_map_lock` — the same locked
walk-then-install dance. *Containment:* every install under the PTE
lock; the descriptor stores precede publication.


## Index addendum (retag-discovered)

| INV id | owner file | contract (headline) |
|---|---|---|
| INV-BINDER-DIAG-COMM | `ipc/binder.rs` | INV-BINDER-DIAG-COMM (unsafe-slim r3): the ONE audited raw-task comm |
| INV-BINDER-ENTRYFRAME | `ipc/binder.rs` | INV-BINDER-ENTRYFRAME (unsafe-slim r3): the ONE audited entry-frame read |
| INV-BINDER-PINTASK | `ipc/binder.rs` | INV-BINDER-PINTASK (unsafe-slim r3c): RAII twin of procfs's PinnedTask |
| INV-BINDER-PRIV | `ipc/binder.rs` | INV-BINDER-PRIV (unsafe-slim r3): only binder_open installs |
| INV-BINDER-PROBE | `ipc/binder.rs` | target's live user page tables. INV-BINDER-PROBE: read-only, the one |
| INV-BINDER-WORLDSEC | `ipc/binder.rs` | (INV-BINDER-WORLDSEC: the table only ever holds those keys). |
| INV-BIO-BHREF | `fs/ext4/file.rs` | INV-BIO-BHREF: bread pins the BufferHead until brelse; the |
| INV-EXT-VIEW | `fs/ext4/mod.rs` | INV-EXT-VIEW twin for the on-disk superblock: the single cast point, |
| INV-EXT4-ATIME | `fs/ext4/file.rs` | INV-EXT4-ATIME: the cached Ext4Inode in inode.sb is |
| INV-EXT4-FILERES | `fs/ext4/file.rs` | INV-EXT4-FILERES (unsafe-slim r3c): the ONE audited resolution of a |
| INV-EXT4-GLOBALFS | `fs/ext4/mod.rs` | GLOBAL_EXT4_FS stores the mount's instance (INV-EXT4-GLOBALFS); |
| INV-EXT4-IBLOCK-HDR | `fs/ext4/file.rs` | INV-EXT4-IBLOCK-HDR: typed header view of the 60-byte root. |
| INV-PIPE-PRIV | `ipc/binder.rs` | fd owning the Arc (same contract as fs/pipe.rs's INV-PIPE-PRIV). |

**F17. binder buffer block-views** — owner `kernel/src/ipc/binder.rs`.
*Shape:* r3c's consolidated view primitives — INV-BINDER-BLOCKVIEW
(the ONE raw window over the mmap'd binder buffer: bounds-checked
slice/word/POD views), INV-BINDER-STAGE-POD (bytewise view of Copy
values), INV-BINDER-HELD (token-checked accessors) — plus the narrow
call sites beneath them. Every access goes through the bounds-checked
view methods; no raw `.add()` escapes the primitive.
*Why no safe shape exists:* the binder buffer is one mmap'd arena
shared with user binder threads, sized by the protocol — the same
device-window relationship as MmioRegion, but with the r3c view API
already measured in place (SLOW-TXN families are gated on it).
*Reference:* Android's flat_binder_object walks over the same arena.
*Containment:* ipc/binder.rs only; world lock discipline per the
guarded-domain notes.

**F18. mm-forensics page-descriptor walkers** — owners
`kernel/src/dfx/mmforensics.rs` (note_ref, capture_frame), 
`kernel/src/dfx/ledger.rs` (the census walks), and
`kernel/src/dfx/pathstash.rs` (per-tid static slot table).
*Reads:* raw `*const Page` descriptors across the whole mapped vmemmap
span (refcount/flags/type/mapping/index — value-only, never
re-dereferenced), exactly like F1's stale-task reads but for PAGE
descriptors; pathstash's static SLOTS are written per-tid exclusively
(publication ordered via tid/len atomics — the slowtask-SLOTS
discipline twin). *Why no safe shape exists:* these instruments exist
to report on descriptors whose invariants are SUSPECTED broken (the
wild-write/fake-OOM hunt lines) — a typed handle would presume the
invariant under investigation. *Reference:* Linux's
`dump_page`/page-type census walkers (the same untyped descriptor
reads). *Containment:* gated by rux:mmforensics / the census flag,
default-off, value-only reads <= one descriptor at a time.

Also under this row's neighborhood (already-per-site-documented
residue): taskdump's %gs:PC_MAGIC read is F5-class (asm register
edge); its CURRENT_PT_REGS frame read is the one-frame-edge
discipline; slowtask's `audit` call is the single-auditor CAS
contract; oops.rs's putchar_no_lock is the panic-path console twin
(F4-adjacent).

**Escape-hatch audit (Layer 4.1, 2026-11-21)** — the tree-wide
inventory of public raw-land hatches on the handle primitives, each
with its per-site row (the unsafe_gate escape_hatch_check rule
enforces: a public hatch without a doc-resolvable INV citation within
25 lines fails the gate):

| hatch | owner INV | contract headline |
|---|---|---|
| KPtr::as_ptr / ::phys | INV-KPTR-4 | address-of for DMA descriptor programming; the KPtr stays alive (no ownership transfer) |
| KPtr::into_raw / from_raw | INV-KPTR-4 | ownership transfer through raw land; re-adopt exactly once or leak |
| DmaSlot::as_ptr / ::phys | INV-DMA-2 | window-lifetime address for descriptor programming; block-lifetime rule applies |
| TaskBorrow::as_ptr / from_raw | INV-TASKREF-1 | shared-borrow read/write only, no store, no yield; adopt-once under the caller's liveness proof |
| TaskRef::as_ptr / into_raw | INV-TASKREF-1 | address-of under the live pin; into_raw transfers the pin (exactly-one task_put) |
| File::get_private_data / private_data_ptr / take_private_data | INV-FS-FILEPRIV-1 | raw-word slot copies (open/close cell discipline); typed views must go through fs/unsafe_edges |

Non-hatches verified: everything else in kernel/src/safety/ is either
typed-safe surface or pub(crate); frame.rs/dma.rs windows are
constructed through their INV'd ctors.

**Layer 2.2 machine-checked rows (Kani, 2026-11-21)** — the two
riskiest INV families now carry bounded proofs (harnesses:
kernel/safety-tests/src/kani.rs, `cargo kani` 12/12 green):

- **INV-KPTR-1/2/4 (exactly-once free, layout lock)** — machine-
  checked: the new->use->Drop lifecycle, the into_raw->from_raw
  re-adoption (exactly one dealloc), and the leak arm (zero
  deallocs, not UB) all verify UB-free for symbolic values; a
  dealloc-layout drift or double free fails the proof.
- **INV-MMIO-1/2/3 (no OOB window)** — machine-checked: for EVERY
  offset value, each accessor (u8/u32/u64, read+write) either
  accesses inside the declared window or reaches the designed
  check() panic; no path performs an OOB access.
- **INV-DMA-2/3 + INV-WINDOW-1/2/3 (the Miri-excluded families)** —
  bounded proofs added: DmaBlock carve/volatile lifecycle and
  KernelStackWindow read_u64's Some-only-in-window discipline.

Two Kani convictions fixed in the same change (the proofs doing
their job on the checker infrastructure itself):
1. MmioRegion::check's alignment debug_assert computed `base + off`
   with raw addition — off near usize::MAX overflowed the CHECKER
   before the window guard ran (wrapping_add now; alignment is a
   modulo property).
2. KernelStackWindow::read_u64's guard computed `addr + 8` raw —
   same class, the guard was UB-prone for the very addresses it
   exists to reject (checked_add now).

## Round-D contract addendum: the fs pre-convention population

| INV id | owner file | contract (headline) |
|---|---|---|
| INV-FILE-ARC-1 | `fs/file.rs` | ArcInner strong-word offset pinned at boot (verify_arc_inner_layout); caller pins the allocation; CAS wins nonzero or fails clean |
| INV-FILE-CELL-1 | `fs/file.rs` | inode/dentry/ops cells write-once at open pre-publication; private_data adds only the close-path take (INV-FS-FILEPRIV-1) |
| INV-FILE-SEND-1 | `fs/file.rs` | File Sync/Send carried per-field: write-once cells, locked pos/cloexec/dirent_cache, atomics, real mutex |
| INV-DENTRY-ADOPT-1 | `fs/dentry.rs` | claim-then-adopt: WalkClaim blocks the retirement drain; increment_strong_count+from_raw under the claim |
| INV-DENTRY-MEMO-1 | `fs/dentry.rs` | child-memo family: seqlock small slots, children-lock Release big table, immutable nslots, claim-blocked drain |
| INV-DENTRY-SEND-1 | `fs/dentry.rs` | Dentry/VfsMountInternal/BigMemoTable Sync/Send carried per-field (locks, seqlock, drain lock) |
| INV-VFS-WALK-1 | `fs/vfs.rs` | path-walk raw-position family: every deref under a held WalkClaim or refreshed from a live Arc; adopts ride INV-DENTRY-ADOPT-1 |
| INV-EXT4-JHANDLE-1 | `fs/ext4/namei.rs` | per-task journal-handle cell: set/clear/get/take_pre_capture all task-local |
| INV-EXT4-SBINFO-1 | `fs/ext4/namei.rs` | in-memory sb_info installed once at mount, counters mutated under EXT4_BIG_LOCK |
| INV-JBD2-BH-1 | `fs/jbd2/recovery.rs` | journal block IO family: bread-pinned blocks, bounds-checked byte views, single-threaded state machines |
| INV-JBD2-INIT-1 | `fs/ext4/journal.rs` | journal init at mount: bh-pinned superblock probe + handle setup (single-threaded) |
| INV-PROC-PIN-1 | `fs/procfs/pid.rs` | PinnedTask: pinned at construction, released in Drop; readers borrow through the pin |
| INV-FSCTX-1 | `fs/fsctx.rs` | fs_context fd family: OPS-identity-checked typed views; close is the single Box reclaim |
| INV-PIDFD-PIN-1 | `fs/pidfd.rs` | pidfd family: pid_hash_lookup_pinned + thread-ring leader pins, Drop releases exactly the pins taken |
| INV-VT-PRIV-1 | `fs/vt.rs` | VT device fd family: Box<DevNo> cell, close is the single reclaim |
| INV-TMPFS-SB-1 | `fs/tmpfs.rs` | tmpfs mount-lifetime family: leaked-in-registry sb (GLOBALSB provenance); name cells under the children lock |
| INV-PIPE-BUF-1 | `fs/pipe.rs` | pipe buffer family: ring pages/cursors mutated only under the pipe inode lock; resize under the same lock |
| INV-TTY-WIRE-1 | `fs/tty.rs` | termios/termio wire converters: POD struct-bytes casts, fixed sizes, no pointers |
| INV-INODE-STATDEV-1 | `fs/inode.rs` | stat-dev registry family: locked map; advisory-mode volatile store (ATIME shape) |
| INV-EXT4-INODEIO-1 | `fs/ext4/inode.rs` | on-disk inode IO: bread-pinned table blocks, fixed-offset field writes, EXT4_BIG_LOCK |
| INV-EXT4-HTREE-1 | `fs/ext4/htree.rs` | htree dir-block views: bounds-checked entry/leaf walks over the pinned dir block |
| INV-BUFCHUNK-1 | `fs/buffer.rs` | chunked-buffer family: CopyBytes-shaped POD views + chunk-table stores under the buffer lock |
| INV-DEVFS-SHARED-1 | `fs/devfs/mod.rs` | coherent shared-backing family: grows/mutates under the entry shared lock; pages pinned through the table |
| INV-INOTIFY-PRIV-1 | `fs/inotify.rs` | inotify fd family: Box'd instance state cell; watches under the group lock; close is the reclaim |
| INV-LOCKS-KEY-1 | `fs/locks.rs` | file-lock family: keys/registries in locked maps; raw words are identity keys, never dereferenced |
| INV-EXT4-IND-1 | `fs/ext4/indirect.rs` | indirect-block family: bread-pinned chain steps, triple bounds-checked |
| INV-SYSRQ-1 | `fs/procfs/sysrq.rs` | sysrq diagnostics: task words/stacks through TaskBorrow pins held across the print |
| INV-SBTYPE-1 | `fs/superblock.rs` | FileSystemType table ABI: raw-pointer extern "C" mount/kill_sb slots |
| INV-CHARDEV-PRIV-1 | `fs/char_dev.rs` | char-device fd family: driver tag/state word cell (FILEPRIV discipline) |
| INV-EXT4-ALLOC-1 | `fs/ext4/allocator.rs` | bitmap family: bread-pinned RMW blocks under EXT4_BIG_LOCK |
| INV-PROCFS-SEND-1 | `fs/procfs/mod.rs` | ProcFSNode Sync/Send carried per-field (children/override fields under their locks) |

## Round-D contract addendum (GAP pass 2)

| INV id | owner file | contract (headline) |
|---|---|---|
| INV-MM-1 | `mm/edges.rs` | ! - INV-MM-1 (node borrow) — the single UMA node's `PglistData` as a |
| INV-MM-2 | `mm/lru.rs` | (INV-MM-2 owned-frame arm). |
| INV-MM-3 | `mm/memblock.rs` | mm::edges (INV-MM-3) use to reach it; never called from elsewhere |
| INV-MM-4 | `mm/edges.rs` | INV-MM-4: the slab allocator's in-region raw accesses. All pointers are |
| INV-MM-COMPACT-1 | `mm/compact.rs` | (INV-MM-COMPACT-1: the compactor family — page migration walks |
| INV-PERCPU-1 | `sync/percpu.rs` | (INV-PERCPU-1: the per-CPU bank family — banks are addressed by |
| INV-PERF-EV-1 | `perf/mod.rs` | (INV-PERF-EV-1: the perf-event family — the registry's Box<PerfEvent> |
| INV-RCU-1 | `sync/rcu.rs` | (INV-RCU-1: the RCU family — grace-period state is per-CPU |
| INV-RWLOCK-1 | `sync/rwlock.rs` | (INV-RWLOCK-1: the rwlock family — reader counts and the writer |
| INV-SCE-11 | `sched/sched.rs` | INV-SCE-11: the tp-register derivation lives in the edges |
| INV-SKB-1 | `net/buffer.rs` | (INV-SKB-1: the sk_buff family — heads are allocator-owned blocks |
| INV-SMALLALLOC-1 | `mm/small_alloc.rs` | (INV-SMALLALLOC-1: the small allocator family — the magazine |
| INV-SPIN-1 | `sync/spinlock.rs` | (INV-SPIN-1: the spinlock family — guards deref through the |
| INV-TASKLET-1 | `interrupt/tasklet.rs` | (INV-TASKLET-1: the tasklet family — the intrusive list protocol is |
| INV-TESTHARNESS-1 | `rux-sched/src/sched_tests/fair_timeline.rs` + `sched/edges.rs` debug_* helpers | (INV-TESTHARNESS-1: the in-kernel test-harness family — the timeline driver moved to the forbid-unsafe rux-sched crate (S1b flip, test-migrate lane); its 21 raw-node unsafe blocks dissolved into the edges.rs audited zone as pub safe debug_* fns; fair_timeline/ |
| INV-UACCESS-SAFE-1 | `arch/uaccess_safe.rs` | (INV-UACCESS-SAFE-1: the capsule's own edges — each safe wrapper |
| INV-VDSO-DATA-1 | `mm/vdso.rs` | (INV-VDSO-DATA-1: the vDSO data page family — the page is a fixed |

| INV-SKB-1 | `net/buffer.rs` | sk_buff family: allocator-owned heads, headroom-tracked data spans, bounds-checked slices |
| INV-NET-SOCKTAB-1 | `net/net_edges.rs` | rung-2 framework edge: static_table_ref/_mut form refs from addr_of_mut! on the stack's static-mut socket/route tables; callers hold the per-table serialization the sites document (crate-flip: rux-net is forbid) |
| INV-NET-ARCREF-1 | `net/net_edges.rs` | rung-2 framework edge: arc_refcount_probe (volatile strong/weak reads at data-16/8, forensics only) + audited_deref (ops-identity-validated live Arc-backed ptr) |
| INV-NET-HDRVIEW-1 | `net/net_edges.rs` | rung-2 framework edge: pod_view/pod_view_mut wire-header views; length AND ALIGNMENT checked (the old inline casts never checked alignment — misaligned skb buffers were silent UB, now drop) |
| INV-RD-CLAIM | `sched/reapdrain.rs` | reap-claim family: ReapClaim carries the NonNull<Task> page-identity TCB handle; Send is the reaper-lock exclusivity (PENDING/GRACE_BATCH statics); the NonNull is dereferenced only by the reaper under the reap lock |
| INV-NET-CONTAINEROF-1 | `net/net_edges.rs` | rung-2 framework edge: container_of maps a wait-queue FIELD ptr back to its UnixSocket via offset_of!; diagnostic-only sites |
| INV-VDSO-DATA-1 | `mm/vdso.rs` | vDSO data page: fixed static mapping, page-constant offsets, seqlock write discipline |
| INV-RWLOCK-1 | `sync/rwlock.rs` | rwlock family: atomic reader counts + writer word with documented pairing; guards ride the held lifetime; panic rows best-effort |
| INV-SPIN-1 | `sync/spinlock.rs` | spinlock guards ride the held-lock lifetime; panic/deadlock-warn rows are PANIC-TIME BEST-EFFORT (tolerate garbage; the machine is already failing) |
| INV-TASKLET-1 | `interrupt/tasklet.rs` | tasklet family: intrusive list is F6-registered; clone-to-claim rides the per-CPU list lock + Release count |
| INV-SMALLALLOC-1 | `mm/small_alloc.rs` | small allocator: magazine spans/class pages owned by the class lock; arena base installed once |
| INV-UACCESS-SAFE-1 | `arch/uaccess_safe.rs` | the capsule's own edges: one audited primitive call per wrapper (INV-UACCESS-1); UserPtr/Slice carry the addr/len pair |
| INV-TESTHARNESS-1 | `tests/*.rs` | in-kernel test harnesses: adversarial raw statics are the thing under test; unit-test feature only |

| INV-KSWAPD-1 | `mm/kswapd.rs` | reclaim scanner: zone/LRU-locked list walks; descriptor updates inside the isolation window |

| INV-SEQLOCK-1 | `sync/seqlock.rs` | seqlock family: even-sequence validated reads, exclusive writer phase, the UnsafeCell IS the discipline |
| INV-RWSEM-1 | `sync/rwsem.rs` | rwsem family: count-word protocol pairs with the waitqueue; guards ride the held lifetime; panic rows best-effort |
| INV-RMAP-1 | `mm/rmap.rs` | reverse-map family: map-locked walks; index words read under the rmap lock's Release publication |
| INV-PAGEDESC-1 | `mm/page_desc.rs` | vmemmap family: pfn-addressed descriptors through the managed-range-checked linear array; lock-free flag atomics |

| INV-SENDSYNC-1 | cross-file | per-field Send/Sync impls on device/node structs (virtio_gpu/input, NetDevice, DebugFsNode, KObject, VfsMount, FsStruct, RootFSPathCache, MmStruct, VmaManager, fair-timeline nodes): each carried by that struct's own locks/atomics/write-once cells — spot-checked per impl at retag |
| INV-COV-1 | `cov.rs` | coverage runtime's instrumented edge (capture under the cov-dump feature only) |
| INV-STATIC-CELL-1 | cross-file | OnceCell/MaybeUninit static-singleton publishes (KERNEL_LAYOUT, NODE_DATA banks, deferred-free POOL, vmemmap stats): single early-boot writer under the init order, shared readers after |
| INV-IOURING-REGION-1 | `io_uring/mod.rs` | the ring region family: alloc/free of the shared rings + page pins; views ride INV-IOURING-RING-1 |
| INV-SYSV-UAPI-2 | `ipc/sysv_*.rs` | residual uaccess staging rows (sops copy edges + shm page ops) on the capsule boundary |
| INV-KTASK-1 | cross-file | (*task)/task-ref rows where the source is a typed &Task/&mut Task or a pinned handle (pid_hash/current callers) — the cargo-cult audit applies, adopt rows ride INV-TASKREF-1 |

## Round-D contract addendum (flip-window drift)

| INV id | owner file | contract (headline) |
|---|---|---|
| INV-AE-ZP | `arch/aarch64/mm/page_fault.rs` | Exclusively owned fresh page inside the linear map (INV-AE-ZP). |
| INV-MM-10 | `mm/mod.rs` | Contract (INV-MM-10): `root_ppn` must be a live page-table root and |
| INV-MM-11 | `mm/edges.rs` | INV-MM-11: the kernel-layout OnceCell pair (init-guarded static). |
| INV-MM-12 | `mm/pglist.rs` | Early boot only, before other CPUs are online (INV-MM-12 init arm |
| INV-MM-13 | `mm/edges.rs` | INV-MM-13: the small-object allocator's in-page layout. Every address |
| INV-MM-5 | `mm/pcp.rs` | Sync proof + raw-row edges live in mm::edges (INV-MM-5). |
| INV-MM-6 | `mm/edges.rs` | kernel buffer (INV-MM-6 pinned-alias arm: the pin keeps the frame |
| INV-MM-7 | `mm/page_alloc.rs` | Fresh allocation, exclusively owned (INV-MM-7 page-slice arm). |
| INV-MM-8 | `mm/deferred_free.rs` | INV-MM-8: the lock is held. |
| INV-MM-9 | `mm/vmemmap.rs` | mutation concurrent with vmemmap reads) — INV-MM-9. |
| INV-RE-ZP | `arch/riscv64/mm/page_fault.rs` | The page is fresh and exclusively owned (INV-RE-ZP: alloc_user_phys_page |
| INV-XE-PIO-1 | `arch/x86_64/cpu.rs` | Port I/O (INV-XE-PIO-1) |
| INV-XE-ZP | `arch/x86_64/mm/unsafe_edges.rs` | frame exclusively — INV-XE-ZP in mmu_init). |

## Round-D contract addendum (milestone-1 drift)

| INV id | owner file | contract (headline) |
|---|---|---|
| INV-FS-FILEINODE-1 | `fs/unsafe_edges.rs` | INV-FS-FILEINODE-1: `File::inode` is an `UnsafeCell<Option<Arc<Inode>>>` |
| INV-FS-FILEINODE-2 | `fs/unsafe_edges.rs` | INV-FS-FILEINODE-2: the install twin of [`file_inode`] — writes the |
| INV-FS-FILEPRIV-2 | `fs/unsafe_edges.rs` | INV-FS-FILEPRIV-2: the take-and-reclaim twin of file_privdata_ref — |

| INV-PC-WIN | `fs/page_cache.rs` | byte window over one exclusively-owned page frame (fill_from/BufView family) |
**Round-5 conversion receipts (what this register is the residue of):**
blkdev fn-pointer ABI de-signed (registration is the audited edge,
INV-BLKDEV-2/3); OnceSlot (INV-SLOT-1/2/3) owns every publish-once
singleton that was `static mut` (virtio disks, evdev, net devices,
softirq vectors, TCP managers, ksoftirqd, PLIC domain); MmioRegion owns
every register window (gicv3, plic — including the asm-lw/sw style
leftovers, hpet, pci config, virtio-gpu config, framebuffer pixels,
ACPI tables); the net header edges (INH-BUF-3/4) own every protocol
parse/build; `File::private_data_ptr` + `net::file_arc` (INV-NETARC-1)
own the file-slot Arc family; the uaccess_safe API owns every
copy_in/copy_out; dfx/sbi_debug.rs deleted (dead hunt module, zero
consumers).

### 5.x e2fsck pre-existing findings (adjudicated 2026-11-18, fio-delalloc lane)

Byte-identical e2fsck findings on clean-poweroff AND hard-kill images
of main vs the lane branch — pre-existing, not introduced by any
current change; all auto-fixable, no structural corruption observed:
- extent-node checksum mismatch (metadata_csum family);
- stale BLOCK_UNINIT in group descriptors;
- block/inode free-count drift.
Open: root-cause each (candidate surfaces: the metadata_csum
compute-on-write path, the group-descriptor init/update path, and
delalloc-vs-free-count accounting), then this register entry closes
with per-finding root cause + fix. Severity: low (fsck -a heals; no
observed data loss in any verify=md5 battery), but the family is
registered per the data-corruption doctrine.

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
- OH bring-up campaign round (2026-11-10..11, oh-crash lane): the
  corruption class struck again and was again contained by the
  taxonomy's discipline — every row below had its fix diff inspected
  for residency before it landed here.
  - munmap same-mm race Face B (x86_64, 427453d5): the >4MB
    deferred-zap window left (VMA gone AND PTEs live); a racing
    same-mm mmap's fresh pages were zapped+freed and the refault
    zero-filled — the classic stray-clear signature, caught as
    mallocng heap corruption in OH services (#GP storm at
    ld-musl+0xba256). Residency: the zap walk is unsafe interior
    (raw PTE surgery). Fixed by the per-piece cursor loop
    (surgery+zap under one guard).
  - munmap Face A (all arches, same commit): chunk search restarted
    from addr_us and ate fresh VMAs in processed holes. Same
    residency class; cursor added everywhere.
  - mremap move non-atomicity (45fa58be): the [mmap(new), copy] gap
    let a sibling clear the move source; copy installed nothing and
    the destination fault-filled zeros — the last zero-producer of
    the hunt. Fixed with MmStruct::mm_surgery, a per-mm whole-syscall
    Rwsem (mmap_lock(write) parity — the reference's answer to
    exactly this family; the 91st then converted the spin form to
    Rwsem after it produced the pristine-boot meltdown).
  - The instrumentation itself is the receipt: REMOTE-BT (live remote
    stack over IRQ_WORK IPI), GP-PTE frame forensics, UNWATCH-COVER
    named the unmapping pid/range at #GP time. Same-rig A/B: armed
    60s hiperf records 3/3 boots zero GPs (was ~60% corrupting).
  - sys_sync metadata durability (fs-integrity lane, in flight as of
    this round): sync(2) returns success while the on-disk inode
    never learns the new size — after a hard reset a 32MB file reads
    back size 0 (md5-of-empty) with its data blocks verifiably ON the
    device (2.6M raw-grep hits of an 8MB pattern in the crashed
    image). Classified (b)-leaning-(a): no wrong-memory access, but
    a durability-contract violation that destroys data as surely.
    The hunt chain (fi3/fi4/fi5/fi7/fi8 boots, FI-* instruments,
    crash-window A/B rigs) is on branch fs-integrity; the fix and
    its zero-loss acceptance are the lane's deliverable.
  - Panic-path zombie face (22789bd2): the dump-then-stop ordering
    let a hung panic report wedge CPUs one by one after
    sysrq-c — smp_send_stop moved before the dumps. Class (c): loud
    by design, now also terminal by design.
  - binder dual BR_DEAD_REPLY (63dbeae4): a dying service proc
    delivered the SAME pending sync transaction's death twice (the
    cross-proc txn_stack walk + the dying side's own drain), which
    desynced libipc's completion bookkeeping into a guard-storm
    (80 BR_FAILED_REPLY hits) and aborted foundation. The reference
    cannot dual-deliver (single-slot return_error); four-piece parity
    fix. Class (b): protocol semantics, in-bounds.
  - futex COW-rekey lost wakes (190301611, futex-lostwake lane): the
    futex key hashed EVERY word — private and shared — by the physical
    frame. An anon word's frame is a COW transient (fork marks the
    parent PTE RO; the next store swaps the frame), so any waiter
    parked across a fork+write cycle became unreachable: the waker
    hashed the fresh frame and scanned an empty bucket, the waiter
    slept in the old frame's bucket forever — 8/8 parkers lost as a
    group with a live re-waking waker and ZERO scheduler forensics
    (lostwake.c fpns surface, 5/5 pre-fix). Same-mechanism evidence:
    run/lostwake-prefix-d1.log wake-miss lines show waiter phys
    0x7ffc7xxx pinned at park vs waker phys changing every kill cycle
    (0x7ff29xxx → 0x2c05cxxx → 0x7ff23xxx), same mm, same uaddr. The
    class also ate PRIVATE-flag futexes (musl ctid/join words under
    fork churn — a candidate slice of the OH storm's stranded-thread
    third). Fix is reference get_futex_key parity: anon words key
    (mm, uaddr) — Linux's PageAnon/FUT_OFF_MMSHARED rule — and only
    MAP_SHARED words key the stable frame; the private hot path also
    got cheaper (no page-table walk per key). Class (b): protocol
    semantics, in-bounds; bug-class elimination (frame-transient keys).
    Gate: fpns wedge futex=0 across 5 debug + 2 release legs
    (run/lostwake-gate-d*.log, run/lostwake-fix2-r*.log), make test
    green, pbench P5 rows at/below baseline bands (p5a_pipe 1.84 µs
    vs 2.1-2.3 band, tcg smp2 rig).
- Merge-day round (2026-11-14, twelve first-parent merges,
  94e1338b7..8f4a4a259): every fix classified against the §2 taxonomy
  before it landed here; two instrument families were convicted by
  their own measurements in the same round (honesty notes inline).
  - Signal-fault semantics parity (arena-mmap, merge 94e1338b7,
    commits 4c96c783/8bb0f731/065e7dd6): SIGSEGV/SIGBUS siginfo now
    carries the real fault address and class on all three arches
    (was: info-less send falling back to the SI_KERNEL placeholder,
    whose si_pid lands at the sigfault._addr offset — observed
    si_addr=0x1 from pid 1, si_code=128; class (b), signal-ABI
    parity). Safety property: an SA_SIGINFO handler can compute the
    faulting page — the FFRT 0x7ff1 fault-loop family (~10 Hz under
    OH load, the mmi_service 148.9 s death) is gone: zero loop
    occurrences across 3 OH boots, mmi received its two historical
    fault faces once each with correct si_addr, installed the page,
    and survived (SIGDEATH 0). With the vDSO DT_STRTAB/DT_SYMTAB
    swap fixed (musl resolves __vdso_clock_gettime,
    found=0x3fbec01800 under AT_SYSINFO_EHDR) the arena
    demand-paging pattern runs reference-identical end to end: 19/19
    probe checks (userspace probes archive :: test/arena.c; host
    ground truth 19/19, serial 19/19). Unaligned MAP_FIXED /
    MAP_FIXED_NOREPLACE now returns EINVAL (was silent truncation
    through VirtAddr masking). riscv64 make test 1328/1328. Artifacts:
    arena-mmap lane run/ARENA-FINDINGS.md + run/arena-rux-serial.log.
  - Program break per-mm on x86_64 (uboot-campaign, merge 28b880495,
    commit d47c73a3f): the §2.1 boundary row's fix ported to main
    with its own A/B on the unified KVM SMP8 8G rig — 24 boots:
    shell-SIGSEGV boots 3/12 -> 0/12, campaign success 11/12 ->
    12/12, guest-true shell-up median 6 s and desktop frame 9.1 s
    unchanged (zero timing cost; one uncontended per-mm brk_lock
    acquire per call). make test 1329/1329. The boundary row above
    was updated by the same commit. Artifacts: uboot-campaign lane
    run/CAMPAIGN.md + run/boots/ (per-boot serial/result/png sets).
  - epoll fds implement real ep_poll semantics (hilog-flood, merge
    7415622ea, commits 41281d207/c1d9ff6c9/deea2dd6): EPOLL_OPS files
    carried no poll fn, so the regular-file fallback reported every
    nested epoll fd POLLIN|POLLRDNORM forever — every
    FFRT/libinput-based OH service loop spun 10-60k rounds/s logging
    empty passes (the hilog flood fabric). Class (b) poll parity;
    Linux ep_poll reports POLLIN iff a wait would deliver now.
    Measured (same rig, same instruments, pre -> post): writev
    attempted 63-102k/s -> 1.7-3.7k/s (25-50x down), EAGAIN flow
    control frozen at 72.5k from t=60 (zero new for 4 min),
    clock_gettime volume 157k/s -> ~15k/s, appspawn fork 130-272 ms
    -> 85-90 ms; guest CPU 579% of 800% at baseline vs the reference
    kernel's 17.5% on the same images (post-fix CPU is
    boot-dependent — the fabric syscall burn is gone, per the lane's
    honest note). Hang-class elimination: the permanently-readable-
    nested-epoll spin family. Two in-tree tests pinning the old
    no-poll shape were updated to the parity shape. Artifacts:
    hilog-flood lane run/HILOG-FINDINGS.md + run/oh-b*.log census
    dumps.
  - Ubuntu 24.04 systemd to graphical.target (ub2404, merge
    6770963ea, commits 3019e35e1/2449a4d89): unix setsockopt errno
    family (SO_TIMESTAMP/SO_PASSSEC/SO_TIMESTAMPNS(+_NEW,
    TIMESTAMPING) accepted; ENOPROTOOPT not EAFNOSUPPORT — journald
    treats the old errno as fatal at boot #1); tmpfs fallocate
    (journald v255 fallocates its seqnum sidecar on /run before
    MAP_SHARED mmap — ENOSYS was fatal; this is the fix that made
    journald start); securityfs/ramfs/mqueue/debugfs/tracefs mount
    arms (dev-mqueue + sys-kernel-{debug,tracing,security} mounts
    succeed kernel-side). systemd 255 as PID 1 now reaches
    multi-user.target AND graphical.target (~1 min 40 s userspace)
    with journald, logind, dbus, udisks2 and network-online live.
    Class (b) throughout. Artifacts: ub2404 lane run/UB2404-STATUS.md
    + run/serial-121527.log.
  - Fault-path default death is a GROUP death (huks-wedge, merge
    5fb35cba7, commit 584c83e7a): Linux force_sig_fault ->
    do_group_exit parity at 15 fault sites x 3 arches (x86_64 8,
    riscv64 3, aarch64 4; was thread-only do_exit(-SIGSEGV) — a
    faulting worker left the daemon alive holding orphaned userspace
    locks). Safety property (signal semantics parity) AND hang-class
    elimination, live-verified on hw7: the identical huks_service
    boundary SEGV (936-byte copy straddling a 28 KB FFRT
    worker-stack guard, 0x478 bytes left) that wedged hw5 (four
    frozen musl __lock words; 60-370 s park chains through
    softbus/accountmgr/foundation/mmi) now dies as a group
    (grpexit=TRUE), init reaps it (SIGCHLD, status 11), the service
    respawns and is reaped again on its next fault — and TXN-HANG
    rows for the dead tgid are ZERO (no orphaned locks, no parked
    loopers). The orphaned-mutex wedge bootstrap (Edge A) is
    eliminated; the no-fault silent-stop class (Edge B) remains
    open. Artifacts: huks-wedge lane run/HUKS-FINDINGS.md +
    run/oh-hw5w1/hw7w1-serial.log.
  - R127 wait-queue hardening + wake-path exoneration (wake-drop,
    merge f4c57e7c6, commits 69f5d128e/857418948/765979874/b80f76a55/
    fecbd8ccc): wait entries keyed by (task, registration pid) with
    a wake-time staleness screen that evicts entries naming freed or
    recycled Task pages — closing the one silent remover the old
    kernel had (remove-by-pointer on a recycled page could strip a
    dead reader's leaked entry and wake a wrong task; corruption-
    class consequence made unreachable); WQLEAK names any future
    exit-without-unwind at exit time (live wq_regs counter,
    re-zeroed on page reuse). The class did not fire in the verdict
    boots (stE=0, WQLEAK=0) — banked as defense-in-depth. Exoneration
    with receipts: 8 verdicted wakes across 4 injected boots — 4
    DELIVERED (state transition INTERRUPTIBLE->RUNNING, GRQ linkage
    observed, target read within microseconds), 4 REFUSED (target
    already running, the legitimate W1 token return), ~880 EMPTY
    pushes (no waiter). The input-chain death is OH userspace (samgr
    kills dhardware mid-boot; audio_server's node-reader never
    re-polls the input fds). Artifacts: wake-drop lane
    run/WAKE-DROP-FINDINGS.md + run/serial-boot-r8..r11.log.
  - Reaper machinery exonerated (reaper-waitid, merge 537f9e6ed,
    commit adc67c975): ~1700 s of probed OH uptime across 5 boots —
    waitid is NEVER used (0 events, 0 polls of any kind); wait4
    (WNOHANG from appspawn, blocking from init's rc scripts) is the
    only channel; 788/788 leader exits reaped, zero zombies older
    than 10 s (ZLINGER=0), worst linger 1.6 s, appspawn reaps land
    in the same 0.1 s tick as the exit. The systemui incarnation
    cycle is a timing-sensitive userspace race (exit_group(-2) mid
    IPC handshake on fd 3, FFRT-arena address family, with
    clock_gettime -1 and writev EAGAIN in the tail) — the kernel
    reap/wait machinery is fully behaved. Negative result with
    receipts, kept per the honesty rule. Artifacts: reaper-waitid
    lane run/REAPER-FINDINGS.md + run/serial*.log, table-boot*.txt.
  - R87 secondary-bringup window closed (riscv-smp8-panic, merge
    f5416e8a4, commits 23c649335/1915f975c/5d2427802): secondaries
    enabled the timer interrupt BEFORE sched::init_secondary()
    published PER_CPU[hart] — during the memory-scaling BSP init
    tail, ticks scheduled on zeroed per-CPU slots and the ensuing
    slot-ownership fight produced mixed-context resumes (tp from one
    task, sp on another's stack, pc on a third's — row 17's family,
    the (a)-consequence class, caught loudly as KERNPANIC illegal
    instruction). Linux-parity fix: no interrupts on a secondary
    until per-CPU state is published (timer after init_secondary;
    the boot-complete wait spins with sie=0). smp2/8G boots with zero
    detector lines (previously 3 at jiffies 1-2). Post-fix matrix
    (minimal-rootfs ls/proc gate, 16 smp/mem cells): 13/16 clean;
    the previously-failing 5/1G, 7/1G, 8/1G, 8/2G cells are now
    clean; the residual ~1/3 timing race on smp8-class cells (the
    fair.rs orphan-sweep ownership fight) is handed to the sched lane
    with named entry points. trap.S MAX_CPUS clamps now derive from
    config (the stale literal 4 made harts 4-7 take CPU0's interrupt
    stack); ksoftirqd bring-up is create->bind->enqueue (Linux
    parity). make test green (one tcp RTT test needed a
    determinism fix — the pre-fix suite had been riding accidental
    secondary ticks). Artifacts: riscv-smp8-panic lane
    run/PANIC-FINDINGS.md + run/mx3-*.log matrix.
  - Honesty note — the R74/R83/R85 "ksp smear" never existed: the
    ksp tripwire printed the qword as TWO decimal numbers with no
    separator; read as one number and split at the "natural" 9/9
    digits, legal stack addresses decode to the hunted signature
    (429496725431981568 re-split at 10/8 is 0xffffffd601e80000 =
    idle[1]'s kernel stack top; the "hi32=0x19999995 magic" was an
    artifact of the mis-split). Hardware watchpoints on the words
    AND their identity/direct-map aliases (hunt13/14/15) never saw a
    garbage store while the tripwire still printed "garbage" in the
    same boots. §2.1's trap.S ti_user_sp/ti_kernel_sp rows
    (c1539f6e/8b8bdd16) are untouched — different bugs, convicted by
    their fix diffs. The R85 ksp repair gates in trap.S stay
    inert-but-harmless (they repair values that were never broken);
    r83hunt.rs's 0x19999995 fingerprint scan can never match —
    cleanup recommended, deliberately not in this round. Instruments
    get convicted too (PTGUARD/FREEGUARD precedent).
  - vDSO DT_VERDEF chain + kernel exculpated at the frozen word
    (rwlock-livelock, merge 95b5e61fb, commits
    a668e77ff/ac33690d2/1d0ed59ab): Go runtimes dereference
    DT_VERDEF unconditionally — snapd died at runtime.vdsoFindVersion
    +0x33 (cr2=0x2, nil section pointer) on a verdef-less vDSO. The
    vDSO now publishes DT_VERDEF/DT_VERDEFNUM/DT_VERSYM with the
    LINUX_2.6 chain (mirroring arch/x86/entry/vdso); glibc is
    indifferent, Go requires it; verified by boots I/J/K (snapd runs
    past its clock reads). The Ubuntu 24.04 mid-boot freeze decoded
    to a glibc pthread_mutex (GLib gthreadpool's
    spawn_thread_queue->mutex) left locked with no owner — and the
    kernel is exculpated AT THE WORD: across boots F/G/J/K/L/M the
    word goes silent forever after formation (zero futex ops on it),
    the one pre-freeze WAKE(val=1, ret=1) was DELIVERED (the woken
    waiter's WAIT returned ret=0 immediately), no kernel code writes
    user futex words on the WAIT/WAKE paths, and no runnable task
    was stranded (tgid dumps: all parked). Verdict (a) of the
    a/b/c ladder: the unlock that would free the waiter is never
    issued by userspace. The terminal shape is word=2 (glibc waiters
    bit — every future unlock MUST futex_wake, and none ever does);
    word=1 stalls can self-heal and are invisible to the census
    heuristic (registered as a follow-up). Artifacts: rwlock-livelock
    lane run/LIVELOCK-FINDINGS.md + run/serial-F..M.log.
  - Honesty note — the fuxwatch instrument was wedging its own
    subject (huks-wedge): the futex write-watch left abandoned watch
    pages W-cleared, spraying SEGV_ACCERR into innocent userspace —
    hw1's "holder vanished" face was the instrument's fault (a
    distributeddata mutex-RELEASE store took the abandoned page's
    fault; the mutex froze for the rest of the boot), not a
    userspace bug. Fixed by fuxwatch_retire_page before every slot
    move + clean shutdown at budget trip + unconditional W-restore
    in the fault hook; hw5 (fixed kernel): zero
    SEGV-on-watched-page events across 420 s. Every pre-hw5 boot
    that armed fuxwatch on a later-abandoned page carries
    instrument-induced SEGVs in its timeline — hunt priors
    re-weighted accordingly. Artifacts: huks-wedge lane
    run/HUKS-FINDINGS.md §3.
  - Tier-1 volume baseline re-banked (gate-infra, merge d8476534e):
    total syscall volume moved from the superseded 126.0M
    tok109-depth family to 15.36M on the tok-r4fix vehicle (release
    f5416e8a4: 15,359,337 @t=120s; debug 95b5e61fb sysrep1:
    15,253,824 @t=720s — 0.7% apart, build-profile-independent);
    dated section and vehicle-protection rules in
    docs/development/perf-gate-baseline.md. Docs-only change.
  - Dump line-atomicity + hilogcensus/spinwatch tracepoint migration
    (obs-hygiene, merge 8f4a4a259, commits 3f87a9ee6/a77685745):
    hunt dumps were per-byte raw UART writes that spliced with the
    /dev/kmsg flood — 115/6783 task lines destroyed (1.7%) on the
    before boot. Per-line staging under printk's console-emit lock
    with PER-CPU stages: 1506/1506 well-formed (r2), 881/883 (r3 —
    both residual splices came from the deliberately-lockless
    preempt-underflow crash printer, not the funnel), ENQ hunt lines
    240/240 whole (was char-mangled). Two in-round defects found by
    the smoke boots themselves and fixed: the shared stage
    byte-mixed under concurrent dumpers (per-CPU stages), and the
    emit-lock wait ran IRQs-off inside the irqsave guard and tripped
    the RCU detector once per boot (the flush now copies the line to
    the stack, drops the guard, flushes with the caller's interrupt
    state; Full budget 50 ms irqs-on for dumps, Brief 100 µs for
    GRQ-held/IRQ-context hunt prints). hilogcensus/spinwatch moved
    to unified tracepoints (events 57-60: rux:hilog_census,
    hilog_kmsg, spinwatch, spinwatch_fd) with the old dfx= tokens as
    boot-time aliases; set_event arming now takes effect live (the
    cached tri-state froze the verdict until reboot). Known residual
    (documented, follow-up lane): brief-budget fallbacks can append
    line-granular onto a long kmsg record's tail (14/238 ENQ refs in
    r2 — whole line visible, no char mixing); the deferred-ring
    design that kills the class is specified in the lane record.
    Artifacts: obs-hygiene lane run/OBS-HYGIENE.md +
    oh-before/r2/r3-serial.log.
  - Rig and gate repairs (same day): run-oh.sh / run-x86-ub.sh
    unified at KVM SMP8 8G (8b21a8440, 2f2c236ee; doc sweep
    27ee22415; the stale ">=3G hits the q35 PCI hole" wall did not
    reproduce — every 8G boot probes all virtio devices clean, the
    above-4G BARs ioremap through the modern path); pbench.c path
    fix after the 2026-11-11 probe re-org (1342fb78f — Tier-2 gate
    runs had been dying at probe build, rc=2); pa-boot.sh census
    token fix (untracked run/ script — note only; with the token
    fixed the census reads return to band, TERM 8 / REFUSED 31 on
    the merged 6770963ea kernel).
- falloc-wedge (2026-11-16, branch falloc-wedge, six commits
  5f58ca02f..3d9ac97c2): the fallocate05/06 family closed end-to-end.
  Bug classes eliminated, all logic bugs in safe code (no unsafe
  implicated): (1) acknowledged-data loss from the fallocate epilogue's
  whole-inode page-cache invalidation dropping still-dirty
  delayed-allocation pages (punchprobe3: sample@0 1 -> 0 across a
  plain mode-0 fallocate, no flush involved, root ext4 AND loop-stacked
  ext2) — replaced by never-invalidate-on-allocate + range-bounded
  span invalidation owned by the punch itself; (2) PUNCH_HOLE that
  deallocated nothing (zero-in-place arms) and clobbered misaligned
  edge bytes — reference-shaped pre-sync + extent split + indirect
  interior free + zeroed edges; (3) the delayed-allocation
  dirty-balloon wedge (243k dirty pages skipping eviction, zone at
  zero, reclaim re-entering the loop io lock, fdreap RCU stall) —
  writer-side writeback watermark + system-wide recovery (D-20), eager
  ENOSPC on extending writes at zero free blocks, dying-task reclaim
  unwind (fatal_signal_pending shape); (4) the loop io_lock spinlock
  monopolist window (holder sleeps through journal/I-O/reclaim with
  preemption off — INV-LOCK-8 rule 3) — SleepMutex. Numbers:
  fallocate05 3/3 + fallocate06 3/3 PASS fresh boots (RC=0, 16/24
  TPASS, 0 TFAIL; run/shell-f56-fin{1,2,3}.log and predecessor
  iso runs run/shell-f05-iso{1,2,3}.log / shell-ver-f06.log), mm
  storm 111 runs zero forensics events (run/res-storm1.txt), triple
  builds rc=0, riscv make test 1328/1328 (run/maketest-final.log).
  WB-WATERMARK rows on the fill show the mark un-tripping (free
  137k vs mark 121k, flushed 15.8k + shrunk 29.6k pages in one
  215-jiffy round). Full record: run/FALLOC-WEDGE.md.

- Batched direct I/O + journal write-through (fio-2x lane,
  5606622b8 + 73f0ee509, 2026-11-17): fio 3.36 static, q35 KVM
  smp8/8G, same 4 GiB ext4 data disk both arms (snapshot=on, identical
  bytes per boot), 3 interleaved reps, medians — 1M direct read
  40.8 → 4711 MiB/s (ref 9600; 226x-behind → 2.0x), 1M direct write
  93.9 → 1583 (ref 4019; 57x → 2.5x), 4k randwrite+fsync 260.6 →
  2727 IOPS (ref 15.8k; 7.7x → 5.8x). The pbench p6_write_fsync gate
  row improves vs same-window main base: 8783 vs 14891 µs (0.59x,
  interleaved KVM debug boards, run/pbench-ab-mkdrmdir.log). Safety
  pairing: the write fast arm's DMA borrows the caller's buffer
  (`WriteBatch::submit_ref`) — the device-owned-buffer window is a
  borrowck-enforced lifetime (drain before return), removing the
  hand-rolled pin/unpin race class, not narrowing it. Honest note:
  the same round found a class-(b) LOGIC bug in SAFE code — the
  ee_len==0x8000 max-written extent decoded as length-0/unwritten at
  every inline decode site, so >=128 MiB contiguous files silently
  read back as ZEROS (in-bounds, no memory corruption; caught by
  in-guest md5sum vs host digest, not by the type system — safe code
  removed the C pointer classes, not logic bugs). Fixed by
  centralizing the on-disk discipline in `Ext4Extent::length` /
  `is_unwritten`, one accessor for all sites; every pre-fix
  measurement on such files is void (the lane record lists which).
  Artifacts: run/FIO-2X.md, run/ab-final3-*.txt,
  run/md5-rand1g.log.
- getdents full-dir sweep (getdents lane, 2026-11-17; pbench
  dual-kernel rig, q35 KVM smp8/2G, 1000-entry ext4 dir, 32 KiB
  buffer, per-sample fresh-fd sweep to EOF, n=1000): 1374.3 ->
  37.0 µs vs reference Linux 7.0.0-38 at 72.2 µs (19.0x behind ->
  1.95x AHEAD; interleaved 3+3+2 boots, medians 1374.3/1378.4/1375.9
  vs 37.21/37.23/36.80, ref 72.23/72.78). Safety pairing: the whole
  getdents64 path for ext4 no longer materializes the directory at
  all — the unbounded-allocation bug class (4 MiB whole-dir Vec
  ALLOCTHROW panic at the le5 V0 boot, the heap-budget guards the
  launcher-errno lane added) is structurally gone from the steady
  path: ext4_readdir_fill packs linux_dirent64 records block-at-a-time
  into the caller's buffer, holding only one pinned BufferHead at a
  time (released on every exit path, including the buffer-full
  break). Honest note: found and fixed a first-draft BufferHead
  refcount leak on the buffer-full exit during lane self-review (no
  measurement showed it; the code path only triggers on small
  buffers) — the functional probe's 512-byte walks now cover it.
  Artifacts: run/GETDENTS.md, run/pb-gdab-*.log, run/pb-gdref-*.log.
- oc-rc round (2026-11-17): §3 close(2) entry above; rust-native
  register §M carries the design + same-rig numbers; the ext4
  capability-matrix pin failed on the deliberately removed close slot
  and was updated in the same change (the shape test working as
  designed — the removal is documented at both the ops table and the
  pin).
- rt-r6 round (2026-11-18): R6 PI chains. Safety properties landed
  with measured evidence: (1) the PI chain walk holds AT MOST ONE PI
  bucket lock at a time (D-27) — the reference's pi_lock/wait_lock
  lock-order hazard class is structurally absent; verified by
  pi_chain 1000-round chain-walk stress with zero wedges and the
  INV-C3 bounded-walk model proof. (2) The glibc robust-PI death
  contract (LOCK_PI returns 0 with FUTEX_OWNER_DIED in the word)
  eliminates glibc's futex-fatal abort on owner death — pi_glibc's
  in-process pthread_exit and cross-process SIGKILL scenarios both
  recover via pthread_mutex_consistent (3/3). (3) The cycle-detection
  fix (owner-side walk) turns a silent graph deadlock (X owns A, Y
  blocked on A, X blocks on B owned by Y — previously queued
  silently) into EDEADLK at block time; caught by the INV model's
  cycle scenario before any boot. (4) Kernel-keyed PI states carry a
  key_is_kernel guard on every user-word access — a kernel rt-mutex
  key's uaddr is a kernel static; writing through it would be kernel
  memory corruption (compile-time-impossible class eliminated by the
  runtime guard, exercised by the rt-mode conversions surviving the
  full battery). Bug-class taxonomy: the EOWNERDEAD contract bug and
  the cycle blindness lived in SAFE code (no unsafe introduced); the
  walk's unsafe surface is the same raw-pointer discipline as the
  existing engine (queued-waiter task pointers under the bucket, the
  documented contract). Honest gaps: multi-lock PI owners can
  chain-restore to an intermediate boosted config (never wrong-low,
  §5 candidate: per-task effective-priority tree in the reference's
  shape); the sticky-boost downgrade divergence (D-26 note). Artifacts:
  run/RT-R6.md (§3-§5), run/c*.log, run/g*.log.

  **Region-based batch allocation for order-0 bursts (bufw-parity,
  2026-11-18).** The page-cache bulk path's frame source (a 1 MiB
  buffered write asks for 256 pages in 64-bursts) paid four locked
  RMWs on shared lines per page inside the zone lock; the batch shape
  now claims each counter once per batch, drains the order-0 list
  directly, full-shatters one higher-order block when the list empties
  mid-burst, and serves the CPU's pcp row first — with the per-page
  safety invariants KEPT (MM-H2 descriptor init stays inside the zone
  lock; the SEED-ALLOC-BAD-POP pop-state forensic runs per page;
  INV-PCP-1/2/3 hold via the same three-state transitions, batched).
  Same-window interleaved A/B (q35 KVM smp8/8G, medians of 3): 1M
  buffered write avg-clat 78.4 vs reference 93.4 µs (1.19x ahead, from
  0.94x), BW 11.7 GiB/s vs 9600 MiB/s; the op's allocation term
  55k → 14k cycles (TSC phase probe). Kernel-vs-kernel pbench boards:
  all comparable rows flat or better (p6_append −3.6%, p6_unlink
  −3.5%, rest ≤ ±5.3%). fio verify=md5 buffered 4k+1M, unaligned-head
  4k+1M, write→fsync→read-back all err=0; make test 1335/1335;
  triple-arch rc=0. [lane worktree bufw-parity run/BUFW-PARITY.md]

  **Journal log-area lifecycle closed (journal-life, 2026-11-19).** The
  ext4-persist residual — journaled fs silently stops persisting after
  ~83 files under churn while create+fsync return success — was a
  two-layer data-loss class: (1) j_last was computed s_first + s_maxlen
  instead of the reference's s_maxlen, claiming one block BEYOND the
  journal inode as usable log; write-through commits keep tail == head,
  so the first commit whose span crossed the phantom block hit a jmap
  hole and aborted the journal permanently; (2) jbd2 returned POSITIVE
  errno across the syscall boundary (Err(30) → sys_mkdirat returned
  +30), so the dead journal surfaced as garbage non-error returns while
  nothing persisted — the API-contract break. Both fixed in the
  reference shape; running out of log now completes write-through-only
  with a WARN instead of aborting (__jbd2_log_wait_for_space is a no-op
  in this design: the log is empty between commits). Bug-class
  taxonomy: SAFE-code bugs (arithmetic + sign convention), no unsafe
  touched. Evidence: hard-kill battery 81/300 → 300/300 (x3) and
  900/900 on the 3x battery, content-OK all survivors, e2fsck in the
  accepted kill-window class, journal-less control unchanged;
  same-rig pbench A/B vs main all fs rows within ±2% (p2 −0.9%,
  write_fsync +0.7%, mkdrmdir +1.6%, unlink −4.4%); riscv make test and
  triple-arch builds green. [lane worktree journal-life
  run/JOURNAL-LIFE.md]

  **The exit-path stack-free safety closure (oh-launch round 10,
  2026-11-19).** Three stacked races could free a kernel stack its
  owner still executed on — the recycled-frame scribbler behind the
  string-byte execution hijacks (rip='s.boot.s'), the 0xb9 PTE face
  and the R83 ti_kernel_sp family: (1) the R86 deadline-heal reused
  the pick-side liveness check whose 50-jiffy age discount reads a
  monopolist/KVM-stalled switch-out as stale, then force-released the
  ownership qword under a still-switching owner; (2) the asm release
  store precedes the register restore, so the switcher's final
  loads/push/ret still ran on its own stack with on_cpu already
  false — a host deschedule there let the free win the race; (3) THE
  stale-rsp0 engine, caught in the act (fr3: STACKFREE-LIVE-RSP0 at
  1.472 s, panic 1 ms later): an aborted switch skips the rsp0
  install and a switch to idle silently keeps the previous value
  (the idle task's static boot-stack top fails the heap-bound
  screen), leaving a dying task's top installed while its stack was
  freed, recycled, and torn by the next trap frame pushed through
  it. The safety shape, all three: positive knowledge before a free
  (any-age SWITCHING_OUT marker, no live rsp0 naming the top),
  recovery paths that DEFER or WITHHOLD instead of proceeding
  (heal-fail re-parks the member for the next sweep; a stale-rsp0
  stack is leaked, never re-carved; bounded waits, never a spinning
  reaper), and two more blind printers unblinded (the rsp0 detector
  was pr_debug; both rsp0 keep-legs printed via putchar_no_lock —
  the fifth and sixth of the campaign). Evidence: the armed battery
  boots fr1/fr2/fr3 struck at 1.2-1.5 s on every prior kernel (the
  #GP-on-ret family); fr4 with the closure is the FIRST armed boot
  through the window — all three defenses fired in production (the
  same top=0xffff888079280000/pid=316 stack that killed fr3 was
  WITHHELD on fr4; FDM-DEFER member=319; FTS-SLOT-LEAK pid=316), 1187
  R56 aborts churned with zero early panic. Remaining honest gap:
  the 5-15 s degradation layer (stranded PICKED picks under the
  abort storm, panic-dumper re-entry halting a CPU, timer stall into
  a GRQ deadlock at 15.5 s) — the historical 6-22 s window family,
  now the outermost. make test 1555/1555 fresh rootfs; triple-arch
  rc=0. [lane worktree oh-launch run/OH-LAUNCH.md round 10]
  **Orphaned-lock-word class eliminated by construction (grq-source-fix,
  2026-11-19).** The round-6 GRQ_HELD per-cpu tracker made every GRQ
  release two steps (tracker-clear, then word-store); a fault landing
  in the gap stranded a word no tracker claimed — the gl3/gl4 shape
  (seven waiters, no holder) that was being papered over by a
  self-invented heal (force-release of an UNCLAIMED word; deleted —
  the reference has no counterpart, and acting on absence of evidence
  is the lying-slot-heal mistake shape). Source fix: the owner lives
  IN the lock word (`0` free, `cpu+1` held-by-cpu; acquire = one
  cmpxchg, release = one ordered Release store of 0 — spin_unlock
  stays the reference's single ordered store). GRQ_HELD, the
  grq_word_orphaned heal, and the TERM-GRQ-ORPHAN-WORD branch are
  gone; `grq_held_here`, the crime-scene dump's holder roles, and the
  terminal release's ownership proof are all DERIVED from the word —
  one authority that cannot disagree with itself. Detectors kept as
  debug_asserts (release-profile-qualified): holder_cpu's
  no-valid-owner assert plus the reference CONFIG_DEBUG_SPINLOCK
  ownership check in unlock(). Evidence: make test 1555/1555 with
  both detectors compiled in, zero fires; triple-arch builds rc=0;
  gl-class OH battery (q35 KVM smp8 8G, userdata-fresh3, release
  kernel, 3 boots + 1 debug): 0 orphaned words, 0 GRQ wedges, 0
  assertion fires, and the word-proven release exercised live on a
  real fault-inside-section event (gsf1: crime scene named cpu1/
  pid597 as holder from the word, TERM-GRQ-FORCE released it, six
  waiters proceeded). Protocol cost: release path drops one store
  (the tracker), acquire adds one per-cpu register read for the
  owner encoding. [lane worktree grq-lock-1st run/GRQ-LOCK-1ST.md
  §9]


  **The crash-path park and terminal-release closures (oh-launch round
  11, 2026-11-19).** Two more safety properties landed on the exit and
  crash paths: (1) the terminal deschedule loop's pre-schedule
  ownership release (the sc191 v2 shape) made {DEAD, !on_cpu} true
  while the dying task still executed its own schedule chain — the
  exit-path frees could not distinguish that from a completed
  switch-out and freed the stack under the running task (the
  rip=0x1/rip=heap/#GP-on-ret family at exit-storm time, multiplied by
  the abort storm's retry rounds); Task.terminal_switch_done — set
  only at the real-switch retire sites with a current() != prev
  discriminator so aborts never set it, re-armed each loop round — is
  now required by every stack-free predicate. Positive knowledge
  before a free, again. (2) A crashed CPU's park loop ran with
  whatever IF the fault arrived under; an interrupt-enabled park woke
  on the next timer tick and ran scheduler_tick on top of the broken
  dumper context, which acquired the runqueue and wedged inside it —
  fr7's crime scene is the first GRQ holder ever named (cpu4/pid356
  inside scheduler_tick, word=0x01 genuinely held, the park-time
  release at zero fires because the acquire came from the post-park
  interrupt). Every crash-path recursion park now disables interrupts
  before the eternal halt (the reference's panic_smp_self_stop shape):
  a dead CPU never takes interrupts again. Evidence: armed-battery
  boots — the panic count by 28 s fell from 4 (fr6) to 1 (fr7) with
  the window closed; the fr7 wedge mechanism convicted and closed
  same-round. Honest gaps: crash-path parks still release only the
  GRQ — a CPU faulting while holding any other lock strands it (fr9:
  the slab allocator lock in kmalloc, and a second static
  simultaneously); the small-int pt_regs writer (rip=1/rsp=0/badaddr=3
  pointer fields) remains at large and seeds every strand. make test
  1555/1555 fresh rootfs; triple-arch rc=0. [lane worktree oh-launch
  run/OH-LAUNCH.md round 11]

  **CR4.SMEP armed (oh-launch round 12, 2026-11-19).** The last
  scribbler face's amplifier closed: without SMEP the kernel EXECUTES
  user-mapped and zero pages, so a kernel rip landing on a small-int
  jump target (a return address or fn pointer overwritten with 1/0/3/7
  — PICK_LIVE, a bool, an enum tag) did not fault at the fetch — it
  executed mostly-zero bytes and only faulted later on a DATA access at
  a small address (the captured rip=0x1 rsp=0x0 badaddr=0x3 /
  badaddr=-0x30 shapes — the pt_regs was never corrupted; execution
  really was wandering the zero page). SMEP is now armed
  (CPUID.7.0:EBX[7]-gated, per-CPU through fpu_init's read-modify-write,
  BSP+APs), matching the reference kernel's default-deny: the same jump
  now faults AT THE TARGET — a clean #PF with badaddr=rip naming the
  hijack address, no zero-page cascade, no follow-on dumper re-entry
  crashing CPUs. The kernel-EXEC-fault routing in mm/exception.rs
  already documented these semantics; arming the bit makes the code
  match its own comment. make test 1555/1555; triple-arch rc=0. [lane
  worktree oh-launch run/OH-LAUNCH.md round 12]

  **The schedule-path state-write closure (oh-launch round 13,
  2026-11-19).** The last unconditional task-state writer on the
  schedule path is gone: the nanosleep-park rescue in __schedule's prev
  preparation flipped prev to RUNNING whenever a stale timed-sleep
  timer record read expired — with no sleeping-state check and no
  dead-check. The terminal deschedule path calls schedule() with
  exactly those stale records, so every retry round resurrected the
  parked-dying task DEAD->RUNNING (fr11's TERM-DESCHED state=0x1 rows,
  the pid-312 strand); every relink screen then legitimately treated
  the corpse as a live RUNNING task. The rescue now requires
  prev.state().is_sleeping() and clears the stale records on the
  refusing leg. Also latched: the user-segv forensic dump now carries
  the R73 re-entrancy guard the KERNPANIC dump has had since sc195 (a
  fault inside the forensics parked CPUs silently — fr13's mid-trace
  death with zero evidence). Evidence: fr13's TERM-DESCHED rows flipped
  from state=0x1 to properly-DEAD; fr14 became the campaign's FIRST
  full-300 s surviving armed boot (3 panics survived, zero deadlocks).
  The remaining failure is now pure liveness — the harvester steals
  dying CPUs' terminal picks (evidence chain and suggested fix shape
  handed to the pick-protocol lane in run/COORD-NOTE-R11.md). make test
  1555/1555 fresh rootfs; triple-arch rc=0. [lane worktree oh-launch
  run/OH-LAUNCH.md round 13]

### bytes-p0 (2026-11-23): the INV-CBYTES-4 contract was narrower than the audited caller — run arm split out as INV-CBYTES-6

The family-6 merge (52861d115) wrapped the ext4 merged copyout's
page-crossing span in pinned_frame_bytes, whose INV-CBYTES-4 debug
assert (`off+len <= PAGE_SIZE`) then panicked every beget-image quiet
boot mid-execve (bytes.rs:132; /tmp/perf-gate/pa-r4-merged-serial.log).
Convicted at the source with a pre-assert probe boot: the firing span
was ino=285 (= /system/bin/init, 439,888 B), off=0 len=8192 — a
2-frame merged group inside do_execve's 16 KiB read_file_via chunk
(pa-p0probe-serial.log line 99). The span was SOUND all along: the
merged copyout (7b8de4918, predates the primitive by 3 weeks) merges
only frames whose adjacency it verified (`run[j].1 == run[j-1].1 +
block`), and get_range_with_entry pins EVERY frame it returns — so a
release build (assert compiled out) served the same bytes with no
corruption; the defect was the contract, not the data. Verdict (b):
the primitive, not the caller. Fix: pinned_frame_run_bytes
(INV-CBYTES-6, "pinned-frame run window") carries the multi-frame
contract with the caller-side adjacency check named as the
contiguity enforcement; the single-frame primitive keeps its tight
assert for its 4 in-page sites. Regression gate: a real order-1
buddy block painted with a position pattern, read across the frame
boundary (test_safety_primitives_frame_run; riscv make test green).
A/B control (pristine main, assert bypassed only) confirmed the
LATER storage_daemon/init wild-RIP oops past this point is a
pre-existing separate defect, not this fix's (pa-p0ctl-serial.log vs
pa-p0final-serial.log). [lane worktree bytes-p0]

### Unsafe-ledger re-bank procedure (2026-11-23, ledger-recover lane): annotations follow the code, and a drop is loud

What went wrong, established from the sidecar's own history (71
distinct sidecar+tree versions, walked with the new remap):

- Region ids are `sha1(path:line:snippet)`, so a sidecar describes
  exactly one tree — but git merges it as text. After the 2026-11-23
  crate merges the committed sidecar described neither parent's tree.
  `--ledger` carried artifacts by exact id only (it never had a content
  remap; its whole history is id carry), so every region whose line or
  path had moved re-entered as GAP, which the gate counts as
  accounted. Re-banked on main at ccb0cabf6: 2099 regions, 1591
  annotated (75.8%), 508 GAP.
- The per-lane re-banks before that attached some artifacts to the
  wrong regions. From 3e276af08 (2026-11-21 16:52) on, artifacts moved
  between neighbouring files in the sidecar's sort order — consistent
  with re-banks that paired rows by position instead of content. All
  55 `arch/*/uaccess.rs` regions, for example, carry F19 (the trap
  entry/exit row) where every earlier sidecar has INV-UACCESS-1.
  Adjudicated groups and their evidence:
  docs/development/unsafe-verification-decisions.tsv.

The rule the tools enforce now:

- `scripts/unsafe_census.py --ledger` remaps annotations by content
  from reference versions of the ledger (a sidecar plus the tree it was
  written against): same owner item, same kind, identical
  comment-stripped text, paired one-to-one and order-preserving; the
  same pairing across git rename records, and for a whole item whose
  fingerprint is unique on both sides. The default references are HEAD,
  MERGE_HEAD and the parents of a merge commit; `--reference REV` and
  `--reference-history` add more. A region is recovered only where
  every reference that annotates that code agrees; a disagreement stays
  GAP and is listed.
- The re-bank prints, per file, the annotated count before and after,
  and exits with status 2 without writing when a file's count would
  drop, unless `--allow-drop FILE` names the file.
- `scripts/unsafe_gate.py --check` fails on any per-file drop in
  annotated count against HEAD, MERGE_HEAD, the parents of a merge
  commit and the previous commit, unless
  docs/development/unsafe-verification-exemptions.tsv carries a row for
  that file, that count, a floor and a reason. Run at each of the ten
  2026-11-23 merge commits, it fails with 1 to 44 unexempted per-file
  drops against a merge parent.
- The tools never rewrite a by-id row. A reviewed change goes in the
  decisions file (`--decide`): one row per group of regions, the value
  it replaces, an exact region count the tool verifies against the
  tree, and the reason.

Limits, stated so that nobody reads a green gate as more than it is.
Both checks count ANNOTATED regions; neither can see a wrong artifact.
The re-bank report's self-check (by-id rows the references annotate
differently) and citation check (does the site's own comment name the
ledger's contract) are what speak to whether an annotation is right,
and the artifact count alone was never proof of that. [lane worktree
ledger-recover]

## Primitive INV index (literal ids — machine-greppable)

Every id below is spelled literally for the unsafe_gate artifact
verifier (compressed "X-1/2/3" prose elsewhere in this file does not
match a literal grep). One line per id; the owning module's header
comment carries the full contract text.

- INV-CBYTES-1 (byte image) — safety/bytes.rs: a `T: Copy` value's byte image is exactly its object representation.
- INV-CBYTES-2 (bounds/alignment) — safety/bytes.rs: sub-object accessors re-derive offsets from the same `T`, checked in-bounds and aligned.
- INV-CBYTES-3 (liveness) — safety/bytes.rs: every accessor takes a reference, so the underlying value is live for the call.
- INV-CBYTES-4 (pinned frame span) — safety/bytes.rs: [off, off+len) lies inside ONE caller-pinned PAGE_SIZE page-cache frame.
- INV-CBYTES-5 (volatile field store) — safety/bytes.rs: advisory volatile store through a live &mut to POD.
- INV-CBYTES-6 (pinned frame run span) — safety/bytes.rs: the span covers caller-pinned, caller-verified-adjacent (direct-map neighbor) PAGE_SIZE frames.
- INV-RING-1 (capacity) — safety/ring.rs: N is a power of two; the mask derives from N, an index can never leave the array.
- INV-RING-2 (SPSC with a CAS consumer) — safety/ring.rs: one producer (the IRQ handler) advances tail; consumers CAS head; put never blocks.
- INV-RING-3 (static liveness) — safety/ring.rs: the ring is a machine-lifetime static of MaybeUninit<T: Copy> cells; volatile cross-hart slot access.
- INV-RING-4 (peek/depth) — safety/ring.rs: depth_hint is a Relaxed hint; the CAS in get is the sole authority.
- INV-WINDOW-1 (bounds) — safety/frame.rs: the KernelStackWindow covers only the stack's own backing pages.
- INV-WINDOW-2 (alignment) — safety/frame.rs: word reads require aligned addresses inside the window.
- INV-WINDOW-3 (stale memory) — safety/frame.rs: reads are volatile — the bytes may be stale or concurrently written task stack.
- INV-FOR-1 (stale adoption) — dfx/forensics_edges.rs: task came from a diagnostic slot; read-only view of a possibly-freed task, pid stable for the struct's lifetime.
- INV-FOR-2 (raw word) — dfx/forensics_edges.rs: addr names memory the producer believes mapped; single volatile read of mapped-or-guarded memory.
- INV-FOR-3 (location key) — dfx/forensics_edges.rs: key was stored from a real event; the value is a rodata-backed Location.
