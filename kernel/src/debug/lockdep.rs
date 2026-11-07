//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! lockdep-lite P0 — the static lock-order registry and its
//! boot-time validation (docs/development/lockdep-lite-design.md).
//!
//! The registry is the machine-readable mirror of
//! docs/architecture/lock-ordering.md: one row per lock class named
//! there, one edge per NAMED nesting pair (INV-LOCK-*). It is the
//! "model of legal order" that the P1 runtime checker will validate
//! observed acquisitions against — and on its own, P0 already checks
//! the DECLARED model for internal consistency at boot:
//!
//! - class names are unique;
//! - every edge references a declared class;
//! - the declared graph is acyclic (INV-LOCK-4 at the document level);
//! - declared edges nest strictly deeper (outer.level < inner.level);
//! - no declared edge puts a sleep-domain class inside a spin-family
//!   one (INV-LOCK-8 rule 1 — the reverse, spin-inside-sleep, is the
//!   normal coarse-lock pattern and stays legal);
//! - every `CanNestInside` impl in sync/lockkey.rs has a matching
//!   declared edge (the mirror list below; trait impls cannot be
//!   enumerated, so the known set is pinned and drift is a defect).
//!
//! P0 has NO hot-path code: the registry is const data, the
//! validation runs once at boot (only when `debug=lockdep` is armed),
//! and `/proc/lockdep` renders the declared view. P1 (runtime acquire
//! tracking, cycle BFS, splats) lands behind the cargo feature
//! `debug-lockdep` per the design document's phasing.
//!
//! Level note: the `level` field is the class's nesting depth in the
//! declared-order DAG (outermost = 0), NOT the level numbers of
//! lock-ordering.md's hierarchy diagram verbatim — the diagram's
//! futex-bucket placement contradicted INV-LOCK-5 and the
//! compile-enforced lockkey impls (bucket is OUTER to GRQ on the
//! futex-wake path); the invariant table won and the diagram was
//! corrected 2026-11-07 (futex bucket is its own Level-2e branch).

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ============================================================================
// Data model (design doc §3)
// ============================================================================

/// Which primitive family a class belongs to. The family decides the
/// context checks: only sleep-domain classes are forbidden inside
/// spin-family holds / atomic context (INV-LOCK-8 rule 1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LockDomain {
    /// Spinlock-family (Spinlock/RawSpinlock/keyed spinlocks): never
    /// sleeps, sections short by construction.
    Spin,
    /// SleepMutex: may be held across blocking I/O, waiters park.
    Sleep,
    /// RwSpinlock (read-write spin family): P0 treats every acquire
    /// as exclusive; read-recursion checks are a P2+ refinement.
    RwSpin,
}

/// Index into the static CLASSES table. Dense and stable per build.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LockClassId(pub u16);

/// One lock class — a static declaration, not a runtime object
/// (design doc §3). One row per class named in lock-ordering.md.
pub struct LockClassDesc {
    /// Short stable name (the lockkey `NAME` spelling where a keyed
    /// type exists, so /proc/lockdep and watchdog prints agree).
    pub name: &'static str,
    /// Primitive family (decides context checks, see LockDomain).
    pub domain: LockDomain,
    /// Nesting depth in the declared-order DAG (outermost = 0).
    pub level: u8,
    /// The lock-ordering.md section/invariant that names this class.
    pub doc: &'static str,
    /// The lockkey key type that mints acquire events for this class
    /// in P1 (None for hand-instrumented or not-yet-keyed classes).
    pub key: Option<&'static str>,
}

/// A declared nesting edge: OUTER held, INNER acquired. Source of
/// truth: lock-ordering.md (the invariant rows); absence of an edge is
/// the negative statement (an observed nesting with no row is either a
/// new document row or a bug — the P1 UNDECLARED-NEST note).
pub struct DeclaredEdge {
    pub outer: LockClassId,
    pub inner: LockClassId,
    /// The invariant row this edge comes from.
    pub inv: &'static str,
}

// ============================================================================
// The lock_classes! declaration table (design doc §4)
// ============================================================================
//
// One declaration site derives every artifact (the tracepoints!
// derive pattern): the per-class id consts and the CLASSES table.
// macro_rules cannot concatenate identifiers, so each row names its
// id const explicitly; the positional index is derived.

/// Derive the per-class positional id consts: each `pub const` counts
/// the rows declared before it.
macro_rules! lock_class_ids {
    (@ids [$($prev:ident)*]) => {};
    (@ids [$($prev:ident)*]
        $id:ident / $name:literal / $domain:ident, $level:literal, $doc:literal, $key:expr,
        $($rest:tt)*
    ) => {
        // Rows without declared edges (positional classes) keep their
        // id consts for future edges and P1 hooks — allowed dead.
        #[allow(dead_code)]
        pub const $id: LockClassId =
            LockClassId(0u16 $(+ { let _ = stringify!($prev); 1u16 })*);
        lock_class_ids!(@ids [$($prev)* $id] $($rest)*);
    };
    ($($item:tt)*) => {
        lock_class_ids!(@ids [] $($item)*);
    };
}

/// Derive the CLASSES table from the same declaration list.
macro_rules! lock_class_table {
    ($($id:ident / $name:literal / $domain:ident, $level:literal, $doc:literal, $key:expr,)* $(,)?) => {
        /// Every declared lock class, in id order. The single source
        /// of truth for class identity (one file, one reviewable
        /// diff per added pair — see design doc §4).
        pub static CLASSES: &[LockClassDesc] = &[
            $(
                LockClassDesc {
                    name: $name,
                    domain: LockDomain::$domain,
                    level: $level,
                    doc: $doc,
                    key: $key,
                },
            )*
        ];
    };
}

// The registry (lock-ordering.md v1.1). Rows appear in nesting-depth
// groups; every class named in the document gets a row, edges exist
// only for NAMED pairs (INV-LOCK-5/6/7/8/PI-1 + the waitqueue wake
// rule documented in sync/spinlock.rs).
lock_class_ids! {
    // Sleep domain (INV-LOCK-8)
    VFS_MUTATION     / "vfs-mutation"      / Sleep,  0, "INV-LOCK-8",               None,
    EXT4_BIG         / "ext4-big"          / Sleep,  1, "INV-LOCK-8",               None,
    // Futex (INV-LOCK-5, INV-LOCK-PI-1)
    FUTEX_BUCKET     / "futex-bucket"      / Spin,   2, "INV-LOCK-5",               Some("FutexBucketKey"),
    WAITER_SLOT      / "futex-waiter-slot" / Spin,   3, "INV-LOCK-5",               Some("WaiterSlotKey"),
    GRQ              / "grq"               / Spin,   3, "INV-LOCK-5",               Some("GrqKey"),
    PI_BUCKET        / "futex-pi-bucket"   / Spin,   3, "INV-LOCK-PI-1",            None,
    PI_POOL          / "futex-pi-pool"     / Spin,   4, "INV-LOCK-PI-1",            None,
    // Net (INV-LOCK-6) / waitqueue (INV-LOCK-9)
    NET_QUEUE        / "net-queue"         / Spin,   2, "INV-LOCK-6",               Some("NetQueueKey"),
    NET_STATS        / "net-stats"         / Spin,   3, "INV-LOCK-6",               Some("NetStatsKey"),
    WAIT_QUEUE       / "wait-queue"        / Spin,   2, "INV-LOCK-9",               None,
    // mm (INV-LOCK-7 + hierarchy diagram)
    VMA_MANAGER      / "vma-manager"       / RwSpin, 1, "INV-LOCK-7",               None,
    PTE_GLOBAL_EPOCH / "pte-global-epoch"  / RwSpin, 2, "INV-LOCK-7",               None,
    PTE_PER_MM       / "pte-per-mm"        / Spin,   2, "INV-LOCK-7",               None,
    PAGE_DESC        / "page-desc"         / Spin,   3, "INV-LOCK-7",               None,
    MM_ZONE          / "mm-zone"           / Spin,   3, "lock-ordering.md L3a",     None,
    // Process/fs positional classes (hierarchy diagram; pairs not
    // named as invariants yet — P1's undeclared-nest notes will
    // collect their real edges for the document).
    PROC_TREE        / "process-tree"      / Spin,   2, "lock-ordering.md L2b",     None,
    INODE            / "inode"             / Spin,   2, "lock-ordering.md L2c",     None,
    DENTRY_CACHE     / "dentry-cache"      / Spin,   2, "lock-ordering.md L2d",     None,
}

