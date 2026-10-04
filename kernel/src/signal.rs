//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Signal Handling Mechanism
//!
//!
//! Core Concepts:
//! - `struct signal_struct`: Signal handling descriptor
//! - `struct sigpending`: Pending signal queue
//! - `struct sigaction`: Signal handling action
//! - Signal sending (kill) and processing (do_signal)

use crate::sync::rwlock::RwSpinlock;
use core::sync::atomic::{AtomicU64, Ordering};
extern crate alloc;
use alloc::boxed::Box;
use crate::process::task::TaskState;

/// Signal number type
pub type SigType = i32;

/// Standard signal definitions (1-31)
///
#[repr(i32)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Signal {
    /// SIGHUP - Hangup
    SIGHUP = 1,
    /// SIGINT - Interrupt (Ctrl+C)
    SIGINT = 2,
    /// SIGQUIT - Quit
    SIGQUIT = 3,
    /// SIGILL - Illegal instruction
    SIGILL = 4,
    /// SIGTRAP - Breakpoint trap
    SIGTRAP = 5,
    /// SIGABRT - Abnormal termination
    SIGABRT = 6,
    /// SIGBUS - Bus error
    SIGBUS = 7,
    /// SIGFPE - Floating-point exception
    SIGFPE = 8,
    /// SIGKILL - Force kill (cannot be caught/ignored)
    SIGKILL = 9,
    /// SIGUSR1 - User-defined signal 1
    SIGUSR1 = 10,
    /// SIGSEGV - Segmentation fault
    SIGSEGV = 11,
    /// SIGUSR2 - User-defined signal 2
    SIGUSR2 = 12,
    /// SIGPIPE - Broken pipe
    SIGPIPE = 13,
    /// SIGALRM - Timer
    SIGALRM = 14,
    /// SIGTERM - Terminate
    SIGTERM = 15,
    /// SIGSTKFLT - Stack fault
    SIGSTKFLT = 16,
    /// SIGCHLD - Child process status changed
    SIGCHLD = 17,
    /// SIGCONT - Continue
    SIGCONT = 18,
    /// SIGSTOP - Stop (cannot be caught/ignored)
    SIGSTOP = 19,
    /// SIGTSTP - Terminal stop (Ctrl+Z)
    SIGTSTP = 20,
    /// SIGTTIN - Background read
    SIGTTIN = 21,
    /// SIGTTOU - Background write
    SIGTTOU = 22,
    /// SIGURG - Urgent data on socket
    SIGURG = 23,
    /// SIGXCPU - CPU time limit exceeded
    SIGXCPU = 24,
    /// SIGXFSZ - File size limit exceeded
    SIGXFSZ = 25,
    /// SIGVTALRM - Virtual timer expired
    SIGVTALRM = 26,
    /// SIGPROF - Profiling timer expired
    SIGPROF = 27,
    /// SIGWINCH - Window size change
    SIGWINCH = 28,
    /// SIGIO - I/O now possible
    SIGIO = 29,
    /// SIGPWR - Power failure
    SIGPWR = 30,
    /// SIGSYS - Bad system call
    SIGSYS = 31,
}

/// Real-time signal range (32-64)
pub const SIGRTMIN: i32 = 32;
pub const SIGRTMAX: i32 = 64;

/// Signal set (sigset_t)
///
/// Uses 64-bit signal set, can represent 64 signals
pub type SigSet = u64;

/// Signal mask operation modes
///
pub mod sigprocmask_how {
    pub const SIG_BLOCK: i32 = 0;     // Add signals to block mask
    pub const SIG_UNBLOCK: i32 = 1;   // Remove signals from block mask
    pub const SIG_SETMASK: i32 = 2;   // Set new block mask
}

/// Signal flags
///
/// ...
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct SigFlags(u64);

impl SigFlags {
    pub const SA_NOCLDSTOP: u64 = 0x00000001;  // Don't send SIGCHLD when child stops
    pub const SA_NOCLDWAIT: u64 = 0x00000002;  // Don't create zombie on child exit
    pub const SA_SIGINFO: u64 = 0x00000004;    // Provide extra info
    pub const SA_ONSTACK: u64 = 0x08000000;    // Use alternate stack
    pub const SA_RESTART: u64 = 0x10000000;    // Restart system call
    pub const SA_NODEFER: u64 = 0x40000000;    // Don't block self during handler
    pub const SA_RESETHAND: u64 = 0x80000000;  // Reset to default after handling

    pub fn new(flags: u64) -> Self {
        Self(flags)
    }

    pub fn bits(&self) -> u64 {
        self.0
    }
}

/// Signal handling action
///
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum SigActionKind {
    /// Default handling
    Default = 0,
    /// Ignore signal
    Ignore = 1,
    /// Catch signal (handler function pointer)
    Handler = 2,
}

/// Signal handler function type
pub type SigHandler = unsafe extern "C" fn(i32);

/// sigaction structure
///
/// RISC-V has no SA_RESTORER: the rt_sigaction user ABI carries no
/// sa_restorer field at all, and the kernel ALWAYS returns from handlers
/// through its own trampoline (Linux uses the vDSO stub; we map a fixed
/// SIGTRAMP_BASE page).
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct SigAction {
    /// Signal handler function pointer
    pub sa_handler: usize,
    /// Signal flags
    pub sa_flags: SigFlags,
    /// Signal mask
    pub sa_mask: u64,
}

impl SigAction {
    /// Create default sigaction
    pub fn new() -> Self {
        Self {
            sa_handler: SigAction::default_handler() as usize,
            sa_flags: SigFlags::new(0),
            sa_mask: 0,
        }
    }

    /// Create ignore action
    pub fn ignore() -> Self {
        Self {
            sa_handler: SigAction::ignore_handler() as usize,
            sa_flags: SigFlags::new(0),
            sa_mask: 0,
        }
    }

    /// Create handler action
    pub fn handler(handler: SigHandler, flags: SigFlags) -> Self {
        Self {
            sa_handler: handler as usize,
            sa_flags: flags,
            sa_mask: 0,
        }
    }

    /// Default handler address
    fn default_handler() -> usize {
        SigActionKind::Default as usize
    }

    /// Ignore handler address
    fn ignore_handler() -> usize {
        SigActionKind::Ignore as usize
    }

    /// Get action type
    pub fn action(&self) -> SigActionKind {
        if self.sa_handler == SigAction::default_handler() as usize {
            SigActionKind::Default
        } else if self.sa_handler == SigAction::ignore_handler() as usize {
            SigActionKind::Ignore
        } else {
            SigActionKind::Handler
        }
    }

    /// Check if has custom handler
    pub fn has_handler(&self) -> bool {
        self.action() == SigActionKind::Handler
    }
}

/// Pending signal set
///
#[repr(C)]
pub struct SigPending {
    /// Pending signal bitmap (64-bit, supports signals 1-64)
    pub signal: AtomicU64,
    /// Signal info queue (for saving siginfo)
    /// For standard signals, only one is saved
    /// For real-time signals, multiple can be queued
    pub queue: SigQueue,
}

/// Signal queue backed by Spinlock<VecDeque<SigInfo>>
pub struct SigQueue {
    inner: crate::sync::spinlock::Spinlock<alloc::collections::VecDeque<SigInfo>>,
}

unsafe impl Send for SigQueue {}
unsafe impl Sync for SigQueue {}

impl SigQueue {
    pub const fn new() -> Self {
        Self {
            inner: crate::sync::spinlock::Spinlock::new(alloc::collections::VecDeque::new()),
        }
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }

    /// Enqueue: Add signal info to queue tail
    pub fn enqueue(&self, info: SigInfo) {
        self.inner.lock().push_back(info);
    }

    /// Dequeue: Remove signal info from queue head
    pub fn dequeue(&self) -> Option<SigInfo> {
        self.inner.lock().pop_front()
    }

    /// Peek at queue head signal info (without removing)
    pub fn peek(&self) -> Option<SigInfo> {
        self.inner.lock().front().cloned()
    }
}

impl SigPending {
    /// Create new pending signal set
    pub fn new() -> Self {
        Self {
            signal: AtomicU64::new(0),
            queue: SigQueue::new(),
        }
    }

    /// Add signal (standard signals keep one, real-time signals can queue)
    pub fn add(&self, sig: i32) {
        if sig < 1 || sig > 64 {
            return;
        }

        // Distinguish standard and real-time signals
        if sig < SIGRTMIN {
            // Standard signals (1-31): only set bitmap, don't queue
            let mask = 1u64 << (sig - 1);
            self.signal.fetch_or(mask, Ordering::AcqRel);
        } else {
            // Real-time signals (32-64): both queue and set bitmap
            let mask = 1u64 << (sig - 1);
            self.signal.fetch_or(mask, Ordering::AcqRel);

            // Add to queue (for sigqueue syscall)
            let info = SigInfo::new(sig, si_code::SI_USER, 0, 0);
            self.queue.enqueue(info);
        }
    }

    /// Add signal with info (for sigqueue)
    pub fn add_info(&self, info: SigInfo) {
        let sig = info.si_signo;
        if sig < 1 || sig > 64 {
            return;
        }

        // Set bitmap
        let mask = 1u64 << (sig - 1);
        self.signal.fetch_or(mask, Ordering::AcqRel);

        // Real-time signals need queuing
        if sig >= SIGRTMIN {
            self.queue.enqueue(info);
        } else {
            // Standard signals coalesce to one pending instance — keep the
            // LATEST siginfo (e.g. SIGCHLD exit status) so an SA_SIGINFO
            // handler sees the most recent child state. Linux mirrors this:
            // the bitmap collapses duplicates while the queued info carries
            // the payload the delivery frame needs.
            let mut queue = self.queue.inner.lock();
            queue.retain(|i| i.si_signo != sig);
            queue.push_back(info);
        }
    }

