//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Typed caller-side one-shot wake guards (arch-s5 pilot, session 2 —
//! the armed/pending/fired typestate on the CALLER side of the timer
//! facade).
//!
//! The wheel already types the fire side (`FiredToken` is produced only
//! by collection and consumed by value in delivery). This module types
//! the arm/cancel side for the scoped one-shot wake pattern — the
//! historically bug-dense family (nanosleep one-shot token discipline,
//! futex/poll timeout arms):
//!
//!   - `WakeArmed` OWNS one registration id. It is not `Copy` and not
//!     `Clone`: two holders of one registration (double-del /
//!     double-rearm races) are unrepresentable.
//!   - `cancel(self)` CONSUMES the guard and returns a `WakeSpent`
//!     tombstone. A second cancel — or any use of a cancelled guard —
//!     is a use-after-move compile error, not a runtime `del_timer`
//!     on a stale id.
//!   - `rearm_if_spent(self)` consumes the stale guard and returns the
//!     fresh one. The old belt-and-braces shape
//!     (`if id != 0 && !pending(id) { id = add(...) }`) could forget
//!     the reassignment and keep operating on the OLD id while a live
//!     second registration leaks; with a consumed guard there is no
//!     stale id left to hold.
//!   - `Drop` cancels an unspent guard: an early return between arm
//!     and cancel cannot leak the wheel slot (the pool-exhaustion
//!     wedge class). Dropping an already-fired one-shot dels a
//!     non-existent id — a no-op find, same cost as the explicit del
//!     every exit path already paid.
//!   - hres arms re-program this hart's timer on success
//!     (`rearm_for_hres` is baked into `arm_hres`/`rearm_if_spent`) —
//!     the pd lane lesson: an hres sleep whose caller forgets the hart
//!     re-program waits for the next 10 ms grid tick. Un-forgettable
//!     by construction now.
//!
//! Shape note: the armed/spent pair are two concrete types (not one
//! generic over state markers) because Rust forbids specializing
//! `Drop` on a generic (`impl Drop for WakeGuard<Armed>` is E0366) —
//! the armed type alone implements Drop.
//!
//! The stored-id families (itimer/POSIX timer/timerfd `kernel_timer_id`
//! fields, cross-syscall lifetimes) keep the raw-id facade API: those
//! ids outlive any guard scope and are deleted from teardown paths
//! where a monotonic-fresh id can never alias (del of a fired id is a
//! safe no-op). The guard is for the SCOPED pattern only — by design.

/// Which arm recipe a guard re-arms with (`rearm_if_spent`).
#[derive(Clone, Copy)]
enum WakeKind {
    /// Jiffy-domain one-shot (futex/poll timeouts, slice fallbacks).
    Jiffy,
    /// Precise one-shot (nanosleep class). The `at` value is the
    /// absolute time-CSR deadline; the wheel derives its own routing
    /// horizon — caller jiffy bookkeeping is not consulted.
    Hres,
}

/// A scoped one-shot wake registration the caller still owns. The id
/// may already have FIRED (one-shot fires are asynchronous and consume
/// the wheel entry; `pending()` is the live query) — "armed" means the
/// caller has not consumed the registration, not that the wheel still
/// holds it.
pub struct WakeArmed {
    kind: WakeKind,
    at: u64,
    pid: u32,
    id: u64,
}

impl WakeArmed {
    /// Arm a jiffy-domain one-shot wake for `pid` at absolute jiffy
    /// `expires`. `None` = timer pool full (callers fall back to their
    /// waker-less path — the same contract as the old `0` id).
    pub fn arm_jiffy(expires: u64, pid: u32) -> Option<WakeArmed> {
        let id = super::add_timer_wakeup(expires, pid);
        Self::wrap(WakeKind::Jiffy, expires, pid, id)
    }

    /// Arm a precise one-shot wake for `pid` at absolute time-CSR tick
    /// `expires_time`. On success this hart's timer is re-programmed to
    /// the earlier deadline (see the module note — baked in, not a
    /// caller duty). `None` = pool full.
    pub fn arm_hres(expires_time: u64, pid: u32) -> Option<WakeArmed> {
        let id = super::add_timer_wakeup_hres(0, expires_time, pid);
        if id != 0 {
            crate::drivers::timer::rearm_for_hres();
        }
        Self::wrap(WakeKind::Hres, expires_time, pid, id)
    }

    fn wrap(kind: WakeKind, at: u64, pid: u32, id: u64) -> Option<Self> {
        if id == 0 {
            None
        } else {
            Some(WakeArmed { kind, at, pid, id })
        }
    }

    /// Is the registration still live in the wheel? `false` once the
    /// one-shot has fired (the wheel consumed the entry) — the
    /// early-fire detector under `rearm_if_spent`.
    pub fn pending(&self) -> bool {
        super::timer_pending(self.id)
    }

    /// The belt-and-braces re-arm (the nanosleep lost-wakeup guard):
    /// when the one-shot token was consumed before its deadline
    /// (early-fire family), arm a FRESH registration and consume this
    /// guard. When still pending, the same guard comes back (the
    /// registration is fine). `None` = re-arm hit a full pool — callers
    /// take their waker-less path, exactly like the old id-0 shape.
    /// Accounts `ww_note_rearm` on the consumed branch (the WW counter
    /// the heartbeat reports).
    pub fn rearm_if_spent(mut self) -> Option<WakeArmed> {
        if self.pending() {
            return Some(self);
        }
        super::ww_note_rearm();
        let (kind, at, pid) = (self.kind, self.at, self.pid);
        // The old registration was consumed by the early fire — its
        // Drop cleanup would del an already-absent id. Neutralize.
        // (Copy fields read out, id zeroed: Drop sees nothing to do.)
        self.id = 0;
        match kind {
            WakeKind::Jiffy => Self::arm_jiffy(at, pid),
            WakeKind::Hres => Self::arm_hres(at, pid),
        }
    }

    /// Cancel the registration exactly once. Consumes the guard; the
    /// returned `WakeSpent` tombstone carries the recipe for
    /// diagnostics and drops as a no-op. A second cancel of the same
    /// registration cannot be written (the first move consumed the
    /// guard).
    pub fn cancel(mut self) -> WakeSpent {
        let (kind, at, pid, id) = (self.kind, self.at, self.pid, self.id);
        let spent = WakeSpent { kind, at, pid };
        self.id = 0; // Drop at scope exit sees a spent guard — no-op
        if id != 0 {
            super::del_timer(id);
        }
        spent
    }

    /// The registration id (diagnostics / WW prints).
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for WakeArmed {
    /// Early-return safety net: an unspent guard cancels its
    /// registration on scope exit. A guard whose entry already fired
    /// dels a non-existent id (no-op find under the table lock — the
    /// same single lock touch every explicit exit path already paid).
    fn drop(&mut self) {
        if self.id != 0 {
            super::del_timer(self.id);
        }
    }
}

/// Tombstone returned by `WakeArmed::cancel` — no registration, no
/// Drop behavior, nothing to double-free.
pub struct WakeSpent {
    kind: WakeKind,
    at: u64,
    pid: u32,
}

impl WakeSpent {
    /// What the cancelled guard was armed as (diagnostics).
    pub fn recipe(&self) -> (&'static str, u64, u32) {
        (
            match self.kind {
                WakeKind::Jiffy => "jiffy",
                WakeKind::Hres => "hres",
            },
            self.at,
            self.pid,
        )
    }
}