lock_class_table! {
    VFS_MUTATION     / "vfs-mutation"      / Sleep,  0, "INV-LOCK-8",               None,
    EXT4_BIG         / "ext4-big"          / Sleep,  1, "INV-LOCK-8",               None,
    FUTEX_BUCKET     / "futex-bucket"      / Spin,   2, "INV-LOCK-5",               Some("FutexBucketKey"),
    WAITER_SLOT      / "futex-waiter-slot" / Spin,   3, "INV-LOCK-5",               Some("WaiterSlotKey"),
    GRQ              / "grq"               / Spin,   3, "INV-LOCK-5",               Some("GrqKey"),
    PI_BUCKET        / "futex-pi-bucket"   / Spin,   3, "INV-LOCK-PI-1",            None,
    PI_POOL          / "futex-pi-pool"     / Spin,   4, "INV-LOCK-PI-1",            None,
    NET_QUEUE        / "net-queue"         / Spin,   2, "INV-LOCK-6",               Some("NetQueueKey"),
    NET_STATS        / "net-stats"         / Spin,   3, "INV-LOCK-6",               Some("NetStatsKey"),
    WAIT_QUEUE       / "wait-queue"        / Spin,   2, "INV-LOCK-9",               None,
    VMA_MANAGER      / "vma-manager"       / RwSpin, 1, "INV-LOCK-7",               None,
    PTE_GLOBAL_EPOCH / "pte-global-epoch"  / RwSpin, 2, "INV-LOCK-7",               None,
    PTE_PER_MM       / "pte-per-mm"        / Spin,   2, "INV-LOCK-7",               None,
    PAGE_DESC        / "page-desc"         / Spin,   3, "INV-LOCK-7",               None,
    MM_ZONE          / "mm-zone"           / Spin,   3, "lock-ordering.md L3a",     None,
    PROC_TREE        / "process-tree"      / Spin,   2, "lock-ordering.md L2b",     None,
    INODE            / "inode"             / Spin,   2, "lock-ordering.md L2c",     None,
    DENTRY_CACHE     / "dentry-cache"      / Spin,   2, "lock-ordering.md L2d",     None,
}

/// The declared nesting edges (a plain static array — design doc §4:
/// no macro machinery needed for edges). Every edge cites the
/// invariant row it comes from.
pub static DECLARED_EDGES: &[DeclaredEdge] = &[
    // INV-LOCK-5: bucket outer, GRQ and waiter slots nest inside.
    DeclaredEdge { outer: FUTEX_BUCKET, inner: GRQ, inv: "INV-LOCK-5" },
    DeclaredEdge { outer: FUTEX_BUCKET, inner: WAITER_SLOT, inv: "INV-LOCK-5" },
    // INV-LOCK-PI-1: plain bucket -> PI bucket -> PI pool slot.
    DeclaredEdge { outer: FUTEX_BUCKET, inner: PI_BUCKET, inv: "INV-LOCK-PI-1" },
    DeclaredEdge { outer: PI_BUCKET, inner: PI_POOL, inv: "INV-LOCK-PI-1" },
    // INV-LOCK-6: stats nest inside the virtio-net queue locks.
    DeclaredEdge { outer: NET_QUEUE, inner: NET_STATS, inv: "INV-LOCK-6" },
    // The waitqueue wake rule (sync/spinlock.rs): wake_up runs under
    // the wait-queue lock and takes the GRQ lock per waiter. Numbered
    // INV-LOCK-9 since 2026-11-07 (was a duplicated INV-LOCK-6 that
    // collided with the virtio-net pair).
    DeclaredEdge { outer: WAIT_QUEUE, inner: GRQ, inv: "INV-LOCK-9" },
    // INV-LOCK-7: vma_manager -> PTE section -> page_desc. The two
    // PTE-section classes have NO edge between them (rule 3: an
    // overlap of the same mm is forbidden; the undeclared check
    // reports any observed nesting in P1).
    DeclaredEdge { outer: VMA_MANAGER, inner: PTE_GLOBAL_EPOCH, inv: "INV-LOCK-7" },
    DeclaredEdge { outer: VMA_MANAGER, inner: PTE_PER_MM, inv: "INV-LOCK-7" },
    DeclaredEdge { outer: PTE_GLOBAL_EPOCH, inner: PAGE_DESC, inv: "INV-LOCK-7" },
    DeclaredEdge { outer: PTE_PER_MM, inner: PAGE_DESC, inv: "INV-LOCK-7" },
    // INV-LOCK-8: the sanctioned sleep-domain nesting (both
    // task-recursive; same-task re-entry is legal, not an edge).
    DeclaredEdge { outer: VFS_MUTATION, inner: EXT4_BIG, inv: "INV-LOCK-8" },
];

/// The mirror of sync/lockkey.rs's `CanNestInside` impl set, as
/// (outer class name, inner class name) pairs. Trait impls cannot be
/// enumerated at runtime, so the known set is pinned here; the
/// validator fails when a mirror pair has no declared edge (the two
/// lists drifted) and the unit test fails when the pin itself goes
/// stale against lockkey.rs.
pub static KEYED_IMPL_MIRROR: &[(&str, &str)] = &[
    // impl CanNestInside<FutexBucketKey> for GrqKey
    ("futex-bucket", "grq"),
    // impl CanNestInside<NetQueueKey> for NetStatsKey
    ("net-queue", "net-stats"),
    // impl CanNestInside<FutexBucketKey> for WaiterSlotKey
    ("futex-bucket", "futex-waiter-slot"),
];

// ============================================================================
// Registry validation (design doc §10, P0)
// ============================================================================

/// One validation finding.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViolationKind {
    /// Two classes share a name (class identity is by name in splats
    /// and the mirror list).
    DuplicateName,
    /// An edge references a class index outside the table.
    UnknownClass,
    /// The declared graph has a cycle (INV-LOCK-4 violated by the
    /// document itself).
    Cycle,
    /// A declared edge does not nest strictly deeper
    /// (outer.level >= inner.level).
    LevelOrder,
    /// A declared edge puts a sleep-domain class inside a spin-family
    /// one (INV-LOCK-8 rule 1).
    SleepInsideSpin,
    /// A keyed-impl mirror pair has no declared edge (the lockkey
    /// impl set and the registry drifted).
    LockkeyMirrorDrift,
}

impl ViolationKind {
    /// The short tag used in boot reports and test output.
    pub fn as_str(self) -> &'static str {
        match self {
            ViolationKind::DuplicateName => "duplicate-name",
            ViolationKind::UnknownClass => "unknown-class",
            ViolationKind::Cycle => "cycle",
            ViolationKind::LevelOrder => "level-order",
            ViolationKind::SleepInsideSpin => "sleep-inside-spin",
            ViolationKind::LockkeyMirrorDrift => "lockkey-mirror-drift",
        }
    }
}

/// Validate a (classes, edges) registry — a pure function so unit
/// tests can exercise the negative halves on synthetic tables without
/// corrupting the real one. Structural checks only (names, edge
/// endpoints, acyclicity, level monotonicity, sleep-inside-spin); the
/// lockkey-impl mirror is a property of the FULL registry and lives in
/// [`validate_lockkey_mirror`]. Returns one entry per finding; an
/// empty vector means the declared model is internally consistent.
pub fn validate(classes: &[LockClassDesc], edges: &[DeclaredEdge]) -> Vec<(ViolationKind, String)> {
    let mut out = Vec::new();

    // 1. Unique class names.
    for (i, c) in classes.iter().enumerate() {
        for d in classes.iter().skip(i + 1) {
            if c.name == d.name {
                out.push((
                    ViolationKind::DuplicateName,
                    format!("class '{}' declared twice", c.name),
                ));
            }
        }
    }

    // 2. Edges reference declared classes.
    for e in edges {
        for id in [e.outer, e.inner] {
            if id.0 as usize >= classes.len() {
                out.push((
                    ViolationKind::UnknownClass,
                    format!("edge {}->{} references class {} outside the table (len {})",
                        id_label(classes, e.outer), id_label(classes, e.inner),
                        id.0, classes.len()),
                ));
            }
        }
    }

    // 3. Acyclicity of the declared graph — iterative 3-color DFS
    //    (no recursion: kernel stack discipline). On a back edge the
    //    gray stack is the cycle path.
    let n = classes.len();
    // 0 = white (unvisited), 1 = gray (on stack), 2 = black (done).
    let mut color = alloc::vec![0u8; n];
    let mut gray: Vec<usize> = Vec::new();
    // Iterate roots explicitly so the loop is fully iterative: each
    // outer iteration walks one white node's subtree with an explicit
    // work stack of (node, next-edge-cursor).
    let mut adjacency: Vec<Vec<usize>> = alloc::vec![Vec::new(); n];
    for e in edges {
        let (o, i) = (e.outer.0 as usize, e.inner.0 as usize);
        if o < n && i < n {
            adjacency[o].push(i);
        }
    }
    for root in 0..n {
        if color[root] != 0 {
            continue;
        }
        // Work stack of (node, cursor into adjacency[node]).
        let mut work: Vec<(usize, usize)> = Vec::new();
        color[root] = 1;
        gray.push(root);
        work.push((root, 0));
        while let Some(&mut (node, ref mut cur)) = work.last_mut() {
            if *cur < adjacency[node].len() {
                let next = adjacency[node][*cur];
                *cur += 1;
                match color[next] {
                    0 => {
                        color[next] = 1;
                        gray.push(next);
                        work.push((next, 0));
                    }
                    1 => {
                        // Back edge: gray stack from `next` onward is
                        // the cycle.
                        let path: Vec<&str> = gray
                            .iter()
                            .skip_while(|&&g| g != next)
                            .map(|&g| classes[g].name)
                            .collect();
                        out.push((
                            ViolationKind::Cycle,
                            format!("declared cycle: {}", path.join(" -> ")),
                        ));
                    }
                    _ => {}
                }
            } else {
                color[node] = 2;
                gray.pop();
                work.pop();
            }
        }
    }

    // 4. Level monotonicity + 5. sleep-inside-spin, over the same
    //    edge walk.
    for e in edges {
        let (o, i) = (e.outer.0 as usize, e.inner.0 as usize);
        if o >= n || i >= n {
            continue; // already reported as UnknownClass
        }
        if classes[o].level >= classes[i].level {
            out.push((
                ViolationKind::LevelOrder,
                format!("edge {}(lvl {}) -> {}(lvl {}) does not nest deeper [{}]",
                    classes[o].name, classes[o].level,
                    classes[i].name, classes[i].level, e.inv),
            ));
        }
        if classes[i].domain == LockDomain::Sleep && classes[o].domain != LockDomain::Sleep {
            out.push((
                ViolationKind::SleepInsideSpin,
                format!("edge {}({:?}) -> {}(Sleep) puts a sleep-domain class inside a spin-family hold [{}]",
                    classes[o].name, classes[o].domain, classes[i].name, e.inv),
            ));
        }
    }

    // (The lockkey-impl mirror check is a full-registry property —
    // see validate_lockkey_mirror below; synthetic test tables do not
    // carry the keyed classes.)

    out
}

