# lockdep-lite — a Rust-native lock dependency checker (design)

Status: design accepted for phased landing; P0 (the static order
registry and its boot-time validation) landed with this document.
Lanes implementing P1/P2 start at §9.

## 0. Purpose and problem class

Rux carries three lines of defense against lock-order bugs today, and
each covers a different slice:

1. **Compile-time fence** (`kernel/src/sync/lockkey.rs`): a pair whose
   invariant is NAMED in `docs/architecture/lock-ordering.md` can adopt
   typed keys, and the reverse nesting stops compiling. Slice covered:
   named pairs that have migrated (GRQ/bucket, waiter-slot/bucket;
   INV-LOCK-6 still pending). Migration is opportunistic — dozens of
   unkeyed lock pairs compile today and new ones appear in every lane.
2. **Spin watchdog** (`RawSpinlock::DEADLOCK_WARN_ITERS`): prints when
   a spinner has spun ~100M iterations. Slice covered: deadlocks that
   actually spin long enough on a live CPU; it says nothing about
   illegal-but-not-yet-deadly orders, and nothing about sleeping locks.
3. **SPIN models** (`kernel/verify/spin/lock_ordering.pml`): proves the
   DOCUMENTED paths deadlock-free. Slice covered: the abstracted paths,
   at model-check time, not the evolving code.

The uncovered class: **an acquisition order that violates
lock-ordering.md on a path nobody modeled, against a pair that is not
keyed** — INV-LOCK-4's "no cycles" is enforced nowhere at runtime, and
INV-LOCK-8 rule 1 (a SleepMutex taken with a spinlock held, or from
interrupt context) is only caught after the wedge. The futex-wake
INV-LOCK-5 incident and the virtio-net stats ABBA (INV-LOCK-6) were
both found the hard way — by wedging under load.

lockdep-lite closes that gap with the reference lockdep's
OBSERVABILITY GOAL: record every held-lock nesting as a graph edge,
check each new edge for cycles as it is observed, and splat with both
class names and acquire sites BEFORE the deadlock happens. It does NOT
copy the reference C implementation shape (dynamic class registration
from stack traces, per-class zapped lists); it uses Rux-native
mechanisms that already exist here: static lock-class sites declared in
one table (the `tracepoints!` derive pattern), the per-task
`preempt_count` bitfield for context classification, and the lockkey
type surface for the hot classes.

## 1. The reference checker's goals (what we align to, documentation level)

Verified against the reference kernel's documentation
(`Documentation/locking/lockdep-design.rst`) and source
(`kernel/locking/lockdep.c`, `lockdep_proc.c`):

- **Lock class**: "a group of locks that are logically the same with
  respect to locking rules" — every `inode` lock instance is ONE class.
  A class is keyed by a static key (storage address of the lock or an
  explicit registered key for dynamic instances), registered at first
  use.
- **Acquisition tracking**: each task carries a held-lock stack; a
  64-bit hash of the stack (the "chain key") is looked up in a
  lock-free hash table so each unique locking SEQUENCE is validated
  only once — the amortization that makes per-acquisition checking
  affordable.
- **Cycle detection**: on each new acquisition, the checker validates
  the new lock against the held locks by searching the class graph for
  a circular dependency (a bounded BFS, `check_noncircular()`); it
  also runs usage-state transition checks (irq-safe vs irq-unsafe).
- **Splat**: names the class (`(&sio_locks[i].lock){-.-.}, at:
  [<ip>] mutex_lock+...`), the usage bits, the acquisition site, and
  the already-held locks.
- **Output**: `/proc/lockdep` (per class: key, forward/backward
  dependency counts FD/BD, 4 usage chars, name, then ` -> [key] name`
  lines for direct dependencies), `/proc/lockdep_chains` (per chain:
  `irq_context:` and the class list), `/proc/lock_stat` (13 columns:
  class name, con-bounces, contentions, waittime-min/max/total/avg,
  acq-bounces, acquisitions, holdtime-min/max/total/avg, plus
  per-contention call-site lines).
- **Self-limiting semantics**: fixed arenas (`MAX_LOCKDEP_KEYS` 8191,
  chains, stack entries); on exhaustion or on the first splat the
  checker sets `debug_locks = 0` and stops ("Turning lockdep off";
  `/proc/lock_stat` then prints `*WARNING* lock debugging disabled!!`).
  A boot parameter (`lockdep=off`) has the same effect up front.

