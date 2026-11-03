//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Typed timer-wheel core (arch-s5 pilot, survey shortlist S-5).
//!
//! Design note (the S-5 divergence, internal-only):
//!
//! The old `timer.rs` table was a flat 1024-slot array whose expiry was
//! re-checked on EVERY softirq pass, with timer classes encoded as
//! RUNTIME FLAG COMBINATIONS on one struct (`expires_time != 0` = hres,
//! `interval_jiffies > 0` = periodic, `wake_pid`/`tfd_addr`/
//! `pid`+`signo` = delivery kind). The historical bug classes were all
//! flag-confusion bugs:
//!
//!   - the lost-wakeup root cause: the jiffy fallback compare fired an
//!     hres entry early (the `expires <= current` arm applied to an
//!     `expires_time != 0` entry) and consumed the one-shot wake;
//!   - the GNOME-oom action leak: one-shot expiry freed the entry but
//!     left the action behind;
//!   - the R37 periodic-no-delivery bug: the re-arm path re-armed the
//!     entry without queueing the delivery;
//!   - the silent if/else-if priority chain in delivery dropped
//!     whichever action fields a malformed slot happened to double-set.
//!
//! This module encodes those classes as TYPES so they do not compile:
//!
//!   1. Clock domains are distinct newtypes. `Deadline<JiffyClock>` and
//!      `Deadline<HresClock>` each have an `is_due` that only accepts
//!      their OWN clock's now — no method anywhere compares an hres
//!      deadline against jiffies or vice versa, so the mixed-compare
//!      early-fire cannot be written (see `Expiry::is_due`: the match
//!      arms physically cannot reach the other clock).
//!   2. Delivery is an `Action` ENUM — `Wake`/`Signal`/`TimerFd` are
//!      mutually exclusive by construction; the silent priority chain
//!      has nothing to be silent about, and adding a variant forces the
//!      delivery match to handle it.
//!   3. Periodicity is `Option<NonZeroU64>` — "interval 0" cannot be
//!      mistaken for a (busy-loop) period; one-shot-ness is `None`.
//!   4. Slot lifecycle is BY VALUE: arming links an entry into the
//!      wheel; collection UNLINKS it and returns a `FiredToken`
//!      carrying the action out; delivery consumes tokens by value. A
//!      one-shot fire frees the slot and moves the action in the same
//!      step (the leak class has no code path), and a periodic fire
//!      re-arms by re-routing the SAME entry while returning the
//!      delivery token in the same step (the re-arm-without-deliver
//!      class has no code path either).
//!   5. Delivery context is a capability token: `deliver_fired` takes
//!      a `SoftirqCtx`, a ZST whose constructor is visible only inside
//!      the timer module. Rux timer "callbacks" are DATA actions (not
//!      fn pointers), all delivered in softirq context; the token pins
//!      that contract. If a task-context delivery class ever appears
//!      it gets its own token type, and the two cannot be swapped.
//!   6. Routing is computed at construction: bucket indices are
//!      derived only inside `route_for` (from the expiry and the
//!      wheel's own cursor) — a mis-slotted deadline cannot be
//!      constructed because nothing outside that function computes a
//!      bucket.
//!
//! Wheel shape (cascading, reference-kernel class): 64 buckets per
//! level, three levels (granularity 1 / 64 / 4096 jiffies, total span
//! 262144 jiffies ≈ 43.7 min at HZ=100) plus a FAR list for deeper
//! horizons, rescanned on every level-2 cascade (every 4096 jiffies
//! ≈ 41 s — 64 rescans of headroom before any FAR deadline can come
//! due, so routing never delays a fire). Per-pass work is O(due
//! entries) instead of O(occupied slots): the collection walk touches
//! only the DUE carry list and the current level-0 bucket.
//!
//! hres ROUTING is grid-derived, never caller-supplied: the arm paths
//! compute the entry's first-check jiffy with `hres_horizon_for`
//! (the timer driver's exact time→jiffy floor), so a bad jiffy
//! bookkeeping value can no longer delay a precise wake (and, in the
//! pre-wheel table, could not accelerate it either — the precise
//! compare alone decides, unchanged).
//!
//! All operations are plain stores on fixed .bss storage — the R37
//! no-allocation-under-the-lock discipline is structural (the pool,
//! bitmap, bucket head/tail arrays and link fields are all static).
//! One lock (the existing TIMER_TABLE spinlock in `timer.rs`) guards
//! the wheel; delivery happens outside it (R12-3, unchanged).