/// The lockkey mirror check, run against the FULL registry: every
/// pinned `CanNestInside` impl pair (KEYED_IMPL_MIRROR) must have a
/// declared edge. Separate from [`validate`] because the mirror is a
/// property of the real table only — synthetic tables in tests do not
/// declare the keyed classes.
pub fn validate_lockkey_mirror(
    classes: &[LockClassDesc],
    edges: &[DeclaredEdge],
) -> Vec<(ViolationKind, String)> {
    let mut out = Vec::new();
    for (outer, inner) in KEYED_IMPL_MIRROR {
        let found = edges.iter().any(|e| {
            let (o, i) = (e.outer.0 as usize, e.inner.0 as usize);
            o < classes.len() && i < classes.len()
                && classes[o].name == *outer && classes[i].name == *inner
        });
        if !found {
            out.push((
                ViolationKind::LockkeyMirrorDrift,
                format!("lockkey impl {} inside {} has no declared edge", inner, outer),
            ));
        }
    }
    out
}

/// Class name for messages (falls back to the raw index for unknown
/// ids so a broken edge still prints usefully).
fn id_label(classes: &[LockClassDesc], id: LockClassId) -> &str {
    classes
        .get(id.0 as usize)
        .map(|c| c.name)
        .unwrap_or("<unknown>")
}

// ============================================================================
// Boot hook + /proc/lockdep (declared view)
// ============================================================================

/// Validate the shipped registry at boot. Runs only when the
/// `debug=lockdep` switch is armed (P0 costs nothing on a normal
/// boot); findings print via pr_warn, success prints one line.
/// P1: the same switch ARMS runtime acquire tracking (when the cargo
/// feature `debug-lockdep` compiled the hooks) and runs the synthetic
/// selftest — one switch, three duties, all zero on a default boot.
pub fn boot_validate() {
    if !crate::debug::switches::enabled(crate::debug::switches::DebugSwitch::LockDep) {
        return;
    }
    let mut findings = validate(CLASSES, DECLARED_EDGES);
    findings.append(&mut validate_lockkey_mirror(CLASSES, DECLARED_EDGES));
    if findings.is_empty() {
        crate::pr_info!(
            "lockdep: registry: {} classes, {} declared edges, validation PASS",
            CLASSES.len(),
            DECLARED_EDGES.len()
        );
    } else {
        for (kind, detail) in &findings {
            crate::pr_warn!("lockdep: registry {}: {}", kind.as_str(), detail);
        }
        crate::pr_warn!(
            "lockdep: registry validation FAIL ({} findings); see docs/development/lockdep-lite-design.md",
            findings.len()
        );
    }
    #[cfg(feature = "debug-lockdep")]
    {
        arm_runtime();
        boot_selftest();
    }
}

/// `/proc/lockdep` — the reference file's layout (per class: key,
/// forward/backward dependency counts, usage chars, name, then
/// ` -> [key] name` lines for direct dependencies), rendering the
/// DECLARED registry plus (P1, feature-gated) the observed live
/// counts. The key column carries the class's table index (Rux
/// classes are table rows; the name IS the key — design doc §8).
/// The usage chars stay `....` until P2 arms irq-state tracking.
pub fn generate_proc_lockdep() -> Vec<u8> {
    let mut out = String::new();
    out.push_str("all lock classes:\n");
    // P1 live edge set, read once under the graph lock (feature on).
    #[cfg(feature = "debug-lockdep")]
    let observed: Vec<(u16, u16, u64)> = observed_edges_snapshot();
    for (idx, c) in CLASSES.iter().enumerate() {
        let fd_declared = DECLARED_EDGES
            .iter()
            .filter(|e| e.outer.0 as usize == idx)
            .count();
        let bd_declared = DECLARED_EDGES
            .iter()
            .filter(|e| e.inner.0 as usize == idx)
            .count();
        // FD/BD per the reference column meanings, counted over
        // declared+observed edges (design doc §8). A pair declared
        // AND observed counts once (the observed edge is the same
        // dependency, now with a live acquire site).
        #[cfg(feature = "debug-lockdep")]
        let fd = {
            let mut n = fd_declared;
            for (o, i, _) in &observed {
                let (o, i) = (*o as usize, *i as usize);
                if o == idx && !DECLARED_EDGES.iter().any(|e| {
                    e.outer.0 as usize == o && e.inner.0 as usize == i
                }) {
                    n += 1;
                }
            }
            n
        };
        #[cfg(feature = "debug-lockdep")]
        let bd = {
            let mut n = bd_declared;
            for (o, i, _) in &observed {
                let (o, i) = (*o as usize, *i as usize);
                if i == idx && !DECLARED_EDGES.iter().any(|e| {
                    e.outer.0 as usize == o && e.inner.0 as usize == i
                }) {
                    n += 1;
                }
            }
            n
        };
        #[cfg(not(feature = "debug-lockdep"))]
        let (fd, bd) = (fd_declared, bd_declared);
        out.push_str(&format!(
            "0x{:04x} FD:{:5} BD:{:5} ....: {} ({}, key={})\n",
            idx, fd, bd, c.name, c.doc, c.key.unwrap_or("-")
        ));
        for e in DECLARED_EDGES.iter().filter(|e| e.outer.0 as usize == idx) {
            let inner = &CLASSES[e.inner.0 as usize];
            out.push_str(&format!(
                " -> [0x{:04x}] {} ({})\n",
                e.inner.0, inner.name, e.inv
            ));
        }
        // Observed-but-undeclared edges (the UNDECLARED-NEST feedback
        // loop, visible without waiting for the serial note).
        #[cfg(feature = "debug-lockdep")]
        for (o, i, site) in &observed {
            let (o, i) = (*o as usize, *i as usize);
            if o == idx {
                let declared = DECLARED_EDGES.iter().any(|e| {
                    e.outer.0 as usize == o && e.inner.0 as usize == i
                });
                let inner = CLASSES.get(i).map(|c| c.name).unwrap_or("<unknown>");
                if !declared {
                    out.push_str(&format!(
                        " -> [0x{:04x}] {} (observed, undeclared; at {})\n",
                        i,
                        inner,
                        crate::debug::ksyms::format_symbol(*site)
                    ));
                } else {
                    out.push_str(&format!(
                        " -> [0x{:04x}] {} (observed at {})\n",
                        i,
                        inner,
                        crate::debug::ksyms::format_symbol(*site)
                    ));
                }
            }
        }
        // The reference separates class blocks with a blank line.
        out.push('\n');
    }
    // Declared-edge census + P1 live counters at the tail (the
    // reference file has no tail; one block, cheap, self-describing).
    #[cfg(feature = "debug-lockdep")]
    {
        let (armed, reason) = runtime_state();
        let (acq, hits, chains, edges, bfs, notes, splats) = counters();
        out.push_str(&format!(
            "# lockdep-lite P1: {} classes, {} declared edges, runtime {} ({}), acquires={} chain-hits={} chains={} edges={} bfs={} undeclared-notes={} splats={}\n",
            CLASSES.len(),
            DECLARED_EDGES.len(),
            if armed { "ARMED" } else { "off" },
            disable_reason_str(reason),
            acq, hits, chains, edges, bfs, notes, splats
        ));
    }
    #[cfg(not(feature = "debug-lockdep"))]
    out.push_str(&format!(
        "# lockdep-lite P0: {} classes, {} declared edges (runtime tracking not compiled: cargo feature debug-lockdep off)\n",
        CLASSES.len(),
        DECLARED_EDGES.len()
    ));
    out.into_bytes()
}