    /// Peek at the queued siginfo for a signal WITHOUT consuming it.
    ///
    /// Delivery-path helper: the frame builder needs the payload (e.g.
    /// SIGCHLD si_status/si_code) while the signal stays pending until
    /// the disposition completes.
    pub fn peek_info(&self, sig: i32) -> Option<SigInfo> {
        let queue = self.queue.inner.lock();
        queue.iter().find(|i| i.si_signo == sig).copied()
    }

    /// Remove signal (from bitmap and queue)
    pub fn remove(&self, sig: i32) {
        if sig < 1 || sig > 64 {
            return;
        }

        let mask = 1u64 << (sig - 1);

        // Drop every queued siginfo for this signal number: RT FIFO
        // entries (old behavior popped only from the head) and the single
        // coalesced standard-signal entry alike, so a consumed SIGCHLD
        // never leaks its payload into a later delivery.
        {
            let mut queue = self.queue.inner.lock();
            queue.retain(|i| i.si_signo != sig);
        }

        // Clear bitmap
        self.signal.fetch_and(!mask, Ordering::AcqRel);
    }

    /// Check if signal is pending
    pub fn has(&self, sig: i32) -> bool {
        if sig < 1 || sig > 64 {
            return false;
        }
        let mask = 1u64 << (sig - 1);
        (self.signal.load(Ordering::Acquire) & mask) != 0
    }

    /// Get first pending signal (from bitmap)
    pub fn first(&self) -> Option<i32> {
        let signals = self.signal.load(Ordering::Acquire);
        if signals == 0 {
            return None;
        }
        // Find lowest set bit
        let sig = signals.trailing_zeros() as i32 + 1;
        Some(sig)
    }

    /// Get first pending signal that is not blocked by the given mask
    pub fn first_unmasked(&self, mask: u64) -> Option<i32> {
        let signals = self.signal.load(Ordering::Acquire);
        let deliverable = signals & !mask;
        if deliverable == 0 {
            return None;
        }
        let sig = deliverable.trailing_zeros() as i32 + 1;
        Some(sig)
    }

    /// Get first pending signal's detailed info (from queue)
    pub fn first_info(&self) -> Option<SigInfo> {
        self.queue.dequeue()
    }

    /// Clear all signals
    pub fn clear(&self) {
        self.signal.store(0, Ordering::Release);
        // Clear queue
        while self.queue.dequeue().is_some() {}
    }

    /// Get all pending signals (bitmap)
    pub fn get_all(&self) -> u64 {
        self.signal.load(Ordering::Acquire)
    }

    /// Atomically dequeue one specific signal: clears its bitmap bit and
    /// removes the matching queued SigInfo (if any). Used by
    /// rt_sigtimedwait/sigwait, which CONSUME the signal instead of
    /// letting the delivery path run a handler.
    pub fn remove_one(&self, sig: i32) -> Option<SigInfo> {
        let bit = 1u64 << ((sig as u32) - 1);
        let mut queue = self.queue.inner.lock();
        self.signal.fetch_and(!bit, Ordering::AcqRel);
        if let Some(pos) = queue.iter().position(|i| i.si_signo == sig) {
            return queue.remove(pos);
        }
        None
    }
}

/// Signal handling structure
///
#[repr(C)]
#[derive(Debug)]
pub struct SignalStruct {
    /// Action for each signal (64 signals)
    /// Use RwLock for interior mutability (needed for Arc sharing)
    action: RwSpinlock<[SigAction; 64]>,
    /// Signal mask
    pub mask: AtomicU64,
    /// Whether this process is a child subreaper (init-style reaper)
    pub is_child_subreaper: core::sync::atomic::AtomicBool,
}

