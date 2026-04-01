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

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
pub struct SigFlags(u32);

impl SigFlags {
    pub const SA_NOCLDSTOP: u32 = 0x00000001;  // Don't send SIGCHLD when child stops
    pub const SA_NOCLDWAIT: u32 = 0x00000002;  // Don't create zombie on child exit
    pub const SA_SIGINFO: u32 = 0x00000004;    // Provide extra info
    pub const SA_ONSTACK: u32 = 0x08000000;    // Use alternate stack
    pub const SA_RESTART: u32 = 0x10000000;    // Restart system call
    pub const SA_NODEFER: u32 = 0x40000000;    // Don't block self during handler
    pub const SA_RESETHAND: u32 = 0x80000000;  // Reset to default after handling

    pub fn new(flags: u32) -> Self {
        Self(flags)
    }

    pub fn bits(&self) -> u32 {
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

/// Signal queue node
#[repr(C)]
pub struct SigQueueNode {
    /// Signal info
    pub info: SigInfo,
    /// Next node
    pub next: Option<Box<SigQueueNode>>,
}

/// Signal queue
///
/// Linked list implementation, supports real-time signal queuing
pub struct SigQueue {
    /// Queue head (for dequeue)
    head: AtomicUsize,
    /// Queue tail (for enqueue)
    tail: AtomicUsize,
}

/// Queue node pointer type (uses Box smart pointer)
type NodePtr = Option<Box<SigQueueNode>>;

unsafe impl Send for SigQueue {}
unsafe impl Sync for SigQueue {}

impl SigQueue {
    pub const fn new() -> Self {
        Self {
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire) == 0
    }

    /// Enqueue: Add signal info to queue tail
    ///
    /// Used for real-time signal queuing (standard signals don't queue, only keep latest)
    pub fn enqueue(&self, info: SigInfo) {
        // Use Box to allocate new node
        let new_node = Box::new(SigQueueNode {
            info,
            next: None,
        });

        let new_node_ptr = Box::leak(new_node) as *mut SigQueueNode as usize;

        // CAS loop: link new node to queue tail
        loop {
            let tail_ptr = self.tail.load(Ordering::Acquire);

            if tail_ptr == 0 {
                // Queue is empty, set both head and tail
                match self.tail.compare_exchange_weak(
                    0,
                    new_node_ptr,
                    Ordering::Release,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        self.head.store(new_node_ptr, Ordering::Release);
                        return;
                    }
                    Err(_) => continue,
                }
            } else {
                // Queue is not empty, link to tail node
                let tail_node = unsafe { &mut *(tail_ptr as *mut SigQueueNode) };
                if tail_node.next.is_none() {
                    // Try to set next pointer
                    tail_node.next = Some(unsafe {
                        Box::from_raw(new_node_ptr as *mut SigQueueNode)
                    });

                    // Update tail pointer
                    match self.tail.compare_exchange_weak(
                        tail_ptr,
                        new_node_ptr,
                        Ordering::Release,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => return,
                        Err(_) => {
                            // CAS failed, rollback next and retry
                            tail_node.next = None;
                            continue;
                        }
                    }
                } else {
                    // next already set, concurrent operation occurred, update tail and retry
                    self.tail.store(new_node_ptr, Ordering::Release);
                    continue;
                }
            }
        }
    }

    /// Dequeue: Remove signal info from queue head
    ///
    /// Returns signal info at head, or None if queue is empty
    pub fn dequeue(&self) -> Option<SigInfo> {
        loop {
            let head_ptr = self.head.load(Ordering::Acquire);

            if head_ptr == 0 {
                return None;
            }

            let head_node = unsafe { &*(head_ptr as *const SigQueueNode) };

            // Get next node pointer (get raw pointer from Box)
            let next_ptr = match &head_node.next {
                Some(next) => Some(next.as_ref() as *const SigQueueNode as usize),
                None => None,
            };

            let next = next_ptr.unwrap_or(0);

            // CAS update head pointer
            match self.head.compare_exchange_weak(
                head_ptr,
                next,
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    // If head and tail both point to same node, also need to update tail
                    if head_ptr == self.tail.load(Ordering::Acquire) {
                        self.tail.compare_exchange_weak(
                            head_ptr,
                            next,
                            Ordering::Release,
                            Ordering::Acquire,
                        );
                    }

                    // Free node memory and return signal info
                    let info = head_node.info;
                    unsafe {
                        let _ = Box::from_raw(head_ptr as *mut SigQueueNode);
                    }
                    return Some(info);
                }
                Err(_) => continue,
            }
        }
    }