/// `/proc/lockdep_chains` — per recorded chain, the reference shape
/// (`irq_context:` line then one `[class] name` line per depth).
/// P1 divergence (documented): every recorded chain's 64-bit key
/// lives in the amortization table, but the class LIST is retained
/// for the most recent [`CHAIN_SNAPS`] chains only (a bounded ring);
/// the reference stores every chain's content. Chains recorded
/// beyond the ring render as their key alone.
pub fn generate_proc_lockdep_chains() -> Vec<u8> {
    let mut out = String::new();
    out.push_str("all lock chains:\n");
    #[cfg(feature = "debug-lockdep")]
    {
        let snaps = chain_ring_snapshot();
        if snaps.is_empty() {
            out.push_str("# (no chains recorded yet)\n");
        }
        for s in &snaps {
            out.push_str("irq_context: 0\n");
            for k in &s.classes {
                let name = CLASSES
                    .get(*k as usize)
                    .map(|c| c.name)
                    .unwrap_or("<unknown>");
                out.push_str(&format!(" [0x{:04x}] {}\n", k, name));
            }
        }
        let (_, _, chains, _, _, _, _) = counters();
        out.push_str(&format!(
            "# {} chains recorded (keys), {} retained with class lists\n",
            chains,
            snaps.len()
        ));
    }
    #[cfg(not(feature = "debug-lockdep"))]
    out.push_str("# runtime tracking not compiled (cargo feature debug-lockdep off)\n");
    out.into_bytes()
}

// ============================================================================
// P1 — runtime acquire tracking (design doc §3/§5/§6)
// ============================================================================
//
// Everything below compiles only under the cargo feature
// `debug-lockdep`. The hook entry points (`lockdep_acquire` /
// `lockdep_release` and the classed twins) exist in BOTH builds with
// an empty body when the feature is off, but the instrumented lock
// paths call them through cfg-gated call sites — a default build
// carries not even the relaxed load (design §7 row 1: the OFF-feature
// build must be disassembly-identical on the lock paths; user
// constraint 2026-11-07: this is the merge-blocking hard bar).

#[cfg(feature = "debug-lockdep")]
mod runtime {
    use super::{CLASSES, DECLARED_EDGES, LockClassId, LockDomain};
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

    // ---- Arena sizes (design doc §3) ---------------------------------

    /// Per-task held-stack depth (the reference's MAX_LOCKDEP_DEPTH).
    pub const MAX_DEPTH: usize = 48;
    /// Observed-edge arena. On exhaustion the checker disables itself
    /// (reference self-limit semantics).
    const MAX_EDGES: usize = 2048;
    /// Chain-key table (power of two for masking).
    const MAX_CHAINS: usize = 8192;
    /// BFS queue capacity (design §6: a full queue disables).
    const BFS_QUEUE: usize = 32;
    /// Class-list ring retained for /proc/lockdep_chains (documented
    /// divergence: the reference stores every chain's content).
    const CHAIN_SNAPS: usize = 128;
    const SNAP_DEPTH: usize = 16;
    /// Bitset words for the per-class adjacency bitsets (256 classes).
    const WORDS: usize = 8;
    /// Cycle-path classes captured for the CYCLE splat. The BFS
    /// visits at most BFS_QUEUE nodes, so the reconstructed path fits.
    const PATH_MAX: usize = 40;

    // ---- Arm-time bitsets (read on the armed path, lock-free) --------

    /// fwd[c] bit i = declared edge (c -> i); rev[c] bit i = declared
    /// edge (i -> c). Built once at arm; the declared registry is
    /// compile-time constant, so these never change while armed.
    static FWD_BITSET: [[AtomicU32; WORDS]; 256] =
        [const { [const { AtomicU32::new(0) }; WORDS] }; 256];
    static REV_BITSET: [[AtomicU32; WORDS]; 256] =
        [const { [const { AtomicU32::new(0) }; WORDS] }; 256];
    /// Bit i set = class i is spin-family (the INV-LOCK-8 context
    /// check reads one word per held level).
    static SPIN_FAMILY: [AtomicU32; WORDS] = [const { AtomicU32::new(0) }; WORDS];

    #[inline]
    fn bit_get(set: &[AtomicU32; WORDS], c: usize) -> bool {
        (set[c / 32].load(Ordering::Relaxed) >> (c % 32)) & 1 != 0
    }

    // ---- Per-task held stack (design doc §3/§5) ----------------------

    /// The current task's held-lock stack: class ids + acquire sites.
    /// Lives in [`crate::process::task::Task`] (allocated with the
    /// task, no per-acquire allocation). Accessed only from the owning
    /// CPU (holds happen with preemption disabled or in task context).
    pub struct HeldStack {
        classes: [u16; MAX_DEPTH],
        sites: [u64; MAX_DEPTH],
        depth: u8,
        /// Reentrancy guard: set while the checker itself runs so a
        /// keyed lock taken by the reporting path can never re-enter
        /// the checker (design §5).
        in_checker: bool,
    }

    impl HeldStack {
        pub const fn new() -> Self {
            Self {
                classes: [0; MAX_DEPTH],
                sites: [0; MAX_DEPTH],
                depth: 0,
                in_checker: false,
            }
        }
        pub fn depth(&self) -> usize {
            self.depth as usize
        }
        fn contains(&self, c: LockClassId) -> bool {
            (0..self.depth()).any(|i| self.classes[i] == c.0)
        }
        /// Push one held entry. Returns false when the stack is full
        /// (the caller disables the checker — reference semantics).
        fn push(&mut self, c: LockClassId, site: u64) -> bool {
            if self.depth as usize >= MAX_DEPTH {
                return false;
            }
            let d = self.depth as usize;
            self.classes[d] = c.0;
            self.sites[d] = site;
            self.depth += 1;
            true
        }
        /// Pop the innermost occurrence of `c` (LIFO in the normal
        /// case; the bounded scan keeps the stack coherent when a
        /// protocol wrapper releases out of order or exit paths
        /// force-drop).
        fn pop_class(&mut self, c: LockClassId) {
            let n = self.depth as usize;
            for i in (0..n).rev() {
                if self.classes[i] == c.0 {
                    for j in i..n - 1 {
                        self.classes[j] = self.classes[j + 1];
                        self.sites[j] = self.sites[j + 1];
                    }
                    self.depth -= 1;
                    return;
                }
            }
        }
        /// The innermost site recorded for class `c` (held-stack
        /// lookup for splat path rendering).
        fn site_of(&self, c: LockClassId) -> Option<u64> {
            (0..self.depth())
                .rev()
                .find(|&i| self.classes[i] == c.0)
                .map(|i| self.sites[i])
        }
    }

    // ---- Verdicts (returned to the selftest and the unit tests) ------