impl SignalStruct {
    /// Create new signal handling structure
    pub fn new() -> Self {
        let mut actions = [SigAction::new(); 64];

        // Set default actions
        actions[Signal::SIGKILL as usize - 1] = SigAction::new();  // SIGKILL: default kill
        actions[Signal::SIGSTOP as usize - 1] = SigAction::new();  // SIGSTOP: default stop

        // SIGCHLD default ignore
        actions[Signal::SIGCHLD as usize - 1] = SigAction::ignore();

        Self {
            action: RwSpinlock::new(actions),
            mask: AtomicU64::new(0),
            is_child_subreaper: core::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Set signal handling action
    pub fn set_action(&self, sig: i32, action: SigAction) -> Result<(), ()> {
        if sig < 1 || sig > 64 {
            return Err(());
        }

        // SIGKILL and SIGSTOP cannot be caught or ignored
        if sig == Signal::SIGKILL as i32 || sig == Signal::SIGSTOP as i32 {
            return Err(());
        }

        let mut actions = self.action.write();
        actions[(sig - 1) as usize] = action;
        Ok(())
    }

    /// Get signal handling action
    pub fn get_action(&self, sig: i32) -> Option<SigAction> {
        if sig < 1 || sig > 64 {
            return None;
        }
        let actions = self.action.read();
        Some(actions[(sig - 1) as usize])
    }

    /// Compute the /proc/[pid]/status SigIgn and SigCgt mask pair in one
    /// lock hold: SigIgn has a bit per signal disposed to SIG_IGN, SigCgt
    /// a bit per signal with a user handler installed.
    pub fn ign_cgt_masks(&self) -> (u64, u64) {
        let actions = self.action.read();
        let mut ign = 0u64;
        let mut cgt = 0u64;
        for (i, a) in actions.iter().enumerate() {
            match a.action() {
                SigActionKind::Ignore => ign |= 1u64 << i,
                SigActionKind::Handler => cgt |= 1u64 << i,
                SigActionKind::Default => {}
            }
        }
        (ign, cgt)
    }

    /// Add signal mask
    pub fn add_mask(&self, sig: i32) {
        if sig < 1 || sig > 64 {
            return;
        }
        let mask = 1u64 << (sig - 1);
        self.mask.fetch_or(mask, Ordering::AcqRel);
    }

    /// Remove signal mask
    pub fn remove_mask(&self, sig: i32) {
        if sig < 1 || sig > 64 {
            return;
        }
        let mask = 1u64 << (sig - 1);
        self.mask.fetch_and(!mask, Ordering::AcqRel);
    }

    /// Check if signal is masked
    pub fn is_masked(&self, sig: i32) -> bool {
        if sig < 1 || sig > 64 {
            return false;
        }
        let mask = 1u64 << (sig - 1);
        (self.mask.load(Ordering::Acquire) & mask) != 0
    }

    /// Reset signal handlers to SIG_DFL on execve (POSIX requirement)
    ///
    /// POSIX: "Signals set to the default action shall be set to the default
    /// for the new process image. Signals set to be caught by the calling
    /// process shall be set to the default action. Signals set to SIG_IGN
    /// shall be set to SIG_IGN."
    ///
    /// If `force_default` is true, even SIG_IGN handlers are reset (used by
    /// some privileged exec paths).
    pub fn flush_handlers(&self, force_default: bool) {
        let mut actions = self.action.write();
        for i in 0..64 {
            let sig = (i + 1) as i32;
            // SIGKILL and SIGSTOP cannot be caught/ignored, skip
            if sig == Signal::SIGKILL as i32 || sig == Signal::SIGSTOP as i32 {
                continue;
            }
            if !force_default && actions[i].action() == SigActionKind::Ignore {
                continue;  // Preserve SIG_IGN across exec
            }
            actions[i] = SigAction::new();  // Reset to SIG_DFL
        }
    }
}

impl Clone for SignalStruct {
    fn clone(&self) -> Self {
        // Read the actions and create a new RwLock with copied data
        let actions = self.action.read();
        Self {
            action: RwSpinlock::new(*actions),
            mask: AtomicU64::new(self.mask.load(Ordering::Acquire)),
            is_child_subreaper: core::sync::atomic::AtomicBool::new(
                self.is_child_subreaper.load(Ordering::Acquire),
            ),
        }
    }
}

/// Signal info structure
///
/// Signal info structure (siginfo_t), 128 bytes matching Linux ABI.
///
/// Layout on RISC-V (little-endian):
///   [0..4]   si_signo   i32
///   [4..8]   si_errno   i32  (always 0 for kernel-generated)
///   [8..12]  si_code    i32
///   [12..16] _pad0      i32
///   [16..24] _sifields._kill.si_pid   i32 (+ padding)
///   [24..32] _sifields._kill.si_uid   u32 (+ padding)
///   [32..128] remaining union/padding  96 bytes
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct SigInfo {
    pub si_signo: i32,
    pub si_errno: i32,
    pub si_code: i32,
    _pad0: i32,
    // _sifields union starts at offset 16. _kill layout:
    //   si_pid @16 (i32), si_uid @20 (u32) — review IPC-M15: si_uid used
    //   to sit at 24 (one word too far).
    pub si_pid: i32,
    pub si_uid: u32,
    // _sigchld continues with si_status @24 (i32) — was written at 32.
    // Remaining 104 bytes to reach 128 total.
    _rest: [u8; 104],
}

impl SigInfo {
    /// Create new signal info
    pub fn new(signo: i32, code: i32, pid: u32, uid: u32) -> Self {
        Self {
            si_signo: signo,
            si_errno: 0,
            si_code: code,
            _pad0: 0,
            si_pid: pid as i32,
            si_uid: uid,
            _rest: [0u8; 104],
        }
    }

    /// Store the si_value payload (siginfo offset 24, 8 bytes — sival_int
    /// or sival_ptr) as sent by the rt_sigqueueinfo caller. SA_SIGINFO
    /// handlers and sigwaitinfo consumers read it verbatim.
    pub fn set_value_bytes(&mut self, bytes: &[u8; 8]) {
        self._rest[..8].copy_from_slice(bytes);
    }

    /// Create child process exit signal info
    pub fn child(pid: u32, uid: u32, status: i32) -> Self {
        let mut info = Self::new(Signal::SIGCHLD as i32, 1, pid, uid);
        // si_status lives at _sifields offset 8 → absolute offset 24
        // (the start of _rest; review IPC-M15: it used to land at 32).
        info._rest[..4].copy_from_slice(&status.to_le_bytes());
        info
    }

    /// Create the SIGCHLD siginfo a parent's SA_SIGINFO handler must see
    /// when a child exits (Linux do_notify_parent semantics).
    ///
    /// - `raw_exit >= 0`: normal exit → si_code = CLD_EXITED,
    ///   si_status = (code & 0xFF) << 8 (the waitpid WSTATUS encoding, so
    ///   WEXITSTATUS(si_status) == code and waitid consumers work).
    /// - `raw_exit < 0`: signal death → si_code = CLD_DUMPED when a core
    ///   was dumped, else CLD_KILLED; si_status = the plain signal number
    ///   (no core bit — the distinction rides on si_code; e.g. toybox
    ///   timeout computes 128+WTERMSIG from si_status).
    pub fn child_exit(pid: u32, uid: u32, raw_exit: i32, core_dumped: bool) -> Self {
        let (code, status) = if raw_exit >= 0 {
            (si_code::CLD_EXITED, ((raw_exit as u32) & 0xFF) << 8)
        } else {
            let sig = (-(raw_exit as i64)) as u32 & 0x7F;
            let c = if core_dumped { si_code::CLD_DUMPED } else { si_code::CLD_KILLED };
            (c, sig)
        };
        let mut info = Self::new(Signal::SIGCHLD as i32, code, pid, uid);
        info._rest[..4].copy_from_slice(&status.to_le_bytes());
        info
    }
}

/// Code values used by kill syscall
pub mod si_code {
    /// Signal sent by user (kill)
    pub const SI_USER: i32 = 0;
    /// Signal sent by kernel
    pub const SI_KERNEL: i32 = 0x80;
    /// Child process exited
    pub const CLD_EXITED: i32 = 1;
    /// Child process killed
    pub const CLD_KILLED: i32 = 2;
    /// Child process abnormal termination
    pub const CLD_DUMPED: i32 = 3;
}

// ============================================================================
// Signal Frame Structures
// ============================================================================

/// RISC-V sigcontext structure
///
/// Layout matches `struct sigcontext` from `arch/riscv/include/uapi/asm/sigcontext.h`.
/// The sc_regs field maps to `struct user_regs_struct` (32 u64 values):
///   [0]=pc, [1]=ra, [2]=sp, [3]=gp, [4]=tp, [5]=t0, ..., [31]=t6
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct SigContext {
    /// General-purpose registers: [pc, ra, sp, gp, tp, t0-t6, s0-s11, a0-a7] (32 entries)
    pub sc_regs: [u64; 32],
    /// Floating-point registers f0-f31, directly after sc_regs — the
    /// RISC-V uapi sigcontext has NO sstatus field between them; the old
    /// extra sc_status shifted the FP state 8 bytes off the ABI (review
    /// M-03). sstatus is stashed in SignalFrame.reserved[3] instead.
    pub sc_fpregs: [u64; 32],
    /// Floating-point control and status register
    pub sc_fcsr: u64,
}

impl Default for SigContext {
    fn default() -> Self {
        Self { sc_regs: [0u64; 32], sc_fpregs: [0u64; 32], sc_fcsr: 0 }
    }
}

impl SigContext {
    pub fn new() -> Self {
        Self::default()
    }
}

/// User context - register state saved during signal handling
///
/// Layout matches `struct ucontext` from `arch/riscv/include/uapi/asm/ucontext.h`:
///   uc_flags, uc_link, uc_stack, uc_sigmask, __unused, uc_mcontext
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct UContext {
    /// Flags
    pub uc_flags: u64,
    /// Link to next ucontext (for swapcontext)
    pub uc_link: u64,
    /// Signal stack (stack_t layout: ss_sp, ss_flags, ss_size)
    pub uc_stack: SignalStack,
    /// Signal context (RISC-V registers) — MUST sit at uc+40: the kernel
    /// ABI (asm/ucontext.h) puts uc_mcontext directly after uc_stack;
    /// glibc SA_SIGINFO handlers and libgcc unwinders read registers at
    /// this fixed offset. The old field order inserted uc_sigmask+padding
    /// between them, so every handler saw the signal MASK where the
    /// register array should be (garbage pointers → the whole abort
    /// family).
    pub uc_mcontext: SigContext,
    /// Signal mask (sigset_t = 1 × u64 on RV64 with 64 signals) — after
    /// mcontext per the ABI.
    pub uc_sigmask: u64,
    /// Padding to 1024 bits of sigset space.
    __unused: [u8; 120],
}

impl UContext {
    /// Create new user context
    pub fn new() -> Self {
        Self {
            uc_flags: 0,
            uc_link: 0,
            uc_stack: SignalStack::new(),
            uc_sigmask: 0,
            __unused: [0u8; 120],
            uc_mcontext: SigContext::new(),
        }
    }
}

/// Signal stack (stack_t / struct sigaltstack)
///
/// Layout matches Linux: ss_sp, ss_flags, ss_size
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct SignalStack {
    /// Stack start address
    pub ss_sp: u64,
    /// Stack flags
    pub ss_flags: i32,
    /// Stack size
    pub ss_size: u64,
}

impl SignalStack {
    /// Create new signal stack
    pub fn new() -> Self {
        Self {
            ss_sp: 0,
            ss_flags: 0,
            ss_size: 0,
        }
    }

    /// Check if disabled
    pub fn is_disabled(&self) -> bool {
        (self.ss_flags as u32 & crate::signal::ss_flags::SS_DISABLE) != 0
    }

    /// Check if on stack
    pub fn is_on_stack(&self) -> bool {
        (self.ss_flags as u32 & crate::signal::ss_flags::SS_ONSTACK) != 0
    }
}

/// Signal stack flags
pub mod ss_flags {
    /// Disable signal stack
    pub const SS_ONSTACK: u32 = 0x00000001;
    /// Disable signal stack
    pub const SS_DISABLE: u32 = 0x00000002;
    /// Auto-disable flag
    pub const SS_AUTODISABLE: u32 = 0x00000004;
}

/// Signal stack minimum size
pub const SIGSTKSZ: usize = 8192;
/// Signal stack minimum size
pub const MINSIGSTKSZ: usize = 2048;

/// Signal return trampoline code (RISC-V)
///
/// When signal handler returns, it jumps to this address,
/// then executes rt_sigreturn syscall to restore context
///
/// RISC-V instruction encoding:
/// - li a7, 139      # rt_sigreturn syscall number
/// - ecall           # Execute syscall
///
/// Encoding:
/// - addi a7, zero, 139 = 0x08b00893 (li a7, 139)
/// - ecall = 0x00000073
const SIGRETURN_TRAMPOLINE_RISCV: &[u8] = &[
    0x93, 0x08, 0x8b, 0x00,  // li a7, 139 (addi a7, zero, 139)
    0x73, 0x00, 0x00, 0x00,  // ecall
];

/// Signal frame - constructed on user stack
///
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct SignalFrame {
    /// Reserved words (alignment and magic)
    pub reserved: [u64; 4],
    /// Signal info
    pub info: SigInfo,
    /// User context
    pub uc: UContext,
    /// Trampoline code (8 bytes for RISC-V: li a7,139 + ecall)
    pub trampoline: [u8; 8],
}

impl SignalFrame {
    /// Calculate total size of signal frame
    pub const fn size() -> usize {
        core::mem::size_of::<SignalFrame>()
    }
}

/// Signal handling related constants
pub mod consts {
    /// Alternate stack size for signal handling
    pub const SIGSTKSZ: usize = 8192;
    /// Minimum alternate stack size
    pub const MINSIGSTKSZ: usize = 2048;

    /// Default signal stack size
    pub const DEFAULT_SIGSTACK_SIZE: usize = SIGSTKSZ;
}

// ============================================================================
// Signal Handling and Delivery
// ============================================================================

/// Check and handle pending signals
///
///
/// # Arguments
///
/// * `regs` - PtRegs pointer, used to modify user context
///
/// # Returns
///
/// * `true` - If there are pending signals
/// * `false` - If no pending signals
pub fn do_signal(regs: *mut crate::arch::riscv64::pt_regs::PtRegs) -> bool {
    use crate::sched;
    use crate::process::task::TaskState;

    // SAFETY: sched::current() returns a valid pointer to the running task;
    // regs is passed from trap handler and is valid for the current context.
    unsafe {
        let current = match sched::current() {
            Some(c) => c,
            None => return false,
        };

        // The body is a LOOP, not a single pass: a signal whose disposition
        // parks the task (group stop, ptrace signal-delivery-stop) resumes
        // execution inside this loop, and the signals that arrived while it
        // was stopped must be re-selected (Linux get_signal() discipline).
        // delivered tracks whether ANY pass consumed a signal, so a resume
        // that empties the queue still reports truthfully.
        let mut delivered = false;
        loop {
        // SMP lost-wait fix (Linux restore_saved_sigmask discipline):
        // rt_sigsuspend parks the task with a TEMPORARY mask and arms
        // sigmask_restore. The restore must happen on EVERY trip back to
        // userspace, delivered or not — the old code never restored when
        // no signal was selected, leaking the suspend mask PERMANENTLY
        // (dash's SIGCHLD left blocked, `wait` wedged). Ordering matters:
        // signal SELECTION uses the mask that was active when the syscall
        // decided to return (for a sigsuspend return that is the SUSPEND
        // mask — POSIX), so the restore runs AFTER selection: right before
        // the handler frame is built (uc_sigmask and the during-handler
        // mask then derive from the REAL mask) and on every no-delivery
        // exit. This mirrors Linux's get_signal()/restore_saved_sigmask()
        // split; restoring before the filter re-blocked the very signal
        // that made sigsuspend return, turning every wait into a hot
        // EINTR livelock (observed with dash).
        let blocked = (*current).sigmask;
        let sig = match (*current).pending.first_unmasked(blocked) {
            Some(s) => s,
            None => {
                if (*current).sigmask_restore_valid {
                    (*current).sigmask = (*current).sigmask_restore;
                    (*current).sigmask_restore_valid = false;
                }
                return delivered;
            }
        };
        if (*current).sigmask_restore_valid {
            (*current).sigmask = (*current).sigmask_restore;
            (*current).sigmask_restore_valid = false;
        }

        // If a handler is already active (sigframe armed), a recorded
        // frame whose region the current sp sits inside MIGHT be live.
        // SMP livelock fix: do NOT drop the delivery on this heuristic.
        // setup_frame always places the new frame BELOW the current sp,
        // so a nested delivery can never overwrite the recorded frame;
        // blocking delivery instead wedged any task whose sp stayed below
        // a stale (abandoned) frame_addr forever — the pending signal was
        // never consumed, and rt_sigsuspend hot-looped on EINTR with the
        // zombies unreapable (observed with dash `wait`). sigreturn reads
        // its restore data from the user-side frame it returns through,
        // so arming a newer frame is safe. This mirrors Linux, which
        // allows nested signal delivery.
        if (*current).sigframe.is_some() {
            (*current).sigframe = None;
            (*current).sigframe_addr = 0;
        }

        // PTRACE interception (P1): a traced task does not run the
        // disposition itself — it stops for its tracer instead. The
        // signal is dequeued into task.ptrace_siginfo so PTRACE_CONT(0)
        // swallows it and CONT(sig) re-injects it. A signal re-injected
        // by CONT is marked ptrace_sigdeliver and delivered for real.
        // SIGKILL is never intercepted (cannot be traced away).
        if (*current).tracer_pid() != 0 && sig != Signal::SIGKILL as i32 {
            // Peek first, consume only on match: a different signal
            // arriving before the re-injected one must not eat the mark.
            if (*current).ptrace_sigdeliver_peek() != sig as u32 {
                // No handler frame is built for a traced stop, so convert
                // any syscall-restart sentinel HERE (else the raw -512
                // leaks to userspace as a bogus errno when the tracee
                // resumes — same engine as review syscallb-H-06).
                restart_syscall_no_handler(regs);
                let info = (*current).pending.peek_info(sig)
                    .unwrap_or_else(|| SigInfo::new(sig, si_code::SI_KERNEL, (*current).pid(), 0));
                (*current).pending.remove(sig);
                // SAFETY: current is the running (about-to-stop) task.
                crate::process::ptrace::ptrace_stop(current, sig, info);
                // ptrace_stop parked the task; on resume re-select — the
                // tracer may have re-injected a signal or sent SIGKILL.
                delivered = true;
                continue;
            }
            (*current).take_ptrace_sigdeliver();
        }

        // Get signal handling action (clone needed data)
        let action = (*current).signal.as_ref()
            .and_then(|s| s.get_action(sig));

        // Snapshot the queued siginfo payload for this signal (kernel-
        // generated signals like SIGCHLD carry si_code/si_status here).
        // It is consumed together with the bitmap bit at the remove()
        // below once the disposition has been taken.
        let queued_info: Option<SigInfo> = (*current).pending.peek_info(sig);

        // Handle signal
        if let Some(action) = action {
            // Check if has custom handler
            if action.has_handler() {
                // Call signal handler
                if !setup_frame(current, sig, &action, regs, queued_info) {
                    // Setup failed, execute default action
                    handle_default_signal(sig);
                } else if action.sa_flags.bits() & SigFlags::SA_RESETHAND != 0 {
                    // POSIX SA_RESETHAND (System V semantics): the
                    // disposition resets to SIG_DFL once delivery has
                    // started, so a second occurrence of the signal takes
                    // the default action unless re-armed. Linux
                    // (kernel/signal.c get_signal) resets ONLY the
                    // sa_handler field — sa_flags/sa_mask survive, so a
                    // subsequent sigaction(NULL, &oact) query still
                    // reports e.g. SA_SIGINFO (LTP sigaction01 case 1:
                    // SA_RESETHAND must not clear SA_SIGINFO).
                    if let Some(sig_struct) = (*current).signal.as_ref() {
                        let mut reset = action;
                        reset.sa_handler = SigAction::default_handler();
                        let _ = sig_struct.set_action(sig, reset);
                    }
                }
            } else if action.action() == SigActionKind::Ignore {
                // SIG_IGN disposition: nothing to execute. Do NOT fall
                // through to handle_default_signal — its terminate list
                // would kill a process that explicitly ignores the signal
                // (reachable when rt_sigaction races the queued signal).
                // No handler frame is built, so convert any syscall
                // restart sentinel here instead of leaking it to userspace.
                restart_syscall_no_handler(regs);
            } else {
                // Execute default action. No handler frame is built here,
                // and only setup_frame converts syscall restart sentinels
                // — so a default-ignore signal (SIGCHLD/SIGURG/SIGWINCH/
                // SIGCONT) interrupting a wait_event_interruptible loop
                // returned the raw -ERESTARTSYS (-512) to userspace as a
                // bogus errno. Linux's do_signal() restarts the syscall
                // on this no-handler path; mirror it.
                restart_syscall_no_handler(regs);
                handle_default_signal(sig);
            }
        }

        // Remove signal from pending queue
        (*current).pending.remove(sig);

        // Group stop (SIGSTOP/SIGTSTP/SIGTTIN/SIGTTOU default action):
        // handle_default_signal did the bookkeeping and marked this task
        // STOPPED — park until continued, then re-select like Linux's
        // get_signal() loop (signals arriving while stopped are handled
        // after the resume).
        if (*current).state().contains(TaskState::STOPPED) {
            park_stopped_task(current);
            delivered = true;
            continue;
        }

        // If process is set to ZOMBIE or STOPPED, set need_resched flag
        // The actual schedule() call happens in trap.S when returning to user mode
        let task_state = (*current).state();
        if task_state.is_dead() || task_state.contains(TaskState::STOPPED) {
            // Set need_resched flag - schedule() will be called in trap.S
            crate::sched::set_need_resched();
        }

        return true;
        }
    }
}

/// Syscall restart for signal delivery without a handler (Linux pattern).
///
/// `setup_frame` converts -ERESTARTSYS/-ERESTARTNOHAND only when a handler
/// frame is built. When the disposition is Default/Ignore nothing did the
/// conversion, and the sentinel set by wait_event_interruptible (wait.rs
/// breaks with -512) leaked to userspace verbatim as a bogus errno.
///
/// Mirrors Linux arch/riscv do_signal()'s "restart the system call — no
/// handlers present" path: rewind epc to the ecall and restore orig_a0 so
/// the syscall re-executes transparently.
///
/// # Safety
/// `regs` is the current task's live PtRegs (from the trap path).
unsafe fn restart_syscall_no_handler(regs: *mut crate::arch::riscv64::pt_regs::PtRegs) {
    use crate::arch::riscv64::pt_regs::Cause;
    const ERESTARTSYS: i64 = -512;
    const ERESTARTNOHAND: i64 = -514;
    let regs = &mut *regs;

    // Only syscall frames (cause still EcallUser at trap exit) carry a
    // restart sentinel in a0; interrupt/page-fault frames hold user data
    // that may coincidentally equal -512.
    if Cause::from_cause(regs.cause) != Cause::EcallUser {
        return;
    }
    let a0 = regs.a0 as i64;
    if a0 == ERESTARTSYS || a0 == ERESTARTNOHAND {
        // ecall is a 4-byte instruction (there is no compressed encoding),
        // and handle_syscall advanced epc by exactly 4 on this path.
        if regs.epc >= 4 {
            regs.epc -= 4;
            regs.a0 = regs.orig_a0;
        }
    }
}

/// Set up signal frame and prepare to call signal handler (RISC-V version)
///
/// # Arguments
///
/// * `task` - Current task
/// * `sig` - Signal number
/// * `action` - Signal handling action
/// * `regs` - PtRegs pointer, used to modify trap frame
///
/// # Returns
///
/// * `true` - Setup successful
/// * `false` - Setup failed
/// Fixed virtual address of the per-process signal-return trampoline page.
/// RISC-V glibc installs handlers WITHOUT sa_restorer (the kernel normally
/// points ra at the vDSO __vdso_rt_sigreturn stub); Rux's vDSO is disabled,
/// so the old stack-trampoline fallback returned into an NX stack page and
/// every glibc signal handler died with SIGSEGV (dash exiting status=11
/// the moment a child sent SIGCHLD was this bug).
pub const SIGTRAMP_BASE: u64 = 0x3FBE_0000_00;

unsafe fn setup_frame(
    task: *mut crate::process::task::Task,
    sig: i32,
    action: &SigAction,
    regs: *mut crate::arch::riscv64::pt_regs::PtRegs,
    queued_info: Option<SigInfo>,
) -> bool {
    let regs = &mut *regs;

    // Check if need to use signal stack
    let use_altstack = (action.sa_flags.bits() & crate::signal::SigFlags::SA_ONSTACK) != 0;

    // Get user stack pointer
    let user_sp = regs.sp;
    const SIGNAL_FRAME_SIZE: u64 = SignalFrame::size() as u64;

    // Decide which stack to use based on flags
    let frame_addr = if use_altstack {
        // Use signal stack
        let sigstack = &(*task).sigstack;

        // Check if signal stack is valid
        if sigstack.is_disabled() || sigstack.ss_sp == 0 {
            // Signal stack unavailable, use normal stack
            user_sp - SIGNAL_FRAME_SIZE
        } else {
            // Calculate signal frame position (at top of signal stack)
            sigstack.ss_sp + sigstack.ss_size - SIGNAL_FRAME_SIZE
        }
    } else {
        // Use normal user stack
        user_sp - SIGNAL_FRAME_SIZE
    };

    // Ensure 16-byte alignment (RISC-V ABI requirement)
    let frame_addr = frame_addr & !0xF;

    // Create signal frame. SA_SIGINFO handlers decode the payload fields
    // (si_code/si_status for SIGCHLD), so prefer the siginfo queued by the
    // sender and only fall back to the generic SI_KERNEL placeholder.
    let mut frame = SignalFrame {
        reserved: [0; 4],
        info: queued_info
            .unwrap_or_else(|| SigInfo::new(sig, crate::signal::si_code::SI_KERNEL, (*task).pid(), 0)),
        uc: UContext::new(),
        trampoline: [
            0x93, 0x08, 0x8b, 0x00,  // li a7, 139 (rt_sigreturn)
            0x73, 0x00, 0x00, 0x00,  // ecall
        ],
    };

    // Save current PtRegs to signal frame (for sigreturn restore)
    // sc_regs layout matches Linux user_regs_struct:
    //   [0]=pc, [1]=ra(x1), [2]=sp(x2), ..., [31]=t6(x31)

    // Save PC
    frame.uc.uc_mcontext.sc_regs[0] = regs.epc;

    // Save registers from PtRegs to sigcontext (x1-x31 → sc_regs[1..32])
    frame.uc.uc_mcontext.sc_regs[1] = regs.ra;   // x1 (ra)
    frame.uc.uc_mcontext.sc_regs[2] = regs.sp;   // x2 (sp)
    frame.uc.uc_mcontext.sc_regs[3] = regs.gp;   // x3 (gp)
    frame.uc.uc_mcontext.sc_regs[4] = regs.tp;   // x4 (tp)
    frame.uc.uc_mcontext.sc_regs[5] = regs.t0;   // x5 (t0)
    frame.uc.uc_mcontext.sc_regs[6] = regs.t1;   // x6 (t1)
    frame.uc.uc_mcontext.sc_regs[7] = regs.t2;   // x7 (t2)
    frame.uc.uc_mcontext.sc_regs[8] = regs.s0;   // x8 (s0/fp)
    frame.uc.uc_mcontext.sc_regs[9] = regs.s1;   // x9 (s1)
    frame.uc.uc_mcontext.sc_regs[10] = regs.a0;  // x10 (a0)
    frame.uc.uc_mcontext.sc_regs[11] = regs.a1;  // x11 (a1)
    frame.uc.uc_mcontext.sc_regs[12] = regs.a2;  // x12 (a2)
    frame.uc.uc_mcontext.sc_regs[13] = regs.a3;  // x13 (a3)
    frame.uc.uc_mcontext.sc_regs[14] = regs.a4;  // x14 (a4)
    frame.uc.uc_mcontext.sc_regs[15] = regs.a5;  // x15 (a5)
    frame.uc.uc_mcontext.sc_regs[16] = regs.a6;  // x16 (a6)
    frame.uc.uc_mcontext.sc_regs[17] = regs.a7;  // x17 (a7)
    frame.uc.uc_mcontext.sc_regs[18] = regs.s2;  // x18 (s2)
    frame.uc.uc_mcontext.sc_regs[19] = regs.s3;  // x19 (s3)
    frame.uc.uc_mcontext.sc_regs[20] = regs.s4;  // x20 (s4)
    frame.uc.uc_mcontext.sc_regs[21] = regs.s5;  // x21 (s5)
    frame.uc.uc_mcontext.sc_regs[22] = regs.s6;  // x22 (s6)
    frame.uc.uc_mcontext.sc_regs[23] = regs.s7;  // x23 (s7)
    frame.uc.uc_mcontext.sc_regs[24] = regs.s8;  // x24 (s8)
    frame.uc.uc_mcontext.sc_regs[25] = regs.s9;  // x25 (s9)
    frame.uc.uc_mcontext.sc_regs[26] = regs.s10; // x26 (s10)
    frame.uc.uc_mcontext.sc_regs[27] = regs.s11; // x27 (s11)
    frame.uc.uc_mcontext.sc_regs[28] = regs.t3;  // x28 (t3)
    frame.uc.uc_mcontext.sc_regs[29] = regs.t4;  // x29 (t4)
    frame.uc.uc_mcontext.sc_regs[30] = regs.t5;  // x30 (t5)
    frame.uc.uc_mcontext.sc_regs[31] = regs.t6;  // x31 (t6)

    // Syscall restart handling (Linux approach):
    // Only intervene when the interrupted syscall returned a restart code
    // (ERESTARTSYS, ERESTARTNOHAND, etc.). The dispatch layer sets a0 to
    // these sentinel values; we check for them here.
    // - If SA_RESTART is set: rewind PC by 4 to re-execute the ecall.
    // - If SA_RESTART is not set: replace a0 with -EINTR.
    // If no restart code is present, leave the registers untouched.
    const ERESTARTSYS: i64 = -512;
    const ERESTARTNOHAND: i64 = -514;
    let a0_val = regs.a0 as i64;
    if a0_val == ERESTARTSYS || a0_val == ERESTARTNOHAND {
        if action.sa_flags.bits() & SigFlags::SA_RESTART != 0 && a0_val == ERESTARTSYS {
            // Rewind PC to re-execute the ecall instruction. The saved a0
            // must be the ORIGINAL first argument, not the -512 sentinel:
            // after the handler returns, rt_sigreturn restores this frame
            // and the ecall re-executes with a0 as syscall arg 0 (Linux's
            // do_signal sets regs->a0 = regs->orig_a0 before building the
            // frame — leaving the sentinel here restarted read(-512, ...)
            // → EBADF / EFAULT).
            frame.uc.uc_mcontext.sc_regs[0] = regs.epc - 4;
            frame.uc.uc_mcontext.sc_regs[10] = regs.orig_a0;
        } else {
            // Convert restart code to -EINTR for userspace.
            frame.uc.uc_mcontext.sc_regs[10] = (-(crate::errno::constants::EINTR as i64)) as u64;
        }
    }

    // Save sstatus (kernel-private stash — not part of the user ABI struct)
    frame.reserved[3] = regs.status;

    // Signal mask during handler execution (POSIX): old mask | sa_mask |
    // the signal itself (unless SA_NODEFER). sa_mask was never merged
    // before, so handlers ran with their configured block mask ignored
    // (review M-01). SIGKILL/SIGSTOP stay unblockable.
    let mut new_sigmask = (*task).sigmask | action.sa_mask;
    if (action.sa_flags.bits() & SigFlags::SA_NODEFER) == 0 {
        new_sigmask |= 1u64 << ((sig as u32) - 1);
    }
    new_sigmask &= !((1u64 << 8) | (1u64 << 18));
    // SMP lost-wait root fix: the frame's uc_sigmask is what rt_sigreturn
    // reinstates — it must hold the PRE-handler mask (the interrupted
    // context), not the during-handler mask. The old code saved
    // `new_sigmask` there, so every completed handler leaked its
    // during-handler block set permanently: after one SIGCHLD handler run
    // the shell kept SIGCHLD (plus sa_mask bits) blocked forever, `wait`
    // then parked in an unsatisfiable sigsuspend with the children
    // unreapable — the -smp pipeline wedge.
    frame.uc.uc_sigmask = (*task).sigmask;
    (*task).sigmask = new_sigmask;

    // Save signal stack info
    frame.uc.uc_stack = (*task).sigstack;

    // Save signal frame to task structure
    (*task).sigframe_addr = frame_addr;
    (*task).sigframe = Some(frame);

    // Copy signal frame to user stack so the handler can access siginfo/ucontext
    let frame_size = core::mem::size_of::<SignalFrame>();
    let uncopied = crate::arch::riscv64::uaccess::copy_to_user(
        frame_addr as *mut u8,
        &frame as *const SignalFrame as *const u8,
        frame_size,
    );
    if uncopied != 0 {
        // copy_to_user failed (e.g., stack overflow) — force SIGSEGV
        (*task).sigframe = None;
        (*task).sigframe_addr = 0;
        handle_default_signal(11);
        return false;
    }

    // Set signal handler arguments (RISC-V calling convention: a0-a7)
    // SA_SIGINFO: void handler(int sig, siginfo_t *info, void *uc)
    // otherwise:  void handler(int sig) — only a0 is defined; a1/a2 are
    // zeroed so a sloppy libc wrapper never dereferences a stale pointer.
    if action.sa_flags.bits() & SigFlags::SA_SIGINFO != 0 {
        regs.a0 = sig as u64;                      // a0 = sig
        regs.a1 = frame_addr + core::mem::offset_of!(SignalFrame, info) as u64;  // a1 = &info
        regs.a2 = frame_addr + core::mem::offset_of!(SignalFrame, uc) as u64;    // a2 = &uc
    } else {
        regs.a0 = sig as u64;                      // a0 = sig
        regs.a1 = 0;
        regs.a2 = 0;
    }

    // Set return address to signal handler
    regs.epc = action.sa_handler as u64;

    // Set user stack pointer to signal frame position
    regs.sp = frame_addr;

    // Return address for rt_sigreturn: RISC-V userspace passes NO
    // sa_restorer (the ABI has no SA_RESTORER — glibc's sigaction wrapper
    // doesn't even write that field, verified against jammy's
    // libc.so.6 __libc_sigaction disassembly). The kernel-owned fixed
    // trampoline page (li a7,139; ecall) is the ONLY return path, exactly
    // like Linux returning into the vDSO stub. Returning into the W^X
    // user stack is not executable, so an in-frame trampoline would
    // SIGSEGV every handler return.
    regs.ra = SIGTRAMP_BASE;

    true  // Success
}

/// Restore signal context from user stack (RISC-V version)
///
/// # Arguments
///
/// * `task` - Current task
/// * `frame_addr` - Signal frame address in user space
/// * `regs` - PtRegs pointer, used to restore trap frame
///
/// # Returns
///
/// * `true` - Restore successful
/// * `false` - Restore failed
pub unsafe fn restore_sigcontext(
    task: *mut crate::process::task::Task,
    frame_addr: u64,
    regs: *mut crate::arch::riscv64::pt_regs::PtRegs,
) -> bool {
    // Validate signal frame address
    if frame_addr == 0 {
        return false;
    }

    // Read the frame back from USER memory first: the handler (or
    // swapcontext/longjmp-style code) may have modified the ucontext on
    // the stack, and those edits must win (review M-04 — the kernel
    // backup copy used to be authoritative). Fall back to the kernel
    // backup only when the user copy is unreadable.
    let mut user_frame: SignalFrame = unsafe { core::mem::zeroed() };
    let copied = unsafe {
        crate::arch::riscv64::uaccess::copy_from_user(
            &mut user_frame as *mut SignalFrame as *mut u8,
            frame_addr as *const u8,
            core::mem::size_of::<SignalFrame>(),
        )
    };
    let frame = if copied == 0 {
        user_frame
    } else {
        match (*task).sigframe {
            Some(f) => f,
            None => return false,
        }
    };

    let regs = &mut *regs;

    // Restore registers from signal frame's uc_mcontext (RISC-V)
    // sc_regs layout: [0]=pc, [1]=ra(x1), [2]=sp(x2), ..., [31]=t6(x31)

    // Restore PC (program counter)
    regs.epc = frame.uc.uc_mcontext.sc_regs[0];

    // Restore all general-purpose registers (x1-x31 → sc_regs[1..32])
    regs.ra = frame.uc.uc_mcontext.sc_regs[1];   // x1 (ra)
    regs.sp = frame.uc.uc_mcontext.sc_regs[2];   // x2 (sp)
    regs.gp = frame.uc.uc_mcontext.sc_regs[3];   // x3 (gp)
    regs.tp = frame.uc.uc_mcontext.sc_regs[4];   // x4 (tp)
    regs.t0 = frame.uc.uc_mcontext.sc_regs[5];   // x5 (t0)
    regs.t1 = frame.uc.uc_mcontext.sc_regs[6];   // x6 (t1)
    regs.t2 = frame.uc.uc_mcontext.sc_regs[7];   // x7 (t2)
    regs.s0 = frame.uc.uc_mcontext.sc_regs[8];   // x8 (s0/fp)
    regs.s1 = frame.uc.uc_mcontext.sc_regs[9];   // x9 (s1)
    regs.a0 = frame.uc.uc_mcontext.sc_regs[10];  // x10 (a0)
    regs.a1 = frame.uc.uc_mcontext.sc_regs[11];  // x11 (a1)
    regs.a2 = frame.uc.uc_mcontext.sc_regs[12];  // x12 (a2)
    regs.a3 = frame.uc.uc_mcontext.sc_regs[13];  // x13 (a3)
    regs.a4 = frame.uc.uc_mcontext.sc_regs[14];  // x14 (a4)
    regs.a5 = frame.uc.uc_mcontext.sc_regs[15];  // x15 (a5)
    regs.a6 = frame.uc.uc_mcontext.sc_regs[16];  // x16 (a6)
    regs.a7 = frame.uc.uc_mcontext.sc_regs[17];  // x17 (a7)
    regs.s2 = frame.uc.uc_mcontext.sc_regs[18];  // x18 (s2)
    regs.s3 = frame.uc.uc_mcontext.sc_regs[19];  // x19 (s3)
    regs.s4 = frame.uc.uc_mcontext.sc_regs[20];  // x20 (s4)
    regs.s5 = frame.uc.uc_mcontext.sc_regs[21];  // x21 (s5)
    regs.s6 = frame.uc.uc_mcontext.sc_regs[22];  // x22 (s6)
    regs.s7 = frame.uc.uc_mcontext.sc_regs[23];  // x23 (s7)
    regs.s8 = frame.uc.uc_mcontext.sc_regs[24];  // x24 (s8)
    regs.s9 = frame.uc.uc_mcontext.sc_regs[25];  // x25 (s9)
    regs.s10 = frame.uc.uc_mcontext.sc_regs[26]; // x26 (s10)
    regs.s11 = frame.uc.uc_mcontext.sc_regs[27]; // x27 (s11)
    regs.t3 = frame.uc.uc_mcontext.sc_regs[28];  // x28 (t3)
    regs.t4 = frame.uc.uc_mcontext.sc_regs[29];  // x29 (t4)
    regs.t5 = frame.uc.uc_mcontext.sc_regs[30];  // x30 (t5)
    regs.t6 = frame.uc.uc_mcontext.sc_regs[31];  // x31 (t6)

    // Restore sstatus (kernel-private stash in reserved[3]) — but strip
    // all privilege-relevant bits: the value came from USER memory (M-04
    // reads back the user frame), and SPP/SPIE/SIE control whether sret
    // returns to S or U mode. Rebuilding them from kernel policy (regression
    // round 5, HIGH: privilege escalation vector).
    let saved_status = frame.reserved[3];
    const SSTATUS_SPP: u64 = 1 << 8;
    const SSTATUS_SPIE: u64 = 1 << 5;
    const SSTATUS_SIE: u64 = 1 << 2;
    const SSTATUS_UBE: u64 = 1 << 6;
    const SSTATUS_MXR: u64 = 1 << 19;
    const SSTATUS_SUM: u64 = 1 << 18;
    regs.status = saved_status
        & !(SSTATUS_SPP | SSTATUS_SPIE | SSTATUS_SIE | SSTATUS_UBE | SSTATUS_MXR)
        // SUM is legitimately user-controllable (for crossing), keep it.
        // SPIE must be FORCED, not merely stripped: M-04 reads the frame
        // back from USER memory, so a handler that zeroed the ucontext
        // would leave SPIE=0 — trap exit restores that sstatus and sret
        // clears SIE, returning to user with interrupts disabled: no
        // timer ticks, no preemption, one wedged CPU (user-triggerable).
        // Kernel policy: return to user always runs with SIE=1.
        | SSTATUS_SPIE;

    // Restore signal mask — SIGKILL/SIGSTOP can never be blocked
    (*task).sigmask = frame.uc.uc_sigmask & !((1u64 << 8) | (1u64 << 18));

    // Clear signal frame
    (*task).sigframe = None;
    (*task).sigframe_addr = 0;

    true
}

/// Get signal frame offsets
///
/// Returns offsets of fields in signal frame, used to locate data on user stack
pub mod frame_offsets {
    /// SigInfo offset in SignalFrame
    pub const SIGINFO_OFFSET: usize = 32;  // reserved [4 * u64]

    /// UContext offset in SignalFrame
    pub const UCONTEXT_OFFSET: usize = 32 + core::mem::size_of::<super::SigInfo>();

    /// uc_mcontext offset in UContext
    /// uc_flags(8) + uc_link(8) + uc_stack(24) + uc_sigmask(8) + __unused(120)
    pub const MCONTEXT_OFFSET: usize = 8 + 8 + core::mem::size_of::<super::SignalStack>() + 8 + 120;
}

/// Handle default signal action
///
fn handle_default_signal(sig: i32) {

    use crate::sched;

    match sig {
        // Ignore by default: SIGCHLD(17), SIGURG(23), SIGWINCH(28)
        17 | 23 | 28 => {
            // Default ignore
        }
        // SIGCONT(18): wake stopped process
        18 => {
            // SIGCONT: wake stopped process
            if let Some(current) = sched::current() {
                if current.state().contains(TaskState::STOPPED) {
                    current.set_state(TaskState::new(TaskState::RUNNING));
                    signal_wake_up(current as *const _ as *mut _);
                }
            }
        }
        // Stop process: SIGSTOP(19), SIGTSTP(20), SIGTTIN(21), SIGTTOU(22)
        19 | 20 | 21 | 22 => {
            // SIGSTOP, SIGTSTP - stop process
            // SAFETY: current is a valid task pointer from sched::current().
            unsafe {
                if let Some(current) = sched::current() {
                    (*current).set_stop_signal(sig);
                    (*current).stop_reported.store(false, core::sync::atomic::Ordering::Release);
                    (*current).set_state(TaskState::new(TaskState::STOPPED));
                    // Notify parent. SIGCHLD is discarded for parents without
                    // a handler (default disposition is SIG_IGN here), so the
                    // waitqueue wake is what actually unblocks a wait4/WUNTRACED
                    // sleeper — same discipline as the exit path.
                    if let Some(parent_ptr) = (*current).parent_ptr() {
                        let parent = parent_ptr as *mut crate::process::task::Task;
                        let _ = crate::signal::send_signal((*parent).pid(), Signal::SIGCHLD as i32);
                        crate::process::exit::wake_group_chldexit(parent);
                        crate::signal::signal_wake_up(parent);
                    }
                    // The actual park happens in do_signal's group-stop
                    // branch (park_stopped_task).
                    sched::set_need_resched();
                }
            }
        }
        // Terminate process (core dump or direct termination)
        1 | 2 | 3 | 4 | 5 | 6   // SIGHUP | SIGINT | SIGQUIT | SIGILL | SIGTRAP | SIGABRT
        | 7 | 8 | 9 | 11 | 13 | 14 | 15  // SIGBUS | SIGFPE | SIGKILL | SIGSEGV | SIGPIPE | SIGALRM | SIGTERM
        | 16 | 10 | 12           // SIGSTKFLT | SIGUSR1 | SIGUSR2
        | 24 | 25 | 26 | 27      // SIGXCPU | SIGXFSZ | SIGVTALRM | SIGPROF
        | 29 | 30 | 31 => {      // SIGIO | SIGPWR | SIGSYS
            // Call do_group_exit to properly terminate the WHOLE thread
            // group (Linux: a fatal signal in any thread runs
            // do_group_exit — the surviving-leader model would leave a
            // process running after e.g. a SIGSEGV in one thread).
            // This releases mm, fdtable, kernel stack, removes from run
            // queue, etc. Store negative signal number (do_wait encodes
            // as waitpid status).
            crate::process::exit::do_exit_group(-(sig as i32));
        }
        _ => {
            // Unknown signal, default ignore
        }
    }
}

/// Send signal to process
///
///
/// # Arguments
///
/// * `pid` - Target process PID
/// * `sig` - Signal number
/// * `info` - Signal info
///
/// # Returns
///
/// * `true` - Signal sent successfully
/// * `false` - Signal send failed
/// User-originated signal (kill/tgkill/tkill): identical to send_signal
/// but the queued siginfo carries the SENDER's pid/uid (SI_USER), so
/// sigwaitinfo/sigtimedwait readers and SA_SIGINFO handlers see si_pid.
/// Kernel-internal senders (timers, OOM, tty) keep using send_signal —
/// their siginfo is SI_KERNEL with no sender.
pub fn send_signal_from_user(pid: u32, sig: i32) -> Result<(), i32> {
    if sig < 1 || sig > 64 {
        return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
    }
    let (sender_pid, sender_uid) = match crate::sched::current() {
        // SAFETY: the current task pointer is valid for the call duration.
        Some(t) => unsafe { ((*t).pid(), (*t).cred().uid) },
        None => (0, 0),
    };
    send_signal_with_info(pid, SigInfo::new(sig, si_code::SI_USER, sender_pid, sender_uid))
}

pub fn send_signal(pid: u32, sig: i32) -> Result<(), i32> {    // Check if signal number is valid
    if sig < 1 || sig > 64 {
        return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
    }

    // SAFETY: pid_hash_lookup_pinned returns a valid Task pointer or null;
    // null is checked below. R15-6: PINNED — synchronize_rcu() is a no-op
    // (RCU_GEN never advances; only rcu_softirq_handler bumps it and the
    // Rcu softirq is never raised), so an unpinned lookup could hand back
    // a Task that release_task() freed and the buddy already reused. The
    // wake below would then enqueue a dead page into the runqueue
    // (on_rq on the freed page reads false) — the S-R illegal-instruction
    // engine. The pin keeps the Task alive until task_put below.
    unsafe {
        // Look up target process via PID hash table
        let task_ptr = crate::process::pid_hash::pid_hash_lookup_pinned(pid);
        if task_ptr.is_null() {
            return Err(crate::errno::Errno::NoSuchProcess.as_neg_i32());
        }

        let result = send_signal_locked(task_ptr, pid, sig);

        // Release the pin taken by the lookup above.
        crate::process::task::Task::task_put(task_ptr);
        result
    }
}

/// Signal-sending core, operating on a PINNED task pointer.
///
/// SAFETY: `task_ptr` is pinned (task_refcnt held by the caller); it cannot
/// be freed for the duration of this call.
unsafe fn send_signal_locked(task_ptr: *mut crate::process::task::Task, _pid: u32, sig: i32) -> Result<(), i32> {
    send_signal_locked_info(task_ptr, sig, None)
}

/// Signal-sending core with optional siginfo (rt_sigqueueinfo family and
/// kernel-generated signals like SIGCHLD that carry a payload).
/// `si: Option<SigInfo>` — queued (RT: appended, standard: coalesced) so
/// SA_SIGINFO handlers and sigtimedwait readers see the real fields.
///
/// The mask check uses the TARGET TASK's per-thread sigmask (Linux
/// wants_signal): the shared SignalStruct's mask field is not per-thread,
/// and threads routinely run with different blocked sets.
///
/// SAFETY: `task_ptr` is pinned (task_refcnt held by the caller).
unsafe fn send_signal_locked_info(
    task_ptr: *mut crate::process::task::Task,
    sig: i32,
    si: Option<SigInfo>,
) -> Result<(), i32> {
    use crate::signal::Signal;

    let task = &*task_ptr;

    // Linux prepare_signal(): the global init process only receives
    // signals it has explicitly installed a handler for — every other
    // signal, INCLUDING SIGKILL/SIGSTOP, is silently discarded. Without
    // this, a broadcast/process-group kill escaping a test (LTP kill10's
    // signal flood between 10 forked pgrps) reaches PID 1, kills the
    // sweep runner, and powers the whole machine off mid-chunk.
    // Kernel-forced kills of a crashing init bypass this funnel (the trap
    // path calls do_exit directly), so a genuinely broken init still dies.
    if task.pid() == 1 && task.tgid() == 1 {
        let handled = task
            .signal
            .as_ref()
            .and_then(|s| s.get_action(sig))
            .map(|a| a.action() == SigActionKind::Handler)
            .unwrap_or(false);
        if !handled {
            return Ok(());
        }
    }

    // SIGKILL and SIGSTOP cannot be ignored
    if sig == Signal::SIGKILL as i32 || sig == Signal::SIGSTOP as i32 {
        if let Some(info) = si {
            task.pending.add_info(info);
        } else {
            task.pending.add(sig);
        }
        signal_wake_up(task_ptr);
        // P1 signalfd hook: wake signalfd readers of this task.
        crate::syscall::misc::signalfd_notify_task(task_ptr);
        return Ok(());
    }

    // Idle task has no signal handling
    let signal_ref: &SignalStruct = match task.signal.as_ref() {
        Some(s) => s,
        None => {
            task.pending.add(sig);
            signal_wake_up(task_ptr);
            // P1 signalfd hook: wake signalfd readers of this task.
            crate::syscall::misc::signalfd_notify_task(task_ptr);
            return Ok(());
        }
    };

    // Linux prepare_signal(): a SIG_IGN disposition discards the signal
    // BEFORE it is marked pending. The old add-then-remove left a
    // transient pending bit that wait_event_interruptible polls could
    // observe — the waiter then returned -ERESTARTSYS for a signal that
    // vanished before delivery, leaking the raw -512 sentinel to
    // userspace as a bogus errno.
    //
    // sig_task_ignored() exception: a TRACED task never ignores a signal
    // (Linux's ptrace check exempts only SIGKILL/SIGSTOP, which bypass
    // this path anyway) — the tracer must see every delivery as a
    // signal-delivery-stop, even one whose disposition is SIG_IGN (LTP
    // ptrace01/ptrace05: a TRACEME'd child SIG_IGN'ing a signal, or the
    // default-ignored SIGCHLD, still has to stop when the signal arrives).
    if let Some(action) = signal_ref.get_action(sig) {
        if action.action() == SigActionKind::Ignore && task.tracer_pid() == 0 {
            return Ok(());
        }
    }

    // Add signal to pending set BEFORE checking mask.
    // Masked signals stay pending and will be delivered when unmasked.
    if let Some(info) = si {
        task.pending.add_info(info);
    } else {
        task.pending.add(sig);
    }

    // Check if the signal is blocked FOR THIS THREAD — still pending,
    // just not delivered now.
    if sig >= 1 && sig <= 64 {
        let bit = 1u64 << (sig - 1);
        if task.sigmask & bit != 0 {
            // A blocked signal that just became pending must still WAKE
            // interruptible sleepers (Linux wakes sigtimedwait/sigwait
            // pollers; si block state only delays handler delivery).
            // Without this, a task that entered rt_sigtimedwait and THEN
            // received the (blocked) signal it waits for slept forever —
            // the LTP sigwait family hang (sigtimedwait01/sigwait01/
            // sigwaitinfo01/rt_sigtimedwait01). Waking is always safe:
            // every sleeper re-checks its own condition and re-sleeps if
            // unmet.
            signal_wake_up(task_ptr);
            // P1 signalfd hook: a BLOCKED signal is exactly the signalfd
            // use case (the app blocks the mask and reads via the fd) —
            // no handler path will run, so this wake is the only thing
            // that unblocks a read(sfd) on this signal.
            crate::syscall::misc::signalfd_notify_task(task_ptr);
            return Ok(());
        }
    }

    // Default or Handler disposition — wake the target so an
    // interruptible sleeper reaches the delivery point.
    signal_wake_up(task_ptr);
    // P1 signalfd hook: wake signalfd readers of this task (in-mask
    // signals are typically blocked, so they stay pending and are only
    // observable through the fd — this wake is what unblocks read(sfd)).
    crate::syscall::misc::signalfd_notify_task(task_ptr);
    Ok(())
}

/// Send a signal with a user-supplied si_code (rt_sigqueueinfo family).
///
/// Queues a full SigInfo for real-time signals (sigwaitinfo consumers)
/// and falls back to the plain bitmap for standard signals.
pub fn send_signal_info(pid: u32, sig: i32, si_code: i32) -> Result<(), i32> {
    if sig < 1 || sig > 64 {
        return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
    }
    let my_pid = crate::process::current_pid();
    let my_uid = match crate::sched::current() {
        Some(c) => (*c).cred().uid,
        None => 0,
    };
    let info = SigInfo::new(sig, si_code, my_pid, my_uid);
    send_signal_with_info(pid, info)
}

/// Send a signal carrying a fully-formed kernel SigInfo.
///
/// Used by kernel-originated signals whose payload matters to userspace
/// (SIGCHLD si_code/si_status from child exit is the canonical case:
/// SA_SIGINFO handlers — toybox timeout, glib's child reaper, systemd —
/// decode the child's outcome from these fields).
pub fn send_signal_with_info(pid: u32, info: SigInfo) -> Result<(), i32> {
    let sig = info.si_signo;
    if sig < 1 || sig > 64 {
        return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
    }
    // SAFETY: pinned lookup keeps the Task alive across the call.
    unsafe {
        let task_ptr = crate::process::pid_hash::pid_hash_lookup_pinned(pid);
        if task_ptr.is_null() {
            return Err(crate::errno::Errno::NoSuchProcess.as_neg_i32());
        }
        let result = send_signal_locked_info(task_ptr, sig, Some(info));
        crate::process::task::Task::task_put(task_ptr);
        result
    }
}

/// Send a signal to all processes in a given process group.
///
/// Used by the TTY ISIG handler to deliver SIGINT/SIGQUIT/SIGTSTP
/// to the foreground process group.
pub fn send_signal_to_pgid(pgid: u32, sig: i32) {
    // Broadcast over the PID hash so sleeping tasks receive it too; each
    // target still goes through the kill permission check (kernel-originated
    // signals from the TTY layer pass because init runs as root).
    crate::process::pid_hash::pid_hash_for_each_task(|task| unsafe {
        if (*task).pgid() == pgid && crate::security::can_send_signal((*task).cred()) {
            let _ = send_signal((*task).pid(), sig);
        }
    });
}

/// Park the CURRENT task while it is in group/ptrace stop.
///
/// The stop bookkeeping (STOPPED state, stop signal, parent/tracer
/// SIGCHLD) is done by the callers; this drives the actual sleep: Linux's
/// signal-stop paths call schedule() from the task's own context
/// (do_signal_stop/ptrace_stop), but Rux previously only set STOPPED +
/// need_resched and relied on the trap-return preemption check — which is
/// effectively disabled (ti_preempt_count is read with a straddling `ld`
/// that is almost always nonzero), so a "stopped" task returned to user
/// mode and kept running. SIGSTOP/ptrace stops were completely lost.
///
/// Loop discipline: any wake sets the state back to RUNNING
/// (wake_up_enqueue), which means "resumed" (SIGCONT / PTRACE_CONT /
/// DETACH) and breaks the park. A signal that arrives while stopped also
/// wakes the task; do_signal's re-select loop then runs its disposition
/// (fatal signals kill the stopped task, traced tasks re-stop with the
/// new signal — Linux signal-delivery-stop semantics).
///
/// # Safety
/// `task` must be the currently running task, in task (syscall/trap)
/// context with a valid kernel stack — schedule() switches away on it.
pub unsafe fn park_stopped_task(task: *mut crate::process::task::Task) {
    use crate::process::task::TaskState;

    // Sleeping with SIE=0 on this hart would starve the timer; every
    // blocking syscall re-enables IRQs before schedule() (see do_wait).
    crate::arch::riscv64::cpu::restore_irq(true);
    loop {
        crate::sched::schedule();
        if (*task).state().contains(TaskState::STOPPED) {
            // schedule() returned without a resume (preempt refusal /
            // next == prev fast path). Keep parking unless a deliverable
            // signal needs a disposition decision.
            // SAFETY: current task, plain field reads.
            if unsafe {
                (*task).pending.first_unmasked((*task).sigmask).is_none()
            } {
                continue;
            }
        }
        break;
    }
}

/// Check and process signals (called before kernel returns to user space)
///
/// # Arguments
///
/// * `regs` - PtRegs pointer, passed from trap.S
///
#[no_mangle]
pub extern "C" fn check_and_deliver_signals(regs: *mut crate::arch::riscv64::pt_regs::PtRegs) {
    use crate::sched;

    // ^C interrupt path (review批次8): deliver any ISIG character the UART
    // RX IRQ recorded. Must run even when no signal is pending YET — this
    // delivery is what CREATES the pending signal, and it lets ^C reach a
    // busy foreground task that is not blocked in read().
    crate::console::tty_isig_deliver_pending();

    // Ctrl-Alt-Del path: same task-context delivery rule. The input
    // detectors (UART RX IRQ, evdev) only latch an atomic; the SIGINT to
    // init (or the immediate-reboot cascade) runs here.
    crate::syscall::process::cad_deliver_pending();

    // SAFETY: regs is passed from trap handler; sched::current() returns the running task.
    unsafe {
        if regs.is_null() {
            return;
        }

        if let Some(current) = sched::current() {
            let pending = (*current).pending().get_all();
            // If there are pending signals, process them
            if pending != 0 {
                do_signal(regs);
            } else {
                // SMP lost-wait fix: no deliverable signal on this trip to
                // userspace — still reinstate a mask saved by rt_sigsuspend.
                // do_signal performs this restore on ITS paths, but it only
                // runs when something is pending; without this branch a
                // sigsuspend that returned EINTR while its triggering
                // signal was consumed elsewhere leaked the temporary
                // suspend mask PERMANENTLY (observed: dash's SIGCHLD left
                // blocked forever — `wait` wedged, whole system hung).
                // Mirrors Linux's restore_saved_sigmask() tail in
                // arch_do_signal_or_restart, which runs unconditionally.
                if (*current).sigmask_restore_valid {
                    (*current).sigmask = (*current).sigmask_restore;
                    (*current).sigmask_restore_valid = false;
                }
            }
        }
    }
}

// ============================================================================
// ============================================================================
// Signal Helper Functions
// ============================================================================

/// Check if current process has pending (unmasked) signals
///
///
/// This function checks for unmasked pending signals.
/// It considers the process signal mask (sigmask), only returning unblocked signals.
///
/// # Returns
/// * `true` - Has pending signals
/// * `false` - No pending signals
///
/// # Use Cases
/// - Check for `-EINTR` return in sleep syscalls
/// - Check for signal interrupt in `do_wait()`
/// - Check for signal arrival in any potentially blocking operation
///
/// # Example
/// ```no_run
/// # use rux::signal;
/// // Check for signals in sleep loop
/// loop {
///     if signal_pending() {
///         // Signal arrived, return EINTR
///         return -4_i64 as u64;  // EINTR
///     }
///     // Continue waiting...
/// }
/// ```
pub fn signal_pending() -> bool {
    use crate::sched;

    // SAFETY: sched::current() returns the running task's valid pointer.
    unsafe {
        if let Some(current) = sched::current() {
            // Get pending signals
            let pending_signals = (*current).pending.get_all();

            // If no pending signals, return false directly
            if pending_signals == 0 {
                return false;
            }

            // Check for unmasked signals
            // sigmask contains blocked signals
            let blocked_signals = (*current).sigmask;

            // If there are pending unmasked signals, return true
            (pending_signals & !blocked_signals) != 0
        } else {
            false
        }
    }
}

/// Wake up process and set state (for signal wakeup)
///
///
/// When signal arrives, need to wake up sleeping process to handle signal.
/// This function will:
/// 1. Wake up process from sleep state (set to Running)
/// 2. Set need_resched flag, trigger scheduling
///
/// # Arguments
/// * `task` - Task to wake up
/// * `state` - Original task state (for verifying if sleeping)
///
/// # Returns
/// * `true` - Successfully woke up
/// * `false` - Task not in sleep state or invalid pointer
///
/// # Use Cases
/// - Wake up target process after sending signal in `kill` syscall
/// - Wake up parent process to handle SIGCHLD in `do_exit()`
/// - Any scenario requiring asynchronous wake up of sleeping process
///
pub fn signal_wake_up_state(task: *mut crate::process::task::Task, _state: crate::process::task::TaskState) -> bool {
    if task.is_null() {
        return false;
    }

    // SAFETY: task is a valid Task pointer from the caller (e.g., signal delivery).
    unsafe {
        let task_state = (*task).state();

        // Wake sleeping tasks, and also STOPPED tasks (R7-3): a stopped
        // task was dequeued when it scheduled away, so SIGCONT/SIGKILL must
        // re-enqueue it — otherwise it can never be continued or killed
        // (job-control DoS: Ctrl+Z then kill -9 does nothing).
        //
        // R46: also route a RUNNING task that is on NO CPU (on_cpu == 0)
        // into Task::wake_up. Such a state is not producible by the
        // scheduler itself (off-CPU prev leaves __schedule either
        // RUNNING+linked or !RUNNING+unlinked) — it is the B-form phantom,
        // and without this it would be permanently unwakeable: every wake
        // of a "RUNNING" target was refused here. Task::wake_up's
        // GRQ-locked re-check authoritatively separates the genuine phantom
        // (healed + linked) from a queued/on-CPU RUNNING task (refused).
        // R49: the on_cpu == 0 condition was dropped — an ORPHANED pick
        // mark (RUNNING, unlinked, owned by no CPU's current slot) reads
        // on_cpu == 1 and is the same unwakeable phantom; only
        // wake_up_enqueue's locked current-slot scan can tell it from a
        // mark a CPU genuinely owns mid-pick. Route every RUNNING suspect;
        // the authoritative re-check refuses the legitimate ones.
        if task_state.is_sleeping()
            || task_state.contains(TaskState::STOPPED)
            || task_state == TaskState::new(TaskState::RUNNING)
        {
            // Use Task::wake_up which properly enqueues the task to its CPU's run queue
            crate::process::Task::wake_up(task);

            true
        } else {
            false
        }
    }
}

/// Wake up process (ignore state check)
///
///
/// This is a simplified version of `signal_wake_up_state()`, doesn't check task state.
///
/// # Arguments
/// * `task` - Task to wake up
///
/// # Returns
/// * `true` - Successfully woke up
/// * `false` - Invalid pointer
pub fn signal_wake_up(task: *mut crate::process::task::Task) -> bool {
    signal_wake_up_state(task, crate::process::task::TaskState::new(TaskState::INTERRUPTIBLE))
}

