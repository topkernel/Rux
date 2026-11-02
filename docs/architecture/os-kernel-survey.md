# Operating-System Kernel Design Survey — designs the reference kernel lacks, and what Rux adopts

Round 1 of the arch-research lane (charter in `TASK.md`, 2026-11-03). Scope:
survey industry operating-system architectures for designs the mainstream
reference kernel does NOT have, and judge each against the house divergence
rule — external POSIX/ABI compatibility intact, LTP green on affected
families, no upper-layer software breakage. Companion documents:
`design-divergences.md` (the register of KEPT divergences, D-1..D-18) and
`rust-design-headroom.md` (the eight internal levers, §1–§8).

Method: primary sources (papers, project docs, LWN, mailing lists) with
numbers preferred over adjectives. No kernel code changed this round; this
document is research + verdicts only. Any divergence acceptance lands in
the register when a future prototype lane delivers evidence. The rig for
future numbers is x86_64 + KVM (platform-priority directive 2026-10-31).

Verdict scale:
- **ADOPT** — evidence and fit are strong enough to schedule a prototype
  lane now.
- **PROTOTYPE** — promising but unproven on our workloads; a bounded
  worktree lane must deliver evidence before any register entry.
- **REJECT** — not adopted, with the reason stated (ABI risk, no consumer,
  contradicted by our own data, or parity work rather than divergence).

## 0. Summary verdict table