use core::marker::PhantomData;
use core::num::NonZeroU64;

use super::TimerSnapshot;

// ==================== Clock domains ====================

/// The jiffies clock (10 ms grid at HZ=100): `Deadline<JiffyClock>`
/// compares against `Now::jiffies` only.
pub struct JiffyClock;

/// The time-CSR clock (100 ns ticks): `Deadline<HresClock>` compares
/// against `Now::time` only.
pub struct HresClock;

/// An absolute deadline in clock domain `C`. The payload is private;
/// the only comparisons available are the per-domain `is_due` impls
/// below, which take that domain's now and nothing else.
pub struct Deadline<C> {
    abs: u64,
    _clock: PhantomData<fn() -> C>,
}

impl<C> Clone for Deadline<C> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<C> Copy for Deadline<C> {}

impl Deadline<JiffyClock> {
    /// Jiffy-domain due check. The ONLY due check reachable for a
    /// jiffy deadline — there is no overload accepting time ticks.
    #[inline]
    pub fn is_due(&self, now_jiffies: u64) -> bool {
        self.abs <= now_jiffies
    }

    /// The absolute jiffy (readback for get_timer_state / timer_list).
    #[inline]
    pub fn abs(&self) -> u64 {
        self.abs
    }
}

impl Deadline<HresClock> {
    /// Hres-domain due check: fires EXACTLY when the time CSR reaches
    /// the deadline and never otherwise. The historical early-fire bug
    /// (jiffy fallback consuming a one-shot hres wake) required
    /// comparing this value against jiffies; that method does not
    /// exist, by construction.
    #[inline]
    pub fn is_due(&self, now_time: u64) -> bool {
        self.abs <= now_time
    }

    /// The absolute time-CSR tick (HRES_NEXT maintenance).
    #[inline]
    pub fn abs(&self) -> u64 {
        self.abs
    }
}

/// One entry's expiry: clock-domain-tagged. On the hres arm, `horizon`
/// is the first jiffy at which the wheel starts CHECKING the entry;
/// the precise compare alone decides firing. The horizon is DERIVED
/// from the precise deadline via the timer driver's grid mapping at
/// arm time (`hres_horizon_for`) — never trusted from callers.
#[derive(Clone, Copy)]
pub enum Expiry {
    /// Jiffy-domain timer (poll slices, itimers, timerfd, DL replenish).
    Jiffy(Deadline<JiffyClock>),
    /// High-resolution timer (nanosleep-class wakes, POSIX timers):
    /// `precise` is the absolute time-CSR deadline, `horizon` the
    /// grid-derived first-check jiffy.
    Hres {
        precise: Deadline<HresClock>,
        horizon: u64,
    },
}

impl Expiry {
    /// Domain-dispatched due check. Each arm uses ONLY its own clock's
    /// compare — the compiler-visible shape of "no fallback".
    #[inline]
    pub fn is_due(&self, now: Now) -> bool {
        match self {
            Expiry::Jiffy(d) => d.is_due(now.jiffies),
            Expiry::Hres { precise, .. } => precise.is_due(now.time),
        }
    }

    /// The absolute time-CSR deadline if this is an hres entry (used
    /// for HRES_NEXT min-maintenance).
    #[inline]
    pub fn hres_abs(&self) -> Option<u64> {
        match self {
            Expiry::Jiffy(_) => None,
            Expiry::Hres { precise, .. } => Some(precise.abs()),
        }
    }

    /// The jiffy-domain bookkeeping value (get_timer_state and
    /// /proc/timer_list readback symmetry with the old slots).
    #[inline]
    pub fn jiffy_bookkeeping(&self) -> u64 {
        match self {
            Expiry::Jiffy(d) => d.abs(),
            Expiry::Hres { horizon, .. } => *horizon,
        }
    }
}

/// A time snapshot: both clocks, read once by the caller. Passing this
/// struct (instead of reading clocks inside the wheel) keeps the wheel
/// pure-logic — the unit tests drive it with synthetic times (the test
/// suite runs with timer IRQs off).
#[derive(Clone, Copy)]
pub struct Now {
    pub jiffies: u64,
    pub time: u64,
}

// ==================== Actions ====================

