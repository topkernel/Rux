//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Task Control Block

use core::sync::atomic::{AtomicU32, Ordering};
use core::ptr;
use crate::mm::pagemap::AddressSpace;
use crate::fs::FdTable;
use crate::signal::{SignalStruct, SigPending};
use crate::config::TIME_SLICE_TICKS as DEFAULT_TIME_SLICE;
use alloc::boxed::Box;
use alloc::alloc::{alloc, dealloc};
use core::alloc::Layout;
use core::mem::offset_of;
use crate::list::ListHead;

/// Kernel stack size - from config
///
/// RISC-V typically uses 16KB kernel stack, but we increase to 32KB
/// because some operations (like FdTable creation) need larger stack space
const KERNEL_STACK_SIZE: usize = crate::config::KERNEL_STACK_SIZE;

// ==================== Stack Cache Implementation ====================

/// Global spinlock for stack cache synchronization
static STACK_CACHE_LOCK: spin::Mutex<()> = spin::Mutex::new(());

/// Free stack entries (linked list)
///
/// Each entry is stack_bottom, we store next pointer at stack_bottom
/// This works because stack_bottom is unused (stack grows down from top)
struct FreeStackEntry {
    next: *mut FreeStackEntry,
}

/// Stack cache structure
struct StackCache {
    /// Number of cached stacks
    count: usize,
    /// Head of free stack list
    head: *mut FreeStackEntry,
}

impl StackCache {
    const fn new() -> Self {
        Self {
            count: 0,
            head: core::ptr::null_mut(),
        }
    }

    /// Maximum number of cached stacks
    const MAX_CACHED_STACKS: usize = 64;

    /// Pop a stack from cache
    fn pop(&mut self) -> Option<*mut u8> {
        if self.head.is_null() {
            return None;
        }
        unsafe {
            let entry = self.head;
            self.head = (*entry).next;
            self.count -= 1;
            Some(entry as *mut u8)
        }
    }

    /// Push a stack to cache
    fn push(&mut self, stack_bottom: *mut u8) {
        if self.count >= Self::MAX_CACHED_STACKS {
            // Cache full, free the stack instead
            unsafe {
                let layout = Layout::from_size_align(KERNEL_STACK_SIZE, 16)
                    .unwrap_or_else(|_| Layout::new::<[u8; KERNEL_STACK_SIZE]>());
                dealloc(stack_bottom, layout);
            }
            return;
        }
        unsafe {
            let entry = stack_bottom as *mut FreeStackEntry;
            (*entry).next = self.head;
            self.head = entry;
            self.count += 1;
        }
    }
}

/// Global stack cache instance
static mut STACK_CACHE: StackCache = StackCache::new();

/// Allocate a kernel stack (with caching)
fn stack_cache_alloc() -> *mut u8 {
    let _guard = STACK_CACHE_LOCK.lock();
    unsafe {
        if let Some(bottom) = STACK_CACHE.pop() {
            // Zero stack before reuse
            core::ptr::write_bytes(bottom, 0, KERNEL_STACK_SIZE);
            return bottom;
        }
    }

    // Cache empty, allocate new
    unsafe {
        let layout = Layout::from_size_align(KERNEL_STACK_SIZE, 16)
            .ok()
            .unwrap_or_else(|| Layout::new::<[u8; KERNEL_STACK_SIZE]>());
        alloc(layout)
    }
}

/// Free a kernel stack (with caching)
fn stack_cache_free(stack_bottom: *mut u8) {
    let _guard = STACK_CACHE_LOCK.lock();
    unsafe {
        STACK_CACHE.push(stack_bottom);
    }
}

/// Process state flags (bitmap form)
///
/// Bitmap representation for process states, allowing combined states
/// e.g.: TASK_UNINTERRUPTIBLE | __TASK_STOPPED
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskState(u32);

impl TaskState {
    /// Runnable state (TASK_RUNNING)
    /// Process is running on CPU or waiting in run queue
    pub const RUNNING: u32 = 0x00000000;

    /// Interruptible sleep (TASK_INTERRUPTIBLE)
    /// Process is waiting for an event, can be woken by signal
    pub const INTERRUPTIBLE: u32 = 0x00000001;

    /// Uninterruptible sleep (TASK_UNINTERRUPTIBLE)
    /// Process is waiting for an event, cannot be woken by signal
    pub const UNINTERRUPTIBLE: u32 = 0x00000002;

    /// Stopped state (__TASK_STOPPED)
    /// Process stopped by signal (SIGSTOP, SIGTSTP, etc.)
    pub const STOPPED: u32 = 0x00000004;

    /// Traced state (__TASK_TRACED)
    /// Process is being traced by ptrace
    pub const TRACED: u32 = 0x00000008;

    /// Exit zombie (EXIT_ZOMBIE)
    /// Process has exited but parent hasn't waited yet
    pub const ZOMBIE: u32 = 0x00000010;

    /// Exit dead (EXIT_DEAD)
    /// Process final state, will be reclaimed
    pub const DEAD: u32 = 0x00000020;

    /// Create new state
    #[inline]
    pub const fn new(bits: u32) -> Self {
        TaskState(bits)
    }

    /// Get bit value
    #[inline]
    pub fn bits(&self) -> u32 {
        self.0
    }

    /// Check if contains specified flag
    #[inline]
    pub fn contains(&self, flag: u32) -> bool {
        (self.0 & flag) != 0
    }

    /// Check if running
    #[inline]
    pub fn is_running(&self) -> bool {
        self.0 == Self::RUNNING
    }

    /// Check if sleeping (interruptible or uninterruptible)
    #[inline]
    pub fn is_sleeping(&self) -> bool {
        self.contains(Self::INTERRUPTIBLE) || self.contains(Self::UNINTERRUPTIBLE)
    }

    /// Check if exited (zombie or dead)
    #[inline]
    pub fn is_dead(&self) -> bool {
        self.contains(Self::ZOMBIE) || self.contains(Self::DEAD)
    }

    /// Check if can be woken by signal
    #[inline]
    pub fn is_interruptible(&self) -> bool {
        self.contains(Self::INTERRUPTIBLE)
    }
}

impl Default for TaskState {
    fn default() -> Self {
        TaskState::new(TaskState::RUNNING)
    }
}

///
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SchedPolicy {
    /// Normal time-sharing scheduling (SCHED_NORMAL)
    Normal = 0,

    /// FIFO real-time scheduling (SCHED_FIFO)
    Fifo = 1,

    /// RR real-time scheduling (SCHED_RR)
    Rr = 2,

    /// Batch scheduling (SCHED_BATCH)
    Batch = 3,

    /// Idle scheduling (SCHED_IDLE)
    Idle = 5,

    /// Deadline scheduling (SCHED_DEADLINE)
    Deadline = 6,
}

/// Task flags (task flags)
///
pub mod task_flags {
    use bitflags::bitflags;

    bitflags! {
        pub struct TaskFlags: u32 {
            const PF_KTHREAD     = 0x00200000; /* I am a kernel thread */
            const PF_EXITING     = 0x00000004; /* Getting shut down */
            const PF_VCPU        = 0x00000010; /* I'm a virtual CPU */
            const PF_WQ_WORKER   = 0x00000020; /* I'm a workqueue worker */
        }
    }
}

