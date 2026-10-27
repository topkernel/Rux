//! Typed per-CPU storage (rust-design-headroom §3, divergence D-7).
//!
//! The mainstream approach this replaces is a `static mut` array
//! indexed by a raw cpu id, whose safety rests on a REVIEW-enFORCED
//! convention: "callers must have IF=0". The paid incident family is
//! the small_alloc magazine divergences — r8final (runs/gnome-r8final,
//! t=515 s, two-CPU row entry via a cpu_id()/ti_cpu divergence) and
//! the 2026-10-26 transient two-CPU entry fixed on main (7a6e1bf4) —
//! both were exactly this convention failing silently.
//!
//! This module encodes the convention in the TYPE instead:
//!
//! - A per-CPU slot ([`CpuLocal`]) is a plain static whose rows are
//!   unreachable except through [`CpuLocal::with`].
//! - `with` disables interrupts, reads the cpu id, and hands the
//!   closure a [`CpuLocalRef`] guard. The guard is the ONLY way to
//!   touch the row; it is `!Send` (cannot migrate to another cpu) and
//!   its lifetime cannot escape the closure, so the interrupt-disabled
//!   window is exactly the closure body — the guard's `Drop` restores
//!   the saved IRQ state.
//! - Rows are cache-line aligned and padded: two CPUs magazine-ing
//!   adjacent rows never share a line.
//! - Two tripwires survive as data, not review notes: an out-of-range
//!   cpu id makes `with` return `None` (the caller falls to its locked
//!   body — the r8final survive-and-taint stance), and a same-slot
//!   re-entry on the same cpu is refused (counted + tainted; the
//!   outer guard's window is still intact because nesting
//!   save/restore is flag-correct).
//!
//! The module is deliberately small: `T: Copy` for the const
//! initializer (per-row constructors can come later), and the GRQ
//! typed lock key named in §3 is the follow-on slice, not this one.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicUsize, Ordering};

/// Divergences refused by the re-entry tripwire (same slot, same cpu,
/// while a guard is live). Zero on a healthy boot; every nonzero value
/// names a call site that must be rewritten — the alias it attempted
/// would have been the r8final class.
static REENTRY_EVENTS: AtomicUsize = AtomicUsize::new(0);

/// Number of re-entry refusals since boot (diagnostic hook for the
/// census/dfx lines).
pub fn reentry_events() -> usize {
    REENTRY_EVENTS.load(Ordering::Relaxed)
}

/// One cpu's slot, padded to a full cache line so adjacent rows never
/// share one (the allocator rows are written under IF=0 on every
/// fast-path op — line splitting between CPUs is measurable traffic).
#[repr(C, align(64))]
struct Row<T> {
    val: T,
    // Plain bool, not AtomicBool: the flag is only ever read/written
    // by the cpu that owns the row, under the same IF=0 exclusivity
    // that guards `val` (an atomic would buy nothing and forfeit the
    // Copy bound the const row initializer needs). Stores go through
    // the raw pointer in the guard, never through a shared reference.
    in_use: bool,
}

// Row participates in const array-repeat initialization (`[Row { .. };
// N]` needs Copy); the manual impl keeps the bound on T explicit.
impl<T: Copy> Clone for Row<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Copy> Copy for Row<T> {}

/// A per-CPU bank of `T`: `CpuLocal<T, N>` holds one `T` per cpu for
/// machines of at most `N` cpus. Build it as a `static` with
/// [`CpuLocal::new`]; reach rows only through [`CpuLocal::with`].
///
/// `Sync` is sound because every access path runs with interrupts
/// disabled on the acting cpu, so a given row is only ever touched by
/// the cpu that owns it, and the re-entry tripwire refuses a second
/// live guard on the same row.
///
/// D-7 incident fix (2026-10-27, run/pa68-d7-rootcause.txt): the rows
/// sit inside [`UnsafeCell`]. The first landing used a plain
/// `[Row<T>; N]`, and `unsafe impl Sync` does NOT confer interior
/// mutability — so the `static MAGAZINES: CpuLocal<...>` in
/// small_alloc was a non-mut static without interior mutability, which
/// rustc emits as an LLVM `constant` global (.rodata). The kernel maps
/// the image P|RW, so debug builds booted by luck; under opt>=1 LLVM
/// deleted every store into the bank (stores to constant memory) and
/// hoisted the loads, the magazine refill loop never terminated, and
/// every release-profile boot died at the first c64 miss. UnsafeCell
/// is the piece that makes the mutation LEGITIMATE to the compiler
/// instead of invisible to it: the static now lands in writable
/// memory and the stores survive at every opt level. The release-strong
/// boot is the standing verification for this family (debug-only
/// testing cannot see it).
pub struct CpuLocal<T, const N: usize> {
    rows: UnsafeCell<[Row<T>; N]>,
}