/// What to do when a timer expires. The old five-field struct allowed
/// illegal combinations (wake_pid AND tfd_addr AND signal fields set
/// simultaneously) and resolved them with a silent if/else-if chain;
/// the enum makes the combinations unrepresentable and the delivery
/// match exhaustive.
#[derive(Clone, Copy, PartialEq)]
pub enum Action {
    /// Deliver nothing (degenerate arm; matches the old all-zero
    /// action, which fired nothing).
    None,
    /// Call wake_up_process on `pid` (the nanosleep one-shot pattern).
    Wake { pid: u32 },
    /// Send `signo` to `pid` (itimer / POSIX timers).
    Signal { pid: u32, signo: i32 },
    /// Notify the timerfd at `tfd` (counter bump + reader wake).
    TimerFd { tfd: u64 },
}

impl Action {
    /// Map the legacy `add_timer_with_action` parameter set onto the
    /// enum (same priority order as the old delivery chain: tfd wins
    /// over signal; wake arms never carried either).
    pub fn from_legacy(pid: u32, signo: i32, tfd_addr: u64) -> Action {
        if tfd_addr != 0 {
            Action::TimerFd { tfd: tfd_addr }
        } else if pid != 0 && signo != 0 {
            Action::Signal { pid, signo }
        } else {
            Action::None
        }
    }

    /// Whether this action delivers anything at all (None arms occupy a
    /// slot but fire nothing — the old behavior for all-zero fields).
    #[inline]
    pub fn delivers(&self) -> bool {
        !matches!(self, Action::None)
    }
}

// ==================== Fired tokens (lifecycle typestate) ====================

/// A detached, ALREADY-COLLECTED expiry awaiting delivery — the
/// "fired" state. Constructed only by `TimerWheel::collect` (which
/// unlinks or re-arms the slot first), consumed BY VALUE by the
/// delivery pass in `timer.rs`. Holding one longer than the pass
/// changes nothing: the slot is gone (one-shot) or already re-armed
/// (periodic), and the action inside has no path back into the wheel.
#[derive(Clone, Copy)]
pub struct FiredToken {
    pub id: u64,
    pub action: Action,
    /// Fire lateness in time ticks (hres one-shots only; 0 otherwise —
    /// wakewatch forensics, same contract as the old tuple element).
    pub late_ticks: u64,
}

/// Delivery-context capability: proves the caller is the timer softirq
/// handler. Zero-sized; the constructor is visible only inside the
/// timer module (`pub(super)`). Rux timer "callbacks" are data actions
/// delivered in softirq context (must not sleep/block); this token is
/// the type-level record of that contract. A future task-context
/// delivery class would be a DIFFERENT token type — the two could not
/// be confused.
pub struct SoftirqCtx {
    _private: (),
}

impl SoftirqCtx {
    /// Single construction site: `timer_softirq_handler`.
    pub(super) fn new() -> Self {
        SoftirqCtx { _private: () }
    }
}

// ==================== The wheel ====================

/// Maximum concurrent timers (unchanged from the flat table).
pub const MAX_TIMERS: usize = 1024;

/// Occupancy bitmap words.
const TIMER_WORDS: usize = MAX_TIMERS / 64;
const _: () = assert!(MAX_TIMERS % 64 == 0);

/// Cascading-wheel geometry: 64 buckets per level, three levels.
/// Level granularity in jiffies: 1, 64, 4096. Total span 262144
/// jiffies (~43.7 min at HZ=100); deeper horizons ride the FAR list.
const LVL_SIZE: u64 = 64;
const LVL1_GRAN: u64 = LVL_SIZE; // 64 jiffies per level-1 bucket
const LVL2_GRAN: u64 = LVL_SIZE * LVL_SIZE; // 4096
const WHEEL_SPAN: u64 = LVL_SIZE * LVL_SIZE * LVL_SIZE; // 262144

/// Bucket-list ids (indices into heads/tails). Layout:
///   [0..64)    level 0 (bucket = expires % 64)
///   [64..128)  level 1 (bucket = (expires >> 6) & 63)
///   [128..192) level 2 (bucket = (expires >> 12) & 63)
///   FAR        horizons beyond the wheel span
///   DUE        the carry list: past-horizon entries awaiting a pass
///              (budget leftovers, hres entries pending their precise
///              compare, entries whose jiffy was vacated by a
///              catch-up advance)
const L0_BASE: u8 = 0;
const L1_BASE: u8 = 64;
const L2_BASE: u8 = 128;
const FAR: u8 = 192;
const DUE: u8 = 193;
const NBUCKETS: usize = 194;
const NIL: u32 = u32::MAX;

