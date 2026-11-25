//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
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

// (INV-PERCPU-1: the per-CPU bank family — banks are addressed by
// cpu id through the fixed static array (bounds by construction on
// our SMP sizes); preemption-off windows own the per-bank access.)
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicUsize, Ordering};

// Row, CpuLocal, CpuLocalRef — the definitions live in sync/types.rs; every
// impl and function that uses them stays in this file. The public ones are
// re-exported so the existing `sync::percpu::…` paths are unchanged; the
// other was private to this file and is imported here (pub(super) in
// types.rs: the sync directory is the smallest visibility boundary Rust
// offers for that split).
pub use super::types::{CpuLocal, CpuLocalRef};
use super::types::{Row};

/// Divergences refused by the re-entry tripwire (same slot, same cpu,
/// while a guard is live). Zero on a healthy boot; every nonzero value
/// names a call site that must be rewritten — the alias it attempted
/// would have been the r8final class.
static REENTRY_EVENTS: AtomicUsize = AtomicUsize::new(0);

/// Number of re-entry refusals since boot (diagnostic hook for the
/// census/debug lines).
pub fn reentry_events() -> usize {
    REENTRY_EVENTS.load(Ordering::Relaxed)
}

// Row participates in const array-repeat initialization (`[Row { .. };
// N]` needs Copy); the manual impl keeps the bound on T explicit.
impl<T: Copy> Clone for Row<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Copy> Copy for Row<T> {}

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
            crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE);
            crate::arch::cpu::restore_irq(flags);
            return None;
        }
        unsafe { *in_use = true };
        let guard = CpuLocalRef {
            val: unsafe { &mut *val },
            cpu,
            flags,
            preempt_guard: false,
            in_use,
            _nosend: PhantomData,
        };
        Some(f(guard))
    }

    /// Run `f` with this cpu's row, PREEMPTION disabled but INTERRUPTS
    /// LEFT ENABLED, for exactly the duration of `f` (106th, the
    /// allocator irq-toggle restructure).
    ///
    /// The exclusivity this guard provides is strictly weaker than
    /// [`CpuLocal::with`]: it excludes a context switch (so the cpu id
    /// read inside cannot go stale — no migration between the read and
    /// the row access) but it does NOT exclude interrupt context on the
    /// same cpu. It is therefore only correct for banks whose IRQ-context
    /// access is ROUTED AWAY from the rows by the caller — the small_alloc
    /// magazines: interrupt-context allocs/frees take the class-lock path
    /// directly and never touch a row, so a task-context row access can be
    /// interrupted but never raced. `with_preempt` returns `None` (nothing
    /// touched) whenever the caller is NOT in task context
    /// (`in_interrupt()` — hardirq/softirq/NMI) or on the same
    /// out-of-range/re-entry refusals as `with`; callers keep their locked
    /// fallback for every `None`.
    ///
    /// Why this exists: `with` pays a cli/sti pair per access. On the
    /// allocation fast paths that is ~46M IF=0 windows per boot, the
    /// second-largest surviving interrupt-off shadow (the deferred-PMI
    /// `enable_irq+0x1d` sample class). A preempt-disable window keeps
    /// the tick and device IRQs flowing through the access.
    pub fn with_preempt<R>(&self, f: impl FnOnce(CpuLocalRef<'_, T>) -> R) -> Option<R> {
        if crate::interrupt::preempt::in_interrupt() {
            return None;
        }
        crate::interrupt::preempt::preempt_count_add(crate::interrupt::preempt::PREEMPT_OFFSET);
        let cpu = crate::arch::cpu_id() as usize;
        if cpu >= N {
            crate::interrupt::preempt::preempt_count_sub(crate::interrupt::preempt::PREEMPT_OFFSET);
            return None;
        }
        let rows = self.rows.get(); // *mut [Row<T>; N], interior-mutable
        // SAFETY: `cpu` is bounds-checked above; the row is private to
        // this cpu for as long as preemption stays disabled (no context
        // switch can migrate the task between the cpu_id read and the row
        // access — the r8final divergence family); interrupt context is
        // excluded by the caller-side routing documented on this method.
        // The in_use flag read/set refuses a second live guard.
        let row = unsafe { rows.cast::<Row<T>>().add(cpu) }; // *mut Row<T>
        let (val, in_use): (*mut T, *mut bool) = unsafe {
            (
                core::ptr::addr_of_mut!((*row).val),
                core::ptr::addr_of_mut!((*row).in_use),
            )
        };
        if unsafe { *in_use } {
            // Same refusal semantics as `with` (see there).
            REENTRY_EVENTS.fetch_add(1, Ordering::Relaxed);
            crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE);
            crate::interrupt::preempt::preempt_count_sub(crate::interrupt::preempt::PREEMPT_OFFSET);
            return None;
        }
        unsafe { *in_use = true };
        let guard = CpuLocalRef {
            val: unsafe { &mut *val },
            cpu,
            flags: false,
            preempt_guard: true,
            in_use,
            _nosend: PhantomData,
        };
        Some(f(guard))
    }
}

impl<T> CpuLocalRef<'_, T> {
    /// The cpu whose row this guard owns (diagnostics; the value is
    /// stable for the guard's lifetime — IF=0 or preempt-off forbid
    /// migration).
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
        // SAFETY: same-cpu exclusivity still holds until the restore
        // below; the raw store is the only writer.
        unsafe { *self.in_use = false };
        if self.preempt_guard {
            crate::interrupt::preempt::preempt_count_sub(
                crate::interrupt::preempt::PREEMPT_OFFSET,
            );
        } else {
            crate::arch::cpu::restore_irq(self.flags);
        }
    }
}