/// Process identifier (PID type)
///
pub type Pid = u32;

// ==================== thread_info style flags ====================

/// TIF_SIGPENDING - has pending signal
pub const TIF_SIGPENDING: u32 = 0;
/// TIF_NEED_RESCHED - needs rescheduling
pub const TIF_NEED_RESCHED: u32 = 1;
/// TIF_NOTIFY_RESUME - notify before returning to user mode
pub const TIF_NOTIFY_RESUME: u32 = 2;
/// TIF_UPROBE - uprobe pending
pub const TIF_UPROBE: u32 = 3;
/// TIF_MEMDIE - exiting (out of memory)
pub const TIF_MEMDIE: u32 = 4;

/// Task Control Block
///
///
/// Core field correspondence:
/// - state: task_struct::state
/// - pid: task_struct::pid
/// - tgid: task_struct::tgid (thread group ID)
/// - prio: task_struct::prio (dynamic priority)
/// - static_prio: task_struct::static_prio (static priority)
/// - normal_prio: task_struct::normal_prio
/// - policy: task_struct::policy
/// - context: cpu_context
/// - mm: task_struct::mm (memory descriptor)
/// - files: task_struct::files (file descriptor table)
/// - signal: task_struct::signal (signal handling)
///
/// Compatibility design:
/// - thread_info style fields at struct beginning
/// - tp register points to Task struct
/// - Kernel stack managed via kernel_sp field
#[repr(C)]
pub struct Task {
    // ==================== thread_info style fields (offset 0) ====================
    // These fields must be at struct beginning for fast access via tp

    /// Process flags (thread_info.flags)
    /// Bit definitions: TIF_SIGPENDING, TIF_NEED_RESCHED, etc.
    ti_flags: AtomicU32,

    /// Preemption count (thread_info.preempt_count)
    /// > 0 means preemption disabled
    ti_preempt_count: core::sync::atomic::AtomicI32,

    /// Kernel stack pointer (thread_info.kernel_sp)
    /// Points to top of kernel stack
    ti_kernel_sp: core::sync::atomic::AtomicU64,

    /// User stack pointer (thread_info.user_sp)
    /// Saves user mode sp, used for trap return
    ti_user_sp: core::sync::atomic::AtomicU64,

    /// Which CPU running on (thread_info.cpu)
    ti_cpu: core::sync::atomic::AtomicI32,

    // ==================== task_struct fields ====================

    /// Process state (volatile, visible across cores)
    state: AtomicU32,

    /// Process ID
    pid: Pid,

    /// Thread group ID (main process PID of thread)
    /// Single-threaded process: tgid == pid
    tgid: Pid,

    /// Scheduling policy
    policy: SchedPolicy,

    /// Dynamic priority (0-139, higher value means lower priority)
    /// - 0-99: Real-time processes
    /// - 100-139: Normal processes
    prio: i32,

    /// Static priority (120 is default for normal processes)
    static_prio: i32,

    /// normal_prio: priority calculated from static_prio and scheduling policy
    normal_prio: i32,

    /// Remaining time slice
    time_slice: u32,

    /// CFS scheduling entity
    ///
    /// Contains vruntime, weight and other CFS scheduling info
    sched_entity: crate::sched::fair::SchedEntity,

    /// RT scheduling priority (0-99, lower value = higher priority)
    ///
    /// Only valid when policy is SCHED_FIFO or SCHED_RR
    rt_priority: u32,

    /// RT run list node (for linking into RT priority queues)
    ///
    /// Used by the RT scheduler to link tasks at the same priority level
    pub rt_run_list: crate::list::ListHead,

    /// RT scheduling entity
    ///
    /// Contains time slice and other RT-specific scheduling info
    rt_entity: crate::sched::rt::SchedRtEntity,

    /// Deadline scheduling entity
    ///
    /// Contains deadline, runtime, period and other DL-specific info
    dl_entity: crate::sched::deadline::SchedDlEntity,

    /// Kernel stack
    /// TODO: Implement kernel stack allocation
    kernel_stack: Option<*mut u8>,

    /// Kernel stack bottom (for overflow detection)
    kernel_stack_bottom: usize,

    /// Fork child flag
    /// If true, this is a child process created by fork, needs to restore from ret_from_fork
    is_fork_child: core::sync::atomic::AtomicBool,

    /// Fork child's PtRegs pointer
    /// When is_fork_child is true, this pointer points to child's PtRegs
    /// Scheduler will use this PtRegs to restore child state
    fork_pt_regs: core::sync::atomic::AtomicU64,

    /// Address space (mm_struct)
    /// None for kernel threads, Some for user processes
    /// Use Arc for CLONE_VM sharing between threads
    address_space: Option<alloc::sync::Arc<AddressSpace>>,

    /// Active address space (active_mm)
    ///
    /// For user processes: active_mm == mm
    /// For kernel threads: active_mm is borrowed address space (for accessing user memory)
    active_mm: Option<*const AddressSpace>,

    /// Architecture-specific thread state
    ///
    /// Stores FPU state, TLS pointer, etc.
    thread: crate::arch::riscv64::thread::ThreadStruct,

    /// File descriptor table (files_struct)
    /// Use Arc for CLONE_FILES sharing between threads
    fdtable: Option<alloc::sync::Arc<FdTable>>,

    /// Signal handling structure (signal_struct)
    /// Use Arc for CLONE_SIGHAND sharing between threads
    pub signal: Option<alloc::sync::Arc<SignalStruct>>,

    /// Pending signals (pending)
    pub pending: SigPending,

    /// Signal mask (blocked)
    ///
    /// Used for sigprocmask syscall
    pub sigmask: u64,

    /// Signal stack (sigaltstack)
    pub sigstack: crate::signal::SignalStack,

    /// Signal frame address (in user space)
    pub sigframe_addr: u64,

    /// Signal frame (kernel space backup)
    pub sigframe: Option<crate::signal::SignalFrame>,

    /// Parent process
    parent: Option<*const Task>,

    /// Exit code (valid in Zombie state)
    exit_code: i32,

    /// Children list
    ///
    /// This is a list head, all children are linked here via their sibling field
    pub children: ListHead,

    /// Sibling process list node
    ///
    /// Used to link this process to parent's children list
    pub sibling: ListHead,

    /// Parent's children list head pointer (for next_sibling boundary check)
    ///
    /// When process is added to parent, saves address of parent's children
    /// Used by next_sibling() to detect list end
    parent_children_head: *mut ListHead,

    /// User space address to clear child TID (set_tid_address)
    ///
    /// When process exits, kernel clears value at this address
    /// Used for pthread thread synchronization
    clear_child_tid: *mut i32,

    /// Robust futex list head (set_robust_list)
    ///
    /// Used for robust mutex implementation
    robust_list_head: *const u8,
    robust_list_len: usize,