/// Maximum expiries collected per softirq pass (R34 discipline: bounds
/// the per-pass work and the detached-token buffer; leftovers stay in
/// their list and drain on the next pass — same jiffy for hres re-runs,
/// next jiffy otherwise).
pub const EXPIRY_BUDGET: usize = 64;

/// One wheel entry, inlined in a fixed slot. 64 bytes (the old slot
/// was 56): the clock-domain tag and enum discriminants cost 8 bytes —
/// the price of the illegal states being unrepresentable.
#[derive(Clone, Copy)]
struct WheelEntry {
    id: u64,
    expiry: Expiry,
    action: Action,
    /// Re-arm interval for periodic entries. `None` = one-shot. A
    /// periodic WAKE timer is unrepresentable: the wake arming paths
    /// pass no interval, so `None` is the only value they can produce.
    interval: Option<NonZeroU64>,
    /// Intrusive next link within `bucket`'s list (NIL = tail).
    next: u32,
    /// Which bucket list this entry is linked into (see the layout
    /// above). Written only by the link/route helpers.
    bucket: u8,
}

/// The cascading wheel: fixed slot pool + occupancy bitmap + bucket
/// lists + the cursor. All fixed .bss storage; every mutation is a
/// plain store (R37 no-alloc-under-lock discipline, structural).
pub struct TimerWheel {
    bitmap: [u64; TIMER_WORDS],
    slots: [Option<WheelEntry>; MAX_TIMERS],
    heads: [u32; NBUCKETS],
    tails: [u32; NBUCKETS],
    /// The jiffy the wheel has advanced to (entries due at jiffies
    /// ≤ `current` live in the cursor bucket or the DUE carry).
    current: u64,
}

impl TimerWheel {
    /// Const initializer for the static (no heap).
    pub const fn new() -> Self {
        TimerWheel {
            bitmap: [0; TIMER_WORDS],
            slots: [None; MAX_TIMERS],
            heads: [NIL; NBUCKETS],
            tails: [NIL; NBUCKETS],
            current: 0,
        }
    }

    /// Route a jiffy-domain absolute expiry against the cursor: which
    /// bucket list it belongs in. THE ONLY place bucket indices are
    /// computed (mis-slotting has no other code path). An already-due
    /// expiry routes to the cursor bucket (walked on every collect
    /// pass); a near-future one to ITS OWN level-0 bucket so the
    /// cursor walk stays O(due), not O(near).
    fn route_for(&self, abs_jiffies: u64) -> u8 {
        if abs_jiffies <= self.current {
            return L0_BASE + (self.current % LVL_SIZE) as u8;
        }
        let d = abs_jiffies - self.current;
        if d < LVL_SIZE {
            L0_BASE + (abs_jiffies % LVL_SIZE) as u8
        } else if d < LVL_SIZE * LVL1_GRAN {
            L1_BASE + (abs_jiffies / LVL1_GRAN % LVL_SIZE) as u8
        } else if d < WHEEL_SPAN {
            L2_BASE + (abs_jiffies / LVL2_GRAN % LVL_SIZE) as u8
        } else {
            FAR
        }
    }

    /// Link `idx` at the tail of bucket `b`'s list (rewrites the
    /// entry's own link fields — the entry must be detached).
    /// Caught by the shape tests' budget-carry case: the first version
    /// linked after the HEAD, orphaning every entry between head and
    /// the previous tail — buckets deeper than two entries silently
    /// lost their middle (lost-wakeup wedge in the suite).
    fn link_tail(&mut self, b: u8, idx: usize) {
        let idx32 = idx as u32;
        if let Some(s) = self.slots[idx].as_mut() {
            s.next = NIL;
            s.bucket = b;
        }
        let tail = self.tails[b as usize];
        if tail == NIL {
            self.heads[b as usize] = idx32;
        } else if let Some(s) = self.slots[tail as usize].as_mut() {
            s.next = idx32;
        }
        self.tails[b as usize] = idx32;
    }