These are the observability goals. The Rux mapping below keeps every
one of them; what it replaces is the C implementation's mechanics.

## 2. Design constraints (from this repo's history — binding)

**(a) SleepMutex/spinlock split (the lostpark lesson).** A lock that
can be held across blocking I/O is a SleepMutex; spinlocks never sleep
anywhere in their acquire (`sync/spinlock.rs` lock() discipline
comment, `sync/mutex.rs` rules 0–2). The checker must model this as a
FIRST-CLASS check, not an ordering edge: acquiring a sleep-domain
class while any spin-domain class is held, while `preempt_count != 0`,
or from hardirq/softirq context is a splat in its own right
(INV-LOCK-8 rule 1) — no graph needed.

**(b) Hot-path cost discipline.** The checker is compile-time gated
(cargo feature `debug-lockdep`) and runtime armed (`debug=lockdep`,
default off). Feature off = the acquire/release paths compile to the
same instructions as today (verified by disassembly diff, §7). Feature
on but disarmed = one relaxed load per acquire (the same contract as
`TraceEvent::armed()` — the proven cost class of the trace
switches). Armed cost is measured per the perf gate (§7) BEFORE any
merge that touches the hooks.

**(c) The model of legal order IS `docs/architecture/lock-ordering.md`.**
The checker does not invent its own hierarchy. Every declared edge in
its registry carries the INV-LOCK-* id it came from; the registry and
the document are updated in the SAME change (a reviewable rule: a
registry row without a document row, or vice versa, is a defect). The
checker's verdicts are stated against that document — "observed edge
X→Y contradicts INV-LOCK-5", or "X→Y is not declared anywhere".

**(d) Memory-safe implementation preference.** Lock classes are static
sites declared in one const table (no first-use dynamic registration);
the graph lives in an owned arena sized at compile time; the hot path
performs NO allocation and takes NO new lock on the amortized path
(chain hash hit). All checker code lives under the debug/ layer policy
(Safe Rust where possible; the graph lock section is the one audited
unsafe-free spot — it is plain safe code around an owned array).

## 3. Data model

```rust
/// Which primitive family a class belongs to. The family decides the
/// context checks of §2(a): only SleepDomain classes are checked
/// against spin-domain holds / preempt_count / irq context.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LockDomain { Spin, Sleep, RwSpin, SeqLock }

/// The documented hierarchy level (lock-ordering.md): 0 = outermost.
/// The registry value is the class's NESTING DEPTH in the declared
/// DAG (validated for strict monotonicity along every declared edge
/// — see the P0 findings in §9); lock-ordering.md's hierarchy
/// diagram numbers are advisory display metadata, and are known
/// stale for the futex bucket placement.
pub type LockLevel = u8;

/// One lock class — a static declaration, not a runtime object.
pub struct LockClassDesc {
    pub name: &'static str,          // "grq", "futex-bucket", "vfs-mutation", ...
    pub domain: LockDomain,
    pub level: LockLevel,
    /// The lock-ordering.md section / invariant that names this class.
    pub doc: &'static str,           // "INV-LOCK-5"
    /// P1: the keyed type that mints acquire events for this class
    /// (None for classes whose acquire sites are hand-instrumented).
    pub key: Option<&'static str>,   // "GrqKey", "FutexBucketKey", ...
}

/// A declared nesting edge: OUTER held, INNER acquired. Source of
/// truth: lock-ordering.md. `inv` names the invariant row.
pub struct DeclaredEdge {
    pub outer: LockClassId,
    pub inner: LockClassId,
    pub inv: &'static str,           // "INV-LOCK-5"
}

/// Runtime state (P1) — owned arenas, no allocation after boot.
pub struct LockGraph {
    /// classes: the static table, `CLASSES.len()` entries.
    /// edges: adjacency in both directions, `MAX_EDGES` owned array.
    /// chains: 64-bit chain keys in an owned open-addressing table.
    /// ...
}
```