    /// Wait queue for child process exit events (Linux: signal->wait_chldexit)
    ///
    /// Parent processes wait on this queue when calling wait4/waitpid
    /// Child process exit wakes up parent via this queue
    pub wait_chldexit: crate::process::wait::WaitQueueHead,

    /// Process heap boundary (brk)
    ///
    /// Points to end address of process heap, managed by sys_brk
    /// Initial value is 0, set to default on first brk call
    brk: core::sync::atomic::AtomicU64,

    /// Filesystem information (cwd, root, umask)
    ///
    /// Shared between threads when CLONE_FS is used
    fs: Option<alloc::sync::Arc<crate::fs::FsStruct>>,

    /// Executable file path
    ///
    /// Stores process's executable file path
    /// Used for /proc/self/exe etc.
    exe_path: alloc::boxed::Box<[u8]>,
}

impl Task {
    /// Create new task
    ///
    pub fn new(pid: Pid, policy: SchedPolicy) -> Self {
        // PRIO_TO_PRIO: static_prio 120 -> prio 120
        let static_prio = 120; // DEFAULT_PRIO
        let normal_prio = static_prio; // For SCHED_NORMAL, normal_prio == static_prio
        let prio = normal_prio;

        // Idle task doesn't need file descriptor table and signal handling
        // Temporarily disable FdTable and Signal creation to avoid heap allocation issues
        let (fdtable, signal) = (None, None);

        let state = AtomicU32::new(TaskState::RUNNING);
        let pending = SigPending::new();
        let sigstack = crate::signal::SignalStack::new();

        let mut task = Self {
            // thread_info style fields
            ti_flags: AtomicU32::new(0),
            ti_preempt_count: core::sync::atomic::AtomicI32::new(0),
            ti_kernel_sp: core::sync::atomic::AtomicU64::new(0),
            ti_user_sp: core::sync::atomic::AtomicU64::new(0),
            ti_cpu: core::sync::atomic::AtomicI32::new(-1),

            // task_struct fields
            state,
            pid,
            tgid: pid, // Single-threaded process tgid == pid
            policy,
            prio,
            static_prio,
            normal_prio,
            time_slice: DEFAULT_TIME_SLICE, // Default time slice (10 clock ticks = 100ms)
            sched_entity: crate::sched::fair::SchedEntity::new(),
            rt_priority: 0,
            rt_run_list: ListHead::new(),
            rt_entity: crate::sched::rt::SchedRtEntity::new(),
            dl_entity: crate::sched::deadline::SchedDlEntity::new(),
            kernel_stack: None,
            kernel_stack_bottom: 0,
            is_fork_child: core::sync::atomic::AtomicBool::new(false),
            fork_pt_regs: core::sync::atomic::AtomicU64::new(0),
            address_space: None,
            active_mm: None,
            thread: crate::arch::riscv64::thread::ThreadStruct::new(),
            fdtable,
            signal,
            pending,
            sigmask: 0,  // Initial signal mask is empty
            sigstack,
            sigframe_addr: 0,
            sigframe: None,
            parent: None,
            exit_code: 0,
            children: ListHead::new(),
            sibling: ListHead::new(),
            parent_children_head: ptr::null_mut(),
            clear_child_tid: ptr::null_mut(),
            robust_list_head: ptr::null(),
            robust_list_len: 0,
            wait_chldexit: crate::process::wait::WaitQueueHead::new(),
            brk: core::sync::atomic::AtomicU64::new(0),
            fs: Some(alloc::sync::Arc::new(crate::fs::FsStruct::new())),
            exe_path: Box::from(&b""[..]),
        };

        // Initialize children and sibling lists (must be after struct construction)
        task.children.init();
        task.sibling.init();
        task.rt_run_list.init();

        task
    }