    /// Unlink `idx` from its tagged bucket list. On a miss (corrupt
    /// tag — impossible without a logic bug; the shape tests pin the
    /// invariants) walk every list before giving up, so a stale link
    /// is never left dangling silently.
    fn unlink(&mut self, idx: usize) {
        let (b, next) = match self.slots[idx] {
            Some(s) => (s.bucket, s.next),
            None => return,
        };
        if self.unlink_from(b, idx, next) {
            return;
        }
        for b in 0..NBUCKETS as u8 {
            let next = match self.slots[idx] {
                Some(s) => s.next,
                None => return,
            };
            if self.unlink_from(b, idx, next) {
                return;
            }
        }
    }

    /// Try to remove `idx` from list `b`; `next` is the entry's saved
    /// forward link. Returns true when found and removed.
    fn unlink_from(&mut self, b: u8, idx: usize, next: u32) -> bool {
        let idx32 = idx as u32;
        let mut prev = NIL;
        let mut cur = self.heads[b as usize];
        while cur != NIL {
            if cur == idx32 {
                if prev == NIL {
                    self.heads[b as usize] = next;
                } else if let Some(s) = self.slots[prev as usize].as_mut() {
                    s.next = next;
                }
                if self.tails[b as usize] == idx32 {
                    self.tails[b as usize] = prev;
                }
                return true;
            }
            prev = cur;
            cur = match self.slots[cur as usize] {
                Some(s) => s.next,
                None => NIL,
            };
        }
        false
    }

    /// Splice bucket `from`'s whole list onto the DUE tail (O(1) splice
    /// + an O(list) retag walk).
    fn splice_to_due(&mut self, from: u8) {
        let head = self.heads[from as usize];
        if head == NIL {
            return;
        }
        let due_tail = self.tails[DUE as usize];
        if due_tail == NIL {
            self.heads[DUE as usize] = head;
        } else if let Some(s) = self.slots[due_tail as usize].as_mut() {
            s.next = head;
        }
        self.tails[DUE as usize] = self.tails[from as usize];
        self.heads[from as usize] = NIL;
        self.tails[from as usize] = NIL;
        let mut cur = head;
        while cur != NIL {
            let next = match self.slots[cur as usize] {
                Some(s) => s.next,
                None => NIL,
            };
            if let Some(s) = self.slots[cur as usize].as_mut() {
                s.bucket = DUE;
            }
            cur = next;
        }
    }

    /// Pop bucket `b`'s whole list (returns the head index; the caller
    /// walks via `next`), clearing the bucket.
    fn pop_bucket(&mut self, b: u8) -> u32 {
        let head = self.heads[b as usize];
        if head != NIL {
            self.heads[b as usize] = NIL;
            self.tails[b as usize] = NIL;
        }
        head
    }

    /// Find the first free slot (lowest index; bitmap-accelerated).
    fn find_free_slot(&self) -> Option<usize> {
        for w in 0..TIMER_WORDS {
            let word = self.bitmap[w];
            if word != u64::MAX {
                let bit = (!word).trailing_zeros() as usize;
                return Some(w * 64 + bit);
            }
        }
        None
    }

    /// Mark occupied + link into `b`.
    fn occupy(&mut self, idx: usize, entry: WheelEntry, b: u8) {
        self.slots[idx] = Some(entry);
        self.bitmap[idx / 64] |= 1u64 << (idx % 64);
        self.link_tail(b, idx);
    }

    /// Unlink + clear a slot (plain stores; cannot fail, cannot alloc).
    fn free(&mut self, idx: usize) {
        self.unlink(idx);
        self.slots[idx] = None;
        self.bitmap[idx / 64] &= !(1u64 << (idx % 64));
    }

    /// Bitmap-accelerated find of the slot holding `id`.
    fn find_by_id(&self, id: u64) -> Option<usize> {
        for w in 0..TIMER_WORDS {
            let mut word = self.bitmap[w];
            while word != 0 {
                let bit = word.trailing_zeros() as usize;
                word &= !(1u64 << bit);
                let idx = w * 64 + bit;
                if let Some(slot) = &self.slots[idx] {
                    if slot.id == id {
                        return Some(idx);
                    }
                }
            }
        }
        None
    }