    /// The outcome of one armed acquire check (design §6). The splat,
    /// when there is one, has already printed when this returns.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Verdict {
        /// Checks passed; chain newly recorded.
        Ok,
        /// Checks passed; chain already recorded (the amortization).
        OkChainHit,
        /// Same-class re-entry by this task (task-recursive legal).
        OkRecursive,
        /// Reverse of a declared edge (a live deadlock-capable order
        /// against lock-ordering.md).
        OrderViolation {
            held: LockClassId,
            acquiring: LockClassId,
            inv: &'static str,
        },
        /// A sleep-domain class acquired in atomic context
        /// (INV-LOCK-8 rule 1).
        SleepInAtomic {
            class: LockClassId,
            preempt: i32,
            held_spin: Option<LockClassId>,
        },
        /// The observed graph gained a cycle (INV-LOCK-4).
        Cycle { path: [u16; PATH_MAX], len: usize },
        /// An arena filled or the BFS bound tripped; the checker
        /// disabled itself (reference self-limit semantics).
        Exhausted,
    }

    // ---- Arm state, counters (design §7 row 4) -----------------------

    /// The runtime arm flag: one relaxed load per acquire site while
    /// disarmed (the TraceEvent::armed() contract). Set by
    /// [`arm_runtime`] when `debug=lockdep` boots; cleared by the
    /// self-disable path (first splat / exhaustion).
    pub static ARMED: AtomicBool = AtomicBool::new(false);

    pub static CTR_ACQUIRES: AtomicU64 = AtomicU64::new(0);
    pub static CTR_CHAIN_HITS: AtomicU64 = AtomicU64::new(0);
    static CTR_CHAINS: AtomicU64 = AtomicU64::new(0);
    static CTR_EDGES: AtomicU64 = AtomicU64::new(0);
    static CTR_BFS: AtomicU64 = AtomicU64::new(0);
    static CTR_NOTES: AtomicU64 = AtomicU64::new(0);
    static CTR_SPLATS: AtomicU64 = AtomicU64::new(0);

    /// Why the checker stopped (0 = armed/running).
    static DISABLE_REASON: AtomicU8 = AtomicU8::new(0);
    pub const DIS_ARMED: u8 = 0;
    pub const DIS_SPLAT: u8 = 1;
    pub const DIS_CHAINS_FULL: u8 = 2;
    pub const DIS_EDGES_FULL: u8 = 3;
    pub const DIS_BFS_BOUND: u8 = 4;
    pub const DIS_DEPTH: u8 = 5;

    pub fn disable_reason_str(r: u8) -> &'static str {
        match r {
            DIS_ARMED => "running",
            DIS_SPLAT => "disabled after first splat (debug_locks=0)",
            DIS_CHAINS_FULL => "disabled: chain table full",
            DIS_EDGES_FULL => "disabled: edge arena full",
            DIS_BFS_BOUND => "disabled: BFS bound exceeded",
            DIS_DEPTH => "disabled: held-stack depth exceeded",
            _ => "unknown",
        }
    }

    /// Arm the runtime tracker (the boot-switch path): build the
    /// adjacency bitsets, then set ARMED.
    pub fn arm_runtime() {
        build_bitsets();
        ARMED.store(true, Ordering::Release);
        DISABLE_REASON.store(DIS_ARMED, Ordering::Relaxed);
        crate::pr_info!(
            "lockdep: runtime tracking armed (cargo feature debug-lockdep; hooks on keyed locks + SleepMutex)"
        );
    }

    fn build_bitsets() {
        for i in 0..WORDS {
            SPIN_FAMILY[i].store(0, Ordering::Relaxed);
            for c in 0..256 {
                FWD_BITSET[c][i].store(0, Ordering::Relaxed);
                REV_BITSET[c][i].store(0, Ordering::Relaxed);
            }
        }
        for (i, c) in CLASSES.iter().enumerate().take(256) {
            if c.domain != LockDomain::Sleep {
                SPIN_FAMILY[i / 32].fetch_or(1 << (i % 32), Ordering::Relaxed);
            }
        }
        for e in DECLARED_EDGES {
            let (o, i) = (e.outer.0 as usize, e.inner.0 as usize);
            if o < 256 && i < 256 {
                FWD_BITSET[o][i / 32].fetch_or(1 << (i % 32), Ordering::Relaxed);
                REV_BITSET[i][o / 32].fetch_or(1 << (o % 32), Ordering::Relaxed);
            }
        }
    }

    /// Test/selftest reset: clear every arena and counter, rebuild the
    /// bitsets, re-arm. Not reachable on a production path (the boot
    /// selftest calls it once before real tracking begins).
    pub fn reset_for_test() {
        let mut g = GRAPH.lock();
        g.edge_count = 0;
        g.ring_next = 0;
        g.ring_count = 0;
        drop(g);
        for k in CHAIN_KEYS.iter() {
            k.store(0, Ordering::Release);
        }
        CTR_ACQUIRES.store(0, Ordering::Relaxed);
        CTR_CHAIN_HITS.store(0, Ordering::Relaxed);
        CTR_CHAINS.store(0, Ordering::Relaxed);
        CTR_EDGES.store(0, Ordering::Relaxed);
        CTR_BFS.store(0, Ordering::Relaxed);
        CTR_NOTES.store(0, Ordering::Relaxed);
        CTR_SPLATS.store(0, Ordering::Relaxed);
        drop(GRAPH.lock()); // keep the leakorigin shadow balanced (no-op)
        arm_runtime();
    }

    pub fn runtime_state() -> (bool, u8) {
        (
            ARMED.load(Ordering::Acquire),
            DISABLE_REASON.load(Ordering::Relaxed),
        )
    }

    pub fn counters() -> (u64, u64, u64, u64, u64, u64, u64) {
        (
            CTR_ACQUIRES.load(Ordering::Relaxed),
            CTR_CHAIN_HITS.load(Ordering::Relaxed),
            CTR_CHAINS.load(Ordering::Relaxed),
            CTR_EDGES.load(Ordering::Relaxed),
            CTR_BFS.load(Ordering::Relaxed),
            CTR_NOTES.load(Ordering::Relaxed),
            CTR_SPLATS.load(Ordering::Relaxed),
        )
    }

    fn disable(reason: u8) {
        ARMED.store(false, Ordering::Release);
        DISABLE_REASON.store(reason, Ordering::Relaxed);
        crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::WARN);
    }

    fn splat_disable() {
        CTR_SPLATS.fetch_add(1, Ordering::Relaxed);
        disable(DIS_SPLAT);
        crate::pr_warn!(
            "LOCKDEP: disabling lockdep (first splat; debug_locks=0 equivalent)"
        );
    }

    // ---- Observed-edge + chain arenas ---------------------------------

    #[derive(Clone, Copy)]
    pub struct ObservedEdge {
        pub outer: u16,
        pub inner: u16,
        pub site: u64,
    }

    #[derive(Clone, Copy)]
    struct ChainSnap {
        depth: u8,
        classes: [u16; SNAP_DEPTH],
    }

    /// Graph state; every mutation happens under GRAPH (one leaf
    /// spinlock held for microseconds over owned arrays — design §5).
    /// The hot-path checks read the arm-time bitsets (separate
    /// statics) and CHAIN_KEYS (lock-free reads), so the armed
    /// amortized path takes NO lock at all.
    struct GraphState {
        edges: [ObservedEdge; MAX_EDGES],
        edge_count: usize,
        ring: [ChainSnap; CHAIN_SNAPS],
        ring_next: usize,
        ring_count: usize,
    }

    static GRAPH: crate::sync::spinlock::Spinlock<GraphState> =
        crate::sync::spinlock::Spinlock::new(GraphState {
            edges: [ObservedEdge { outer: 0, inner: 0, site: 0 }; MAX_EDGES],
            ring: [ChainSnap { depth: 0, classes: [0; SNAP_DEPTH] }; CHAIN_SNAPS],
            edge_count: 0,
            ring_next: 0,
            ring_count: 0,
        });

    /// Chain keys, open addressing, lock-free reads (Acquire loads),
    /// insertions under GRAPH (Release stores). 0 = empty slot.
    static CHAIN_KEYS: [AtomicU64; MAX_CHAINS] =
        [const { AtomicU64::new(0) }; MAX_CHAINS];

    /// 64-bit FNV-1a over the held stack's class ids plus the id
    /// being acquired (design §3: dense stable ids, no address
    /// mixing; a collision costs one extra BFS run at most).
    fn chain_key(stack: &HeldStack, c: LockClassId) -> u64 {
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x1000_0000_01b3;
        let mut h = FNV_OFFSET;
        h = (h ^ (stack.depth as u64)).wrapping_mul(FNV_PRIME);
        for i in 0..stack.depth() {
            h = (h ^ (stack.classes[i] as u64)).wrapping_mul(FNV_PRIME);
        }
        h = (h ^ (c.0 as u64)).wrapping_mul(FNV_PRIME);
        // Slot value 0 means empty; remap the (astronomically
        // unlikely) zero key.
        if h == 0 { 1 } else { h }
    }

    /// Lock-free lookup: probe until an empty slot (miss) or a match.
    fn chain_lookup(key: u64) -> bool {
        let mask = MAX_CHAINS - 1;
        let mut slot = (key as usize) & mask;
        for _ in 0..MAX_CHAINS {
            let v = CHAIN_KEYS[slot].load(Ordering::Acquire);
            if v == key {
                return true;
            }
            if v == 0 {
                return false;
            }
            slot = (slot + 1) & mask;
        }
        false
    }

    /// Insert under GRAPH. Ok(true) = already present, Ok(false) =
    /// inserted, Err(()) = table full (caller disables — reference
    /// semantics).
    fn chain_insert(key: u64) -> Result<bool, ()> {
        let mask = MAX_CHAINS - 1;
        let mut slot = (key as usize) & mask;
        for _ in 0..MAX_CHAINS {
            let v = CHAIN_KEYS[slot].load(Ordering::Relaxed);
            if v == key {
                return Ok(true);
            }
            if v == 0 {
                CHAIN_KEYS[slot].store(key, Ordering::Release);
                return Ok(false);
            }
            slot = (slot + 1) & mask;
        }
        Err(())
    }

    fn name(id: LockClassId) -> &'static str {
        CLASSES.get(id.0 as usize).map(|c| c.name).unwrap_or("<unknown>")
    }

    // ---- The hook entry points ---------------------------------------

    /// Keyed-lock acquire hook. Feature OFF: empty body (and the
    /// instrumented lock paths do not even call it — the call sites
    /// are cfg-gated). Feature ON, disarmed: the relaxed load plus
    /// the (register) site capture. Feature ON, armed: the full
    /// check pipeline of design §6.
    #[inline(always)]
    pub fn lockdep_acquire<K: crate::sync::lockkey::NestKey>(site: u64) {
        acquire_classed(K::LOCK_CLASS, site);
    }

    /// Keyed-lock release hook (pops the per-task held stack; no
    /// lock, no checks).
    #[inline(always)]
    pub fn lockdep_release<K: crate::sync::lockkey::NestKey>() {
        release_classed(K::LOCK_CLASS);
    }

    /// Classed acquire (the SleepMutex funnel and the selftest path).
    /// Captures the current task's held stack and runs the checks.
    pub fn acquire_classed(class: LockClassId, site: u64) {
        if !ARMED.load(Ordering::Relaxed) {
            return;
        }
        let Some(task) = crate::sched::current() else {
            return; // no task context (very early boot / secondary
                    // harts): nothing to record against
        };
        let st = &mut task.lockdep_held;
        if st.in_checker {
            return; // the checker's own reporting path
        }
        st.in_checker = true;
        let preempt = crate::interrupt::preempt::preempt_count();
        let _ = check_acquire(st, class, site, preempt);
        st.in_checker = false;
    }

    /// Classed release (the SleepMutex funnel). Pops the innermost
    /// occurrence of the class; deliberately lock-free.
    pub fn release_classed(class: LockClassId) {
        if !ARMED.load(Ordering::Relaxed) {
            return;
        }
        if let Some(task) = crate::sched::current() {
            let st = &mut task.lockdep_held;
            if !st.in_checker {
                st.pop_class(class);
            }
        }
    }

    // ---- The check pipeline (design doc §6) ---------------------------

    /// One armed acquire of `class` with held stack `st`, acquire
    /// `site` (a return address, symbolized through ksyms at splat
    /// time) and the caller's `preempt` count. Pure over its inputs
    /// plus the checker's own arenas — the unit tests and the boot
    /// selftest drive it directly with local stacks (design §10).
    ///
    /// Splats print via pr_warn as they are found; the checker
    /// self-disables on the first splat or on arena exhaustion
    /// (reference semantics).
    pub fn check_acquire(
        st: &mut HeldStack,
        class: LockClassId,
        site: u64,
        preempt: i32,
    ) -> Verdict {
        CTR_ACQUIRES.fetch_add(1, Ordering::Relaxed);

        // 1. Same-class re-entry by this task: task-recursive legal
        //    (SleepMutex recursion; the keyed surface fences
        //    cross-class nesting at compile time). Record, skip
        //    checks (a self-edge is not a cycle).
        if st.contains(class) {
            if !st.push(class, site) {
                disable(DIS_DEPTH);
                return Verdict::Exhausted;
            }
            return Verdict::OkRecursive;
        }

        // 2. Context check (INV-LOCK-8 rule 1) — cheap, first, fires
        //    even with an empty stack.
        if CLASSES
            .get(class.0 as usize)
            .is_some_and(|c| c.domain == LockDomain::Sleep)
        {
            let mut spin_held: Option<LockClassId> = None;
            for i in 0..st.depth() {
                let h = st.classes[i] as usize;
                if h < 256 && bit_get(&SPIN_FAMILY, h) {
                    spin_held = Some(LockClassId(st.classes[i]));
                    break;
                }
            }
            if preempt != 0 || spin_held.is_some() {
                splat_sleep_in_atomic(class, preempt, spin_held, st, site);
                let _ = st.push(class, site);
                splat_disable();
                return Verdict::SleepInAtomic { class, preempt, held_spin: spin_held };
            }
        }

        // 3. Declared-order check: acquiring a class that a declared
        //    edge names as OUTER (FWD[c] has h: the declared edge is
        //    c -> h) while h is held = reverse of a declared edge =
        //    ORDER-VIOLATION. (The forward nesting h -> c is the
        //    legal arm and must fall through.)
        {
            let c = class.0 as usize;
            if c < 256 {
                for i in 0..st.depth() {
                    let h = st.classes[i] as usize;
                    if h < 256
                        && (FWD_BITSET[c][h / 32].load(Ordering::Relaxed)
                            & (1 << (h % 32)))
                            != 0
                    {
                        let inv = DECLARED_EDGES
                            .iter()
                            .find(|e| {
                                e.outer.0 as usize == c && e.inner.0 as usize == h
                            })
                            .map(|e| e.inv)
                            .unwrap_or("?");
                        splat_order_violation(
                            LockClassId(st.classes[i]),
                            class,
                            inv,
                            st,
                            site,
                            preempt,
                        );
                        let held = LockClassId(st.classes[i]);
                        let _ = st.push(class, site);
                        splat_disable();
                        return Verdict::OrderViolation { held, acquiring: class, inv };
                    }
                }
            }
        }

        // 4. Chain-key amortization: an acquire whose chain is
        //    already recorded skips everything below (the armed
        //    steady state — no lock taken at all).
        let key = chain_key(st, class);
        if chain_lookup(key) {
            CTR_CHAIN_HITS.fetch_add(1, Ordering::Relaxed);
            let _ = st.push(class, site);
            return Verdict::OkChainHit;
        }

        // 5. New chain: record it, add the observed edges, run the
        //    bounded BFS (check_noncircular's shape).
        CTR_BFS.fetch_add(1, Ordering::Relaxed);
        let mut notes: [(u16, u16); 8] = [(0, 0); 8];
        let mut n_notes = 0usize;
        let mut exhaust_reason: u8 = 0;
        let mut cycle: Option<([u16; PATH_MAX], usize)> = None;
        {
            let mut g = GRAPH.lock();
            // Re-check under the lock (another CPU may have recorded
            // this chain while we were acquiring GRAPH).
            match chain_insert(key) {
                Ok(true) => {
                    drop(g);
                    CTR_CHAIN_HITS.fetch_add(1, Ordering::Relaxed);
                    let _ = st.push(class, site);
                    return Verdict::OkChainHit;
                }
                Ok(false) => {}
                Err(()) => {
                    drop(g);
                    disable(DIS_CHAINS_FULL);
                    return Verdict::Exhausted;
                }
            }
            CTR_CHAINS.fetch_add(1, Ordering::Relaxed);
            // Ring snapshot for /proc/lockdep_chains.
            let mut snap_classes = [0u16; SNAP_DEPTH];
            for i in 0..st.depth().min(SNAP_DEPTH) {
                snap_classes[i] = st.classes[i];
            }
            snap_classes[st.depth().min(SNAP_DEPTH - 1)] = class.0;
            let rn = g.ring_next;
            g.ring[rn] = ChainSnap {
                depth: (st.depth() + 1).min(SNAP_DEPTH) as u8,
                classes: snap_classes,
            };
            g.ring_next = (g.ring_next + 1) % CHAIN_SNAPS;
            g.ring_count = (g.ring_count + 1).min(CHAIN_SNAPS);

            // Observed edges h[i] -> class, with the acquire site.
            for i in 0..st.depth() {
                let (o, i2) = (st.classes[i], class.0);
                let mut dup = false;
                for e in &g.edges[..g.edge_count] {
                    if e.outer == o && e.inner == i2 {
                        dup = true;
                        break;
                    }
                }
                if dup {
                    continue;
                }
                if g.edge_count >= MAX_EDGES {
                    exhaust_reason = DIS_EDGES_FULL;
                    break;
                }
                let ec = g.edge_count;
                g.edges[ec] = ObservedEdge { outer: o, inner: i2, site };
                g.edge_count += 1;
                CTR_EDGES.fetch_add(1, Ordering::Relaxed);
                // Undeclared in EITHER direction = the
                // UNDECLARED-NEST feedback loop (fires at edge
                // insertion, which happens once per pair — the
                // rate-limit for free).
                let (ou, iu) = (o as usize, i2 as usize);
                let declared = (ou < 256
                    && iu < 256
                    && ((FWD_BITSET[ou][iu / 32].load(Ordering::Relaxed)
                        & (1 << (iu % 32)))
                        != 0
                        || REV_BITSET[ou][iu / 32].load(Ordering::Relaxed)
                            & (1 << (iu % 32))
                            != 0))
                    || DECLARED_EDGES
                        .iter()
                        .any(|e| e.outer.0 == i2 && e.inner.0 == o);
                if !declared && n_notes < notes.len() {
                    notes[n_notes] = (o, i2);
                    n_notes += 1;
                }
            }

            // Bounded BFS from `class` through observed+declared
            // edges looking for any held class (a cycle through the
            // just-added edges). Design §6.4: the new edge
            // next->prev is a deadlock iff prev is reachable from
            // next.
            if exhaust_reason == 0 {
                match bfs_find_held(&g, class, st) {
                    BfsResult::Found(path, len) => cycle = Some((path, len)),
                    BfsResult::NotFound => {}
                    BfsResult::Overflow => exhaust_reason = DIS_BFS_BOUND,
                }
            }
        }
        if exhaust_reason != 0 {
            disable(exhaust_reason);
            return Verdict::Exhausted;
        }
        // UNDECLARED-NEST notes: printed AFTER the graph lock (keep
        // the lock section short); deliberately NOT a splat.
        for (o, i2) in &notes[..n_notes] {
            CTR_NOTES.fetch_add(1, Ordering::Relaxed);
            crate::pr_warn!(
                "LOCKDEP: UNDECLARED-NEST {} -> {} (no declared edge; feed docs/architecture/lock-ordering.md)",
                name(LockClassId(*o)),
                name(LockClassId(*i2))
            );
        }
        if let Some((path, len)) = cycle {
            splat_cycle(&path, len, st, site, preempt);
            let _ = st.push(class, site);
            splat_disable();
            return Verdict::Cycle { path, len };
        }
        if !st.push(class, site) {
            disable(DIS_DEPTH);
            return Verdict::Exhausted;
        }
        Verdict::Ok
    }

    enum BfsResult {
        NotFound,
        Found([u16; PATH_MAX], usize),
        Overflow,
    }

    /// BFS from `from` over observed+declared edges to any class in
    /// the held stack. Queue bound BFS_QUEUE (design §6); exceeding
    /// it disables the checker.
    fn bfs_find_held(g: &GraphState, from: LockClassId, st: &HeldStack) -> BfsResult {
        let mut visited = [false; 256];
        let mut parent = [-1i16; 256];
        let mut queue = [0u16; BFS_QUEUE];
        let mut head = 0usize;
        let mut tail = 0usize;
        let src = from.0;
        if src as usize >= 256 {
            return BfsResult::NotFound;
        }
        queue[tail] = src;
        tail += 1;
        visited[src as usize] = true;
        let mut visited_count = 1usize;
        while head < tail {
            let u = queue[head] as usize;
            head += 1;
            for v in successors(g, u) {
                let v = v as usize;
                if v >= 256 || visited[v] {
                    continue;
                }
                visited[v] = true;
                visited_count += 1;
                if visited_count > 256 {
                    return BfsResult::Overflow;
                }
                parent[v] = u as i16;
                // Target: any held class.
                if (0..st.depth()).any(|i| st.classes[i] as usize == v) {
                    // Reconstruct v -> ... -> from.
                    let mut path = [0u16; PATH_MAX];
                    let mut len = 0usize;
                    let mut cur = v;
                    while cur != src as usize && len < PATH_MAX {
                        path[len] = cur as u16;
                        len += 1;
                        cur = parent[cur] as usize;
                    }
                    if len < PATH_MAX {
                        path[len] = src;
                        len += 1;
                    }
                    return BfsResult::Found(path, len);
                }
                if tail < BFS_QUEUE {
                    queue[tail] = v as u16;
                    tail += 1;
                } else {
                    return BfsResult::Overflow;
                }
            }
        }
        BfsResult::NotFound
    }

    /// Successors of class `u` over observed then declared edges
    /// (a declared edge that is also observed yields the same node
    /// twice; the visited set absorbs that).
    fn successors<'a>(g: &'a GraphState, u: usize) -> impl Iterator<Item = u16> + 'a {
        let obs = g
            .edges
            .iter()
            .take(g.edge_count)
            .filter(move |e| e.outer as usize == u)
            .map(|e| e.inner);
        let decl = DECLARED_EDGES
            .iter()
            .filter(move |e| e.outer.0 as usize == u)
            .map(|e| e.inner.0);
        obs.chain(decl)
    }

    // ---- Splats (design doc §6 shape) --------------------------------

    fn ctx_line(preempt: i32) {
        let pid = crate::process::current_pid();
        let cpu = crate::arch::smp::cpu_id();
        let (in_irq, in_soft) = (
            preempt & crate::interrupt::preempt::HARDIRQ_OFFSET != 0,
            preempt & crate::interrupt::preempt::SOFTIRQ_OFFSET != 0,
        );
        let comm = crate::sched::current()
            .map(|t| t.comm_str().to_string())
            .unwrap_or_default();
        crate::pr_warn!(
            "  task={} pid={} cpu={} preempt={:#x} in_irq={} in_softirq={}",
            comm,
            pid,
            cpu,
            preempt,
            in_irq,
            in_soft
        );
    }

    fn held_lines(st: &HeldStack) {
        for i in 0..st.depth() {
            crate::pr_warn!(
                "  held:      {} at {} (depth {})",
                name(LockClassId(st.classes[i])),
                crate::debug::ksyms::format_symbol(st.sites[i]),
                i
            );
        }
    }

    fn splat_order_violation(
        held: LockClassId,
        acquiring: LockClassId,
        inv: &str,
        st: &HeldStack,
        site: u64,
        preempt: i32,
    ) {
        crate::pr_warn!(
            "LOCKDEP: ORDER-VIOLATION {} -> {} contradicts {} (declared: {} -> {})",
            name(held),
            name(acquiring),
            inv,
            name(acquiring),
            name(held)
        );
        crate::pr_warn!(
            "  acquiring: {} at {}",
            name(acquiring),
            crate::debug::ksyms::format_symbol(site)
        );
        held_lines(st);
        ctx_line(preempt);
    }

    fn splat_sleep_in_atomic(
        class: LockClassId,
        preempt: i32,
        held_spin: Option<LockClassId>,
        st: &HeldStack,
        site: u64,
    ) {
        crate::pr_warn!(
            "LOCKDEP: SLEEP-IN-ATOMIC-CONTEXT acquiring sleep-domain {} (INV-LOCK-8 rule 1)",
            name(class)
        );
        match held_spin {
            Some(h) => crate::pr_warn!("  spin-family {} held", name(h)),
            None => crate::pr_warn!("  preempt_count={:#x} (atomic context)", preempt),
        }
        crate::pr_warn!(
            "  acquiring: {} at {}",
            name(class),
            crate::debug::ksyms::format_symbol(site)
        );
        held_lines(st);
        ctx_line(preempt);
    }

    fn splat_cycle(path: &[u16], len: usize, st: &HeldStack, site: u64, preempt: i32) {
        // path[0] is the held class the BFS reached; path[len-1] is
        // the class being acquired. Render the loop starting at the
        // held class: held -> ... -> acquiring -> ... -> held.
        let mut line = String::from("LOCKDEP: CYCLE ");
        for i in 0..len {
            if i > 0 {
                line.push_str(" -> ");
            }
            line.push_str(name(LockClassId(path[i])));
        }
        line.push_str(" -> ");
        line.push_str(name(LockClassId(path[0])));
        crate::pr_warn!("{}", line);
        crate::pr_warn!(
            "  acquiring: {} at {}",
            name(LockClassId(path[len - 1])),
            crate::debug::ksyms::format_symbol(site)
        );
        for i in 0..len {
            let c = LockClassId(path[i]);
            let site_i = st.site_of(c).unwrap_or_else(|| {
                // Not in this task's stack: the observed-edge site of
                // an edge pointing at this class, or 0.
                let g = GRAPH.lock();
                g.edges
                    .iter()
                    .take(g.edge_count)
                    .find(|e| e.inner == path[i])
                    .map(|e| e.site)
                    .unwrap_or(0)
            });
            crate::pr_warn!(
                "  path: {} at {}",
                name(c),
                crate::debug::ksyms::format_symbol(site_i)
            );
        }
        ctx_line(preempt);
    }

    // ---- /proc snapshots ----------------------------------------------

    pub fn observed_edges_snapshot() -> Vec<(u16, u16, u64)> {
        let g = GRAPH.lock();
        g.edges
            .iter()
            .take(g.edge_count)
            .map(|e| (e.outer, e.inner, e.site))
            .collect()
    }

    pub struct ChainSnapView {
        pub classes: Vec<u16>,
    }

    pub fn chain_ring_snapshot() -> Vec<ChainSnapView> {
        let g = GRAPH.lock();
        let mut out = Vec::new();
        // Newest first: index back from the ring head.
        for k in 0..g.ring_count {
            let idx = (g.ring_next + CHAIN_SNAPS - 1 - k) % CHAIN_SNAPS;
            let s = &g.ring[idx];
            out.push(ChainSnapView {
                classes: s.classes[..s.depth as usize].to_vec(),
            });
        }
        out
    }

    // ---- Boot selftest + the per-acquire cost probe -------------------

    /// The keyed key->class mapping must agree with the registry
    /// table (the armed slow path cross-checks by NAME; this checks
    /// it up front). Returns a human-readable problem list.
    fn key_mapping_problems() -> Vec<&'static str> {
        use crate::sync::lockkey::{
            FutexBucketKey, GrqKey, NestKey, NetQueueKey, NetStatsKey, WaiterSlotKey,
        };
        let pairs: [(LockClassId, &'static str, &str); 5] = [
            (GrqKey::LOCK_CLASS, GrqKey::NAME, "grq"),
            (FutexBucketKey::LOCK_CLASS, FutexBucketKey::NAME, "futex-bucket"),
            (WaiterSlotKey::LOCK_CLASS, WaiterSlotKey::NAME, "futex-waiter-slot"),
            (NetQueueKey::LOCK_CLASS, NetQueueKey::NAME, "net-queue"),
            (NetStatsKey::LOCK_CLASS, NetStatsKey::NAME, "net-stats"),
        ];
        let mut out = Vec::new();
        for (class, key_name, table_name) in pairs {
            match CLASSES.get(class.0 as usize) {
                Some(c) if c.name == key_name && c.name == table_name => {}
                _ => out.push("key LOCK_CLASS does not map to its registry row"),
            }
        }
        out
    }

    /// The hook-cost probe (design §7 row 4/5 evidence): N
    /// acquire/release pairs on a keyed lock, measured with the hooks
    /// disarmed (one relaxed load per site) and armed (the full
    /// pipeline, chain-hit amortized after warmup). Reported as
    /// ns/pair (per-acquire = half); cycles when the counter
    /// frequency is uncalibrated.
    fn cost_probe() {
        use crate::sync::lockkey::{GrqKey, KeyedRawSpinlock};
        static PROBE: KeyedRawSpinlock<GrqKey> = KeyedRawSpinlock::new();
        const N: u32 = 200_000;
        // Warm both legs (branch predictors, the chain-key table).
        for _ in 0..1000 {
            PROBE.lock_claim();
            PROBE.unlock();
        }
        // Disarmed leg: the hook's relaxed load + branch only.
        ARMED.store(false, Ordering::Release);
        let t0 = crate::arch::cpu::read_counter();
        for _ in 0..N {
            PROBE.lock_claim();
            PROBE.unlock();
        }
        let disarmed_cyc = crate::arch::cpu::read_counter().saturating_sub(t0);
        // Armed leg: full pipeline (chain-key hit after warmup).
        ARMED.store(true, Ordering::Release);
        let t1 = crate::arch::cpu::read_counter();
        for _ in 0..N {
            PROBE.lock_claim();
            PROBE.unlock();
        }
        let armed_cyc = crate::arch::cpu::read_counter().saturating_sub(t1);
        let freq = crate::arch::cpu::get_counter_freq();
        let n = N as u64;
        if freq > 0 {
            crate::pr_info!(
                "lockdep: cost-probe keyed acquire/release pair (N={}): disarmed {} ns, armed {} ns (hook delta {} ns)",
                N,
                disarmed_cyc * 1_000_000_000 / freq / n,
                armed_cyc * 1_000_000_000 / freq / n,
                armed_cyc.saturating_sub(disarmed_cyc) * 1_000_000_000 / freq / n
            );
        } else {
            crate::pr_info!(
                "lockdep: cost-probe keyed acquire/release pair (N={}): disarmed {} cyc, armed {} cyc (freq uncalibrated)",
                N,
                disarmed_cyc / n,
                armed_cyc / n
            );
        }
        // The selftest scenarios below each reset via reset_for_test
        // (which re-arms); leave ARMED as reset_for_test set it.
    }

    /// Synthetic proof battery (design §10, P1): declared-order
    /// forward passes; the FORCED reverse of a declared edge produces
    /// ORDER-VIOLATION; the INV-LOCK-8 context negatives produce
    /// SLEEP-IN-ATOMIC-CONTEXT; a synthetic two-step nesting produces
    /// CYCLE. Driven through direct [`check_acquire`] calls on local
    /// held stacks — no real wedging. Runs only under `debug=lockdep`
    /// (the same boot arm); resets the checker state afterwards so
    /// runtime tracking starts clean.
    pub fn boot_selftest() {
        use crate::debug::lockdep::{EXT4_BIG, FUTEX_BUCKET, GRQ, INODE, PROC_TREE, VFS_MUTATION};
        let mut failures = 0usize;

        // 0. Key->registry mapping consistency.
        if key_mapping_problems().is_empty() {
            crate::pr_info!("lockdep: selftest s0 key<->registry mapping: PASS");
        } else {
            failures += 1;
            crate::pr_warn!("lockdep: selftest s0 key<->registry mapping: FAIL");
        }

        // 1. Cost probe (disarmed/armed ns per acquire/release pair).
        cost_probe();

        // 2. Declared-order forward: bucket held, GRQ acquired
        //    (INV-LOCK-5 forward) must pass.
        reset_for_test();
        {
            let mut st = HeldStack::new();
            let _ = check_acquire(&mut st, FUTEX_BUCKET, 0x1000, 0);
            match check_acquire(&mut st, GRQ, 0x1004, 0) {
                Verdict::Ok => {
                    crate::pr_info!("lockdep: selftest s1 declared forward (bucket -> grq): PASS");
                }
                v => {
                    failures += 1;
                    crate::pr_warn!("lockdep: selftest s1 declared forward: FAIL ({:?})", v);
                }
            }
        }

        // 3. Declared-order VIOLATION: GRQ held, FUTEX_BUCKET
        //    acquired — the reverse of INV-LOCK-5. The money splat.
        reset_for_test();
        {
            let mut st = HeldStack::new();
            let _ = check_acquire(&mut st, GRQ, 0x2000, 0);
            match check_acquire(&mut st, FUTEX_BUCKET, 0x2004, 0) {
                Verdict::OrderViolation { held, acquiring, inv } => {
                    if held == GRQ && acquiring == FUTEX_BUCKET && inv == "INV-LOCK-5" {
                        crate::pr_info!(
                            "lockdep: selftest s2 declared-order violation (grq held, bucket acquired): PASS (splat above)"
                        );
                    } else {
                        failures += 1;
                        crate::pr_warn!("lockdep: selftest s2 violation fields wrong");
                    }
                }
                v => {
                    failures += 1;
                    crate::pr_warn!("lockdep: selftest s2 declared-order violation: FAIL ({:?})", v);
                }
            }
        }

        // 4. INV-LOCK-8 context negative, spin-held arm: a sleep-class
        //    acquire with a spin-family class held.
        reset_for_test();
        {
            let mut st = HeldStack::new();
            let _ = check_acquire(&mut st, GRQ, 0x3000, 0);
            match check_acquire(&mut st, VFS_MUTATION, 0x3004, 0) {
                Verdict::SleepInAtomic { class, held_spin, .. } => {
                    if class == VFS_MUTATION && held_spin == Some(GRQ) {
                        crate::pr_info!(
                            "lockdep: selftest s3 sleep-in-atomic, spin held (grq -> vfs-mutation): PASS (splat above)"
                        );
                    } else {
                        failures += 1;
                        crate::pr_warn!("lockdep: selftest s3 fields wrong");
                    }
                }
                v => {
                    failures += 1;
                    crate::pr_warn!("lockdep: selftest s3 sleep-in-atomic spin-held: FAIL ({:?})", v);
                }
            }
        }

        // 5. INV-LOCK-8 context negative, preempt arm: sleep-class
        //    acquire with preempt_count != 0, nothing held.
        reset_for_test();
        {
            let mut st = HeldStack::new();
            match check_acquire(&mut st, EXT4_BIG, 0x4004, 0x1) {
                Verdict::SleepInAtomic { preempt, held_spin, .. } => {
                    if preempt == 0x1 && held_spin.is_none() {
                        crate::pr_info!(
                            "lockdep: selftest s4 sleep-in-atomic, preempt!=0: PASS (splat above)"
                        );
                    } else {
                        failures += 1;
                        crate::pr_warn!("lockdep: selftest s4 fields wrong");
                    }
                }
                v => {
                    failures += 1;
                    crate::pr_warn!("lockdep: selftest s4 sleep-in-atomic preempt: FAIL ({:?})", v);
                }
            }
        }

        // 6. CYCLE: two synthetic tasks build a cross nesting
        //    (PROC_TREE -> INODE, then INODE -> PROC_TREE); the
        //    second acquire's BFS finds the loop.
        reset_for_test();
        {
            let mut st1 = HeldStack::new();
            let _ = check_acquire(&mut st1, PROC_TREE, 0x5000, 0);
            match check_acquire(&mut st1, INODE, 0x5004, 0) {
                Verdict::Ok => {}
                v => {
                    failures += 1;
                    crate::pr_warn!("lockdep: selftest s5 first nest: FAIL ({:?})", v);
                }
            }
            let mut st2 = HeldStack::new();
            let _ = check_acquire(&mut st2, INODE, 0x5008, 0);
            match check_acquire(&mut st2, PROC_TREE, 0x500c, 0) {
                Verdict::Cycle { .. } => {
                    crate::pr_info!("lockdep: selftest s5 cycle via observed edges: PASS (splat above)");
                }
                v => {
                    failures += 1;
                    crate::pr_warn!("lockdep: selftest s5 cycle: FAIL ({:?})", v);
                }
            }
        }

        // Clean slate for real tracking.
        reset_for_test();
        if failures == 0 {
            crate::pr_info!("lockdep: selftest PASS (s0 mapping + s1-s5 scenarios)");
        } else {
            crate::pr_warn!("lockdep: selftest FAIL ({} scenarios)", failures);
        }
    }
}

