# The Seed-Core design (晶核) — minimal TCB, every subsystem else a safe crate

Naming (2026-11-22): **Seed-Core / 晶核** — the materials-science metaphor:
the ~3,000-line hand-written core is the SEED CRYSTAL; every service
line crystallizes into safe code on the type-system lattice (forbid +
borrow checking); the compiler is the crystallization condition. The
seed is tiny yet determines the integrity of the whole crystal. The
name deliberately distinguishes from MICROKERNEL (address-space
isolation, ~1,000-cycle IPC boundaries) — this design has no address-
space boundary at all.

Status: DESIGN OF RECORD (2026-11-22, user directive: "shrink the TCB to the
minimum; ideally every subsystem becomes a crate and is safe"). This document
SUPERSEDES the end-state section of crate-plan.md (whose extraction mechanics
— the five-step flip recipe — remain binding). Execution follows this
document rung by rung; nothing lands out of order.

## 1. Goal and the one criterion

The kernel ends as **one small trusted crate plus forbid-enforced service
crates**. The criterion for every line of code is a single question:

> Can this code live under `#![forbid(unsafe_code)]`?

- YES → it belongs in a service crate (compiler-guaranteed safe).
- NO → it belongs in `rux-core`, carries an INV contract or F-row, and is
 covered by the verification stack (Kani/Miri/Verus).

There is no third bucket for UNSAFE. "Framework" as a catch-all for
leftovers is abolished; what remains in the core is there because it
*cannot* be safe, not because it was not yet converted.

### 1a. The size criterion (user directive 2026-11-23): the core is as
small as possible, performance unchanged

The criterion above settles where UNSAFE lives. It does not settle how
much SAFE code stays in the core, and by 2026-11-23 the answer was "most
of it": kernel/src held 286,139 lines with only 1,838 unsafe blocks and
238 unsafe fn/impl bodies. The directive for the rest of the campaign:

> The core's code should be as small as possible, as long as
> performance is unaffected. Code that leaves the core is organized by
> subsystem.

Two measured facts define the method.

**Fact 1 — what actually has to stay is small, and the direction of
the call is what decides it.** Two things force code to stay in the
core:

1. a function that a hot-path site calls directly by name. After the
   move that call would have to go through the hook table (an indirect
   call the compiler cannot inline), so the function stays where the
   hot caller can reach it directly;
2. a FIELD that a hot path reads or writes directly. Cargo forbids
   core -> service, so a field the switch or fault path touches must
   live in the core. This is a constraint on fields, not on whole
   types: a service crate uses a core-defined type freely (service ->
   core is the allowed direction), and a type's cold fields can live
   outside the core behind a pointer the hot fields never follow.
   Measured 2026-11-23 on `Task` (105 fields): the context switch
   touches 7 (thread, kernel_stack_bottom, address_space, state, pid,
   on_cpu, pick_state), the trap path 10, the scheduler 6, the VFS
   walk 7 — about 20 in all. The other ~85 (ptrace state, robust list,
   syscall/exit/reap bookkeeping, statistics) are never read on a hot
   path. Splitting such a type into a core block (the hot fields, same
   offsets, same cache-line neighbours as today) and a service block
   reached by one pointer costs the hot paths nothing, because they
   follow only core-block fields. That split is structural work across
   every subsystem and comes AFTER the function-level moves; until
   then a type with any hot field stays whole in the core, and the
   lanes move its cold METHODS out as free functions.

Everything else leaves. A function that hot paths do not call is
reached only from cold sites, and cold upward calls through the table
are fine; the moved code's own calls INTO the core are direct
cross-crate calls that inline (crate-plan §3a). The service -> core
direction costs nothing. Only core -> service costs, and only on hot
paths.

Measured on the 2026-11-23 tree (scripts/core_size_map.py; hot-path
files are the trap/switch/fault/walk/lock/alloc/tick files; a call in
a hot file that names a function defined elsewhere is an upward hot
call):

| | conservative scan | path-resolved scan |
|---|---|---|
| function bodies in kernel/src (tests excluded) | 228,305 | 237,908 |
| must stay in the core (in a hot file, or called upward from one) | ~97,000 (42%) | 76,194 (32%) |
| leaves the core | ~131,000 (58%) | 159,714 (68%) |

Both scans over-keep: any call a hot file writes to a name defined
elsewhere counts as upward. The path-resolved scan is stricter about
which calls actually reach the callee and so keeps less. The true
figure lies at or below the lower one and is settled per subsystem as
each lane moves it — a function the scans call cold but that turns out
to sit on a hot path stays, and the lane records why. The working
target is therefore: **the core shrinks from 286k lines to under
100k**, the rest organized into per-subsystem service crates.

**Fact 2 — files are mixed, so the unit of the move is the function,
not the file.** Only 34 files (7,122 lines) have no hot-called function
at all. 93 more files are under 25% hot: a few hot functions and a
large cold remainder in the same file. Moving whole files therefore
caps out far below the target. Every subsystem is split at the
function boundary:

- hot functions stay in the core, in the subsystem's core-side file
  (the existing `*_edges.rs` pattern, or `<subsystem>_hot.rs`), where
  hot callers reach them by direct call and the compiler can inline;
- every other function of that subsystem moves to the subsystem's
  service crate: `rux-sync`, `rux-process`, `rux-signal`, `rux-debug`,
  `rux-ipc`, `rux-drivers`, `rux-fs`, `rux-mm`, `rux-sched`, … one crate
  per subsystem, so a reader finds the scheduler's policy in
  `rux-sched` and the futex protocol logic in `rux-sync`, not in a
  bin of miscellaneous leftovers.

**The performance rule that makes this free.** The boundary is placed
so that hot callers never cross it upward. A function that a hot path
calls directly is, by definition, in the core. What moves is what hot
paths do not call: cold upward calls may go through the hook table
(crate-plan §3a rule 2), and the crate calls DOWN into the core
freely — a direct cross-crate call, inlinable. The S1 case study (§4)
is the record of what happens when this rule is broken and how it is
fixed; every lane measures its hot rows before and after.

Per subsystem, the cold remainder measured on 2026-11-23 with the
path-resolved scan (lines of function bodies not called upward from
any hot-path file and not defined inside one):

| subsystem | stays | leaves |
|---|---|---|
| fs | 22,977 | 24,609 |
| mm | 9,509 | 13,682 |
| sync | 3,098 | 17,434 |
| drivers | 409 | 18,100 |
| process | 3,056 | 15,263 |
| debug | 254 | 15,064 |
| arch (three architectures) | 17,235 | 18,848 |
| root files (signal.rs, console.rs, cmdline.rs, …) | 2,738 | 12,292 |
| sched | 12,542 | 2,960 |
| ipc | 0 | 9,857 |
| syscall table, timer, io_uring, net, module | 0 | 6,607 |
| trace, perf, security | 429 | 4,528 |
| safety, interrupt | 3,947 | 0 |

The numbers are a map, not a promise: a "cold" function that turns out
to be on a path the measurement missed stays, and the lane that finds
it records why. What is binding is the direction — every subsystem's
cold majority leaves the core, organized by subsystem, with no hot
call made indirect.

## 2. The irreducible floor (what stays in rux-core, and why)

Every kernel hand-writes these; the reference kernel expresses them in
arch/*.S. Rux's floor, with the campaign's registered rows as evidence:

| Family | Est. dual lines | Reference counterpart | F-rows |
|---|---:|---|---|
| Trap entry/exit + signal frames | ~500 | entry.S, sigreturn | F19 |
| Context switch (3 arches) + tp/SPEL | ~250 | `__switch_to`, cpu_switch_to | F-CS-X/R/A, F-TP |
| uaccess copy bottoms | ~320 | uaccess.S / copy_user_64.S | F-UACC |
| FPU/RVV/FPSIMD save-restore | ~450 | fpu/*.S, vector.S, fpsimd.S | F-FPU/F-RV/F-AFPU |
| Page-table atomics + walks bottom | ~300 | pgtable.h atomics | F20 |
| Boot tables (GDT/TSS/8259/csr/boot.S) | ~350 | head.S et al | F-BOOT |
| CR/MSR/CSR/PIO register ops | ~250 | special_insns.h, io.h | F-CR, F-XE-PIO |
| SBI/hypercall/GIC ABIs | ~150 | kvm.c, irq-gic-v3.c | F-HVCALL, F-GIC |
| Sync primitives' interiors | ~180 | spinlock.c interiors | INV-SY/SCE rows |
| safety/ primitives themselves | ~60 | — | INV-KPTR/MMIO/... |
| Task page lifecycle (INV-TL 6 sites) | ~120 | task freeing | INV-TL |
| Timer/preemption drivers | ~60 | hrtimer cores | — |
| **Core subtotal** | **≈3,000** | | |

Additions that stay core-ADJACENT (TCB by dependency, not by unsafe):
the lock protocol implementations already audited, the printk ring, the
scheduler's switch/tp interfaces. The core's audit surface = the INV/F-row
registry — already 97%+ ledgered.

## 2a. Why not address-space isolation (the microkernel comparison)

The classic way to buy fault isolation is separate address spaces per
service (seL4/L4 family): a bad driver cannot corrupt kernel memory
because it cannot address it. The price is the boundary — every
cross-service operation is an IPC through a context switch, ~1,000
cycles on the best fastpaths, multiplied along fault->driver->server
chains (the Mach lesson; L4's fastpaths recovered much but not all of
syscall-heavy paths).

This design buys the SAME memory-safety property statically, at zero
runtime cost: a `#![forbid(unsafe_code)]` crate CANNOT EXPRESS a wild
pointer, use-after-free, or data race — the Rust type system provides
what address spaces provide in a C microkernel, without a runtime
boundary. Cross-crate calls are plain function calls (LTO inlines; the
M3 flip verified zero behavior change with the dispatch tables live).
This is the RedLeaf/Framekernel line: language isolation without
address spaces, keeping the hardware backstop (below) instead of
betting everything on the type system.

What remains true for both designs: logic bugs (deadlocks, wrong
decisions, leaks) pass through; only memory-safety classes differ.

**The hardware backstop — MPK (Layer 3 of the safety plan, measured):**
for defense-in-depth BELOW the type layer, Intel protection keys give
page-grain hardware domains inside the ONE address space: WRPKRU
transitions measured at **47-49 cycles** on this host (100k TSC-timed
ops; the ERIM native band is 20-90), vs ~1,000+ for an address-space
switch with TLB flush. Protected pages fault in hardware outside their
window (fault-injection proven: the identical write lands with the key
enabled and takes a hardware #PF with it disabled). The mpkguard
framework is landed behind `mpkguard=1` (default off): boot arm with
explicit keys, PKRU zones, context-switch re-arm, tagged-page
discipline, the escape self-test. Phase-1 target: the vring AVAIL
sections (~2.4k windows/s at pbench fs rates — the <1% budget holds);
GRQ (~160k/s) is phase-2 candidate by measured frequency.

The end-state defense stack, each layer at its measured cost:
1. TYPE layer (0 cycles): forbid crates — service columns structurally
 immune to memory unsafety.
2. HARDWARE layer (~50 cycles/window, <1%): MPK-protected core
 metadata — even a type-system soundness hole or smuggled unsafe
 meets a hardware fault, not silent corruption.
3. VERIFICATION layer (0 runtime): Kani/Miri/Verus over the ~3,000
 core lines — the remaining hand-written unsafe is machine-checked.

A microkernel spends ~1,000 cycles per boundary to buy (1); this design
buys (1)+(2)+(3) for 0+~50+0.

## 2b. The landscape: industry Rust kernels, and where Seed-Core stands

Industry Rust kernels fall into four broad families. Each answers "where
does unsafe live, and how is the TCB drawn" differently. Seed-Core
(defined in §1) is not one family carried to an extreme — it COMBINES
the specific strengths of several of them and hardens the one thing
none of them machine-checks.

**Family 1 — microkernels: Redox, Tock, Hubris.** Redox is a
MINIX-inspired microkernel (~16,000-line kernel; drivers and services
run in userspace, isolated by the MMU). Tock targets embedded: a
minimal trusted kernel with drivers compiled in as "capsules" written
in Safe Rust, user processes protected by the MPU — at the price that
every added capsule grows the TCB (measured TCB share ~43.8%). Hubris
(Oxide) assigns all tasks statically at build time; kernel, drivers,
and tasks live in separate protection domains enforced by hardware
memory protection. Common position: isolation by HARDWARE (MMU/MPU), a
tiny kernel, and an IPC cost on every cross-component call.

**Family 2 — the framekernel: Asterinas (the closest relative).**
A framekernel (the term comes from Asterinas, USENIX ATC'25) splits the
kernel LOGICALLY into a Framework (the only zone allowed unsafe) and
Services (all Safe Rust), in ONE address space and privilege level —
service calls are plain function calls, no IPC overhead. Measured:
TCB share ~14.0% (vs Tock 43.8%, Theseus 62.4%, RedLeaf 66.1%);
LMBench geometric mean 1.08x of the reference monolithic, Nginx 1.17,
Redis 1.31. This is structurally the same mechanism/policy split our
design implements — the difference is that Asterinas' Framework is a
LOGICAL layer held by convention and review inside one crate, while we
draw the boundary on the CRATE COMPILATION UNIT and let the toolchain
check it. That is a difference in enforcement hardness, not style.

**Family 3 — leaf-style: the in-tree Rust leaf work in the reference
monolithic.** No kernel rewrite: scheduler, mm, VFS, and RCU remain C;
Rust lives in the leaves (drivers, file-system modules, binder-class
subsystems). The unsafe boundary is the safe wrapper set the kernel
crate exports; underneath it is FFI into C. The TCB is the ENTIRE C
kernel — the Rust-side guarantees cover only newly written leaves. Its
reference value for us is not architecture but ABSTRACTION CRAFT:
deliberately minimal traits (FileSystem, Operations, INodeOperations
kept separate rather than one 50-method mega-trait), static dispatch
preferred over trait objects, thin mechanical error mapping. We use
exactly these techniques to keep our indirection costs low.

**Family 4 — single-address-space cells: Theseus.** One address space,
one privilege level; the thesis is "intralingual design" — hand as
much OS bookkeeping as possible to the Rust compiler itself — with
components as crates ("cells") that can load dynamically, to avoid
state spill (persistent state changes caused by component interaction
that make fault isolation and live update hard). TCB share ~62.4%,
higher than Asterinas because more stays trusted. Theseus explicitly
justifies the single-address-space, single-privilege model as matching
the Rust compiler's execution model — an independent endorsement of
our mechanism-stays-in-core decision.

| Family | Representative | Isolation mechanism | Where unsafe lives | TCB share |
|---|---|---|---|---|
| Microkernel | Redox / Tock / Hubris | MMU/MPU + userspace | A very small kernel core | Tock ~43.8% |
| Framekernel | Asterinas | Compiler + one address space | The Framework layer | ~14.0% |
| Leaf-style | RfL in the monolithic | C kernel + Safe Rust leaves | The entire C kernel | n/a |
| Single-address-space cells | Theseus | Compiler + dynamically loaded crates | Trusted components | ~62.4% |
| **Seed-Core (ours)** | **Rux** | **Compiler at crate granularity + Cargo DAG + hardware backstop** | **An itemized ~3,000-line floor under the INV/F-row ledger** | **services 0 by construction; core = the ledger** |

**What Seed-Core takes from each family, and what it adds.** From the
microkernels: the discipline of a minimal trusted core — our floor is
itemized line by line with a reference counterpart and an F-row id per
entry, not a zone. From the framekernel: the single address space and
the zero-IPC boundary — cross-crate calls are plain function calls,
LTO-inlined, a full protocol layer thinner than RPC; where a interface's
indirect-call cost surfaced, we engineered it away (context-switch
+25.1% recovered to +1.7% by devirtualization) rather than accepting
it. From Theseus: the crate as THE isolation unit, and the
single-address-space execution model as the one the compiler itself
understands. From the leaf-style work: the minimal-trait,
static-dispatch, thin-error-mapping craft that keeps our hook tables
cheap. What NO other Rust kernel does — Asterinas included — is put
the boundary in the BUILD SYSTEM: per-crate `#![forbid(unsafe_code)]`
(writing unsafe in the service crates is a COMPILE ERROR, not a review
finding — negative-probe verified), Cargo's package DAG refusing
dependency cycles (layering as a build constraint), a CI ratchet on
the attribute set, and an MPK hardware backstop behind the type layer
(WRPKRU at 47-49 measured cycles, fault-injection proven) where the
type-soundness bets of the other families have none. A logical layer
erodes by convention; a build error cannot be written. That is the
claim: monolithic-class performance, a TCB smaller than every measured
share above, and the only boundary in the space defended by the
toolchain rather than the process.

### 2b-1. Focused deltas vs the two closest relatives

| Axis | Asterinas (Framekernel, USENIX ATC'25) | RedLeaf (PLOS'23) | Seed-Core (this design) |
|---|---|---|---|
| Isolation enforced by | Convention + review inside ONE kernel crate | The type system, via inter-domain zero-copy RPC | The COMPILER at crate granularity: per-crate `#![forbid(unsafe_code)]` (negative-probe verified) + Cargo's package DAG refusing cycles (dependency-direction acyclicity machine-checked) + a CI ratchet on the attribute set |
| Boundary shape | Intra-binary function calls | Typed RPC channels over domain boundaries | Plain cross-crate function calls, LTO-inlined — thinner than RPC by a whole protocol layer |
| Type-soundness failure mode | No backstop | No backstop (their stated bet) | MPK hardware backstop: pkey-tagged core metadata pages fault outside their window (WRPKRU at 47-49 measured cycles; fault-injection proven) |
| TCB form | The framework zone (review-audited) | A small verified runtime | ~3,000 lines ITEMIZED (each with reference counterpart + F-row id), covered by the verification stack (Kani all-input proofs, Miri zero-UB, Verus narrow-adopt for unbounded arithmetic lemmas) |
| Unique here | — | — | The census/INV/F-row ledger (97%+ artifact coverage: the audit surface is a ledger, not a code read); the policy/core splits S1/M1 (decision logic exits the core, mechanism stays); the binding measure-first flip ladder with shelving-as-success |
| Where they lead | Full syscall surface shipped earlier | Complete zero-copy-RPC formalism | — (honest) |

## 3. The end-state topology

```
rux-core (≈3,000-3,500 dual lines; the entire TCB)
 arch/{x86_64,riscv64,aarch64} entry/switch/uaccess/pt/asm zones
 safety/ sync-primitives task-lifecycle timer-core printk-ring
 ← verification stack: Kani 12/12, Miri 0-UB, Verus narrow-adopt

rux-sched forbid-safe (S1 DONE 2026-11-23: sched_policy 5,488
 lines sole-copied into the crate — fair/rt/deadline/
 wakewatch/census policy; SchedHooks 78 rows after the
 observability devirtualization (was 111: statics moved
 to framework entities.rs with direct fns, three rows
 arm-gated); p2_ctxsw +1.7% vs same-window main after
 the fix (was +25.1%)
rux-mm forbid-safe (M1a DONE 2026-11-23, origin c0e46ed73: the
 per-arch mm policy — mmap/munmap placement, brk, stack
 alloc, permission changes, VMA fork decisions, fault
 routing, swap fault, fault-around — 6,876 lines with ZERO
 unsafe blocks; 6-row mm_hooks interface over ~40 framework
 call sites; p3b_fault_cow 1.994 within the same-day
 control band 1.974-2.024. M1b DONE 2026-11-23: seven of
 the nine §4 policy files (hugepage, kswapd, oom_kill,
 vmscan, lru, compact, swap — 3,706 lines) sole-copied
 under the crate forbid via the 31-row MmPolicyHooks
 interface + interface-vocabulary codec/types; swap's static-mut
 device table converted to full interior mutability
 (framework unsafe -2 blocks). SHELVED with numbers
 (mm-split-rung.md): vma + mm_struct — the TYPE
 substrate (88/50 framework-side type refs, zero fn
 refs; inherent impls must live with their types; the
 interface itself names MmStruct/VmaFlags/VmaType and Cargo
 forbids the rux->rux-mm cycle). Unblocks only via a
 types-in-shared-crate redesign, not a file move)
rux-syscall forbid-safe (DONE at M3; the pattern-proven first rung)
rux-fs forbid-safe (DONE 2026-11-23, origin 50f0cc97a: pipe/fifo/
 inotify/memfd/selinuxfs sole-copied under crate forbid;
 FsHooks 26 rows + the service-test registration. Two boot
 regressions caught by the x86_64 battery and fixed
 in-merge: early selinuxfs mount_register caused the 'R'
 beacon reboot loop; the pipe_file_close clear/take
 inversion panicked on every pipe close)
rux-ext4 forbid-safe (deepest fs consumer)
rux-net forbid-safe (DONE at R2)
rux-drivers forbid-safe (DONE 2026-11-23, origin 8672a0ebf:
 watchdog/access_tokenid/loopback/fb_simple/fbdev
 sole-copied, 1,156 lines with zero unsafe of their own;
 virtio/intc/timer/pci/drm stay on the floor with
 per-family reasons; 12 DriverHooks rows + the leveled
 initcall table)
rux-ipc forbid-safe (DONE 2026-11-23, ipc-flip lane: the
 movable policy — sysv_msg/sysv_shm/posix_mq/keys/
 access_token, 5,406 lines — sole-copied under crate
 forbid; the unsafe blocks converted via the
 uaccess_safe typed twins (+copy_in_t_slice), live
 &Task exit signatures, and framework ipc/edges.rs
 (INV-IPC-PAGEOP-1); IpcHooks 24 rows, all COLD paths.
 SHELVED with numbers: binder.rs — 46 blocks + 10
 unsafe impls in 7 floor-shaped clusters (TxnBlock
 block-view windows INV-BINDER-BLOCKVIEW ~14 sites,
 UnsafeCell interiors 4, Send/Sync markers on the
 raw-pointer object model 10, intrusive
 key/container_of 2, file->Arc adoption 4, trap-regs/
 page-table diagnostic walkers 15, task raw derefs 5);
 needs a binder_substrate extraction + object-model
 Send/Sync restructure, a lane of its own.
 sysv_sem + util — SHELVED on a BEHAVIORAL finding:
 sysv_sem compiled in the crate deadlocks the
 sysv_sem_block cross-task test (helper kthread
 linked/on_rq/vruntime=0, never scheduled; boot task
 park-loops with instant schedule() returns; 4/4
 repro, position- and load-independent) while the
 byte-identical source in-tree (interface state) passes
 809/809 and fork-point base 801/801 — the delta is
 the crate packaging alone; root cause open, taskdump
 evidence in the lane record. util stays framework as
 the shared substrate both sides consume)
rux-debug-svc forbid-safe (subject to §7 retirement review)
```

Cargo refuses cycles: services → core (and → lower services) only; the
constraint is machine-checked once the rungs land.

**The census effect**: every service crate's unsafe count is structurally
zero (compiler-guaranteed). The whole-kernel dual number BECOMES the core
size. See §6 for the stage math.

## 4. The two policy splits (the new rungs this design adds)

> Naming (user directive, 2026-11-22): the service crates carry NO
> -policy suffix — `rux-sched` (S1) and `rux-mm` (M1). The split
> design below is unchanged; only the crate names shorten.

The campaign's earlier framing kept sched and mm whole in "the framework".
Both bundle decidable policy over a small unsafe core; the splits separate
them.

### Rung S1 — sched policy / core split

**S1 is the first compiler-enforced implementation of MECHANISM/POLICY
SEPARATION** (the classic OS design principle): the MECHANISM — the
low-level state-changing operations (context switch, the global run
queue's locking protocol, the current-task pointer's register read,
task-slot allocation and reclamation) — directly maintains the kernel's
data-structure invariants and stays in the core crate under audit; the
POLICY — the decisions built on the mechanism (which task runs next,
which CPU a wake-up is placed on, when to throttle) — accesses data only
through the mechanism's type-safe interface and moves to a separate
service crate under forbid. Traditionally this separation was maintained
by code review alone; here the compiler statically guarantees it: any
unsafe use in policy code is a compile error. The callback-function
table and the private-interface API constitute the mechanism's complete
calling contract published to the policy. M1 applies the same
separation to mm.

- MOVES to `rux-sched` (forbid): CFS/RT/deadline decision logic,
 fairness math, wakeup placement policy, load-balancing decisions,
 cgroup scheduling policy, idle balancing, the census/diagnostic walks.
- STAYS in core: `__switch_to` + tp derivation + stack switch (F-CS/F-TP),
 the GRQ lock primitive (owner-in-word), the terminal-switch protocol
 (reapdrain's R85/pacing machinery — page-identity TCB), NEED_RESCHED
 atomics, the runqueue's intrusive links under the lock discipline.
- THE INTERFACE: policy calls core via safe APIs — `switch_to(next)`,
 `current()`, `grq_lock()/unlock()`, `enqueue/dequeue(task_ref)` — the
 typed handles (TaskBorrow/TaskRef) already exist for exactly this.
- MEASURE FIRST (the standing rule): count policy→core and core→policy
 edges on the current tree before any move. Abort criterion: if the
 policy's decision logic reads raw task pointers in more than a handful
 of sites that cannot ride `task_shared`, the rung shelves with numbers.

#### S1 case study: the interface-cost regression and its fix (BOTH guarantees held)

S1 landed with a performance regression that the merge gate refused to
wave through, and its resolution is the standing proof that this design
guarantees safety AND performance together — neither is traded for the
other. The record, with numbers:

1. **The regression.** After the flip, pbench p2_ctxsw read 2.149 µs
 against 1.718 µs on same-window main (+25.1%) — outside the ±20%
 sched guarded-domain band. The merge was held until fixed (gate
 policy: functionally correct but performance-regressing does not
 merge).
2. **Root cause — NOT "cross-crate calls are slow".** Direct
 cross-crate calls inline fine (rlib metadata carries MIR; LTO
 completes it). The cost was the DISPATCH MECHANISM: upward calls
 (core→policy) ride a registered function-pointer table, and a call
 through a function pointer is indirect — the callee is a runtime
 value loaded from the table, invisible to the compiler, NOT
 inlinable by LTO or anything else. The first SchedHooks table
 exposed 111 rows, and observability dominated: per context switch
 ~40 rows fired (sched_cs_stamp ×4/schedule, pick_trace ×2-3/switch,
 pick_gen publish/retire, ww_record, wake census, vr_canary, class
 counters), each paying shim → wrapper → slot load → indirect call.
 At ~10 ns each in the debug class that is ~400 ns — the measured
 delta (431 ns) to within noise.
3. **The fix — make the hot calls direct again.** Three moves, no
 behavior change: (a) the 33 observability rows LEFT the table —
 their statics now live framework-side (entities.rs) and the
 framework calls them as direct static calls, which inline; counters
 record identically. (b) The genuinely dynamic dispatch collapsed to
 ONE fat entry: pick_next_task — the whole pick walk runs policy-side
 inside a single crossing, not one crossing per step. (c) The three
 conditional diagnostic rows became arm-gated wrappers (the wrapper
 checks the same framework-side flag the body checks; disarmed boots
 never touch the interface). Armed-mode output verified byte-identical
 (the exact 40-line WWTICK budget).
4. **Result.** p2_ctxsw 1.758 vs same-window main 1.728 = **+1.7%,
 in band**; P4 storm rows +2.6%; zero panics across 10 measurement
 boots + the armed boot; rux-sched's forbid untouched — the fix cost
 ZERO unsafe: it moved plain-safe state and reshaped call sites.
5. **The rule extracted** (crate-plan §3a, binding for every future
 interface): fn-pointer table rows are for COLD paths only; a hot path
 takes (a) the whole loop behind one fat entry so the boundary is
 crossed once, (b) an existing vtable FIELD (FileOps.fault/write_at —
 field dispatch, R5's read/write paths at zero new indirection), or
 (c) direct static calls over framework-side statics. Observability
 statics live framework-side by default.

The lesson stated once, plainly: the crate boundary's safety is free;
the DISPATCH MECHANISM is what costs. Design the interface, measure it, and if it
regresses, fix the interface — never the safety property.

### Rung M1 — mm policy / core split

- MOVES to `rux-mm` (forbid): the VMA tree and its lock protocol's
 policy side, mmap/munmap/mprotect DECISIONS, COW/fork mapping policy,
 fault ROUTING (which arm), brk policy, oom/kill policy, compaction
 decisions, kswapd policy.
- STAYS in core: page-table walks + PTE atomics (PtTable primitives),
 the uaccess bottoms, the fill/copy frame primitives (FrameWindow,
 INV-PC-WIN), the VMA-find lock rule's primitive side (find under
 read-guard as a core primitive — the defect-C lesson makes this interface
 explicit), buddy allocator (already Framework floor), vmemmap/page_desc.
- THE INTERFACE: policy consumes `PtTable`, `FrameWindow`, `vma_find_guarded`,
 `map/unmap_page` safe APIs. The fault path becomes: core traps →
 policy decides → core executes the primitive.

### The existing rungs (unchanged, per crate-plan.md)

- Rung 1 syscall: M1-M3 (in flight; State B = file deletion closes it).
- Rung 2 net: measured at ~35 reverse sites (~2 lane-days post-interface).
- Rung 3 fs-svc, Rung 4 drivers, Rung 5 ext4: measured first, per the
 standing method; a rung whose reverse surface will not decompose is
 SHELVED with the numbers recorded in this document's §8.

## 5. What happens to the edges files

The per-subsystem `edges.rs`/`unsafe_edges.rs` files dissolve by rung:
each of their functions is either (a) a call into a core primitive — the
edge fn becomes a thin re-export and then dies, or (b) genuinely local
unsafe — it MOVES into rux-core under its INV row. At the end state there
are no per-subsystem edges files; there is the core's registry.

## 6. The census math (honest, staged)

Current: ~10,300 dual (whole kernel, single-crate census).

| Stage | Lands | Expected dual |
|---|---|---:|
| Now | — | ~10,300 |
| + Rung 1 close (syscall crate, files deleted) | census column split begins | ~10,000 (unchanged mass; structural change) |
| + Rung 2 net | net column → 0 | −~200 |
| + S1 sched-policy split | sched policy column → 0 | −~300 |
| + M1 mm-policy split | mm policy column → 0 | −~250 |
| + Rung 3 fs-svc + 5 ext4 | fs service columns → 0 | −~2,400 |
| + Rung 4 drivers | drivers column → 0 | −~370 |
| + debug retirement review (§7) | dead instruments retire | −600~1,000 (EXECUTED 2026-11-23: −915 debug/ lines, r83hunt+treehunt; record in docs/development/debug-retirement-review.md §7a) |
| **End state** | **= rux-core size** | **≈4,000-4,700** |

The ≤5,000 target is met with margin iff the debug review retires its
dead instruments; without it the landing is ≈5,000-5,400 and the review
becomes decisive. The percentage target (≤1%) is met trivially at any
of these values.

## 7. The debug retirement review (feeds §6)

Per-instrument adjudication: hunt closed → instrument retires (delete);
hunt alive → instrument moves to `rux-debug-svc` behind its flag (forbid if
its unsafe can ride core primitives; else core-adjacent with INV rows).
Input: L4's alive-call-site census (scribble 49 sites, leakorigin 26,
r83hunt 8, wildwatch 5, treehunt 1). Each verdict one line in §8's table.

EXECUTED (2026-11-23, debug-retire lane): the full adjudication record,
per-family verdicts with evidence and exit criteria, lives in
docs/development/debug-retirement-review.md. Phase 1 landed (r83hunt and
treehunt deleted with their call-site stubs and gate events; −915
debug/ lines); the KEEP families (scribble, leakorigin, wildwatch +
the 6 permanent switches) carry written exit criteria; the debug→debug
directory rename is deferred on crate-lane coordination (it rewrites
`crate::debug::` tokens inside the six service crates), recipe current
in the review doc §8.

## 8. The execution ladder (order is binding; each rung = measure → design interface → flip → forbid → battery)

1. **R1 close** (in flight): syscall State B — delete in-lib files, full
 battery, negative probe. Gate: triple-arch, make test, fio 3-case,
 smoke; census columns begin.
2. **S1 measure** (next): sched policy/core edge count (one day, no code).
 Then interface design → policy crate flip → forbid → battery incl. P4
 storm + ctxsw pbench (guarded domain!).
3. **M1 measure**: mm policy/core edges. Then as S1 (battery incl. the
 defect-C pair + mmap LTP rows).
4. **R2 net**: the measured 35-site relocation + flip.
5. **R3 fs-svc + R5 ext4**: the biggest mass; measured per crate-plan.
6. **R4 drivers**: last service rung.
7. **debug review**: per §7.
8. **Core audit freeze**: the final INV/F-row registry pass — every core
 line under a row; the per-block irreducibility proof (the D-round
 deliverable) then covers exactly rux-core.

Every rung's gates: the standing set (triple-arch build, make test,
unsafe_gate, print ratchet, smoke) + the rung's domain battery + the
forbid negative probe at crate scope. A rung that fails its battery
twice halts the ladder (no piling changes on an unstable base).

## 9. Risks and honest limits

- **The core is still ~3,000 lines of hand-written unsafe.** The design
 minimizes it to the reference kernel's own floor class; it does not
 eliminate it. The verification stack is the compensating control.
- **Policy/core interfaces can be wrong** (the defect-C lesson: interfaces between
 lock domains are where races live). Each split's interface gets the fault
 battery, not just the suite.
- **Hot-path cost**: policy-in-crate means cross-crate calls on scheduler
 and fault paths. Mitigation: LTO + inlining across the workspace is
 already the build shape; the ctxsw/fault pbench rows gate each split.
- **Rung shelving is success, not failure**: a measured non-decomposable
 surface keeps that subsystem core-side with its numbers recorded here.

## 10. Relationship to existing documents

- crate-plan.md: extraction mechanics remain binding; its end-state
 section is superseded by §3 here.
- unsafe-5000-plan.md Part I: the ladder's D-round proof now targets
 rux-core (§8 step 8). Part II status board unchanged.
- safety-and-performance.md: the INV/F-row registry becomes the core's
 audit surface; no format change.