    /// Number of occupied slots (bitmap popcount).
    pub fn len(&self) -> usize {
        self.bitmap.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Arm a timer (jiffy domain). `id` must be non-zero and fresh
    /// (the facade's monotonic counter). Returns false when the pool
    /// is full (the facade maps that to the old `0` result).
    pub fn arm_jiffy(
        &mut self,
        expires: u64,
        action: Action,
        interval: Option<NonZeroU64>,
        id: u64,
    ) -> bool {
        let b = self.route_for(expires);
        let idx = match self.find_free_slot() {
            Some(i) => i,
            None => return false,
        };
        let entry = WheelEntry {
            id,
            expiry: Expiry::Jiffy(Deadline::<JiffyClock> {
                abs: expires,
                _clock: PhantomData,
            }),
            action,
            interval,
            next: NIL,
            bucket: b,
        };
        self.occupy(idx, entry, b);
        true
    }

    /// Arm a high-resolution timer. `horizon` MUST be the grid-derived
    /// first-check jiffy (`super::hres_horizon_for` computes it from
    /// the precise deadline — callers' jiffy parameters stay
    /// diagnostics-only, exactly as the pre-wheel comment promised).
    pub fn arm_hres(
        &mut self,
        precise: u64,
        horizon: u64,
        action: Action,
        interval: Option<NonZeroU64>,
        id: u64,
    ) -> bool {
        let b = self.route_for(horizon);
        let idx = match self.find_free_slot() {
            Some(i) => i,
            None => return false,
        };
        let entry = WheelEntry {
            id,
            expiry: Expiry::Hres {
                precise: Deadline::<HresClock> {
                    abs: precise,
                    _clock: PhantomData,
                },
                horizon,
            },
            action,
            interval,
            next: NIL,
            bucket: b,
        };
        self.occupy(idx, entry, b);
        true
    }

    /// Remove a timer by id. Returns `Some(hres_abs)` when the removed
    /// entry was high-resolution (the caller recomputes HRES_NEXT),
    /// `None` for a jiffy entry, `None` from `find_by_id` miss wrapped
    /// as Option<Option<u64>> — see the facade for the bool mapping.
    pub fn remove(&mut self, id: u64) -> RemoveOutcome {
        let idx = match self.find_by_id(id) {
            Some(i) => i,
            None => return RemoveOutcome::NotFound,
        };
        let was_hres = self.slots[idx].and_then(|s| s.expiry.hres_abs());
        self.free(idx);
        match was_hres {
            Some(t) => RemoveOutcome::RemovedHres(t),
            None => RemoveOutcome::RemovedJiffy,
        }
    }

    /// Is `id` currently armed?
    pub fn contains(&self, id: u64) -> bool {
        self.find_by_id(id).is_some()
    }

    /// (expires jiffies, interval jiffies) for a pending timer — the
    /// getitimer/timerfd readback pair (0 interval = one-shot).
    pub fn state_of(&self, id: u64) -> Option<(u64, u64)> {
        let slot = self.slots[self.find_by_id(id)?].as_ref()?;
        Some((
            slot.expiry.jiffy_bookkeeping(),
            slot.interval.map(|i| i.get()).unwrap_or(0),
        ))
    }

    /// Earliest outstanding hres deadline (u64::MAX = none) — HRES_NEXT
    /// maintenance. Full bitmap walk, same cost class as before (runs
    /// on hres_due passes and hres deletes only).
    pub fn min_hres(&self) -> u64 {
        let mut next = u64::MAX;
        for w in 0..TIMER_WORDS {
            let mut word = self.bitmap[w];
            while word != 0 {
                let bit = word.trailing_zeros() as usize;
                word &= !(1u64 << bit);
                if let Some(slot) = &self.slots[w * 64 + bit] {
                    if let Some(t) = slot.expiry.hres_abs() {
                        if t < next {
                            next = t;
                        }
                    }
                }
            }
        }
        next
    }

    /// Overdue hres entries (wakewatch heartbeat census).
    pub fn overdue_hres(&self, now_time: u64) -> u64 {
        let mut od = 0u64;
        for w in 0..TIMER_WORDS {
            let mut word = self.bitmap[w];
            while word != 0 {
                let bit = word.trailing_zeros() as usize;
                word &= !(1u64 << bit);
                if let Some(slot) = &self.slots[w * 64 + bit] {
                    if let Some(t) = slot.expiry.hres_abs() {
                        if t <= now_time {
                            od += 1;
                        }
                    }
                }
            }
        }
        od
    }

    /// Push /proc/timer_list snapshots (slot order, like the old table;
    /// the consumer is the wakewatch census, which counts).
    pub fn push_snapshots(&self, out: &mut alloc::vec::Vec<TimerSnapshot>) {
        for w in 0..TIMER_WORDS {
            let mut word = self.bitmap[w];
            while word != 0 {
                let bit = word.trailing_zeros() as usize;
                word &= !(1u64 << bit);
                if let Some(s) = &self.slots[w * 64 + bit] {
                    let (pid, signo, wake_pid, tfd_addr) = match s.action {
                        Action::None => (0, 0, 0, 0),
                        Action::Wake { pid } => (0, 0, pid, 0),
                        Action::Signal { pid, signo } => (pid, signo, 0, 0),
                        Action::TimerFd { tfd } => (0, 0, 0, tfd),
                    };
                    out.push(TimerSnapshot {
                        id: s.id,
                        expires_time: s.expiry.hres_abs().unwrap_or(0),
                        expires: s.expiry.jiffy_bookkeeping(),
                        pid,
                        signo,
                        interval_jiffies: s.interval.map(|i| i.get()).unwrap_or(0),
                        wake_pid,
                        tfd_addr,
                    });
                }
            }
        }
    }

    /// Advance the wheel to `now.jiffies`, stepping one jiffy at a
    /// time so no intermediate bucket is skipped (catch-up jumps under
    /// load drain through the DUE carry). Cascades run when the cursor
    /// crosses level boundaries; the FAR list is rescanned on every
    /// level-2 cascade.
    pub fn advance(&mut self, now: Now) {
        while self.current < now.jiffies {
            // Vacate the bucket of the jiffy being LEFT: its remaining
            // entries (budget leftovers, hres pending precision) move
            // to the DUE carry wholesale.
            let vacated = L0_BASE + (self.current % LVL_SIZE) as u8;
            self.splice_to_due(vacated);
            self.current += 1;
            if self.current % LVL_SIZE == 0 {
                let b = L1_BASE + (self.current / LVL1_GRAN % LVL_SIZE) as u8;
                self.cascade(b);
                if self.current % (LVL_SIZE * LVL1_GRAN) == 0 {
                    let b2 = L2_BASE + (self.current / LVL2_GRAN % LVL_SIZE) as u8;
                    self.cascade(b2);
                    self.rescan_far();
                }
            }
        }
    }

    /// Cascade one coarse bucket: pop all entries, re-route each by its
    /// exact expiry (everything lands inside the newly-covered window,
    /// by the routing invariants).
    fn cascade(&mut self, b: u8) {
        let mut cur = self.pop_bucket(b);
        while cur != NIL {
            let next = match self.slots[cur as usize] {
                Some(s) => s.next,
                None => NIL,
            };
            let expiry = match self.slots[cur as usize] {
                Some(s) => s.expiry,
                None => {
                    cur = next;
                    continue;
                }
            };
            let route = match expiry {
                Expiry::Jiffy(d) => self.route_for(d.abs()),
                Expiry::Hres { horizon, .. } => self.route_for(horizon),
            };
            self.link_tail(route, cur as usize);
            cur = next;
        }
    }

    /// Rescan the FAR list: re-route everything that now fits inside
    /// the wheel span (still-far entries are re-linked to FAR — the
    /// popped list is empty, so re-linking rebuilds it in order).
    fn rescan_far(&mut self) {
        let mut cur = self.pop_bucket(FAR);
        while cur != NIL {
            let next = match self.slots[cur as usize] {
                Some(s) => s.next,
                None => NIL,
            };
            let expiry = match self.slots[cur as usize] {
                Some(s) => s.expiry,
                None => {
                    cur = next;
                    continue;
                }
            };
            let route = match expiry {
                Expiry::Jiffy(d) => self.route_for(d.abs()),
                Expiry::Hres { horizon, .. } => self.route_for(horizon),
            };
            self.link_tail(route, cur as usize);
            cur = next;
        }
    }

    /// Collect due expiries into `out` (bounded by EXPIRY_BUDGET and
    /// `out.len()` — the facade passes the per-CPU token buffer).
    /// One-shot entries are unlinked, freed, and their action MOVED
    /// into the token (the leak class is gone: nothing remains in the
    /// slot). Periodic entries are re-armed IN PLACE onto
    /// `now.jiffies + interval` (jiffy domain — the precise deadline is
    /// dropped on re-arm, same semantics as the old `expires_time = 0`
    /// reset) while the delivery token for THIS expiry is returned in
    /// the same step (the re-arm-without-deliver class is gone: one
    /// code path produces both). Entries not yet precisely due (hres
    /// pending) stay in their list untouched.
    pub fn collect(&mut self, now: Now, out: &mut [FiredToken]) -> usize {
        let mut n = 0;
        // Walk order: DUE carry first (oldest lateness), then the
        // cursor bucket. Both walks leave survivors linked; fired
        // entries are unlinked by `unlink`/`free` (which fix the
        // predecessor links themselves).
        let cursor_bucket = L0_BASE + (self.current % LVL_SIZE) as u8;
        for b in [DUE, cursor_bucket] {
            let mut cur = self.heads[b as usize];
            while cur != NIL {
                let next = match self.slots[cur as usize] {
                    Some(s) => s.next,
                    None => NIL,
                };
                let slot = match self.slots[cur as usize] {
                    Some(s) => s,
                    None => {
                        cur = next;
                        continue;
                    }
                };
                if !slot.expiry.is_due(now) || n >= out.len() || n >= EXPIRY_BUDGET {
                    cur = next;
                    continue;
                }
                // Lateness: hres one-shots only (the old probe bug —
                // jiffy entries reported the whole uptime as lateness —
                // is structurally avoided: only the hres arm computes
                // a lateness).
                let late_ticks = match slot.expiry {
                    Expiry::Hres { precise, .. } => now.time.saturating_sub(precise.abs()),
                    Expiry::Jiffy(_) => 0,
                };
                match slot.interval {
                    Some(interval) => {
                        // Periodic: unlink, re-arm the SAME slot at
                        // now + interval, and deliver — one step.
                        let next_expires = now.jiffies.saturating_add(interval.get());
                        let idx = cur as usize;
                        self.unlink(idx);
                        if let Some(s) = self.slots[idx].as_mut() {
                            s.expiry = Expiry::Jiffy(Deadline::<JiffyClock> {
                                abs: next_expires,
                                _clock: PhantomData,
                            });
                        }
                        let route = self.route_for(next_expires);
                        self.link_tail(route, idx);
                        out[n] = FiredToken {
                            id: slot.id,
                            action: slot.action,
                            late_ticks: 0,
                        };
                        n += 1;
                    }
                    None => {
                        // One-shot: free the slot, move the action out.
                        let idx = cur as usize;
                        self.free(idx);
                        out[n] = FiredToken {
                            id: slot.id,
                            action: slot.action,
                            late_ticks,
                        };
                        n += 1;
                    }
                }
                cur = next;
            }
        }
        n
    }

    // ---- shape-test / diagnostics introspection ----

    /// Routing probe: which bucket would `abs_jiffies` route to at
    /// cursor `current`. Pure function; mirrors route_for exactly.
    pub fn probe_route(abs_jiffies: u64, current: u64) -> u8 {
        if abs_jiffies <= current {
            return L0_BASE + (current % LVL_SIZE) as u8;
        }
        let d = abs_jiffies - current;
        if d < LVL_SIZE {
            L0_BASE + (abs_jiffies % LVL_SIZE) as u8
        } else if d < LVL_SIZE * LVL1_GRAN {
            L1_BASE + (abs_jiffies / LVL1_GRAN % LVL_SIZE) as u8
        } else if d < WHEEL_SPAN {
            L2_BASE + (abs_jiffies / LVL2_GRAN % LVL_SIZE) as u8
        } else {
            FAR
        }
    }

    /// Bucket an armed entry sits in (shape-test introspection).
    pub fn bucket_of(&self, id: u64) -> Option<u8> {
        self.slots[self.find_by_id(id)?].map(|s| s.bucket)
    }

    /// The cursor (shape-test introspection).
    pub fn cursor(&self) -> u64 {
        self.current
    }

    /// DUE-carry length (shape-test introspection).
    pub fn due_len(&self) -> usize {
        let mut n = 0;
        let mut cur = self.heads[DUE as usize];
        while cur != NIL {
            n += 1;
            cur = match self.slots[cur as usize] {
                Some(s) => s.next,
                None => NIL,
            };
        }
        n
    }
}

/// Result of `remove` (which class was deleted — drives HRES_NEXT
/// recomputation exactly like the old `removed_hres` flag).
#[derive(PartialEq)]
pub enum RemoveOutcome {
    NotFound,
    RemovedJiffy,
    RemovedHres(u64),
}