| # | Design | Where it ships | Reference kernel lacks it? | Verdict |
|---|--------|----------------|---------------------------|---------|
| 1.1 | Redox scheme/URL namespace | Redox OS (2015–) | Yes (resource URLs, userspace-provider-first) | REJECT (ABI); internal slice already covered by D-6 |
| 1.2 | Theseus intralingual design | Theseus (OSDI '20) | Yes (types carry kernel invariants) | ADOPT (continue the §1–§8 program) |
| 1.3 | Hermit unikernel fast paths | RustyHermit, OSv | Yes (syscall-as-function-call) | REJECT (contradicts multi-process ABI) |
| 2.1 | seL4 capabilities + formal verification | seL4 (SOSP '09), CertiKOS | Yes (machine-checked kernel proofs) | ADOPT the *method* on concurrency primitives; REJECT capability-kernel architecture |
| 2.2 | Zircon handle table + vDSO-only surface | Fuchsia/Zircon (Nest Hub, 2021–) | Yes (rights per handle; syscall entry restricted) | PROTOTYPE (internal rights-checked object table); vDSO itself is parity work, not divergence |
| 3.1 | Barrelfish multikernel (per-core state, messaging) | Barrelfish (SOSP '09) | Yes (no message-only inter-core model) | REJECT whole-arch at 4 cores (contradicts D-1 ruling); message-slice idea parked |
| 3.2 | Flat combining | SPAA '10; DISC '10 queues | Partially (no in-tree flat-combined hot structures) | REJECT now — our GRQ hold data is volume-coupled, not contention-bound |
| 4.1 | FlexSC detached syscall batching | OSDI '10 (research) | Yes (exception-less generic syscalls) | REJECT (io_uring covers the batch-async need with less surface) |
| 4.2 | Pervasive async / io_uring lineage | Reference kernel 5.1+; Rux io_uring module exists | No (parity) | PROTOTYPE the *internal* completion-ring substrate for writeback |
| 4.3 | vDSO-only syscall entry | Zircon; reference has a vDSO too | Rux lacks a vDSO on x86_64 (reference has one) | Parity gap, not divergence — flagged for the perf lane, not this register |
| 5.1 | EEVDF (post-CFS fairness) | Reference kernel v6.6+ | No (parity) | Parity item — excluded from shortlist; D-14 already covers our preempt-dance fix |
| 5.2 | Pluggable scheduling (ghOSt / sched_ext) | ghOSt SOSP '21; sched_ext v6.12 | No (parity) | REJECT for Rux now (no in-kernel BPF host; no consumer) |
| 6.1 | uffd-style page-fault delegation | Reference kernel 4.3+ | No (parity); Rux lacks it too | REJECT (no internal consumer today) |
| 6.2 | Persistent-memory-first FS | NOVA (FAST '16); Optane (EOL 2022) | Partially (DAX exists; PM hardware withdrawn) | REJECT (no PM device in the rig; Optane discontinued) |
| 7.1 | Capsicum rights-carrying descriptors | FreeBSD 9 (2012) | Yes (upstream RFC 2014 not taken) | REJECT (access_token/tokenid already serve the OH sandbox need) |
| 7.2 | Plan 9 per-process namespaces | Plan 9 / 9front | Yes (reference namespaces are coarser, clone-time) | REJECT now (no consumer; deep VFS change) |
| 7.3 | RUMP anykernel (in-host kernel modules) | NetBSD rump (2009–) | Yes as a first-class test discipline | PROTOTYPE (in-host tests of ext4/jbd2) |
| 7.4 | Model checking on kernel primitives | seL4/CertiKOS method, Rust tooling (Kani, loom) | Yes (mainstream kernels verify by review + fuzzing) | ADOPT (pilot harness on magazine/GRQ/futex) |

---

## 1. Rust-native kernels

### 1.1 Redox: scheme/URL resource namespace

**What it is.** Every resource is addressed as a URL — `scheme:resource`
(e.g. `file:/home/x`, `tcp:127.0.0.1/3000`, `disk:0:0`). A *scheme* is at
once the resource type, the namespace root, and the userspace daemon that
implements it; the kernel only routes `open` to the scheme's provider and
the provider defines read/write/seek semantics. The Redox Book lists the
kernel-implemented set (irq, event, proc, memory, time, ...) as the small
exception — policy is "when possible, schemes should be implemented in
userspace", so drivers and filesystems (redoxfs as `file:`, `ptyd`,
`smolnetd` for tcp/udp/ip) are daemons. Sources: Redox Book, Schemes
chapter (https://doc.redox-os.org/book/schemes.html); the `redox-scheme`
crate for writing providers (https://lib.rs/crates/redox-scheme); the
"10 Years of Redox" retrospective notes the namespace participates only in
Open — subsequent I/O goes through the open handle (the Scheme API converts
messages), i.e. the URL layer is a *resolution-time* mechanism, not a
per-operation cost.

**What the reference kernel lacks.** Resource addressing is plain paths
with mount composition; there is no uniform "provider" abstraction where a
driver daemon is the default implementation vehicle for a resource class
(userspace drivers exist there per-device — FUSE, vfio — not as the
composition rule).

**Rux verdict: REJECT (as an external design), already covered internally.**
The external URL surface breaks POSIX path semantics outright (gate 1
fails). The internal essence — one uniform dispatch trait behind every
resource type, missing-op behavior decided in one place — is precisely
divergence D-6 (`FileOps::builder()` capability declarations), already
landed tree-wide with the `fileops-caps` shape tests. No further adoption.
Cost: n/a. Dependencies: none.

### 1.2 Theseus: intralingual design (types carry kernel invariants)

**What it is.** "Intralingual design": OS invariants are expressed in the
same language-level mechanisms the compiler already checks (types, traits,
ownership), so conventions enforced elsewhere by review become compile-time
facts. The OSDI 2020 paper ("Theseus: an Experiment in Operating System
Structure and State Management", Boos, Ijaz (Rice); Liyanage, Zhong (Yale);
https://www.usenix.org/conference/osdi20/presentation/boos) names the
second axis: minimizing *state spill* — state one component holds about
another — via many tiny components with runtime-persistent bounds, which is
what enables their live evolution / fault recovery experiments. The Theseus
book records follow-up work connecting intralingual design with formal
verification (https://www.theseus-os.com/Theseus/book/misc/papers_presentations.html),
and the repo (https://github.com/theseus-os/Theseus) keeps the system
buildable.

**What the reference kernel lacks.** Any compile-time invariant enforcement
in C beyond the type checker; its Rust adoption (drivers, since v6.1
onward) is gradual and per-subsystem, with no intralingual structuring
goal.

**Rux verdict: ADOPT — this is the house program, continue it.** The
eight-lever list (`rust-design-headroom.md`) is an intralingual program by
another name, and four levers already landed as register entries with
evidence: §1 → D-6 (capability-checked file ops), §3 → D-7 (typed per-CPU)
and D-11 (typed lock keys), §4 → D-4/D-15 (procfile/tracepoint single
declarations), §5 → D-10 (Result-typed errors). The unlanded remainder is
§2 (ownership campaign), §6 (binder transaction state machine), §7 (typed
timer wheel), §8 (driver capability proofs) — see the cross-reference in
§8 of this document and the shortlist in §9. The pieces of Theseus Rux does
NOT take: live evolution / hot module swapping (no Rux consumer; the
state-transfer machinery is a project in itself) and full zero-unsafe
outside core (Rux's kernel uses targeted unsafe with documented SAFETY
comments; converting the whole tree is not on any goal's critical path).
Three-gate risk: none by construction — every lever is internal-only; the
gate that bites is the perf gate (typed guards must inline to the same
instructions; the D-7 rodata incident is the recorded failure mode and its
guard — a release-strong boot before any re-land). Cost: per-lever, see
shortlist. Dependencies: none external.

### 1.3 Hermit/RustyHermit: unikernel no-syscall fast paths

**What it is.** A library OS: the kernel is linked into the application,
so "syscalls" are plain function calls — no trap, no privilege transition.
RustyHermit ("RustyHermit: A Scalable, Predictable, and Elastic Unikernel
for Multi-core Systems", CuKU workshop 2018; SIGAPP Applied Computing
Reviews 2020; https://github.com/hermit-os/kernel) targets HPC
single-application workloads; rhyve is its dedicated hypervisor
(https://github.com/hermit-os/rhyve). The Virginia Tech 2023 dissertation
"Improving Operating System Security, Reliability, and Performance Using
Rust and RustyHermit" (M. Sung,
https://vtechworks.lib.vt.edu/bitstreams/ae3b79d1-c54b-467d-be64-42d34dcd6b99/download)
measures the property directly: sub-microsecond operation latency, syscall
overhead ~0 because there is no boundary; against that, a hypervisor
exit/entry costs the 1–3 µs class. OSv made the same argument earlier
(https://mikelangelo-project.eu/technology/universal-unikernel-osv).

**What the reference kernel lacks.** A single-address-space, single-privilege
deployment mode; its kernel-as-library ports are test artifacts, not a
product shape.

**Rux verdict: REJECT.** Rux exists to boot unmodified multi-process distro
images (Ubuntu 22.04, OpenHarmony) — the unikernel model removes the
process/isolation ABI those images are built on (gate 1 unfixable). The
only internal slice worth keeping is already had: Rux's in-kernel callers
of VFS/IPC code pay function-call dispatch, not traps. Cost: n/a.
Dependencies: none.

---

## 2. Capability / object kernels

### 2.1 seL4: capabilities + machine-checked correctness

**What it is.** A microkernel with a machine-checked proof of functional
correctness: the SOSP 2009 paper ("seL4: Formal Verification of an OS
Kernel", Klein et al.) reports ~8,700 lines of C verified against ~170k
lines of Isabelle/HOL proof script, ~20 person-years total effort (~11
kernel-specific); the accumulated proof base passed 1M lines by 2022
(Heiser, seL4 Summit 2022 state-of-research talk). Performance was kept:
the IPC fastpath runs in hundreds of cycles on the era's ARM11 target, and
no functional-correctness defect has been found in the proven kernel in
15+ years of deployment (Hensoldt/Cyber, the DARPA HACMS Boeing AH-6
program). CertiKOS (OSDI '16, Gu et al.) demonstrates the method extends
to multiprocessor kernels. Capabilities are the access-control model:
every authority is a kernel-tracked capability, and the proof covers
authority confinement (integrity).

**What the reference kernel lacks.** Machine-checked proofs of any kernel
component; authority confinement as a type/proof property (its access
control is DAC/LSM policy layers, checked at runtime, verified by review).

**Rux verdict: split.** The capability-kernel *architecture* is REJECTED —
Rux is monolithic with the POSIX syscall surface; a capability decomposition
would rewrite every subsystem against gates that exist precisely to prevent
that. The *method* is ADOPTED as a bounded pilot: model checking of our
concurrency primitives with Rust-native tooling (Kani bounded model
checking; loom-style exhaustive interleaving). The repo's paid incident
list is exactly the class these tools catch: r8final magazine two-CPU
re-entry (D-7's motivating incident), the R74 double-pick torn-context
family, the 2b092c49 discarded-guard GRQ incident (D-11), the D-7 rodata
codegen class (which Kani would NOT catch, but the shape suite plus a
release-strong boot does — the incident remains the argument for keeping
both). seL4-scale proof effort (11+ person-years on 8.7kLoC) is out of
reach; Kani/loom on 3 chosen primitives is days, not years. Three-gate
risk: zero — test-time tooling, no kernel image change; the gate here is
only that the harness must run in `make test`-adjacent host mode so it
actually runs. Cost: pilot 1–2 sessions (shortlist S-1). Dependencies:
host toolchain only (cargo kani / loom crates); no kernel-code dependency.

### 2.2 Fuchsia/Zircon: handle table with rights + vDSO-only syscall surface

**What it is.** Every kernel object reference is a *handle*: a per-process
table entry binding an object to a rights mask, checked at every syscall
that takes a handle ("Zircon Handles", https://fuchsia.dev/fuchsia-src/concepts/kernel/handles;
kernel concepts doc, https://fuchsia.googlesource.com/fuchsia/+/HEAD/docs/concepts/kernel/concepts.md).
All syscalls enter through the vDSO — "The Zircon vDSO is the sole means
of access to system calls"; the kernel can verify at entry that the return
address lies in the vDSO page (https://fuchsia.dev/fuchsia-src/concepts/kernel/vdso).
The surface is not minimal — 170+ syscalls (A. Popov, "A Kernel Hacker
Meets Fuchsia OS", 2022, https://a13xp0p0v.tech/2022/05/24/pwn-fuchsia.html)
— but it is uniform: handles, rights, and message-passing primitives
(channels, ports) with FIDL as the IDL. Zircon ships commercially in
Google Nest Hub devices (Fuchsia rollout 2021).

**What the reference kernel lacks.** Rights per descriptor (its fd table
is bare integers with mode bits buried in each file's own state; revoking
or narrowing an fd's authority is per-subsystem policy), and a single
controlled syscall entry plane (raw entry instructions are legal).

**Rux verdict: PROTOTYPE (internal rights-checked kernel-object table),
with the vDSO part re-classified.** Internally, Rux can attach a rights
word to every file-description/socket/ipc object and check operations
against it at the dispatch point D-6 already owns — the OH `access_token`
/ `tokenid` infrastructure is a domain-specific precedent inside the tree.
Three-gate risk: low — internal checks default to "all rights" so behavior
is unchanged until a policy narrows them; the risk to watch is per-op cost
on the dispatch hot path (perf gate Tier 1/2 must be reported). The
vDSO-only-entry idea is NOT a divergence for us (the reference kernel has
a vDSO); for Rux the relevant fact is the opposite direction — x86_64 Rux
has NO vDSO (`kernel/src/process/exec.rs` keeps AT_SYSINFO_EHDR absent),
so clock_gettime and friends pay full trap cost on our primary rig. That
is parity work with a direct perf payoff, routed to the perf lane, not
this register. Cost: rights table prototype ~2 sessions; vDSO parity
(separate, perf lane) 2–3 sessions. Dependencies: D-6 dispatch (landed);
coordination with the OH lane if access_token semantics get generalized.

---

## 3. Multikernel / scalability

### 3.1 Barrelfish: per-core replicated state, messaging instead of sharing

**What it is.** "The Multikernel: A new OS architecture for scalable
multicore systems" (Baumann et al., SOSP 2009; ETH Zurich + Microsoft
Research): treat the machine as a distributed system — each core runs its
own kernel instance with replicated per-core state; the ONLY inter-core
communication is message passing over URPC channels (shared-memory
buffers between monitor threads); hardware knowledge lives in a System
Knowledge Base queried via a constraint solver. Position paper: "Your
computer is already a distributed system" (HotOS '09,
https://www.usenix.org/legacy/event/hotos09/tech/slides/baumann.pdf).
Follow-up Barrelfish/DC (Zellweger et al., "Decoupling Cores, Kernels,
and Operating Systems") pushes the model to hot-swap kernel instances per
core.

**What the reference kernel lacks.** A no-shared-memory inter-core model;
its SMP design is shared data + RCU + per-CPU caches + IPIs, with the
well-known scalability ceiling on shared locks that motivated decades of
scalability work (per-CPU runqueues, lock-free lists).

**Rux verdict: REJECT as an architecture at our scale; park one slice.**
D-1 records the user ruling: the GRQ stays at MAX_CPUS ≤ 4, measured hold
5.1–6.1 µs and volume-coupled, not contention-bound — a replicated-state
message kernel is a redesign whose motivating problem (cache-coherence
traffic at high core counts) we do not have. Historical footnote: a global
run queue is itself pre-SMP classic-UNIX design (Bach, *The Design of the
UNIX Operating System*, 1986 — the single global dispatch queue), so D-1
is a deliberate re-adoption at small core counts, not novelty. Parked
slice, revisit only if core counts grow: cross-core TLB shootdown and
deferred work as message queues rather than shared-flag arrays (worth a
look only with shootdown-cost data from the x86_64 rig). Three-gate risk
of the parked slice: internal-only, low; would ride the interrupt-domain
perf gate. Cost: n/a now. Dependencies: none now.

### 3.2 Flat combining and lock-free structures

**What it is.** Flat combining (Hendler, Incze, Shavit, Tzafrir, "Flat
Combining and the Synchronization-Parallelism Tradeoff", SPAA 2010,
https://dl.acm.org/doi/10.1145/1810479.1810540;
PDF: https://people.csail.mit.edu/shanir/publications/Flat%20Combining%20SPAA%2010.pdf):
under contention, ONE thread holds the lock and performs the combined
requests of all others posted to a shared announcement array — beating
lock-free algorithms on contended structures because cache-line ping-pong,
not the lock itself, is the cost. Applied to synchronous queues in DISC
2010 (https://link.springer.com/chapter/10.1007/978-3-642-15763-9_8) and
extended to NUMA/PGAS settings (Holt et al., PGAS 2013,
https://sampa.cs.washington.edu/new/papers/holt-pgas13.pdf).

**What the reference kernel lacks.** In-tree flat-combined hot structures
(its answers are per-CPU caches, RCU, and sharded locks).

**Rux verdict: REJECT now, with data.** Our GRQ hold measurements
(5.1–6.1 µs average hold, volume-coupled, D-1) say the single-queue cost
is execution volume, not lock-line contention — flat combining optimizes
the latter. The futex buckets and small_alloc already have the classic
answers (sharding via hashing; per-CPU magazines — the magazine design IS
the per-CPU-cache answer, post-magazine global class-lock sections ≤ ~25M
and falling). Revisit trigger: IF=0/hold measurements ever show
contention-bound behavior (waiters spinning on an acquired lock) at higher
core counts. Three-gate risk if ever adopted: internal-only; perf-gate
Tier 2 (sched/ipc rows) governs. Cost: n/a now. Dependencies: none.

---

## 4. Syscall interface shapes

### 4.1 FlexSC: detached, exception-less system calls

**What it is.** "FlexSC: Flexible System Call Scheduling with
Exception-less System Calls" (Soares & Stumm, University of Toronto,
OSDI 2010, https://www.usenix.org/conference/osdi10/flexsc): threads never
trap; they write syscall descriptors into shared syscall pages, and
dedicated kernel threads execute them in batches, posting completion
status back. Decouples syscall invocation from execution; the follow-up
("Exception-less System Calls for Event-Driven Servers", USENIX ATC 2011,
https://www.eecg.toronto.edu/~stumm/Papers/Soares-USENIXATC11.pdf) ports
event-driven servers to the model.

**What the reference kernel lacks.** Any exception-less generic-syscall
interface (its io_uring covers async I/O operations only).

**Rux verdict: REJECT.** Three reasons: (1) the win FlexSC delivers —
avoiding traps and letting the kernel batch — is delivered for I/O-shaped
work by io_uring, which Rux already has a module for; (2) a second
generic syscall surface doubles the security/audit surface for every
syscall ever added (exactly the wrong direction for the LTP and image
compatibility goals); (3) the scheduling-decoupling half (kernel threads
executing on behalf of parkable user threads) presumes a threading model
change in userspace — gate 3 (upper-layer software) breaks, since unmodified
glibc/musl never use it. Cost: n/a. Dependencies: none.

### 4.2 Pervasive async — the io_uring lineage, internalized

**What it is.** Submission/completion ring pairs shared with userspace,
merged into the reference kernel v5.1 (2019, Jens Axboe). Measured:
~20% more requests/sec than epoll for network workloads in independent
tests; with polling, within ~10% of kernel-bypass SPDK on NVMe
("Understanding Modern Storage APIs", SOSP '22,
https://dl.acm.org/doi/10.1145/3534056.3534945; CHEOPS '23 comparison:
https://atlarge-research.com/pdfs/2023-cheops-iostack.pdf; early numbers
in liburing issue #97, https://github.com/axboe/liburing/issues/97).

**What the reference kernel lacks.** Nothing, as an interface — this is a
parity item. The gap is *internal*: its own kernel paths use completion
rings per-device (block-mq completions, NAPI), but internal deferred work
is not uniformly expressed as completion-ring operations.

**Rux verdict: PROTOTYPE the internal substrate.** Rux already has the
two halves separately: an io_uring module (its FileOps table is one of
the D-6 conversions) and batch-async bio with one-kick/one-drain windows
(divergence D-12). The candidate is the *writeback* side: dirty
page-cache/buffer-cache writeback expressed as a single completion-ring
pipeline feeding publication batches (D-13's PUB_BATCH=64 windows), with
the aim of deleting the remaining synchronous-write classes outside the
provisioning phase that the fsw counters still see. Three-gate risk:
medium — ordering against jbd2 commit brackets (D-8) and the H1/H2
out-of-band constraint guards must be re-proven; external behavior
(fsync/sync durability contracts) unchanged by design, so gates 1–3 hold
if the durability points stay as they are. Cost: 3–4 sessions (shortlist
S-4). Dependencies: D-8/D-13 stance (read first), fsw counters, fs-domain
perf gate.

### 4.3 vDSO-only syscall entry (and Rux's missing vDSO)

Recorded here so it is not mistaken for a divergence candidate: Zircon's
vDSO-only entry (§2.2) is a design the reference kernel ALSO has (its own
vDSO accelerates clock_gettime/gettimeofday and friends). For Rux the
actionable item is parity, not divergence: x86_64 Rux ships no vDSO
(`kernel/src/process/exec.rs` — "x86_64 has no vDSO mapping —
AT_SYSINFO_EHDR stays absent"), so every vDSO-eligible call from glibc
images pays a full trap. On the x86_64-first rig this is measurable in
the syscost per-op averages (clock_gettime). Route: perf lane, sized 2–3
sessions, gated by Tier 1 per-op averages. Not entered in the divergence
register — it is the reference kernel's own design, adopted as-is.

---

## 5. Scheduling beyond CFS/EEVDF

### 5.1 EEVDF (what the reference kernel now has)

EEVDF — Earliest Eligible Virtual Deadline First, from Stoica's 1995
work — replaced CFS in the reference kernel v6.6 (October 2023) after a
15-year gestation as an RFC (series by P. Zijlstra; overview coverage at
https://lwn.net/Articles/978911/ is sched_ext, EEVDF covered in the
v6.6 cycle). Mechanism: per-task lag tracking against an average
vruntime (avg_vruntime) decides *eligibility*; eligible tasks are ordered
by virtual deadline — latency-nice becomes a first-class input rather
than a heuristic.

**Rux position: parity item, excluded from the divergence shortlist.** Our
scheduler already deviates deliberately (D-1 GRQ; D-14 measured-wait
wakeup-preemption gate, which fixed a 275 µs-per-record preempt dance to
~2.5 µs-class behavior). Adopting EEVDF's lag/eligibility accounting
internally is allowed by the house rule (scheduling policy has no ABI
surface) and MAY improve interactive fairness under mixed load — but it
is mainstream-kernel parity work, not an industry divergence, so it does
not belong on this lane's shortlist. If the perf lane later wants it, the
guard set is already defined (GRQ hold band, census bands, P2/P5 rows).

### 5.2 Pluggable scheduling: ghOSt and sched_ext

**What it is.** ghOSt ("ghOSt: Fast & Flexible User-Space Delegation of
[kernel] Scheduling", Humphries et al., SOSP 2021,
https://dl.acm.org/doi/10.1145/3477132.3483542, Google production use on
search/VM fleets) moved scheduling policy to userspace agents with
kernel-side data paths; its successor sched_ext — BPF-implemented
scheduler policies with a watchdog fallback to the fair class — shipped
in the reference kernel v6.12 (November 2024), enabling policies like
scx_rustland/scx_lavd as loadable programs.

**What the reference kernel lacks.** Nothing anymore (parity since v6.12).

**Rux verdict: REJECT for now.** Rux has no in-kernel BPF host, and a
runtime-pluggable policy framework without external policy authors is
complexity without a consumer — every scheduler change here lands through
the sched-domain perf gate in days, which is the workflow sched_ext
exists to unblock elsewhere. The idea worth keeping from ghOSt is the
*separation*: policy decision (who next) separated from mechanism
(enqueue/dequeue/pick) so decision functions become independently
measurable — D-1's segment-wise hold decomposition already moves that
direction without a plugin API. Revisit trigger: a product need for
per-workload policies. Cost: n/a. Dependencies: none.

---

## 6. Memory-side designs

### 6.1 uffd-style page-fault delegation

**What it is.** userfaultfd (reference kernel v4.3, 2015, A. Arcangeli;
RFC thread https://lkml.org/lkml/2015/3/5/576; userfaultfd(2) man page,
man-pages project, M. Kerrisk): a userspace
process registers an address range and receives page-fault events on an
fd, resolving faults by copying pages — built for KVM postcopy live
migration; write-protect mode (v4.11) and minor-fault mode (v5.13)
followed (LWN: https://lwn.net/Articles/786896,
https://lwn.net/Articles/845754).

**What the reference kernel lacks.** Nothing — parity.

**Rux verdict: REJECT.** Two reasons: no internal consumer (Rux has no
live-migration feature, and the fake-OOM work on the GNOME image is an
allocator-accounting problem, not a fault-delegation problem); and Rux
does not expose userfaultfd today, so adding it would be new ABI surface
(parity work) — only worth doing if a workload demands it. The internal
pattern (fault events resolved asynchronously by a non-faulting agent)
is already embodied by the batch fault-around window on the read-miss
path (D-12). Cost: n/a. Dependencies: none.

### 6.2 Persistent-memory-first designs

**What it is.** Filesystems designed on the assumption that storage is
byte-addressable, memory-mapped persistent memory: PMFS (USENIX ATC '13)
and NOVA ("NOVA: A Log-structured File System for Hybrid
Volatile/Non-Volatile Main Memories", FAST '16, Xu & Swanson, UCSD,
https://www.usenix.org/system/files/conference/fast16/fast16-papers-xu.pdf)
— per-inode logs, light journaling, DAX mmap directly into user address
space.

**What the reference kernel lacks.** Little that matters now: it has DAX
and ext4-DAX; NOVA-class designs stayed out-of-tree. The hardware
context collapsed — Intel discontinued the Optane PMem business in
mid-2022; the research line continues toward CXL-attached memory.

**Rux verdict: REJECT.** No PM or CXL device exists in the QEMU virt rig
as configured; building a PM-first path with no way to measure it
violates the merge-gate principle ("if it is not measured, add the tool
that measures it" — no tool can measure hardware we do not have).
ext4-on-virtio remains the measured path. Cost: n/a. Dependencies: none.
Revisit trigger: a CXL/PM device model in the rig.

### 6.3 Folios (noted for completeness)

The reference kernel's folio work (v5.16+) — larger page-cache units
replacing struct page — is parity-relevant to Rux's D-12 batch windows,
which already operate on window-sized units (coalesced runs up to 64
blocks/128 KiB per request). Not a divergence candidate; noted so the
cross-reference in §8 is complete.

---

## 7. Extension candidates (beyond the charter list)

### 7.1 Capsicum: rights-carrying descriptors

**What it is.** FreeBSD 9 (2012): file descriptors become capabilities
with rights masks (CAP_READ/CAP_WRITE/...), refineable but never
widened, plus a capability mode where global namespaces (paths, PIDs)
are unreachable. Founding paper: "Capsicum: practical capabilities for
UNIX" (Watson, Anderson, Drysdale, USENIX Security 2010; project page
https://www.cl.cam.ac.uk/research/security/capsicum/projects.html;
manual https://man.freebsd.org/cgi/man.cgi?query=capsicum&sektion=4freebsd).
An upstream port to the reference kernel was proposed in 2014
(https://lkml.org/lkml/2014/6/30/170) and not taken.

**What the reference kernel lacks.** Rights on descriptors; its
sandboxing answers are seccomp/LSM layers around the syscall surface
instead of on the objects.

**Rux verdict: REJECT (as external design); internal slice merges into
§2.2.** Externally, capability mode breaks unmodified images (gate 3).
Internally, per-object rights are exactly the §2.2 prototype; the OH
access_token/tokenid machinery already provides the one sandbox consumer
we have. Cost: n/a separate from §2.2. Dependencies: §2.2 decision.

### 7.2 Plan 9: per-process namespaces

**What it is.** "The Use of Name Spaces in Plan 9" (Pike et al.,
https://9p.io/sys/doc/names.html): every process composes its own name
space from binds and mounts — union directories stack multiple sources
under one path; the file protocol (9P) is uniform across local and
remote providers.

**What the reference kernel lacks.** Per-process namespace composition;
its namespaces are clone-time, per-process-group (mount net, pid net...)
and far coarser than Plan 9's per-process bind/union.

**Rux verdict: REJECT now.** Rux's VFS is global-mount with a shared
dcache (recent chroot fast-base + negative-memo work builds on exactly
that). Per-process views would fork path resolution per task — the LTP
path/lookup families and every OH image's assumption of a shared root
are the immediate risk, and there is no requesting consumer. The
capability-relevant idea (namespace as composable per-process data
rather than global kernel state) is worth remembering if a multi-tenant
OH use case appears. Cost: n/a. Dependencies: none.

### 7.3 RUMP anykernel: kernel subsystems as testable in-host libraries

**What it is.** NetBSD's rump ("Rump File Systems: Kernel Code Reborn",
A. Kantee, https://www.13thmonkey.org/documentation/NetBSD/rump.pdf;
Rump Kernel Book, https://contents.meetbsd.ir/ebook/rumpkernel-bookv2-20160802.pdf):
the same kernel code — drivers, filesystems, TCP/IP — runs in-kernel OR
linked into a userspace process behind a hypercall layer, because the
"anykernel" discipline keeps subsystems independent of the environment.
NetBSD's test suite runs kernel filesystem code as userspace processes:
kernel bugs cannot crash the test host, and iteration is process-fast
(https://blog.netbsd.org/tnf/entry/revolutionizing_kernel_development_testing_with).

**What the reference kernel lacks.** Kernel-as-library ports exist as
test artifacts, but in-host testing of subsystem code is not a first-class
discipline with CI-scale usage.

**Rux verdict: PROTOTYPE.** The value proposition maps directly onto our
goal list: LTP mutation-family work (creat/unlink/rmdir/rename/fsync
classes) iterates through ext4/jbd2 today only inside a booted kernel;
an anykernel slice that runs the ext4 namei/alloc/journal-replay core
against in-host cargo tests (block device mocked behind a trait, real
jbd2 code paths) turns a ~90 s boot-per-iteration loop into a
seconds-class host loop, and the same harness becomes the fuzzing entry
that rump enabled for NetBSD. Three-gate risk: zero runtime risk if the
extraction keeps the kernel build byte-equivalent (trait boundary
compiled out in-kernel or a cargo feature); the gate to watch is suite
parity — in-kernel `make test` and LTP must be unchanged. Cost: 2
sessions for the pilot slice (shortlist S-3). Dependencies: ext4/jbd2
module boundaries (D-8/D-13 authors' comments), test/ harness
conventions.

### 7.4 Model checking on kernel primitives (seL4's method, Rust tooling)

**What it is.** Bounded model checking (Kani,
https://github.com/model-checking/kani) and exhaustive interleaving
exploration (loom, https://github.com/tokio-rs/loom) applied to Rust
concurrency code: Kani proves absence of panics/index errors over bounded
inputs for single-threaded logic; loom explores thread interleavings of
atomics/locks against a memory model. The seL4/CertiKOS lineage (§2.1)
is the evidence that verification pays where the incident classes are;
Rust-native tooling brings the entry cost from person-years to days.

**What the reference kernel lacks.** Any automated proof over its
concurrency primitives (fuzzing and review only).

**Rux verdict: ADOPT (pilot).** Targets chosen from the register's own
incident history: (a) small_alloc magazine invariants — row indexing,
re-entry refusal, the r8final len/MAG_DEPTH guards (D-7); (b) the GRQ
claim/unlock protocol — the R74 double-pick and 2b092c49 discarded-guard
classes (D-11); (c) the futex bucket→GRQ nesting chain (D-11 second
slice). Each is small, self-contained state machine code where loom can
exhaust interleavings and Kani can bound data. Success criterion for the
pilot: the harness reproduces at least one known historical failure mode
(planted bug) and returns clean on current main — that is the evidence
bar for making it a standing pre-merge check for changes touching those
files. Three-gate risk: none (host-side tooling). Cost: 1–2 sessions
(shortlist S-1). Dependencies: none beyond host cargo.

---

## 8. Cross-reference: the eight levers vs industry precedent

| Lever (rust-design-headroom.md) | Rux status | Industry precedent | Assessment |
|---|---|---|---|
| §1 file ops as capability traits | LANDED as D-6 (builder + one-place missing-op dispatch) | Theseus traits (intralingual); Zircon object ops; Capsicum runtime rights | Precedent strong; Rux's one-place errno mapping goes beyond all three |
| §2 ownership over lifetime discipline | OPEN (D-16 records the unaccounted-borrow gap that blocks RCU for mm/task blocks) | Theseus state-spill minimization; Redox Arc-based scheme handles | Precedent strong; remaining work is the mm/task borrower accounting |
| §3 typed per-CPU + lock keys | LANDED as D-7 (CpuLocal) and D-11 (typed lock keys) | Barrelfish per-core state (different mechanism, same goal); no mainstream kernel type-enforces either discipline | Rux ahead of precedent — the register incidents (r8final, 2b092c49) are the evidence base |
| §4 derived interface formats (procfile!) | LANDED as D-4 | NONE in mainstream kernels — hand-written format strings are endemic there | No precedent — this lever is Rux-original; validated in-house (five-defect incident class) |
| §5 Result-typed error APIs | LANDED as D-10 (files/xattr/inode-ops converted; deep layers bridged) | Theseus/Redox use Result-heavy APIs | Precedent moderate; Rux's single-boundary mapping rule is the sharper form |
| §6 IPC transaction state machine | OPEN (binder.rs 9,170 lines behind the WORLD lock; BHIST ring is hand-rolled forensics) | Theseus enum-state modeling (general principle); Zircon channel message lifecycle (untyped) | Weak direct precedent; general Theseus principle applies — shortlist S-2 |
| §7 typed timer-wheel slots | OPEN (timer.rs 1,309 lines; fix-on-fix history of empty-raise/resync) | NONE — timer wheels in mainstream kernels are untyped C arrays | No precedent; internal-only lever — shortlist S-5 |
| §8 driver capability proofs | OPEN (virtio completion paths historically most-reworked) | Theseus driver traits; Redox scheme provider traits | Moderate precedent; blocked on a concrete incident-class to target |

Register context: the two entries the charter named (D-1 GRQ, D-2 packed
stacks) have since been joined by sixteen more; D-12/D-13 (batch fill,
batched publication) mean Rux's page-cache I/O already operates on
window-sized units — the folio parity note of §6.3.

Two levers (§4, §7) have no industry precedent: §4 is already proven
in-house by its own incident class, and §7 rides the same intralingual
principle (§1.2) with the same no-ABI-risk property. "No precedent" is
not a blocker under the house rule — the three gates, not precedent, are
the acceptance bar; precedent matters for cost estimation and risk
sizing.

---

## 9. Ranked adoption shortlist (top 5, sized for worktree lanes)

Ranked by (value against current goals: GNOME boot, LTP pass rate, perf
gate) / (cost × risk). All five are internal-only designs; none touches
an external interface, so gate 1 is clean by construction — the binding
risks are gates 2–3 (behavior must stay identical) plus the perf gate.

### S-1. Model-checking pilot on concurrency primitives (from §7.4, seL4 lineage)

- **Goal.** Standing host-side proof harness for the three incident-dense
  state machines: small_alloc magazine (D-7), GRQ claim/unlock (D-11),
  futex bucket→GRQ nesting (D-11 slice 2).
- **Mechanism.** loom exhaustive interleaving over the protocol types
  extracted behind a trait (or a mirror crate compiled from the same
  source via include!); Kani bounds on the magazine's len/MAG_DEPTH
  guard logic. No kernel image change.
- **Exit criteria.** (a) Harness reproduces the r8final two-CPU re-entry
  class and the 2b092c49 discarded-guard class when planted; (b) clean on
  current main; (c) run time under 5 minutes so it fits `make test`
  adjacency; (d) written rule: changes touching those files run it.
- **Cost.** 1–2 sessions. **Dependencies.** none (host cargo only).
  **Guards to report.** none (no runtime change) — this lane's product
  IS a guard.

### S-2. Binder transaction typed state machine (from §1.2 / lever §6)

- **Goal.** Turn the hand-maintained transaction lifecycle of
  `kernel/src/ipc/binder.rs` (Pending → InFlight → Completed → Reaped /
  Aborted) into an enum state machine whose illegal transitions do not
  compile; derive the BHIST forensics from the transition log instead of
  maintaining both.
- **Mechanism.** `TxnState` enum + `fn advance(self, ev: TxnEvent) ->
  Result<TxnState, IllegalTransition>`; every mutation site goes through
  `advance`; the IllegalTransition arm carries the SLOW-TXN census hook.
  Behavior-preserving refactor behind the existing WORLD lock.
- **Exit criteria.** (a) shape test group `binder-fsm` pinning the legal
  transition table; (b) full unit suite + binder-domain perf gate rows
  (SLOW-TXN classes, transaction throughput) in band on a standard boot;
  (c) one OH boot with zero IllegalTransition hits and unchanged
  SLOW-TXN volumes.
- **Cost.** 2–3 sessions, staged (state enum → call-site migration in 2–3
  slices → forensics derivation). **Dependencies.** coordination with
  the OH lane (binder.rs is their hot file); D-6/D-11 conversion
  doctrine (opportunistic slices). **Guards.** binder perf-gate domain;
  trace-smoke2 binder events must stay byte-shaped (D-15).

### S-3. Anykernel test slice for ext4/jbd2 (from §7.3, RUMP precedent)

- **Goal.** ext4 namei/alloc/journal-replay core runnable as in-host
  cargo tests against a trait-exposed block device, cutting the
  mutation-family iteration loop from boot-class (~90 s) to host-class
  (seconds), and opening the fuzzing entry rump gave NetBSD.
- **Mechanism.** Extract the pure-logic boundary (inode alloc, extent
  mapping, jbd2 descriptor/revoke block logic) behind a `BlockIo` trait;
  in-kernel build unchanged (same monomorphization), host tests drive it
  with a Vec-backed device. Journal replay tests replay crash images
  produced by the write-through commit stance (D-8) — today replay logic
  is only exercised by real crashes.
- **Exit criteria.** (a) in-kernel build byte-equivalent (objdiff of the
  linked kernel before/after = 0 or justified); (b) suite + LTP
  spot-families green; (c) at least 3 replay scenarios covered that
  today need a crash to test; (d) measured iteration-time number
  reported (host round vs boot round).
- **Cost.** 2 sessions pilot (namei+journal replay only). 
  **Dependencies.** ext4/jbd2 owners' comments (D-8/D-13); test/
  conventions. **Guards.** suite parity is the guard.

### S-4. Internal completion-ring writeback substrate (from §4.2, io_uring lineage)

- **Goal.** Delete the remaining synchronous-write classes outside the
  provisioning phase (fsw counters) by expressing dirty-buffer writeback
  as one completion-ring pipeline feeding the D-13 publication batches.
- **Mechanism.** A per-device completion ring (SQ/CQ equivalent in kernel
  memory) between the dirty-buffer queues and bio submission; drain
  points: PUB_BATCH threshold (64), durability points (fsync/sync/
  eviction/commit — unchanged per D-8/D-13), memory pressure. No
  userspace-visible change.
- **Exit criteria.** (a) fsw shows zero new sync-write classes outside
  provisioning on standard boots; (b) pbench fs rows (p6_mkdrmdir,
  p6_unlink, p6_write_fsync, p6_append) within band or improved; (c)
  buffer-cache hit rate ≥ baseline; (d) crash-replay scenarios from the
  S-3 harness pass (durability ordering re-proven).
- **Cost.** 3–4 sessions. **Dependencies.** S-3 (replay scenarios), the
  D-8/D-13 stance docs, fs-domain gate tooling. **Guards.** filesystem
  hot-paths domain + Tier 1; release-strong boot requirement.
- **Risk note.** Highest-risk item on the list — touching jbd2 commit
  ordering; that is why it ranks below the protective items despite the
  perf payoff.

### S-5. Typed timer-wheel slot ranges (from §1.2 / lever §7)

- **Goal.** The hrtimer/wheel split's fix-on-fix history (empty-raise
  gates, resync thresholds) becomes structurally impossible: deadline
  objects carry their own slot arithmetic; wheel buckets enforce
  invariants by construction; const-generic wheel sizes per use class.
- **Mechanism.** `WheelSlot<const SLOTS: usize>` + a `Deadline` newtype
  wrapping (absolute expiry, bucket id) so a mis-slotted deadline cannot
  be constructed; opportunistic conversion of `kernel/src/timer.rs`
  internals with the public API unchanged.
- **Exit criteria.** (a) shape test group `timer-wheel` (slot round-trip,
  wheel-wrap arithmetic, the historical empty-raise/resync regressions
  as pinned tests); (b) Tier 1 syscost boot in band (timer paths ride
  per-op averages — nanosleep within ±20%); (c) sched-domain census in
  band.
- **Cost.** 1–2 sessions. **Dependencies.** none; touches timer/, so
  sched/timer perf-gate domain reporting applies.

**What did not make the shortlist, and why (one line each):** Zircon
internal rights table (§2.2) — no consumer beyond access_token today,
park until a second sandbox consumer appears; EEVDF parity (§5.1) — real
work, wrong lane; FlexSC (§4.1), unikernel (§1.3), Barrelfish (§3.1),
PM-first (§6.2), Plan 9 namespaces (§7.2), Capsicum (§7.1), uffd (§6.1),
pluggable schedulers (§5.2), flat combining (§3.2) — REJECT per their
sections; vDSO (§4.3) — parity perf work, routed to the perf lane.

---

## 10. Session log reference

Research for this round was conducted 2026-11-03 against the sources
cited inline (primary papers, project documentation, LWN, mailing-list
threads). Numbers quoted are the sources' own measurements; no new
measurements were produced this round (no kernel changes, no boots).
Future prototype lanes produce their numbers on x86_64 + KVM.
