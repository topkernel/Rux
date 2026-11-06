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
        return;
    }
    for (kind, detail) in &findings {
        crate::pr_warn!("lockdep: registry {}: {}", kind.as_str(), detail);
    }
    crate::pr_warn!(
        "lockdep: registry validation FAIL ({} findings); see docs/development/lockdep-lite-design.md",
        findings.len()
    );
}

/// `/proc/lockdep` — the reference file's layout (per class: key,
/// forward/backward dependency counts, usage chars, name, then
/// ` -> [key] name` lines for direct dependencies), rendering the
/// DECLARED registry. The key column carries the class's table index
/// (Rux classes are table rows; the name IS the key — design doc §8).
/// The usage chars stay `....` until P2 arms irq-state tracking.
pub fn generate_proc_lockdep() -> Vec<u8> {
    let mut out = String::new();
    out.push_str("all lock classes:\n");
    for (idx, c) in CLASSES.iter().enumerate() {
        // FD/BD over the declared graph (live counts join in P1).
        let fd = DECLARED_EDGES
            .iter()
            .filter(|e| e.outer.0 as usize == idx)
            .count();
        let bd = DECLARED_EDGES
            .iter()
            .filter(|e| e.inner.0 as usize == idx)
            .count();
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
        // The reference separates class blocks with a blank line.
        out.push('\n');
    }
    // Declared-edge census at the tail (not in the reference file;
    // one line, cheap, and it makes the P0 file self-describing).
    out.push_str(&format!(
        "# lockdep-lite P0: {} classes, {} declared edges (runtime tracking: P1)\n",
        CLASSES.len(),
        DECLARED_EDGES.len()
    ));
    out.into_bytes()
}
