//! DFX Subsystem — Debug, Fault, and Diagnostics
//!
//! Unified diagnostic subsystem for the Rux kernel. Consolidates scattered
//! debugging features into a single module and implements critical missing
//! capabilities for production kernel debugging.
//!
//! # Modules
//!
//! - `taint`      — Kernel taint bitmask (Linux-compatible)
//! - `backtrace`  — Reusable `dump_stack()`, `dump_regs()`, `dump_csrs()`
//! - `bug`        — `WARN()`, `BUG()` macros (Linux-compatible)
//! - `hexdump`    — Hex/memory dump utility
//! - `softlockup` — Softlockup detector (per-CPU timestamp check)
//! - `hung_task`  — Hung task detector (khungtaskd kernel thread)
//!
//! # Macros
//!
//! ```rust
//! warn_on!(condition)       // Non-fatal warning if condition is true
//! warn_on_once!(condition)  // Same but fires only once per callsite
//! bug_on!(condition)        // Fatal BUG if condition is true
//! bug!()                    // Unconditional fatal BUG
//! ```

pub mod taint;
pub mod backtrace;
pub mod bug;
pub mod hexdump;
pub mod softlockup;
pub mod hung_task;

/// Initialize the DFX subsystem.
///
/// Called during kernel boot after `sched::init()`.
/// Starts the softlockup detector and hung task detector.
pub fn init() {
    softlockup::init();
    // hung_task detector is deferred — it requires kthread infrastructure
    // that may not be fully ready during early boot. Enable after testing.
    // hung_task::init();
    crate::pr_info!("dfx: diagnostic subsystem initialized");
}

// ============================================================================
// Re-exported macros
// ============================================================================

/// `warn_on!` — Non-fatal warning if condition is true.
///
/// Returns `true` if the condition was true (warning was printed),
/// `false` otherwise. Always evaluates the condition.
///
/// # Example
/// ```rust
/// if warn_on!(ptr.is_null()) {
///     // warning was printed
/// }
/// ```
#[macro_export]
macro_rules! warn_on {
    ($cond:expr) => {{
        if $cond {
            $crate::dfx::bug::warn(file!(), line!(), stringify!($cond));
            true
        } else {
            false
        }
    }};
}

/// `warn_on_once!` — Non-fatal warning, fires only once per callsite.
///
/// Uses a static `AtomicBool` per callsite to ensure the warning
/// is printed at most once per boot.
#[macro_export]
macro_rules! warn_on_once {
    ($cond:expr) => {{
        static ONCE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        if $cond && $crate::dfx::bug::warn_on_once_check(&ONCE) {
            $crate::dfx::bug::warn(file!(), line!(), stringify!($cond));
            true
        } else {
            false
        }
    }};
}

/// `bug_on!` — Fatal BUG if condition is true.
///
/// If the condition is true, prints a BUG message with stack trace
/// and calls `panic!()`. The kernel will not continue.
#[macro_export]
macro_rules! bug_on {
    ($cond:expr) => {
        if $cond {
            $crate::dfx::bug::bug(file!(), line!());
        }
    };
}

/// `bug!` — Unconditional fatal BUG.
///
/// Always triggers a BUG and calls `panic!()`.
#[macro_export]
macro_rules! bug {
    () => {
        $crate::dfx::bug::bug(file!(), line!());
    };
}