// SAFETY: see the struct comment — rows are cpu-private by
// construction of `with`.
unsafe impl<T, const N: usize> Sync for CpuLocal<T, N> {}

impl<T: Copy, const N: usize> CpuLocal<T, N> {
    /// Const constructor: every cpu's row starts as a copy of `val`.
    pub const fn new(val: T) -> Self {
        CpuLocal {
            rows: UnsafeCell::new([Row {
                val,
                in_use: false,
            }; N]),
        }
    }
}

impl<T, const N: usize> CpuLocal<T, N> {
    /// Run `f` with this cpu's row, interrupts disabled for exactly
    /// the duration of `f`. Returns `None` — with interrupts restored
    /// — when the acting cpu id is out of range for this bank (the
    /// r8final divergence family; callers keep their locked fallback)
    /// or on a refused same-slot re-entry.
    #[inline]
    pub fn with<R>(&self, f: impl FnOnce(CpuLocalRef<'_, T>) -> R) -> Option<R> {
        let flags = crate::arch::cpu::save_and_disable_irq();
        let cpu = crate::arch::cpu_id() as usize;
        if cpu >= N {
            crate::arch::cpu::restore_irq(flags);
            return None;
        }
        let rows = self.rows.get(); // *mut [Row<T>; N], interior-mutable
        // SAFETY (pointer arithmetic and both field pointers): `cpu`
        // is bounds-checked above; the row is private to this cpu for
        // as long as interrupts stay disabled (they are — the guard
        // restores at drop); the in_use flag read/set below refuses a
        // second live guard, so the &mut handed out is exclusive. All
        // access goes through raw pointers derived from the UnsafeCell
        // — never through a `&`-path (the D-7 incident family).
        let row = unsafe { rows.cast::<Row<T>>().add(cpu) }; // *mut Row<T>
        let (val, in_use): (*mut T, *mut bool) = unsafe {
            (
                core::ptr::addr_of_mut!((*row).val),
                core::ptr::addr_of_mut!((*row).in_use),
            )
        };
        if unsafe { *in_use } {
            // Same slot, same cpu, a guard is already live (the outer
            // window still holds — flags were saved with IRQs already
            // off and false restores nothing). Refuse the alias,
            // count + taint: this is a call-site bug of the r8final
            // class, and the taint names it instead of absorbing it.
            REENTRY_EVENTS.fetch_add(1, Ordering::Relaxed);
            crate::dfx::taint::add_taint(crate::dfx::taint::TaintFlags::BAD_PAGE);
            crate::arch::cpu::restore_irq(flags);
            return None;
        }
        unsafe { *in_use = true };
        let guard = CpuLocalRef {
            val: unsafe { &mut *val },
            cpu,
            flags,
            in_use,
            _nosend: PhantomData,
        };
        Some(f(guard))
    }
}

/// The only handle to a per-CPU row. Exists solely inside a
/// [`CpuLocal::with`] closure: it cannot be sent to another cpu
/// (`!Send` via the raw-pointer marker) and cannot outlive the
/// closure, so the interrupt-disabled window it depends on is its
/// own lifetime.
pub struct CpuLocalRef<'a, T> {
    val: &'a mut T,
    cpu: usize,
    flags: bool,
    in_use: *mut bool,
    _nosend: PhantomData<*mut ()>,
}

impl<T> CpuLocalRef<'_, T> {
    /// The cpu whose row this guard owns (diagnostics; the value is
    /// stable for the guard's lifetime — IF=0 forbids migration).
    pub fn cpu(&self) -> usize {
        self.cpu
    }
}

impl<T> Deref for CpuLocalRef<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.val
    }
}

impl<T> DerefMut for CpuLocalRef<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        self.val
    }
}

impl<T> Drop for CpuLocalRef<'_, T> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: same-cpu exclusivity still holds (IF=0 until the
        // line below restores it); the raw store is the only writer.
        unsafe { *self.in_use = false };
        crate::arch::cpu::restore_irq(self.flags);
    }
}