Class ids are `u16` indices into the static `CLASSES` table. Sizes for
P1: `MAX_CLASSES = 256` (Rux has ~60 lock sites today), `MAX_EDGES =
2048`, `MAX_CHAINS = 8192`, per-task held stack `MAX_DEPTH = 48` (the
reference's depth). On exhaustion the checker disables itself and
taints (reference semantics, §1 last bullet).

The **chain key** is a 64-bit FNV-1a over the held stack's class ids
plus the id being acquired, exactly the reference's "lightweight
64-bit hash of the chain" amortization: an acquire whose chain is
already recorded skips the BFS. Class ids are dense and stable
(compile-time table), so the hash is over small integers — no address
mixing needed, and two different chains colliding only costs one
extra BFS run (correctness never depends on the hash).

## 4. Site-declaration API (the `lock_classes!` table)

One declaration site derives every artifact, the same derive pattern
as `trace/mod.rs`'s `tracepoints!` (rust-design-headroom §4): the
static table row, the per-class `pub const` id, and — in P1 — the
acquire-event glue. The table lives in `kernel/src/debug/lockdep.rs`;
subsystems do NOT each declare their own rows (unlike tracepoints,
lock classes are a global ordering table — one file, one reviewable
diff per added pair):

```rust
lock_classes! {
    //  id-const           name                  domain     lvl  doc           key
    VFS_MUTATION      / "vfs-mutation"       / Sleep,   0, "INV-LOCK-8",   None,
    EXT4_BIG          / "ext4-big"           / Sleep,   1, "INV-LOCK-8",   None,
    FUTEX_BUCKET      / "futex-bucket"       / Spin,    2, "INV-LOCK-5",   Some("FutexBucketKey"),
    WAITER_SLOT       / "futex-waiter-slot"  / Spin,    3, "INV-LOCK-5",   Some("WaiterSlotKey"),
    GRQ               / "grq"                / Spin,    3, "INV-LOCK-5",   Some("GrqKey"),
    NET_QUEUE         / "net-queue"          / Spin,    2, "INV-LOCK-6",   Some("NetQueueKey"),
    NET_STATS         / "net-stats"          / Spin,    3, "INV-LOCK-6",   Some("NetStatsKey"),
    // ... one row per class named in lock-ordering.md (18 at P0)
}

declared_edges! {
    // outer       inner        invariant
    FUTEX_BUCKET, GRQ,         "INV-LOCK-5",  // bucket -> GRQ -> ...
    FUTEX_BUCKET, WAITER_SLOT, "INV-LOCK-5",
    FUTEX_BUCKET, PI_BUCKET,   "INV-LOCK-PI-1",
    PI_BUCKET,    PI_POOL,     "INV-LOCK-PI-1",
    NET_QUEUE,    NET_STATS,   "INV-LOCK-6",
    WAIT_QUEUE,   GRQ,         "spinlock.rs waitqueue rule",
    VMA_MANAGER,  PTE_*,       "INV-LOCK-7",  // -> epoch | per-mm
    PTE_*,        PAGE_DESC,   "INV-LOCK-7",
    VFS_MUTATION, EXT4_BIG,    "INV-LOCK-8",  // ... -> I/O
    // negative edges (forbidden pairs) are the absence of an edge PLUS
    // the lockkey impl absence; the checker reports an OBSERVED edge
    // with no declared row as UNDECLARED (§6), which covers the reverse
    // direction without a second table.
}
```

`declared_edges!` is deliberately a plain `static` array (no macro
machinery needed): the interesting derive work is all in
`lock_classes!` (id consts so call sites never hand-write integers).

## 5. Runtime acquire tracking (P1) — hooks and the disabled-path cost

Instrumented surfaces, in landing order:

1. **Keyed classes** (`KeyedRawSpinlock`/`KeyedSpinlock`): the key type
   already names the class — `lock()`, `lock_nested()`, `try_lock()`,
   `lock_claim()`/`unlock()` call `lockdep_acquire::<K>()` /
   `lockdep_release::<K>()`. Zero new site edits; the key IS the class.
2. **The SleepMutex pair** (`VFS_MUTATION_LOCK`, `EXT4_BIG_LOCK`):
   two hand hook calls in `sync/mutex.rs` (acquire/release of the
   outermost guard), statically classed — this is where the INV-LOCK-8
   context check gets its teeth.
3. **Opt-in per subsystem** (later P1 slices): statics like the PTE
   global epoch, zone locks, process-tree lock — one hook line each,
   added when a subsystem's pair gets an invariant row (the same
   opportunistic doctrine as lockkey adoption).