    /// Peek at queue head signal info (without removing)
    pub fn peek(&self) -> Option<SigInfo> {
        let head_ptr = self.head.load(Ordering::Acquire);
        if head_ptr == 0 {
            return None;
        }
        let head_node = unsafe { &*(head_ptr as *const SigQueueNode) };
        Some(head_node.info)
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
        }
        // Standard signals only keep latest info, don't queue
    }

    /// Remove signal (from bitmap and queue)
    pub fn remove(&self, sig: i32) {
        if sig < 1 || sig > 64 {
            return;
        }

        let mask = 1u64 << (sig - 1);

        // If real-time signal and queue not empty, remove from queue
        if sig >= SIGRTMIN && !self.queue.is_empty() {
            // Try to remove signal from queue head
            while let Some(info) = self.queue.peek() {
                if info.si_signo == sig {
                    self.queue.dequeue();
                } else {
                    break;
                }
            }
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
}

/// Signal handling structure
///
#[repr(C)]
#[derive(Debug)]
pub struct SignalStruct {
    /// Action for each signal (64 signals)
    /// Use RwLock for interior mutability (needed for Arc sharing)
    action: spin::RwLock<[SigAction; 64]>,
    /// Signal mask
    pub mask: AtomicU64,
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
            action: spin::RwLock::new(actions),
            mask: AtomicU64::new(0),
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
}

impl Clone for SignalStruct {
    fn clone(&self) -> Self {
        // Read the actions and create a new RwLock with copied data
        let actions = self.action.read();
        Self {
            action: spin::RwLock::new(*actions),
            mask: AtomicU64::new(self.mask.load(Ordering::Acquire)),
        }
    }
}

/// Signal info structure
///
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct SigInfo {
    /// Signal number
    pub si_signo: i32,
    /// Signal code
    pub si_code: i32,
    /// Sending process PID
    pub si_pid: u32,
    /// Sending process UID
    pub si_uid: u32,
    /// Exit status or error value
    pub si_status: i32,
}

impl SigInfo {
    /// Create new signal info
    pub fn new(signo: i32, code: i32, pid: u32, uid: u32) -> Self {
        Self {
            si_signo: signo,
            si_code: code,
            si_pid: pid,
            si_uid: uid,
            si_status: 0,
        }
    }

    /// Create child process exit signal info
    pub fn child(pid: u32, uid: u32, status: i32) -> Self {
        Self {
            si_signo: Signal::SIGCHLD as i32,
            si_code: 1, // CLD_EXITED
            si_pid: pid,
            si_uid: uid,
            si_status: status,
        }
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
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct SigContext {
    /// General-purpose registers x1-x31 (ra, sp, gp, tp, t0-t6, s0-s11, a0-a7)
    pub regs: [u64; 31],
    /// Program counter (sepc)
    pub pc: u64,
    /// sstatus CSR
    pub status: u64,
}

impl SigContext {
    pub fn new() -> Self {
        Self::default()
    }
}

/// User context - register state saved during signal handling
///
/// RISC-V specific version
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct UContext {
    /// Signal mask
    pub uc_sigmask: u64,
    /// Reserved field
    pub uc_flags: u64,
    /// Link to next ucontext (for swapcontext)
    pub uc_link: u64,
    /// Signal stack
    pub uc_stack: SignalStack,
    /// Signal context (RISC-V registers)
    pub uc_mcontext: SigContext,
    /// Reserved space (alignment and future expansion)
    pub uc_reserved: [u64; 4],
}

impl UContext {
    /// Create new user context
    pub fn new() -> Self {
        Self {
            uc_sigmask: 0,
            uc_flags: 0,
            uc_link: 0,
            uc_stack: SignalStack::new(),
            uc_mcontext: SigContext::new(),
            uc_reserved: [0; 4],
        }
    }
}

/// Signal stack - alternate signal handling stack
///
/// Used for sigaltstack syscall
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct SignalStack {
    /// Stack start address
    pub ss_sp: u64,
    /// Stack size
    pub ss_size: u64,
    /// Stack flags
    pub ss_flags: u32,
}

impl SignalStack {
    /// Create new signal stack
    pub fn new() -> Self {
        Self {
            ss_sp: 0,
            ss_size: 0,
            ss_flags: 0,
        }
    }

    /// Check if disabled
    pub fn is_disabled(&self) -> bool {
        (self.ss_flags & crate::signal::ss_flags::SS_DISABLE) != 0
    }

    /// Check if on stack
    pub fn is_on_stack(&self) -> bool {
        (self.ss_flags & crate::signal::ss_flags::SS_ONSTACK) != 0
    }
}

/// Signal stack flags
pub mod ss_flags {
    /// Disable signal stack
    pub const SS_DISABLE: u32 = 0x00000001;
    /// Signal handler is using this stack
    pub const SS_ONSTACK: u32 = 0x00000002;
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

    unsafe {
        let current = match sched::current() {
            Some(c) => c,
            None => return false,
        };

        // Check for pending signals (respecting signal mask)
        let blocked = (*current).sigmask;
        let sig = match (*current).pending.first_unmasked(blocked) {
            Some(s) => s,
            None => return false,
        };

        // Get signal handling action (clone needed data)
        let action = (*current).signal.as_ref()
            .and_then(|s| s.get_action(sig));

        // Handle signal
        if let Some(action) = action {
            // Check if has custom handler
            if action.has_handler() {
                // Call signal handler
                if !setup_frame(current, sig, &action, regs) {
                    // Setup failed, execute default action
                    handle_default_signal(sig);
                }
            } else {
                // Execute default action
                handle_default_signal(sig);
            }
        }

        // Remove signal from pending queue
        (*current).pending.remove(sig);

        // If process is set to ZOMBIE or STOPPED, set need_resched flag
        // The actual schedule() call happens in trap.S when returning to user mode
        let task_state = (*current).state();
        if task_state.is_dead() || task_state.contains(TaskState::STOPPED) {
            // Set need_resched flag - schedule() will be called in trap.S
            crate::sched::set_need_resched();
        }

        true
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
unsafe fn setup_frame(
    task: *mut crate::process::task::Task,
    sig: i32,
    action: &SigAction,
    regs: *mut crate::arch::riscv64::pt_regs::PtRegs,
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

    // Create signal frame
    let mut frame = SignalFrame {
        reserved: [0; 4],
        info: SigInfo::new(sig, crate::signal::si_code::SI_KERNEL, (*task).pid(), 0),
        uc: UContext::new(),
        trampoline: [
            0x93, 0x08, 0x8b, 0x00,  // li a7, 139 (rt_sigreturn)
            0x73, 0x00, 0x00, 0x00,  // ecall
        ],
    };

    // Save current PtRegs to signal frame (for sigreturn restore)
    // RISC-V SigContext saves x1-x31 and pc
    // regs[0] = ra (x1), regs[1] = sp (x2), ... regs[30] = t6 (x31)

    // Save registers from PtRegs to sigcontext
    frame.uc.uc_mcontext.regs[0] = regs.ra;   // x1 (ra)
    frame.uc.uc_mcontext.regs[1] = regs.sp;   // x2 (sp)
    frame.uc.uc_mcontext.regs[2] = regs.gp;   // x3 (gp)
    frame.uc.uc_mcontext.regs[3] = regs.tp;   // x4 (tp)
    frame.uc.uc_mcontext.regs[4] = regs.t0;   // x5 (t0)
    frame.uc.uc_mcontext.regs[5] = regs.t1;   // x6 (t1)
    frame.uc.uc_mcontext.regs[6] = regs.t2;   // x7 (t2)
    frame.uc.uc_mcontext.regs[7] = regs.s0;   // x8 (s0/fp)
    frame.uc.uc_mcontext.regs[8] = regs.s1;   // x9 (s1)
    frame.uc.uc_mcontext.regs[9] = regs.a0;   // x10 (a0)
    frame.uc.uc_mcontext.regs[10] = regs.a1;  // x11 (a1)
    frame.uc.uc_mcontext.regs[11] = regs.a2;  // x12 (a2)
    frame.uc.uc_mcontext.regs[12] = regs.a3;  // x13 (a3)
    frame.uc.uc_mcontext.regs[13] = regs.a4;  // x14 (a4)
    frame.uc.uc_mcontext.regs[14] = regs.a5;  // x15 (a5)
    frame.uc.uc_mcontext.regs[15] = regs.a6;  // x16 (a6)
    frame.uc.uc_mcontext.regs[16] = regs.a7;  // x17 (a7)
    frame.uc.uc_mcontext.regs[17] = regs.s2;  // x18 (s2)
    frame.uc.uc_mcontext.regs[18] = regs.s3;  // x19 (s3)
    frame.uc.uc_mcontext.regs[19] = regs.s4;  // x20 (s4)
    frame.uc.uc_mcontext.regs[20] = regs.s5;  // x21 (s5)
    frame.uc.uc_mcontext.regs[21] = regs.s6;  // x22 (s6)
    frame.uc.uc_mcontext.regs[22] = regs.s7;  // x23 (s7)
    frame.uc.uc_mcontext.regs[23] = regs.s8;  // x24 (s8)
    frame.uc.uc_mcontext.regs[24] = regs.s9;  // x25 (s9)
    frame.uc.uc_mcontext.regs[25] = regs.s10; // x26 (s10)
    frame.uc.uc_mcontext.regs[26] = regs.s11; // x27 (s11)
    frame.uc.uc_mcontext.regs[27] = regs.t3;  // x28 (t3)
    frame.uc.uc_mcontext.regs[28] = regs.t4;  // x29 (t4)
    frame.uc.uc_mcontext.regs[29] = regs.t5;  // x30 (t5)
    frame.uc.uc_mcontext.regs[30] = regs.t6;  // x31 (t6)

    // Save PC
    frame.uc.uc_mcontext.pc = regs.epc;

    // SA_RESTART / EINTR handling:
    // If SA_RESTART is not set, set saved a0 to -EINTR so the syscall
    // returns EINTR when rt_sigreturn restores the context.
    // If SA_RESTART is set, the saved epc already points to the ecall
    // instruction, so the syscall will automatically restart.
    if (action.sa_flags.bits() & SigFlags::SA_RESTART) == 0 {
        frame.uc.uc_mcontext.regs[9] = (-(crate::errno::constants::EINTR as i64)) as u64;
    }

    // Save sstatus
    frame.uc.uc_mcontext.status = regs.status;

    // Save signal mask
    frame.uc.uc_sigmask = (*task).sigmask;

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
    // int sigaction_handler(int sig, siginfo_t *info, void *uc)
    regs.a0 = sig as u64;                      // a0 = sig
    regs.a1 = frame_addr + 32;                 // a1 = &info
    regs.a2 = frame_addr + 32 + core::mem::size_of::<SigInfo>() as u64;  // a2 = &uc

    // Set return address to signal handler
    regs.epc = action.sa_handler as u64;

    // Set user stack pointer to signal frame position
    regs.sp = frame_addr;

    // Set return address to trampoline (for rt_sigreturn)
    // ra points to trampoline code
    let trampoline_addr = frame_addr + core::mem::size_of::<SignalFrame>() as u64 - 8;
    regs.ra = trampoline_addr;

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

    // Get signal frame from kernel space backup
    let frame = match (*task).sigframe {
        Some(f) => f,
        None => return false,
    };

    let regs = &mut *regs;

    // Restore registers from signal frame's uc_mcontext (RISC-V)
    // SigContext.regs saves x1-x31

    // Restore all general-purpose registers
    regs.ra = frame.uc.uc_mcontext.regs[0];   // x1 (ra)
    regs.sp = frame.uc.uc_mcontext.regs[1];   // x2 (sp)
    regs.gp = frame.uc.uc_mcontext.regs[2];   // x3 (gp)
    regs.tp = frame.uc.uc_mcontext.regs[3];   // x4 (tp)
    regs.t0 = frame.uc.uc_mcontext.regs[4];   // x5 (t0)
    regs.t1 = frame.uc.uc_mcontext.regs[5];   // x6 (t1)
    regs.t2 = frame.uc.uc_mcontext.regs[6];   // x7 (t2)
    regs.s0 = frame.uc.uc_mcontext.regs[7];   // x8 (s0/fp)
    regs.s1 = frame.uc.uc_mcontext.regs[8];   // x9 (s1)
    regs.a0 = frame.uc.uc_mcontext.regs[9];   // x10 (a0)
    regs.a1 = frame.uc.uc_mcontext.regs[10];  // x11 (a1)
    regs.a2 = frame.uc.uc_mcontext.regs[11];  // x12 (a2)
    regs.a3 = frame.uc.uc_mcontext.regs[12];  // x13 (a3)
    regs.a4 = frame.uc.uc_mcontext.regs[13];  // x14 (a4)
    regs.a5 = frame.uc.uc_mcontext.regs[14];  // x15 (a5)
    regs.a6 = frame.uc.uc_mcontext.regs[15];  // x16 (a6)
    regs.a7 = frame.uc.uc_mcontext.regs[16];  // x17 (a7)
    regs.s2 = frame.uc.uc_mcontext.regs[17];  // x18 (s2)
    regs.s3 = frame.uc.uc_mcontext.regs[18];  // x19 (s3)
    regs.s4 = frame.uc.uc_mcontext.regs[19];  // x20 (s4)
    regs.s5 = frame.uc.uc_mcontext.regs[20];  // x21 (s5)
    regs.s6 = frame.uc.uc_mcontext.regs[21];  // x22 (s6)
    regs.s7 = frame.uc.uc_mcontext.regs[22];  // x23 (s7)
    regs.s8 = frame.uc.uc_mcontext.regs[23];  // x24 (s8)
    regs.s9 = frame.uc.uc_mcontext.regs[24];  // x25 (s9)
    regs.s10 = frame.uc.uc_mcontext.regs[25]; // x26 (s10)
    regs.s11 = frame.uc.uc_mcontext.regs[26]; // x27 (s11)
    regs.t3 = frame.uc.uc_mcontext.regs[27];  // x28 (t3)
    regs.t4 = frame.uc.uc_mcontext.regs[28];  // x29 (t4)
    regs.t5 = frame.uc.uc_mcontext.regs[29];  // x30 (t5)
    regs.t6 = frame.uc.uc_mcontext.regs[30];  // x31 (t6)

    // Restore PC (program counter) - return to position before signal interrupt
    regs.epc = frame.uc.uc_mcontext.pc;

    // Restore sstatus
    regs.status = frame.uc.uc_mcontext.status;

    // Restore signal mask
    (*task).sigmask = frame.uc.uc_sigmask;

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
    /// uc_sigmask(8) + uc_flags(8) + uc_link(8) + uc_stack(24)
    pub const MCONTEXT_OFFSET: usize = 8 + 8 + 8 + core::mem::size_of::<super::SignalStack>();
}

/// Handle default signal action
///
fn handle_default_signal(sig: i32) {
    use crate::sched;

    match sig {
        // Ignore these signals
        17 | 18 | 21 | 22 => {
            // SIGCHLD, SIGCONT, SIGTTIN, SIGTTOU - ignore
            // SIGCHLD: child process status changed, default ignore
            // SIGCONT: continue stopped process, ignore if process not stopped
            // SIGTTIN, SIGTTOU: background terminal I/O, default ignore
        }
        // Stop process
        19 | 20 => {
            // SIGSTOP, SIGTSTP - stop process
            unsafe {
                if let Some(current) = sched::current() {
                    (*current).set_stop_signal(sig);
                    (*current).set_state(TaskState::new(TaskState::STOPPED));
                    // Notify parent
                    if let Some(parent_ptr) = (*current).parent_ptr() {
                        let parent = parent_ptr as *mut crate::process::task::Task;
                        let _ = crate::signal::send_signal((*parent).pid(), Signal::SIGCHLD as i32);
                        crate::signal::signal_wake_up(parent);
                    }
                    // Set need_resched flag
                    sched::set_need_resched();
                }
            }
        }
        // Terminate process (core dump or direct termination)
        1 | 2 | 3 | 4 | 5 | 6   // SIGHUP | SIGINT | SIGQUIT | SIGILL | SIGTRAP | SIGABRT
        | 7 | 8 | 9 | 11 | 13 | 14 | 15  // SIGBUS | SIGFPE | SIGKILL | SIGSEGV | SIGPIPE | SIGALRM | SIGTERM
        | 16 | 10 | 12 => {          // SIGSTKFLT | SIGUSR1 | SIGUSR2
            // Call do_exit to properly terminate process
            // This releases mm, fdtable, kernel stack, removes from run queue, etc.
            // Store negative signal number (do_wait encodes as waitpid status)
            crate::process::exit::do_exit(-(sig as i32));
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
pub fn send_signal(pid: u32, sig: i32) -> Result<(), i32> {
    use crate::signal::Signal;
    use crate::config::MAX_CPUS;
    use crate::config::MAX_TASKS;

    // Check if signal number is valid
    if sig < 1 || sig > 64 {
        return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
    }

    unsafe {
        // Traverse all CPU run queues to find target process
        for cpu_id in 0..MAX_CPUS {
            if let Some(rq) = crate::sched::cpu_rq(cpu_id) {
                let rq_inner = rq.lock();

                for i in 0..MAX_TASKS {
                    let task_ptr = rq_inner.tasks[i];
                    if task_ptr.is_null() {
                        continue;
                    }

                    let task = &*task_ptr;

                    // Check if PID matches
                    if task.pid() != pid {
                        continue;
                    }

                    // SIGKILL and SIGSTOP cannot be ignored
                    if sig == Signal::SIGKILL as i32 || sig == Signal::SIGSTOP as i32 {
                        task.pending.add(sig);
                        drop(rq_inner);
                        signal_wake_up(task_ptr);
                        return Ok(());
                    }

                    // Idle task has no signal handling
                    let signal_ref: &SignalStruct = match task.signal.as_ref() {
                        Some(s) => s,
                        None => {
                            task.pending.add(sig);
                            drop(rq_inner);
                            signal_wake_up(task_ptr);
                            return Ok(());
                        }
                    };

                    // Check if signal is masked
                    if signal_ref.is_masked(sig) {
                        return Err(crate::errno::Errno::TryAgain.as_neg_i32());
                    }

                    // Check signal handling action
                    if let Some(action) = signal_ref.get_action(sig) {
                        match action.action() {
                            SigActionKind::Ignore => {
                                return Ok(());
                            }
                            SigActionKind::Default => {
                                task.pending.add(sig);
                                drop(rq_inner);
                                signal_wake_up(task_ptr);
                                return Ok(());
                            }
                            SigActionKind::Handler => {
                                task.pending.add(sig);
                                drop(rq_inner);
                                signal_wake_up(task_ptr);
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }

        // Process not found
        Err(crate::errno::Errno::NoSuchProcess.as_neg_i32())
    }
}

/// Send a signal to all processes in a given process group.
///
/// Used by the TTY ISIG handler to deliver SIGINT/SIGQUIT/SIGTSTP
/// to the foreground process group.
pub fn send_signal_to_pgid(pgid: u32, sig: i32) {
    crate::sched::for_each_task(|task| unsafe {
        if (*task).pgid() == pgid {
            let _ = send_signal((*task).pid(), sig);
        }
    });
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

    unsafe {
        if regs.is_null() {
            return;
        }

        if let Some(current) = sched::current() {
            let pending = (*current).pending().get_all();
            // If there are pending signals, process them
            if pending != 0 {
                do_signal(regs);
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

    unsafe {
        let task_state = (*task).state();

        // Only need to wake up if in sleep state
        if task_state.is_sleeping() {
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