#[cfg(feature = "debug-lockdep")]
pub use runtime::{
    acquire_classed, arm_runtime, boot_selftest, chain_ring_snapshot, check_acquire,
    counters, lockdep_acquire, lockdep_release, observed_edges_snapshot, release_classed,
    reset_for_test, runtime_state, HeldStack, Verdict, ARMED, DIS_ARMED,
};

#[cfg(feature = "debug-lockdep")]
use runtime::disable_reason_str;

// ---- Feature-off stubs: identical public surface, empty bodies ------
//
// The instrumented lock paths cfg-gate their call sites, so these are
// only for unconditional callers (tests registered in both builds,
// /proc renderers). A default build never executes them.

#[cfg(not(feature = "debug-lockdep"))]
#[allow(dead_code)]
#[inline(always)]
pub fn lockdep_acquire<K>(site: u64) {
    let _ = site;
    // Feature off: no code exists on any lock path (design §7 row 1).
}

#[cfg(not(feature = "debug-lockdep"))]
#[allow(dead_code)]
#[inline(always)]
pub fn lockdep_release<K>() {}

#[cfg(not(feature = "debug-lockdep"))]
#[allow(dead_code)]
pub fn runtime_state() -> (bool, u8) {
    (false, 0)
}

#[cfg(not(feature = "debug-lockdep"))]
#[allow(dead_code)]
pub fn counters() -> (u64, u64, u64, u64, u64, u64, u64) {
    (0, 0, 0, 0, 0, 0, 0)
}

#[cfg(not(feature = "debug-lockdep"))]
#[allow(dead_code)]
pub fn disable_reason_str(_r: u8) -> &'static str {
    "not compiled"
}

#[cfg(not(feature = "debug-lockdep"))]
#[allow(dead_code)]
pub fn observed_edges_snapshot() -> alloc::vec::Vec<(u16, u16, u64)> {
    alloc::vec::Vec::new()
}