Generic heap `Spinlock<T>` instances (no key): P1 does NOT hook them
globally. Class-per-construction-site (the reference's dynamic-key
scheme) is deferred to P2, so P1's cost and blast radius stay bounded;
undocumented heap-lock pairs remain covered by the spin watchdog as
today. This is a stated scope cut, not an oversight: every pair with a
NAMED invariant is covered by 1–3.

The hook body, disabled-path contract:

```rust
#[inline(always)]
pub fn lockdep_acquire<K: NestClass>() {
    if !ARMED.load(Relaxed) { return; }     // the entire disarmed cost
    lockdep_acquire_slow(K::CLASS_ID, K::NAME);
}
```

`ARMED` is one static `AtomicBool`, set when the `debug=lockdep`
switch is on (and cleared by the self-disable path). With the cargo
feature off, `lockdep_acquire` compiles to nothing (empty `#[cfg]`
body) — §7 verifies instruction-identity.

The slow path (armed): push class id on the current task's held stack
(a fixed `[u16; 48]` + depth in `Task` — allocated with the task, no
per-acquire allocation); compute the chain key; if recorded, return
(hot after warmup); else take the graph lock (one spinlock, IRQs off —
held for microseconds over an owned array), run the checks of §6,
insert the chain key and any new edge, drop the graph lock. Release
pops the stack (no lock). The graph lock is a leaf by construction:
nothing else is acquired inside the checker, and the checker itself is
never re-entered (a flag on the held stack prevents recursive hook
execution from the checker's own printing path).

## 6. Checks and the cycle algorithm (reference-shaped)

On an armed acquire of class `c` with held stack `h[0..n]`:

1. **Context check** (cheap, first — catches INV-LOCK-8 rule 1 even
   when the stack is empty):
   - class domain is Sleep AND (any held class is Spin/RwSpin/SeqLock
     domain OR `preempt_count() != 0` OR in hardirq/softirq context
     per the preempt_count bitfield) → splat
     `SLEEP-IN-ATOMIC-CONTEXT`.
2. **Declared-order check**: for each held `h[i]`, if the pair
   `(h[i], c)` has a declared edge in EITHER direction:
   - declared `h[i] → c`: legal, continue;
   - declared `c → h[i]` (reverse): splat `ORDER-VIOLATION` naming the
     invariant id — this is a live deadlock-capable order against the
     document, reported even though no cycle may exist on this path.
3. **Chain-key check**: chain `(h[0..n], c)` already recorded → return
   (the reference's amortization).
4. **Cycle check** (only for a NEW chain): for each held `h[i]`, add
   observed edge `h[i] → c` if absent, then run a bounded BFS from `c`
   through observed+declared edges looking for `h[i]`. Found → splat
   `CYCLE` with the path (the BFS parent array reconstructs it). This
   is `check_noncircular()`'s shape: new edge next→prev is a deadlock
   iff prev is reachable from next.
5. **Undeclared-pair note**: an observed edge with no declared row in
   either direction prints one `UNDECLARED-NEST` line (rate-limited to
   once per pair) — the feedback loop into lock-ordering.md: either a
   new row is added (and possibly a lockkey migration) or the nesting
   is a bug. This is deliberately NOT a splat; today's kernel has
   legitimate nestings the document has not named yet, and the firehose
   would teach people to turn the checker off.

BFS bound: queue capacity 32; visiting more nodes than `MAX_CLASSES`
or a full queue disables the checker (reference self-limit semantics)
rather than looping forever on a corrupt graph.

**Splat shape** (one line plus context, printk, kallsyms-symbolized):

```text
LOCKDEP: CYCLE grq -> futex-bucket -> grq
  path: grq(acquired at schedule+0x1a2) -> futex-bucket(futex_wake+0x64) -> grq(__schedule+0x3f)
  held: futex-bucket (this task, depth 1)  cpu=2  task=rcu_gp/12  in_irq=0 preempt=1
  LOCKDEP: disabling lockdep (first splat; debug_locks=0 equivalent)
```

Acquire sites come from `#[track_caller]`'s `Location::caller()` (all
lock entry points already carry it) and symbolize through the existing
`debug::ksyms` table.

## 7. Gating and the cost-measurement plan (perf gate alignment)

Gating layers, per phase:

| layer | P0 | P1 | P2 |
|---|---|---|---|
| cargo feature `debug-lockdep` (hooks compiled at all) | — | yes | yes |
| boot switch `debug=lockdep` (runtime arm) | yes | yes | yes |
| static registry rodata (always compiled) | ~1 KiB | ~1 KiB | ~1 KiB |

P0 has no hot-path code — the registry is const data and the
validation runs once at boot, only when armed. It therefore ships
behind the boot switch alone (zero instructions on any steady-state
path when off). P1/P2 hook acquire/release and get the cargo feature,
so a production build carries not even the relaxed load.

Measurement plan (a P1 merge runs all rows; P0 merges run row 1 only):

1. **Feature OFF, build-identity**: `objdump -d` the
   `RawSpinlock::lock`/`unlock`, `KeyedRawSpinlock::lock`,
   `SleepMutex::acquire` symbols on x86_64 for main vs the change —
   instruction-identical is the pass condition (no codegen drift).
   Plus the standard dual-arch build gate.
2. **Feature OFF, Tier 1**: one quiet syscost boot (KVM, standard
   machine config) — census in band, ticks ≥99.5/s, syscall volume
   within ±10%, key per-op averages within ±20%. Expectation:
   indistinguishable from main (the hooks do not exist); the run
   proves the registry rodata and module code shifted nothing.
3. **Feature ON, disarmed, Tier 1 + Tier 2**: syscost boot per row 2,
   PLUS one `pbench` boot: P1–P7 medians within ±20% of the baseline
   column or improved (the hooks sit on every lock path — this is the
   row that catches a slow `#[inline]` mistake). Scheduling domain:
   GRQ average hold and census in band (guarded-domain report).
4. **Feature ON, armed (debug class — report, do not gate)**: one
   syscost boot + pbench boot with `debug=lockdep`; report total
   syscall volume, read/write/clock_gettime/nanosleep averages, pbench
   medians, and checker-internal counters (chains recorded, BFS runs,
   edges observed) so the amortization claim ("chain hit skips the
   BFS") is visible in numbers. Armed mode is a diagnostic build
   class, like `syscost=1` — its numbers land in this document's
   landing notes, not the gate table.
5. **Per-pair microbench (new, required by the gate principle "if it
   is not measured, add the tool")**: extend `test/pbench.c` with a P8
   row — N iterations of uncontended `Spinlock<u64>` lock/unlock and
   keyed-lock lock/unlock, median ns per pair, reported for OFF / ON-
   disarmed / ON-armed builds. This is the direct measurement of the
   hook cost class, the same role `pbench` P2 plays for context
   switch.

## 8. Output files

`/proc/lockdep` (P0 ships the declared-registry view; P1 adds live
counts), reference layout, Rux keys printed as the class name's table
index in the key column (classes are table rows, not addresses — the
name IS the key; acquire sites carry the symbolized addresses):

```text
all lock classes:
                   [grq] FD:    0 BD:    1 ....: grq  (INV-LOCK-5)
 -> [futex-bucket] futex-bucket (INV-LOCK-5)
...
```

FD/BD = forward/backward dependency counts over declared+observed
edges (reference column meanings kept); the 4 usage chars are `....`
until P2 arms irq-state tracking.

`/proc/lockdep_chains` (P1): per recorded chain `irq_context: 0 |
hardirq | softirq` then one `[class] name` line per depth, the
reference shape.

`/proc/lock_stat` (P2): the reference's exact 13-column header
(`class name con-bounces contentions waittime-min waittime-max
waittime-total waittime-avg acq-bounces acquisitions holdtime-min
holdtime-max holdtime-total holdtime-avg`) with per-contention-site
` -> [site]` lines. Contentions = acquire slow-path entered (the
spin-loop entry for spinlocks, the park for SleepMutex); wait/hold
times sampled on contended acquires only, via the existing timer (ns
granularity is NOT required — the reference itself samples at tick
granularity; report µs). When the checker has self-disabled, the first
line is the reference's `*WARNING* lock debugging disabled!! - possibly
due to a lockdep warning`.

## 9. Phased landing

**P0 (landed with this document)** — the static order registry:
- `kernel/src/debug/lockdep.rs`: `lock_classes!` + `declared_edges!`
  tables mirroring every class and pair named in lock-ordering.md
  (18 classes, 11 declared edges at landing); `validate()` — the
  boot-time check of the DECLARED model (unique names, known edge
  endpoints, acyclicity via iterative 3-color DFS, strict level
  monotonicity, no sleep-domain class inside a spin-family edge,
  lockkey-impl mirror consistency); `/proc/lockdep` (declared view);
  `debug=lockdep` arms the boot-time validation print.
- No hot-path code. No behavior change when the switch is off beyond
  the rodata table and the /proc file.
- **P0 findings** (surfaced by encoding the document as data):
  1. lock-ordering.md's hierarchy diagram places the futex bucket
     (L3b) INSIDE the GRQ lock (L2a), which contradicts INV-LOCK-5,
     Path 4, and the compile-enforced lockkey impls — the bucket is
     OUTER to GRQ on the futex-wake path. The registry encodes the
     invariant-table truth (bucket outer, level 2; GRQ inner, level
     3); the diagram's numbering is stale and should be corrected by
     the lock-ordering.md owner.
  2. `INV-LOCK-6` is defined TWICE in the tree with different
     meanings: lock-ordering.md's table says the virtio-net
     stats/queue pair, while `sync/spinlock.rs`'s module comment says
     the virtio-blk waitqueue rule. The registry carries both pairs
     (the waitqueue edge labeled `spinlock.rs waitqueue rule`); the
     id collision should be renumbered by the lock-ordering.md owner
     (one of the two becomes INV-LOCK-9 or similar).

**P1 — runtime acquire tracking + cycle detection**:
- cargo feature `debug-lockdep`; hooks per §5 surfaces 1–2;
  per-task held stack in `Task`; chain-key table; BFS per §6;
  splats; self-disable + taint; `/proc/lockdep` live view +
  `/proc/lockdep_chains`.
- Measurement rows 1–5 of §7 run with the merge.

**P2 — irq-usage classes and lock_stat**:
- usage bits: acquired-in-hardirq / acquired-in-softirq /
  acquired-with-irqs-enabled per class, sourced from the
  `preempt_count` bitfield (already maintained at every acquire
  site's protocol layers — no new bookkeeping on the hot path, one
  read in the armed slow path); state-transition checks (a class
  both irq-safe and irq-unsafe = splat, the reference's safe/unsafe
  exclusivity).
- `/proc/lock_stat` contention/holdtime stats (contended acquires
  only).
- Heap-lock class keys per construction site (dynamic-key scheme) —
  re-evaluated against P1's noise numbers before committing.

Deferred beyond P2 (recorded, not designed): recursive-read strong-path
checks for RwSpinlock (P1–P2 treat every rw acquire as exclusive —
sound, possibly noisy; the reference's 18-state read recursion waits
until real rw nesting exists in the registry), lockdep_assert_held()
annotations, zapped-class reclamation (static classes never die).

## 10. Test plan

Per phase, all under `make test` (riscv64) plus the phase's §7 rows:

- **P0** (in tree with this document):
  - `tests/lockdep`: registry validation passes on the shipped table;
    synthetic bad tables (a cycle, a reverse-level edge, an unknown
    class reference) each produce the expected verdict — the
    validator's negative halves are tested on throwaway tables, not by
    corrupting the real one.
  - cross-check: every `CanNestInside` impl in `sync/lockkey.rs` has
    a same-pair declared edge (a hand-listed mirror of the impl set —
    trait impls cannot be enumerated, so the test pins the known set
    and fails when the two lists drift).
  - boot smoke with `debug=lockdep`: the validation PASS line prints.
- **P1**:
  - unit: with the checker force-armed on a dedicated test task —
    declared-order acquire passes; forced reverse acquire of a
    declared pair produces ORDER-VIOLATION; forced sleep-class acquire
    under a spin hold produces SLEEP-IN-ATOMIC-CONTEXT; a synthetic
    two-task cycle produces CYCLE (test-only API: direct
    `lockdep_acquire` calls, no real wedging); chain-key amortization
    counter increments once per unique chain.
  - full `make test` green (1096+ suite) with the feature ON and
    armed for at least one run — the checker must run silent over the
    whole unit-test workload (any UNDECLARED-NEST note found there is
    either a new lock-ordering.md row or a finding; both are
    outcomes, not test noise).
  - build-identity disassembly diff row of §7.
- **P2**: usage-bit transitions (fake hardirq context via
  preempt_count manipulation in the unit-test harness); lock_stat
  columns populated on a contended-lock microscenario; self-disable
  semantics (first splat clears ARMED, /proc/lock_stat warns).

## 11. Appendix — RCU stall detector adjudication

**Premise check (state the facts first).** The lane charter said Rux
has "no RCU grace-period machinery, only the softirq vector name".
That is stale: `kernel/src/sync/rcu.rs` is a working Tiny-RCU — read
side is `preempt_disable` (rcu_read_lock), quiescent states are
reported from `__schedule`/idle/user-return, grace periods are
computed by WAITERS (`synchronize_rcu`, `synchronize_rcu_bounded_*`,
`synchronize_rcu_forced_*` — bounded, sleepy, polled variants), and
the RCU softirq (vector 9) drains `call_rcu` callback batches and
advances the generation. It is not the reference's tree RCU: there is
no grace-period kthread, no GP state machine, no expedited/passive
mode distinction — a grace period exists only while some waiter is
inside a synchronize call.

**Would a reference-shaped RCU stall detector fit?** The reference
detector hangs off the GP kthread: a heartbeat that, if a grace
period fails to complete within `rcu_cpu_stall_timeout` (default
21 s), splats every CPU's stack and names the stuck CPU. Mapping that
shape onto Rux means inventing the anchor the reference detector
hangs from — a periodic "is any grace period older than T" check
needs a GP engine that outlives individual waiters, which Rux does
not have. Blocked-on-design is therefore the honest verdict, but with
a sharper trigger than "until/unless Rux adopts RCU": the detector
becomes designable the day RCU grows a GP engine (kthread-driven
grace periods). Until then the WAITER-side diagnostics already cover
the reference detector's failure class with better attribution:
`synchronize_rcu_*` budgets print `RCU-GP-BOUND site=... waited>Nms`
naming the non-quiescent CPU, `RCU-STUCK-CPU` adds the monopolist
task's comm/pid/preempt_count/last-syscall/tick-aliveness, and 32
consecutive failed rounds trigger a cross-CPU task dump. A
reference-shaped detector would report the same event with LESS
context (the reference splat names the stuck CPU; ours names the
stuck TASK and why its tick is blind).

**Does a placeholder (softirq-vector watch) have value?** No — three
reasons, tested against what the watch would actually see:

1. RCU softirq firing is not a grace-period signal in Rux. The vector
   fires only when a CPU has `call_rcu` callbacks to drain; a fully
   quiescent system raises zero RCU softirqs while grace periods
   complete instantly. "RCU softirq hasn't run in N seconds" has no
   failure it detects — a healthy quiet boot trips it.
2. The one real hazard the vector could see — a callback backlog on
   a CPU that never runs softirqs (IF=0 monopolist) — is already
   covered by better-placed instruments: the softlockup detector
   (same class of wedge), `allocwatch` (names the class-lock holder),
   the syscost tick census (tick-alive accounting), and the waiter
   diagnostics above (which fire even when the backlog belongs to a
   DIFFERENT CPU's waiter).
3. A watch with no true-positive class still costs review surface and
   a serial line that cries wolf — the exact anti-pattern the
   debug-switch catalog avoids by only carrying instruments that have
   caught something.

Recommendation: land nothing RCU-side with this design. If a future
lane moves Rux RCU to a GP-engine design, the stall detector belongs
IN that change (it is a GP-engine component, not a debug-layer
observer), and this document's §6 splat shape is the report format to
reuse. Recorded as: RCU stall detector — blocked-on-design, anchor
condition "GP engine exists", placeholder rejected with the reasoning
above.

---

Document Version: v1.0
Last Updated: 2026-11-07