    /// Construct idle task at specified memory location
    ///
    /// This function avoids creating large objects on stack, directly constructs Task at given address
    ///
    /// # Safety
    ///
    /// ptr must be aligned and point to a large enough memory block
    pub unsafe fn new_idle_at(ptr: *mut Task) {
        use core::ptr;
        use core::mem::offset_of;

        // Initialize thread_info style fields (must be at beginning)
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_flags)) as *mut AtomicU32,
            AtomicU32::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_preempt_count)) as *mut core::sync::atomic::AtomicI32,
            core::sync::atomic::AtomicI32::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_kernel_sp)) as *mut core::sync::atomic::AtomicU64,
            core::sync::atomic::AtomicU64::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_user_sp)) as *mut core::sync::atomic::AtomicU64,
            core::sync::atomic::AtomicU64::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_cpu)) as *mut core::sync::atomic::AtomicI32,
            core::sync::atomic::AtomicI32::new(-1),
        );

        // Use ptr::write and offset_of to safely initialize each field
        ptr::write(
            (ptr as usize + offset_of!(Task, state)) as *mut AtomicU32,
            AtomicU32::new(TaskState::RUNNING),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, pid)) as *mut Pid,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, tgid)) as *mut Pid,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, policy)) as *mut SchedPolicy,
            SchedPolicy::Idle,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, prio)) as *mut i32,
            120,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, static_prio)) as *mut i32,
            120,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, normal_prio)) as *mut i32,
            120,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, time_slice)) as *mut u32,
            100,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sched_entity)) as *mut crate::sched::fair::SchedEntity,
            crate::sched::fair::SchedEntity::new(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, rt_priority)) as *mut u32,
            0,
        );
        let rt_run_list_ptr = (ptr as usize + offset_of!(Task, rt_run_list)) as *mut ListHead;
        (*rt_run_list_ptr).init();
        ptr::write(
            (ptr as usize + offset_of!(Task, rt_entity)) as *mut crate::sched::rt::SchedRtEntity,
            crate::sched::rt::SchedRtEntity::new(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, dl_entity)) as *mut crate::sched::deadline::SchedDlEntity,
            crate::sched::deadline::SchedDlEntity::new(),
        );

        // Initialize idle task thread struct
        // Idle task uses thread.sp for its kernel stack
        fn idle_loop_wrapper() -> ! {
            loop {
                unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
            }
        }
        ptr::write(
            (ptr as usize + offset_of!(Task, thread)) as *mut crate::arch::riscv64::thread::ThreadStruct,
            {
                let mut thread = crate::arch::riscv64::thread::ThreadStruct::new();
                thread.ra = idle_loop_wrapper as u64;  // Return address = idle loop
                thread.sp = 0;  // Will be set when kernel stack is allocated
                thread
            },
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, kernel_stack)) as *mut Option<*mut u8>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, is_fork_child)) as *mut core::sync::atomic::AtomicBool,
            core::sync::atomic::AtomicBool::new(false),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, fork_pt_regs)) as *mut core::sync::atomic::AtomicU64,
            core::sync::atomic::AtomicU64::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, address_space)) as *mut Option<Box<AddressSpace>>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, active_mm)) as *mut Option<*const AddressSpace>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, thread)) as *mut crate::arch::riscv64::thread::ThreadStruct,
            crate::arch::riscv64::thread::ThreadStruct::new(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, fdtable)) as *mut Option<Box<FdTable>>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, signal)) as *mut Option<Box<SignalStruct>>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, pending)) as *mut SigPending,
            SigPending::new(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigmask)) as *mut u64,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigstack)) as *mut crate::signal::SignalStack,
            crate::signal::SignalStack::new(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigframe_addr)) as *mut u64,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigframe)) as *mut Option<crate::signal::SignalFrame>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, parent)) as *mut Option<*mut Task>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, exit_code)) as *mut i32,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, parent_children_head)) as *mut *mut ListHead,
            ptr::null_mut(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, clear_child_tid)) as *mut *mut i32,
            ptr::null_mut(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, robust_list_head)) as *mut *const u8,
            ptr::null(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, robust_list_len)) as *mut usize,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, fs)) as *mut Option<alloc::sync::Arc<crate::fs::FsStruct>>,
            Some(alloc::sync::Arc::new(crate::fs::FsStruct::new())),
        );

        // Initialize children and sibling lists
        let children_ptr = (ptr as usize + offset_of!(Task, children)) as *mut ListHead;
        (*children_ptr).init();
        let sibling_ptr = (ptr as usize + offset_of!(Task, sibling)) as *mut ListHead;
        (*sibling_ptr).init();
    }

    /// Construct normal task at specified memory location
    ///
    /// This function avoids creating large objects on stack, directly constructs Task at given address
    ///
    /// # Safety
    ///
    /// ptr must be aligned and point to a large enough memory block
    pub unsafe fn new_task_at(ptr: *mut Task, pid: Pid, policy: SchedPolicy) {
        use crate::console::putchar;
        use core::ptr;
        use core::mem::offset_of;

        let static_prio = 120; // DEFAULT_PRIO
        let normal_prio = static_prio;
        let prio = normal_prio;

        // Initialize thread_info style fields (must be at beginning)
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_flags)) as *mut AtomicU32,
            AtomicU32::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_preempt_count)) as *mut core::sync::atomic::AtomicI32,
            core::sync::atomic::AtomicI32::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_kernel_sp)) as *mut core::sync::atomic::AtomicU64,
            core::sync::atomic::AtomicU64::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_user_sp)) as *mut core::sync::atomic::AtomicU64,
            core::sync::atomic::AtomicU64::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_cpu)) as *mut core::sync::atomic::AtomicI32,
            core::sync::atomic::AtomicI32::new(-1),
        );

        // Write each field
        ptr::write(
            (ptr as usize + offset_of!(Task, state)) as *mut AtomicU32,
            AtomicU32::new(TaskState::RUNNING),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, pid)) as *mut Pid,
            pid,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, tgid)) as *mut Pid,
            pid,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, policy)) as *mut SchedPolicy,
            policy,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, prio)) as *mut i32,
            prio,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, static_prio)) as *mut i32,
            static_prio,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, normal_prio)) as *mut i32,
            normal_prio,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, time_slice)) as *mut u32,
            HZ,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sched_entity)) as *mut crate::sched::fair::SchedEntity,
            crate::sched::fair::SchedEntity::new(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, rt_priority)) as *mut u32,
            0,
        );
        let rt_run_list_ptr = (ptr as usize + offset_of!(Task, rt_run_list)) as *mut ListHead;
        (*rt_run_list_ptr).init();
        ptr::write(
            (ptr as usize + offset_of!(Task, rt_entity)) as *mut crate::sched::rt::SchedRtEntity,
            crate::sched::rt::SchedRtEntity::new(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, dl_entity)) as *mut crate::sched::deadline::SchedDlEntity,
            crate::sched::deadline::SchedDlEntity::new(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, kernel_stack)) as *mut Option<*mut u8>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, is_fork_child)) as *mut core::sync::atomic::AtomicBool,
            core::sync::atomic::AtomicBool::new(false),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, fork_pt_regs)) as *mut core::sync::atomic::AtomicU64,
            core::sync::atomic::AtomicU64::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, address_space)) as *mut Option<Box<AddressSpace>>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, active_mm)) as *mut Option<*const AddressSpace>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, thread)) as *mut crate::arch::riscv64::thread::ThreadStruct,
            crate::arch::riscv64::thread::ThreadStruct::new(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, fdtable)) as *mut Option<Box<FdTable>>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, signal)) as *mut Option<Box<SignalStruct>>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, pending)) as *mut SigPending,
            SigPending::new(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigmask)) as *mut u64,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigstack)) as *mut crate::signal::SignalStack,
            crate::signal::SignalStack::new(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigframe_addr)) as *mut u64,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigframe)) as *mut Option<crate::signal::SignalFrame>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, parent)) as *mut Option<*mut Task>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, exit_code)) as *mut i32,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, parent_children_head)) as *mut *mut ListHead,
            ptr::null_mut(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, clear_child_tid)) as *mut *mut i32,
            ptr::null_mut(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, robust_list_head)) as *mut *const u8,
            ptr::null(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, robust_list_len)) as *mut usize,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, brk)) as *mut core::sync::atomic::AtomicU64,
            core::sync::atomic::AtomicU64::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, fs)) as *mut Option<alloc::sync::Arc<crate::fs::FsStruct>>,
            Some(alloc::sync::Arc::new(crate::fs::FsStruct::new())),
        );

        // Initialize children and sibling lists
        let children_ptr = (ptr as usize + offset_of!(Task, children)) as *mut ListHead;
        (*children_ptr).init();
        let sibling_ptr = (ptr as usize + offset_of!(Task, sibling)) as *mut ListHead;
        (*sibling_ptr).init();

        // Allocate kernel stack
        let task_ref = &mut *ptr;
        if task_ref.alloc_kernel_stack().is_none() {
            const MSG_ERR: &[u8] = b"Task::new_task_at: failed to allocate kernel stack\n";
            for &b in MSG_ERR {
                putchar(b);
            }
        }
    }

    /// Get process state
    #[inline]
    pub fn state(&self) -> TaskState {
        TaskState::new(self.state.load(Ordering::Relaxed))
    }

    /// Set process state
    #[inline]
    pub fn set_state(&self, state: TaskState) {
        self.state.store(state.bits(), Ordering::Release);
    }

    /// Check if process is in specified state
    #[inline]
    pub fn is_state(&self, flag: u32) -> bool {
        self.state.load(Ordering::Relaxed) & flag != 0
    }

    /// Process sleep and wake mechanism

    /// Put current process to sleep
    ///
    /// After calling this function, process enters sleep state and triggers scheduling
    ///
    /// # Arguments
    /// - `state`: Sleep state (TaskState::INTERRUPTIBLE or TaskState::UNINTERRUPTIBLE)
    ///
    /// # Safety
    /// After calling this function, current process will be scheduled out until woken
    ///
    /// # Example
    /// ```no_run
    /// # use rux::process::task::TaskState;
    /// // Interruptible sleep (can be woken by signal)
    /// Task::sleep(TaskState::new(TaskState::INTERRUPTIBLE));
    ///
    /// // Uninterruptible sleep
    /// Task::sleep(TaskState::new(TaskState::UNINTERRUPTIBLE));
    /// ```
    #[inline(never)]
    pub fn sleep(state: TaskState) {
        // Set current process to sleep state
        if let Some(current) = crate::sched::current() {
            unsafe {
                (*current).set_state(state);
            }
        }

        // Release kernel lock (must release before sleep, otherwise other processes can't acquire)
        crate::sync::kernel_lock_release();

        // Trigger scheduling, select other process to run
        crate::sched::schedule();

        // Re-acquire kernel lock after wakeup (continue syscall execution)
        crate::sync::kernel_lock_acquire();
    }

    /// Wake up process
    ///
    ///
    /// Wake process from sleep state, making it schedulable again
    ///
    /// # Arguments
    /// - `task`: Process to wake
    ///
    /// # Returns
    /// - true: Successfully woken
    /// - false: Process not in sleep state
    ///
    /// # Example
    /// ```no_run
    /// # use rux::sched;
    /// if let Some(child) = sched::find_task_by_pid(2) {
    ///     sched::wake_up_process(child);
    /// }
    /// ```
    #[inline(never)]
    pub fn wake_up(task: *mut Task) -> bool {
        if task.is_null() {
            return false;
        }

        unsafe {
            let old_state = (*task).state();

            // Only wake if in sleep state
            if old_state.is_sleeping() {
                // Wake process: set to RUNNING state
                (*task).set_state(TaskState::new(TaskState::RUNNING));

                // Add process to run queue (critical!)
                crate::sched::enqueue_task(&mut *task);

                // Set need_resched flag, trigger rescheduling
                crate::sched::set_need_resched();

                true
            } else {
                false
            }
        }
    }

    /// Get PID
    #[inline]
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Preemptive scheduling support

    /// Decrement time slice
    ///
    ///
    /// # Returns
    /// - true: Time slice remaining
    /// - false: Time slice exhausted
    #[inline]
    pub fn tick_time_slice(&mut self) -> bool {
        if self.time_slice > 0 {
            self.time_slice -= 1;
            true
        } else {
            false
        }
    }

    /// Reset time slice
    ///
    /// Called when process is rescheduled to CPU
    #[inline]
    pub fn reset_time_slice(&mut self) {
        self.time_slice = DEFAULT_TIME_SLICE;
    }

    /// Check if time slice exhausted
    #[inline]
    pub fn time_slice_expired(&self) -> bool {
        self.time_slice == 0
    }

    /// Get remaining time slice
    #[inline]
    pub fn get_time_slice(&self) -> u32 {
        self.time_slice
    }

    /// Set time slice
    #[inline]
    pub fn set_time_slice(&mut self, slice: u32) {
        self.time_slice = slice;
    }

    /// End of preemptive scheduling support

    // ==================== CFS scheduling support ====================

    /// Get CFS scheduling entity
    #[inline]
    pub fn sched_entity(&self) -> &crate::sched::fair::SchedEntity {
        &self.sched_entity
    }

    /// Get CFS scheduling entity (mutable reference)
    #[inline]
    pub fn sched_entity_mut(&mut self) -> &mut crate::sched::fair::SchedEntity {
        &mut self.sched_entity
    }

    /// Get scheduling policy
    #[inline]
    pub fn policy(&self) -> SchedPolicy {
        self.policy
    }

    /// Set scheduling policy
    #[inline]
    pub fn set_policy(&mut self, policy: SchedPolicy) {
        self.policy = policy;
    }

    /// Get static priority
    #[inline]
    pub fn static_prio(&self) -> i32 {
        self.static_prio
    }

    /// Get dynamic priority
    #[inline]
    pub fn prio(&self) -> i32 {
        self.prio
    }

    /// Set dynamic priority
    #[inline]
    pub fn set_prio(&mut self, prio: i32) {
        self.prio = prio;
    }

    /// Get RT priority (0-99, lower value = higher priority)
    #[inline]
    pub fn rt_priority(&self) -> u32 {
        self.rt_priority
    }

    /// Set RT priority
    #[inline]
    pub fn set_rt_priority(&mut self, prio: u32) {
        self.rt_priority = prio.min(99);
    }

    /// Get RT scheduling entity
    #[inline]
    pub fn rt_entity(&self) -> &crate::sched::rt::SchedRtEntity {
        &self.rt_entity
    }

    /// Get RT scheduling entity (mutable reference)
    #[inline]
    pub fn rt_entity_mut(&mut self) -> &mut crate::sched::rt::SchedRtEntity {
        &mut self.rt_entity
    }

    /// Get Deadline scheduling entity
    #[inline]
    pub fn dl_entity(&self) -> &crate::sched::deadline::SchedDlEntity {
        &self.dl_entity
    }

    /// Get Deadline scheduling entity (mutable reference)
    #[inline]
    pub fn dl_entity_mut(&mut self) -> &mut crate::sched::deadline::SchedDlEntity {
        &mut self.dl_entity
    }

    /// Get nice value
    ///
    /// Nice value range: -20 to +19
    /// Calculated from static_prio: nice = static_prio - 120
    #[inline]
    pub fn nice(&self) -> i32 {
        self.static_prio - 120
    }

    /// Set nice value
    ///
    /// Also updates static_prio and scheduling entity weight
    pub fn set_nice(&mut self, nice: i32) {
        // Nice value range: -20 to +19
        let nice = nice.clamp(-20, 19);

        // Update static_prio
        self.static_prio = nice + 120;
        self.normal_prio = self.static_prio;
        self.prio = self.normal_prio;

        // Update scheduling entity weight
        self.sched_entity.set_nice(nice);
    }

    // ==================== Process tree management ====================

    /// Get parent process PID (PPID)
    #[inline]
    pub fn ppid(&self) -> Pid {
        match self.parent {
            Some(parent_ptr) => unsafe { (*parent_ptr).pid },
            None => 0, // No parent process, return 0
        }
    }

    /// Check if fork child process
    #[inline]
    pub fn is_fork_child(&self) -> bool {
        self.is_fork_child.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Set as fork child process
    #[inline]
    pub fn set_fork_child(&self, pt_regs_ptr: *const crate::arch::riscv64::pt_regs::PtRegs) {
        self.is_fork_child.store(true, core::sync::atomic::Ordering::Relaxed);
        self.fork_pt_regs.store(pt_regs_ptr as u64, core::sync::atomic::Ordering::Relaxed);
    }

    /// Get fork child's PtRegs pointer
    #[inline]
    pub fn fork_pt_regs(&self) -> *const crate::arch::riscv64::pt_regs::PtRegs {
        self.fork_pt_regs.load(core::sync::atomic::Ordering::Relaxed) as *const crate::arch::riscv64::pt_regs::PtRegs
    }

    /// Clear fork child flag
    /// Called after child is first scheduled and starts executing
    #[inline]
    pub fn clear_fork_child(&self) {
        self.is_fork_child.store(false, core::sync::atomic::Ordering::Relaxed);
        self.fork_pt_regs.store(0, core::sync::atomic::Ordering::Relaxed);
    }

    /// Get TGID
    #[inline]
    pub fn tgid(&self) -> Pid {
        self.tgid
    }

    /// Set TGID (for CLONE_THREAD)
    #[inline]
    pub fn set_tgid(&mut self, tgid: Pid) {
        self.tgid = tgid;
    }

    /// Get mutable reference to address space
    /// Note: AddressSpace has interior mutability, so &self is sufficient
    pub fn address_space_mut(&mut self) -> Option<&AddressSpace> {
        self.address_space.as_ref().map(|arc| arc.as_ref())
    }

    /// Get reference to address space
    pub fn address_space(&self) -> Option<&AddressSpace> {
        self.address_space.as_ref().map(|arc| arc.as_ref())
    }

    /// Set address space
    pub fn set_address_space(&mut self, addr_space: Option<alloc::sync::Arc<AddressSpace>>) {
        // Update active_mm pointer
        if let Some(ref aspace) = addr_space {
            self.active_mm = Some(aspace.as_ref() as *const AddressSpace);
        } else {
            self.active_mm = None;
        }
        self.address_space = addr_space;
    }

    /// Get Arc reference to address space (for CLONE_VM)
    pub fn address_space_arc(&self) -> Option<alloc::sync::Arc<AddressSpace>> {
        self.address_space.clone()
    }

    /// Get active address space (for kernel threads, this is borrowed address space)
    pub fn active_mm(&self) -> Option<&AddressSpace> {
        if let Some(ref aspace) = self.address_space {
            Some(aspace.as_ref())
        } else if let Some(mm_ptr) = self.active_mm {
            unsafe { Some(&*mm_ptr) }
        } else {
            None
        }
    }

    /// Clear active_mm (used by exit_mm during process exit)
    pub fn clear_active_mm(&mut self) {
        self.active_mm = None;
    }

    /// Get architecture-specific thread state
    pub fn thread(&self) -> &crate::arch::riscv64::thread::ThreadStruct {
        &self.thread
    }

    /// Get mutable reference to architecture-specific thread state
    pub fn thread_mut(&mut self) -> &mut crate::arch::riscv64::thread::ThreadStruct {
        &mut self.thread
    }

    // ==================== thread_info style accessors ====================

    /// Get thread_info flags
    #[inline]
    pub fn ti_flags(&self) -> u32 {
        self.ti_flags.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Set thread_info flags
    #[inline]
    pub fn set_ti_flags(&self, flags: u32) {
        self.ti_flags.store(flags, core::sync::atomic::Ordering::Release);
    }

    /// Test thread_info flag bit
    #[inline]
    pub fn test_ti_flag(&self, flag: u32) -> bool {
        (self.ti_flags.load(core::sync::atomic::Ordering::Relaxed) & flag) != 0
    }

    /// Set thread_info flag bit
    #[inline]
    pub fn set_ti_flag(&self, flag: u32) {
        self.ti_flags.fetch_or(flag, core::sync::atomic::Ordering::Release);
    }

    /// Clear thread_info flag bit
    #[inline]
    pub fn clear_ti_flag(&self, flag: u32) {
        self.ti_flags.fetch_and(!flag, core::sync::atomic::Ordering::Release);
    }

    /// Check if needs rescheduling
    #[inline]
    pub fn need_resched(&self) -> bool {
        self.test_ti_flag(TIF_NEED_RESCHED)
    }

    /// Set need rescheduling flag
    #[inline]
    pub fn set_need_resched_flag(&self) {
        self.set_ti_flag(TIF_NEED_RESCHED);
    }

    /// Clear need rescheduling flag
    #[inline]
    pub fn clear_need_resched_flag(&self) {
        self.clear_ti_flag(TIF_NEED_RESCHED);
    }

    /// Check if has pending signal
    #[inline]
    pub fn has_pending_signal(&self) -> bool {
        self.test_ti_flag(TIF_SIGPENDING)
    }

    /// Set pending signal flag
    #[inline]
    pub fn set_pending_signal_flag(&self) {
        self.set_ti_flag(TIF_SIGPENDING);
    }

    /// Get preempt count
    #[inline]
    pub fn preempt_count(&self) -> i32 {
        self.ti_preempt_count.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Increment preempt count
    #[inline]
    pub fn inc_preempt_count(&self) {
        self.ti_preempt_count.fetch_add(1, core::sync::atomic::Ordering::Release);
    }

    /// Decrement preempt count
    #[inline]
    pub fn dec_preempt_count(&self) {
        self.ti_preempt_count.fetch_sub(1, core::sync::atomic::Ordering::Release);
    }

    /// Check if preemptible
    #[inline]
    pub fn preemptible(&self) -> bool {
        self.preempt_count() == 0
    }

    /// Get kernel stack pointer (thread_info.kernel_sp)
    #[inline]
    pub fn ti_kernel_sp(&self) -> u64 {
        self.ti_kernel_sp.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Set kernel stack pointer (thread_info.kernel_sp)
    #[inline]
    pub fn set_ti_kernel_sp(&self, sp: u64) {
        self.ti_kernel_sp.store(sp, core::sync::atomic::Ordering::Release);
    }

    /// Get user stack pointer (thread_info.user_sp)
    #[inline]
    pub fn ti_user_sp(&self) -> u64 {
        self.ti_user_sp.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Set user stack pointer (thread_info.user_sp)
    #[inline]
    pub fn set_ti_user_sp(&self, sp: u64) {
        self.ti_user_sp.store(sp, core::sync::atomic::Ordering::Release);
    }

    /// Get running CPU (thread_info.cpu)
    #[inline]
    pub fn ti_cpu(&self) -> i32 {
        self.ti_cpu.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Set running CPU (thread_info.cpu)
    #[inline]
    pub fn set_ti_cpu(&self, cpu: i32) {
        self.ti_cpu.store(cpu, core::sync::atomic::Ordering::Release);
    }

    // ==================== Kernel stack management ====================

    /// Allocate kernel stack
    ///
    ///
    /// Allocates a kernel stack for current task, size is KERNEL_STACK_SIZE (16KB)
    ///
    /// # Returns
    /// Some(stack top address) on success, None on failure
    pub fn alloc_kernel_stack(&mut self) -> Option<*mut u8> {
        // Use stack cache for allocation
        let stack_ptr = stack_cache_alloc();

        if !stack_ptr.is_null() {
            // Set stack top address (stack grows downward)
            let stack_top = unsafe { stack_ptr.add(KERNEL_STACK_SIZE) };
            self.kernel_stack = Some(stack_top);

            // Save stack bottom for overflow detection
            self.kernel_stack_bottom = stack_ptr as usize;

            // Also set ti_kernel_sp
            self.set_ti_kernel_sp(stack_top as u64);

            Some(stack_top)
        } else {
            None
        }
    }

    /// Free kernel stack
    ///
    ///
    /// Free current task's kernel stack (returns to cache)
    pub fn free_kernel_stack(&mut self) {
        if let Some(stack_top) = self.kernel_stack {
            // Calculate stack bottom address (stack top - stack size)
            let stack_bottom = unsafe { stack_top.sub(KERNEL_STACK_SIZE) };

            // Return to cache instead of deallocating
            stack_cache_free(stack_bottom);

            // Clear reference
            self.kernel_stack = None;
            self.kernel_stack_bottom = 0;
            // Clear ti_kernel_sp
            self.set_ti_kernel_sp(0);
        }
    }

    /// Get kernel stack top address
    ///
    /// Used to set SP register during context switch
    pub fn get_kernel_stack(&self) -> Option<*mut u8> {
        self.kernel_stack
    }

    /// Get kernel stack bottom address (for overflow detection)
    #[inline]
    pub fn kernel_stack_bottom(&self) -> usize {
        self.kernel_stack_bottom
    }

    /// Get kernel stack top address (highest address + 1)
    #[inline]
    pub fn kernel_stack_top(&self) -> usize {
        self.kernel_stack_bottom + KERNEL_STACK_SIZE
    }

    /// Check if address is within kernel stack range (for overflow detection)
    ///
    /// # Arguments
    /// - `addr`: Address to check
    ///
    /// # Returns
    /// true if address is within kernel stack range
    #[inline]
    pub fn is_in_kernel_stack(&self, addr: usize) -> bool {
        if self.kernel_stack_bottom == 0{
            return false;
        }
        let stack_top = self.kernel_stack_bottom + KERNEL_STACK_SIZE;
        addr >= self.kernel_stack_bottom && addr < stack_top
    }

    /// Check if stack pointer indicates potential overflow
    ///
    /// # Arguments
    /// - `sp`: Stack pointer value to check
    ///
    /// # Returns
    /// true if SP is below stack bottom (overflow detected)
    #[inline]
    pub fn is_stack_overflow(&self, sp: usize) -> bool {
        if self.kernel_stack_bottom == 0 {
            return false;  // No stack allocated yet
        }
        sp < self.kernel_stack_bottom
    }

    /// Get pt_regs pointer at top of kernel stack (Linux-style)
    ///
    /// Linux: task_pt_regs(task) = (struct pt_regs *)(task->stack + THREAD_SIZE - sizeof(struct pt_regs))
    ///
    /// In Rux, kernel_stack stores the stack top address, so:
    /// pt_regs = kernel_stack - sizeof(PtRegs)
    ///
    /// # Returns
    /// Pointer to pt_regs structure at top of kernel stack, or null if no stack
    #[inline]
    pub fn pt_regs(&self) -> *mut super::super::arch::riscv64::pt_regs::PtRegs {
        if let Some(stack_top) = self.kernel_stack {
            let pt_regs_addr = stack_top as usize - core::mem::size_of::<super::super::arch::riscv64::pt_regs::PtRegs>();
            pt_regs_addr as *mut super::super::arch::riscv64::pt_regs::PtRegs
        } else {
            core::ptr::null_mut()
        }
    }

    /// Has address space (user process)
    #[inline]
    pub fn has_address_space(&self) -> bool {
        self.address_space.is_some()
    }

    /// Check if has file descriptor table
    #[inline]
    pub fn has_fdtable(&self) -> bool {
        self.fdtable.is_some()
    }

    /// Get file descriptor table (Option version)
    #[inline]
    pub fn try_fdtable(&self) -> Option<&FdTable> {
        self.fdtable.as_ref().map(|arc| arc.as_ref())
    }

    /// Get file descriptor table
    #[inline]
    pub fn fdtable(&self) -> &FdTable {
        self.fdtable.as_ref().expect("FdTable not initialized")
    }

    /// Get mutable reference to file descriptor table (Option version)
    /// Note: FdTable has interior mutability, &self is sufficient
    #[inline]
    pub fn try_fdtable_mut(&self) -> Option<&FdTable> {
        self.fdtable.as_ref().map(|arc| arc.as_ref())
    }

    /// Get mutable reference to file descriptor table
    /// Note: FdTable has interior mutability, so &FdTable is sufficient for mutation
    #[inline]
    pub fn fdtable_mut(&self) -> &FdTable {
        self.fdtable.as_ref().expect("FdTable not initialized")
    }

    /// Set file descriptor table
    #[inline]
    pub fn set_fdtable(&mut self, fdtable: Option<alloc::sync::Arc<FdTable>>) {
        self.fdtable = fdtable;
    }

    /// Get Arc reference to fdtable (for CLONE_FILES)
    pub fn fdtable_arc(&self) -> Option<alloc::sync::Arc<FdTable>> {
        self.fdtable.clone()
    }

    /// Get Arc reference to signal struct (for CLONE_SIGHAND)
    pub fn signal_arc(&self) -> Option<alloc::sync::Arc<SignalStruct>> {
        self.signal.clone()
    }

    /// Set signal struct
    pub fn set_signal(&mut self, signal: Option<alloc::sync::Arc<SignalStruct>>) {
        self.signal = signal;
    }

    /// Set parent process
    pub fn set_parent(&mut self, parent: *const Task) {
        if parent.is_null() {
            self.parent = None;
        } else {
            self.parent = Some(parent);
        }
    }

    /// Get parent process pointer
    #[inline]
    pub fn parent_ptr(&self) -> Option<*const Task> {
        self.parent
    }

    /// Get exit code
    #[inline]
    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    /// Set exit code
    #[inline]
    pub fn set_exit_code(&mut self, code: i32) {
        self.exit_code = code;
    }

    // ==================== Process Tree Management ====================

    /// Get first child
    ///
    ///
    /// # Returns
    /// Some(child pointer) if has children, None otherwise
    pub fn first_child(&self) -> Option<*mut Task> {
        unsafe {
            // children list may be empty
            if self.children.is_empty() {
                return None;
            }

            // Get first sibling node from children list head
            // Then use list_entry to get Task struct containing that sibling
            let first_sibling = self.children.next;
            // Calculate Task struct pointer containing that sibling
            // sibling field is at end of Task struct
            let task_ptr = (first_sibling as usize - offset_of!(Task, sibling)) as *mut Task;
            Some(task_ptr)
        }
    }

    /// Get next sibling
    ///
    ///
    /// # Safety
    /// Caller must ensure self is not parent's children list head
    ///
    /// # Returns
    /// Some(pointer) if has next sibling, None otherwise
    pub unsafe fn next_sibling(&self) -> Option<*mut Task> {
        // If parent's children list head not saved, not in any parent's children list
        if self.parent_children_head.is_null() {
            return None;
        }

        let next_sibling = self.sibling.next;

        // If next points to parent's children list head, reached list end
        if next_sibling == self.parent_children_head {
            return None;
        }

        // Calculate Task struct pointer containing that sibling
        let task_ptr = (next_sibling as usize - offset_of!(Task, sibling)) as *mut Task;
        Some(task_ptr)
    }

    /// Check if has children
    ///
    /// # Returns
    /// true if has children, false otherwise
    pub fn has_children(&self) -> bool {
        !self.children.is_empty()
    }

    /// Add child to process tree
    ///
    ///
    /// # Safety
    /// Caller must ensure:
    /// - self is valid parent process reference
    /// - child is valid child process pointer
    /// - child is not in any process tree
    ///
    /// # Arguments
    /// - `child`: Child process pointer to add
    pub unsafe fn add_child(&self, child: *mut Task) {
        // Set child's parent
        (*child).parent = Some(self as *const _ as *mut Task);

        // Save parent's children list head pointer (for next_sibling boundary check)
        (*child).parent_children_head = &self.children as *const _ as *mut ListHead;

        // Link child's sibling to parent's children list
        // Use add_tail to add to list end
        (*child).sibling.add_tail(&self.children as *const _ as *mut ListHead);
    }

    /// Remove child from process tree
    ///
    ///
    /// # Safety
    /// Caller must ensure:
    /// - child is valid child process pointer
    /// - child is in current process's children list
    ///
    /// # Arguments
    /// - `child`: Child process pointer to remove
    pub unsafe fn remove_child(&self, child: *mut Task) {
        // Remove child's sibling from parent's children list
        (*child).sibling.del();

        // Reinitialize sibling list (prevent dangling pointer)
        (*child).sibling.init();

        // Clear parent pointer
        (*child).parent = None;

        // Clear parent children list head pointer
        (*child).parent_children_head = ptr::null_mut();
    }

    /// Iterate over all children
    ///
    ///
    /// # Arguments
    /// - `f`: Closure to call for each child
    ///
    /// # Safety
    /// Caller must ensure self is valid and process tree is not modified during iteration
    pub unsafe fn for_each_child<F>(&self, mut f: F)
    where
        F: FnMut(*mut Task),
    {
        let head = &self.children as *const _ as *mut ListHead;
        let mut iterations = 0usize;
        ListHead::for_each(head, |node| {
            iterations += 1;
            if iterations > 1000 {
                // Prevent infinite loop
                return;
            }
            let task_ptr = (node as usize - offset_of!(Task, sibling)) as *mut Task;
            f(task_ptr);
        });
    }

    /// Find child by PID
    ///
    /// # Arguments
    /// - `pid`: Process ID to find
    ///
    /// # Returns
    /// Some(child pointer) if found, None otherwise
    ///
    /// # Safety
    /// Caller must ensure self is valid
    pub unsafe fn find_child_by_pid(&self, pid: Pid) -> Option<*mut Task> {
        let head = &self.children as *const _ as *mut ListHead;
        let mut result = None;
        let mut iterations = 0usize;
        ListHead::for_each(head, |node| {
            iterations += 1;
            if iterations > 1000 {
                // Prevent infinite loop
                return;
            }
            let task_ptr = (node as usize - offset_of!(Task, sibling)) as *mut Task;
            if (*task_ptr).pid == pid {
                result = Some(task_ptr);
            }
        });
        result
    }

    /// Get child count
    ///
    /// # Returns
    /// Number of children
    ///
    /// # Safety
    /// Caller must ensure self is valid
    pub unsafe fn count_children(&self) -> usize {
        let head = &self.children as *const _ as *mut ListHead;
        let mut count = 0;
        ListHead::for_each(head, |_| {
            count += 1;
        });
        count
    }


    /// Get reference to pending signal queue
    #[inline]
    pub fn pending(&self) -> &crate::signal::SigPending {
        &self.pending
    }

    // ==================== musl libc support (set_tid_address, set_robust_list) ====================

    /// Set clear_child_tid address
    ///
    /// When process exits, kernel clears value at this address
    #[inline]
    pub fn set_clear_child_tid(&mut self, tidptr: *mut i32) {
        self.clear_child_tid = tidptr;
    }

    /// Get clear_child_tid address
    #[inline]
    pub fn clear_child_tid(&self) -> *mut i32 {
        self.clear_child_tid
    }

    /// Set robust list
    ///
    /// Used for robust mutex implementation
    #[inline]
    pub fn set_robust_list(&mut self, head: *const u8, len: usize) {
        self.robust_list_head = head;
        self.robust_list_len = len;
    }

    /// Get robust list head pointer
    #[inline]
    pub fn robust_list_head(&self) -> *const u8 {
        self.robust_list_head
    }

    /// Get robust list length
    #[inline]
    pub fn robust_list_len(&self) -> usize {
        self.robust_list_len
    }

    /// Get current brk value
    #[inline]
    pub fn get_brk(&self) -> u64 {
        self.brk.load(core::sync::atomic::Ordering::Acquire)
    }

    /// Set brk value
    #[inline]
    pub fn set_brk(&self, value: u64) {
        self.brk.store(value, core::sync::atomic::Ordering::Release);
    }

    /// Get current working directory
    pub fn get_cwd(&self) -> alloc::boxed::Box<[u8]> {
        if let Some(ref fs) = self.fs {
            fs.cwd_slice()
        } else {
            alloc::boxed::Box::from(&b"/"[..])
        }
    }

    /// Set current working directory
    pub fn set_cwd(&self, path: &[u8]) {
        if let Some(ref fs) = self.fs {
            fs.set_cwd(path);
        }
    }

    /// Get Arc reference to fs_struct (for CLONE_FS)
    pub fn fs_arc(&self) -> Option<alloc::sync::Arc<crate::fs::FsStruct>> {
        self.fs.clone()
    }

    /// Set fs_struct
    pub fn set_fs(&mut self, fs: Option<alloc::sync::Arc<crate::fs::FsStruct>>) {
        self.fs = fs;
    }

    /// Get umask
    pub fn get_umask(&self) -> u32 {
        if let Some(ref fs) = self.fs {
            fs.get_umask()
        } else {
            0o022
        }
    }

    /// Set umask and return old value
    pub fn set_umask(&self, mask: u32) -> u32 {
        if let Some(ref fs) = self.fs {
            fs.set_umask(mask)
        } else {
            0o022
        }
    }

    /// Get executable file path
    pub fn get_exe_path(&self) -> &[u8] {
        &self.exe_path
    }

    /// Set user stack pointer
    pub fn set_user_sp(&self, sp: u64) {
        self.ti_user_sp.store(sp, core::sync::atomic::Ordering::Release);
    }

    /// Set executable file path
    pub fn set_exe_path(&mut self, path: &[u8]) {
        self.exe_path = Box::from(path);
    }
}

///
/// Optional: 100, 250, 300, 1000
const HZ: u32 = 100;

// ==================== Offset constants (for assembly use) ====================

/// Task struct thread_info field offsets
#[allow(dead_code)]
pub mod task_offsets {
    use super::*;

    pub const TI_FLAGS: usize = core::mem::offset_of!(Task, ti_flags);
    pub const TI_PREEMPT_COUNT: usize = core::mem::offset_of!(Task, ti_preempt_count);
    pub const TI_KERNEL_SP: usize = core::mem::offset_of!(Task, ti_kernel_sp);
    pub const TI_USER_SP: usize = core::mem::offset_of!(Task, ti_user_sp);
    pub const TI_CPU: usize = core::mem::offset_of!(Task, ti_cpu);

    // Other common field offsets
    pub const TASK_STATE: usize = core::mem::offset_of!(Task, state);
    pub const TASK_PID: usize = core::mem::offset_of!(Task, pid);
    pub const TASK_THREAD: usize = core::mem::offset_of!(Task, thread);
    pub const TASK_KERNEL_STACK_BOTTOM: usize = core::mem::offset_of!(Task, kernel_stack_bottom);
}

/// Export offset constants
pub use task_offsets::*;
