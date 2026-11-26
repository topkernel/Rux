//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Unit test module
//!
//! All unit test functions are in this module, controlled by `unit-test` feature.
//!
//! Run tests:
//! ```bash
//! cargo build --package rux-core --features riscv64,unit-test
//! qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
//!   -kernel target/riscv64gc-unknown-none-elf/debug/rux
//! ```

use core::sync::atomic::{AtomicUsize, Ordering};
use alloc::vec::Vec;
use crate::sync::spinlock::Spinlock;

/// Direct UART output for test results.
/// println! routes through printk (ring buffer only, no UART output).
/// Tests need visible output, so we write directly to UART via console::puts.
#[cfg(feature = "unit-test")]
pub fn test_println(args: core::fmt::Arguments) {
    use core::fmt::Write;
    let mut uart = crate::console::lock();
    let _ = write!(uart, "{}", args);
    uart.putc(b'\r');
    uart.putc(b'\n');
}

/// Macro wrapper for test_println (convenience, same as println! syntax).
#[cfg(feature = "unit-test")]
#[macro_export]
macro_rules! test_println {
    () => ({
        $crate::tests::test_println_str("\n")
    });
    ($($arg:tt)*) => ({
        $crate::tests::test_println(format_args!($($arg)*))
    });
}

#[cfg(feature = "unit-test")]
fn test_println_str(s: &str) {
    // Same byte stream as the old per-byte loop: raw bytes, with a '\r'
    // AFTER each '\n' (the original emitted b then '\r').
    let mut buf = [0u8; 64];
    let mut n = 0usize;
    for &b in s.as_bytes() {
        if n < buf.len() {
            buf[n] = b;
            n += 1;
        }
        if b == b'\n' && n < buf.len() {
            buf[n] = b'\r';
            n += 1;
        }
    }
    let uart = crate::console::lock();
    uart.puts_batch(&buf[..n]);
}

/// Maximum number of failed tests to record
const MAX_FAILED_TESTS: usize = 32;

/// Failed test record
#[cfg(feature = "unit-test")]
struct FailedTest {
    name: [u8; 64],
    name_len: usize,
    reason: [u8; 128],
    reason_len: usize,
}

#[cfg(feature = "unit-test")]
impl FailedTest {
    const fn new() -> Self {
        Self {
            name: [0; 64],
            name_len: 0,
            reason: [0; 128],
            reason_len: 0,
        }
    }

    fn set(&mut self, name: &str, reason: &str) {
        self.name_len = name.as_bytes().len().min(64);
        self.name[..self.name_len].copy_from_slice(&name.as_bytes()[..self.name_len]);
        self.reason_len = reason.as_bytes().len().min(128);
        self.reason[..self.reason_len].copy_from_slice(&reason.as_bytes()[..self.reason_len]);
    }

    fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("???")
    }

    fn reason(&self) -> &str {
        core::str::from_utf8(&self.reason[..self.reason_len]).unwrap_or("???")
    }
}

/// Global test statistics
#[cfg(feature = "unit-test")]
static TEST_PASSED: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "unit-test")]
static TEST_FAILED: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "unit-test")]
static TEST_CURRENT: AtomicUsize = AtomicUsize::new(0);

/// Failed test list
#[cfg(feature = "unit-test")]
static FAILED_TESTS: Spinlock<[FailedTest; MAX_FAILED_TESTS]> = Spinlock::new([const { FailedTest::new() }; MAX_FAILED_TESTS]);
#[cfg(feature = "unit-test")]
static FAILED_COUNT: AtomicUsize = AtomicUsize::new(0);

// SERVICE-TEST REGISTRATION (crate-flip rung 3): guest tests that moved
// out with their service leaves register here at boot — the composition
// bin (bin) forwards them from rust_main, before any init — and
// run_all_tests executes them in registration order at the position the
// family used to occupy. Same shape as the FsHooks table: the framework
// owns the mechanism (registry + reporting counters), the service owns
// the test bodies.
//
// Each entry is an OnceSlot (the audited write-once publication
// primitive), and the table takes NO lock. It must not: registration
// runs from rust_main, before arch::smp::init() establishes the
// per-CPU area, and every Spinlock in this kernel reads per-CPU state
// on acquisition (preempt_disable, the holder cpu id) — on x86_64
// that is %gs-relative with the GS base still 0, which faulted the
// unit-test kernel before its first print. Every crate's register_all
// publishes through OnceSlot for the same reason; the first version of
// this table used a Spinlock.
//
// INV-SVCTEST-1: a slot is written exactly once (publish, Release) by
// the boot CPU before SMP start; readers take it with get() (Acquire)
// from run_all_tests. INV-SVCTEST-2: the length counter is published
// only after the slot it covers (Release), so index < len implies the
// slot at that index is populated.
#[cfg(feature = "unit-test")]
pub type ServiceTestFn = fn();
#[cfg(feature = "unit-test")]
const MAX_SERVICE_TESTS: usize = 8;
#[cfg(feature = "unit-test")]
type ServiceTestTable = [crate::safety::slot::OnceSlot<ServiceTestFn>; MAX_SERVICE_TESTS];
#[cfg(feature = "unit-test")]
const fn empty_service_test_table() -> ServiceTestTable {
    [const { crate::safety::slot::OnceSlot::new() }; MAX_SERVICE_TESTS]
}

/// Store `f` into the next free entry. Returns false when the table is
/// full (the entry is then not registered — the suite size gate makes
/// the resulting smaller suite fail make test).
#[cfg(feature = "unit-test")]
fn service_test_push(table: &ServiceTestTable, len: &AtomicUsize, f: ServiceTestFn) -> bool {
    let i = len.load(Ordering::Relaxed);
    if i >= table.len() || !table[i].publish(f) {
        return false;
    }
    len.store(i + 1, Ordering::Release);
    true
}

/// Read entry `i` (INV-SVCTEST-1/2).
#[cfg(feature = "unit-test")]
fn service_test_get(table: &ServiceTestTable, len: &AtomicUsize) -> Vec<ServiceTestFn> {
    let n = len.load(Ordering::Acquire).min(table.len());
    table[..n].iter().filter_map(|slot| slot.get().copied()).collect()
}

/// Register a service crate's guest-test entry (called from bin's
/// rust_main). `late`: run at the POST-fs_io point instead — the ipc flip's
/// finding: the sysv_sem BLOCKING test's cross-task handoff depends on
/// the post-fs_io machine state (the family's historical position was
/// after fs_io/fs_pins); running it early parks the boot task forever.
/// fs (pipe2) stays early (its family's original position).
#[cfg(feature = "unit-test")]
pub fn register_service_tests(f: ServiceTestFn) {
    let _ = service_test_push(&SERVICE_TESTS, &SERVICE_TEST_LEN, f);
}

/// Register into the LATE table (the post-fs_io point).
#[cfg(feature = "unit-test")]
pub fn register_service_tests_late(f: ServiceTestFn) {
    let _ = service_test_push(&SERVICE_TESTS_LATE, &SERVICE_TEST_LATE_LEN, f);
}

#[cfg(feature = "unit-test")]
static SERVICE_TESTS: ServiceTestTable = empty_service_test_table();
#[cfg(feature = "unit-test")]
static SERVICE_TEST_LEN: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "unit-test")]
static SERVICE_TESTS_LATE: ServiceTestTable = empty_service_test_table();
#[cfg(feature = "unit-test")]
static SERVICE_TEST_LATE_LEN: AtomicUsize = AtomicUsize::new(0);

/// Run the registered service tests (called from run_all_tests). The
/// entries are read out first: the test bodies run holding nothing, so
/// a body cannot re-enter its own table.
#[cfg(feature = "unit-test")]
fn run_registered_service_tests() {
    for f in service_test_get(&SERVICE_TESTS, &SERVICE_TEST_LEN) {
        f();
    }
}

/// Run the LATE service tests (the post-fs_io point — the ipc
/// family's historical position). Same read-then-run shape.
#[cfg(feature = "unit-test")]
fn run_registered_service_tests_late() {
    for f in service_test_get(&SERVICE_TESTS_LATE, &SERVICE_TEST_LATE_LEN) {
        f();
    }
}

/// Record test pass
#[cfg(feature = "unit-test")]
pub fn test_pass(name: &str) {
    TEST_PASSED.fetch_add(1, Ordering::SeqCst);
    test_println!("test:   \u{1b}[32mPASS\u{1b}[0m {}", name);
}

/// Record test fail
#[cfg(feature = "unit-test")]
pub fn test_fail(name: &str, reason: &str) {
    TEST_FAILED.fetch_add(1, Ordering::SeqCst);
    test_println!("test:   \u{1b}[31mFAIL\u{1b}[0m {} - {}", name, reason);

    // Record failed test
    let idx = FAILED_COUNT.fetch_add(1, Ordering::SeqCst);
    if idx < MAX_FAILED_TESTS {
        let mut failed = FAILED_TESTS.lock();
        failed[idx].set(name, reason);
    }
}

/// Record test skip
#[cfg(feature = "unit-test")]
pub fn test_skip(name: &str, reason: &str) {
    test_println!("test:   \u{1b}[33mSKIP\u{1b}[0m {} - {}", name, reason);
}

/// Start a test group
#[cfg(feature = "unit-test")]
pub fn test_group_start(name: &str) {
    let idx = TEST_CURRENT.fetch_add(1, Ordering::SeqCst);
    test_println!("\ntest: [{}] {} ================================", idx + 1, name);
}

/// Assert macro - records but doesn't panic on failure
#[cfg(feature = "unit-test")]
#[macro_export]
macro_rules! test_assert {
    ($cond:expr, $name:expr) => {
        if $cond {
            $crate::tests::test_pass($name);
        } else {
            $crate::tests::test_fail($name, "assertion failed");
        }
    };
    ($cond:expr, $name:expr, $reason:expr) => {
        if $cond {
            $crate::tests::test_pass($name);
        } else {
            $crate::tests::test_fail($name, $reason);
        }
    };
}

/// Assert equal macro
#[cfg(feature = "unit-test")]
#[macro_export]
macro_rules! test_assert_eq {
    ($left:expr, $right:expr, $name:expr) => {
        if $left == $right {
            $crate::tests::test_pass($name);
        } else {
            $crate::tests::test_fail($name, concat!("expected ", stringify!($left), " == ", stringify!($right)));
        }
    };
}

// ===== Pure logic tests =====
#[cfg(feature = "unit-test")]
pub mod binder;
#[cfg(feature = "unit-test")]
pub mod fs_io;
#[cfg(feature = "unit-test")]
pub mod fs_pins;
#[cfg(feature = "unit-test")]
pub mod mm_pins;
// pub mod net_hdrs; — MOVED to the net crate (rung-2 flip); runs in-guest
// via the SERVICE-TEST SEAM below (registered by bin, same counters)
#[cfg(feature = "unit-test")]
// pub mod net_pins; — service-internals test (rung 2); follows the files to the crate
#[cfg(feature = "unit-test")]
// pub mod procfs; — MOVED to the fs crate (fs-substrate); runs in-guest via the SERVICE-TEST SEAM
#[cfg(all(feature = "unit-test", feature = "x86_64"))]
pub mod pt_regs;
#[cfg(feature = "unit-test")]
pub mod perf;
#[cfg(feature = "unit-test")]
// pub mod tmpfs; — MOVED to the fs crate (fs-substrate); runs in-guest via the SERVICE-TEST SEAM
#[cfg(feature = "unit-test")]
pub mod dev_t;
#[cfg(feature = "unit-test")]
// pub mod checksum; — service-internals test (rung 2); follows the files to the crate
#[cfg(feature = "unit-test")]
pub mod errno_test;
#[cfg(feature = "unit-test")]
pub mod config_test;
#[cfg(feature = "unit-test")]
pub mod vma_flags;

// ===== Core data structures =====
#[cfg(feature = "unit-test")]
pub mod listhead;
#[cfg(feature = "unit-test")]
pub mod safety_primitives;
#[cfg(feature = "unit-test")]
pub mod path;
#[cfg(feature = "unit-test")]
pub mod file_flags;
#[cfg(feature = "unit-test")]
// pub mod fileops_caps; — service-internals test (State B final; follows the files to the crate)
#[cfg(feature = "unit-test")]
pub mod fdtable;
// pub mod signal; — MOVED to the signal service crate; runs in-guest
// via the SERVICE-TEST SEAM below (registered by bin)

// ===== Memory management =====
#[cfg(feature = "unit-test")]
pub mod heap_allocator;
#[cfg(feature = "unit-test")]
pub mod small_alloc;
#[cfg(feature = "unit-test")]
pub mod page_allocator;
#[cfg(feature = "unit-test")]
pub mod mm_pcp;
#[cfg(feature = "unit-test")]
pub mod buffer_state;
#[cfg(feature = "unit-test")]
pub mod file_buffer;
#[cfg(feature = "unit-test")]
pub mod mount_flags;
#[cfg(feature = "unit-test")]
pub mod percpu;
pub mod lockkey;
#[cfg(feature = "unit-test")]
pub mod sync_locks;

// ===== Process management =====
#[cfg(feature = "unit-test")]
pub mod scheduler;
#[cfg(feature = "unit-test")]
pub mod process_tree;
pub mod reparent_safety;
#[cfg(feature = "unit-test")]
// pub mod fork; — MOVED to the process crate (process rung); runs
// in-guest via the SERVICE-TEST SEAM (registered by bin)
#[cfg(feature = "unit-test")]
// pub mod execve; — service-internals test (State B final; follows the files to the crate)
#[cfg(feature = "unit-test")]
// pub mod wait4; — service-internals test (State B final)
#[cfg(feature = "unit-test")]
pub mod getpid;
#[cfg(feature = "unit-test")]
pub mod sleep_wakeup;
#[cfg(feature = "unit-test")]
// pub mod pid_test; — MOVED to the process crate (process rung)

// ===== Synchronization =====
#[cfg(feature = "unit-test")]
pub mod semaphore;
// pub mod futex_test; — moved to tests/services/ (sync-upper lane);
// runs via the SERVICE-TEST SEAM (sync::run_unit_tests)

// ===== Scheduler =====
#[cfg(feature = "unit-test")]
pub mod smp;
#[cfg(feature = "unit-test")]
// pub mod smp_schedule; — MOVED to the process crate (process rung)
#[cfg(feature = "unit-test")]
pub mod preemptive_scheduler;
// pub mod fair_timeline; — MOVED to the sched crate (S1b flip); runs
// in-guest via the SERVICE-TEST SEAM below (registered by bin)

// ===== Filesystem =====
#[cfg(feature = "unit-test")]
pub mod file_open;
#[cfg(feature = "unit-test")]
pub mod dcache;
#[cfg(feature = "unit-test")]
pub mod icache;
#[cfg(feature = "unit-test")]
pub mod fstat;
#[cfg(feature = "unit-test")]
pub mod fcntl;
#[cfg(feature = "unit-test")]
pub mod mkdir_unlink;
#[cfg(feature = "unit-test")]
pub mod link;
#[cfg(feature = "unit-test")]
// pub mod pipe2; — MOVED to the fs crate (rung 3 flip); runs in-guest
// via the SERVICE-TEST SEAM below (registered by bin, same counters)
#[cfg(feature = "unit-test")]
// pub mod ext4_allocator; — MOVED to the ext4 service crate (rung 5)
#[cfg(feature = "unit-test")]
// pub mod ext4_file_write; — MOVED to the ext4 service crate (rung 5)

// ===== IPC =====
// pub mod signal_procmask; — MOVED to the signal service crate; runs
// via the SERVICE-TEST SEAM below (registered by bin)
#[cfg(feature = "unit-test")]
// pub mod ipc_poll; — service-internals test (State B final; follows the files to the crate)
#[cfg(feature = "unit-test")]
// pub mod ipc_epoll; — service-internals test (State B final; follows the files to the crate)
#[cfg(feature = "unit-test")]
// pub mod ipc_eventfd; — service-internals test (State B final; follows the files to the crate)

// ===== IPC System V =====
#[cfg(feature = "unit-test")]
// pub mod ipc_sysv; — MOVED to the ipc service crate (this rung;
// runs through the SERVICE-TEST SEAM, same pass/fail counters)
#[cfg(feature = "unit-test")]
pub mod sysv_sem_block;

// ===== Memory syscalls =====
#[cfg(feature = "unit-test")]
// pub mod mem_mmap; — service-internals test (State B final; follows the files to the crate)
#[cfg(feature = "unit-test")]
// pub mod mem_cow; — service-internals test (State B final; follows the files to the crate)

// ===== Network =====
#[cfg(feature = "unit-test")]
// pub mod tcp_handshake; — service-internals test (rung 2); follows the files to the crate
#[cfg(feature = "unit-test")]
// pub mod tcp_semantics; — service-internals test (rung 2); follows the files to the crate
#[cfg(feature = "unit-test")]
pub mod virtio_net;
#[cfg(feature = "unit-test")]
// pub mod network; — service-internals test (rung 2); follows the files to the crate

// ===== Drivers =====
#[cfg(feature = "unit-test")]
pub mod virtio_queue;
#[cfg(feature = "unit-test")]
pub mod framebuffer;

// ===== Boundary (destructive) =====
#[cfg(feature = "unit-test")]
// pub mod boundary; — MOVED to the process crate (process rung)

// ===== VFS create-mode rules (creat09 / CVE-2018-13405) =====
#[cfg(feature = "unit-test")]
pub mod sgid_mode;

// ===== initrd (gzip inflate + cpio newc unpack) =====
#[cfg(feature = "unit-test")]
// pub mod initrd; — MOVED to the fs crate (fs-substrate); runs in-guest via the SERVICE-TEST SEAM

// ===== tmpfs mknod (OH Phase 1b: device nodes on a tmpfs /dev) =====
#[cfg(feature = "unit-test")]
// pub mod tmpfs_mknod; — MOVED to the fs crate (fs-substrate); runs in-guest via the SERVICE-TEST SEAM

// ===== getdents64 listing cache (F4 route-4) =====
#[cfg(feature = "unit-test")]
pub mod getdents_cache;

// ===== System call interface =====
#[cfg(feature = "unit-test")]
// pub mod syscall_file; — service-internals test (State B final; follows the files to the crate)
#[cfg(feature = "unit-test")]
// // pub mod syscall_io; — service-internals test (State B final; follows the files to the crate) — moved to the syscall service crate (State B final); the in-kernel unit tests that poked service internals follow the files
// pub mod loop_flags; — service-internals test (State B final; follows the files to the crate)
#[cfg(feature = "unit-test")]
// pub mod syscall_process; — moved to the syscall service crate (State B final); the in-kernel unit tests that poked service internals follow the files
#[cfg(feature = "unit-test")]
// pub mod syscall_memory; — moved to the syscall service crate (State B final); the in-kernel unit tests that poked service internals follow the files
#[cfg(feature = "unit-test")]
// pub mod syscall_time; — moved to the syscall service crate (State B final); the in-kernel unit tests that poked service internals follow the files
#[cfg(feature = "unit-test")]
pub mod timer_wheel;
#[cfg(feature = "unit-test")]
// pub mod syscall_network; — moved to the syscall service crate (State B final); the in-kernel unit tests that poked service internals follow the files
#[cfg(feature = "unit-test")]
// pub mod syscall_sched; — moved to the syscall service crate (State B final); the in-kernel unit tests that poked service internals follow the files
#[cfg(feature = "unit-test")]
// pub mod syscall_signal; — moved to the syscall service crate (State B final); the in-kernel unit tests that poked service internals follow the files
#[cfg(feature = "unit-test")]
// // pub mod syscall_misc; — service-internals test (State B final; follows the files to the crate) — moved to the syscall service crate (State B final); the in-kernel unit tests that poked service internals follow the files

/// Install a fresh fdtable (with the UART console as fd 0) on the current
/// boot task so fd-allocating syscall tests exercise the real fd path.
/// See the call site in `run_all_tests` for why this is safe.
#[cfg(feature = "unit-test")]
fn setup_boot_task_fdtable() {
    use alloc::sync::Arc;

    let Some(task) = crate::sched::hotpath::current() else {
        return;
    };
    // SAFETY: we hold the sole &'static mut to the current task for the
    // duration of the test suite (timer interrupts are disabled and the
    // scheduler is not running); nothing else can alias it concurrently.
    unsafe {
        if (*task).try_fdtable().is_none() {
            (*task).set_fdtable(Some(Arc::new(crate::fs::FdTable::new())));
        }
        // Console file as fd 0: TTY-class ioctls (TCGETS/TIOCGWINSZ/...)
        // are gated on file_is_tty(), and the CONDEV_OPS backing is the
        // same one /dev/console opens hand out.
        let console_file = Arc::new(crate::fs::File::new(
            crate::fs::file::FileFlags::new(crate::fs::file::FileFlags::O_RDWR),
        ));
        console_file.set_ops(crate::fs::leaf_glue::devfs::condev_ops());
        if let Some(ft) = (*task).try_fdtable() {
            if let Some(fd) = ft.alloc_fd() {
                let _ = ft.install_fd(fd, console_file);
            }
        }
    }
}

#[cfg(feature = "unit-test")]
pub fn run_all_tests() {
    test_println!("test: ===== Starting Rux OS Unit Tests =====");

    // The suite runs on the kernel-main boot task (pid 0) which carries no
    // fdtable — fd-allocating syscall tests (eventfd/epoll/ioctl/pipes)
    // would all see EBADF. Give it a throwaway table with the UART console
    // installed as fd 0 (mirroring init's stdio): the unit-test kernel
    // never execs init (it halts or panics when the suite finishes), so
    // nothing else observes this state.
    setup_boot_task_fdtable();

    // ===== 1. Pure logic tests =====
    test_group_start("dev_t");
    dev_t::test_dev_t();

    test_group_start("checksum");
    // (gated with the module: checksum)

    test_group_start("errno");
    errno_test::test_errno();

    test_group_start("config");
    config_test::test_config();

    // initrd: MOVED to the fs crate (fs-substrate) — runs through
    // the SERVICE-TEST SEAM (fs::run_unit_tests).

    // tmpfs(-mknod): MOVED to the fs crate (fs-substrate) — runs
    // through the SERVICE-TEST SEAM (fs::run_unit_tests).

    // getdents64 listing cache (F4 route-4): stability + invalidation.
    getdents_cache::test_getdents_cache();

    // procfile! shape floors + conversion pins (leg-35 wiring: the
    // bare #[cfg(test)] forms never join the unit-test feature build,
    // D-4 kvtab slice (refactor lane): /proc/[pid]/status — key order,
    // tab shape and value formats pinned against a live render.
    // procfs renderer pins (kernel/core/src/tests/procfs.rs): /proc/cpuinfo
    // per-arch key table + /proc/[pid]/status shape and order. Formerly
    // in-module cpuinfo_file_tests/status_file_tests blocks.
    // procfs: MOVED to the fs crate (fs-substrate) — SERVICE-TEST SEAM

    // tmpfs group (kernel/core/src/tests/tmpfs.rs): page store, truncate
    // tails, dir ops, and the mount-option page budget. Formerly an
    // in-module unit-test fn plus a dead #[cfg(test)] block in
    // fs/tmpfs.rs — all tmpfs unit tests live under kernel/core/src/tests/.
    // tmpfs::test_tmpfs(): the tmpfs budget tests moved with the leaf
    // into the fs crate (tmpfs_test, SERVICE-TEST SEAM).

    test_group_start("vma_flags");
    vma_flags::test_vma_flags();

    // ===== 2. Core data structures =====
    test_group_start("listhead");
    listhead::test_listhead();

    // Safety primitives (unsafe-slim round 3 lane A): KPtr/StaticRef/
    // DmaBlock/MmioRegion/safety_cast.
    test_group_start("safety primitives");
    safety_primitives::test_safety_primitives();
    safety_primitives::test_safety_primitives_r4();
    safety_primitives::test_safety_primitives_frame_run();

    test_group_start("path");
    path::test_path();

    test_group_start("file_flags");
    file_flags::test_file_flags();

    // D-6 (§1 capability declarations): capability-matrix pins for the
    // builder-converted ops tables + the central missing-op errno map.
    test_group_start("fileops-caps");
// (gated with the module: fileops_caps)

    test_group_start("fdtable");
    fdtable::test_fdtable();

    // (signal: MOVED to the signal crate — runs via the
    // SERVICE-TEST SEAM below, same group name/counters)

    // perf events: ABI record encoding + ring arithmetic + freq floor
    // (moved from the in-module perf::tests block into tests/perf.rs).
    perf::test_perf();

    // ===== 3. Memory management =====
    test_group_start("heap_allocator");
    heap_allocator::test_heap_allocator();

    test_group_start("small_alloc");
    small_alloc::test_small_alloc();

    test_group_start("percpu");
    percpu::test_percpu();

    // Typed lock keys (§3 GRQ typed-key slice): the CanNestInside
    // mechanics + the GRQ claim pattern.
    test_group_start("lockkey");
    lockkey::test_lockkey();

    // Sync primitives (review-s2-c): SeqLock writer-guard accounting
    // (the try_write fix) + RwSpinlock two-phase write mechanics.
    test_group_start("sync_locks");
    sync_locks::test_sync_locks();

    test_group_start("page_allocator");
    page_allocator::test_page_allocator();

    test_group_start("mm_pcp");
    mm_pcp::test_pcp_pagesets();

    test_group_start("buffer_state");
    buffer_state::test_buffer_state();

    test_group_start("file_buffer");
    file_buffer::test_file_buffer();

    test_group_start("mount_flags");
    mount_flags::test_mount_flags();

    // ===== 4. Process management =====
    // x86_64 ABI pins (kernel/core/src/tests/pt_regs.rs) — run only on
    // x86_64 kernels (make test PLATFORM=x86_64).
    #[cfg(feature = "x86_64")]
    pt_regs::test_pt_regs();

    test_group_start("scheduler");
    scheduler::test_scheduler();

    test_group_start("process_tree");
    process_tree::test_process_tree();

    test_group_start("reparent_safety");
    reparent_safety::test_reparent_safety();

    test_group_start("fork");
// (gated with the module: fork — runs via the SERVICE-TEST SEAM, process)

    test_group_start("execve");
// (gated with the module: execve)

    test_group_start("wait4");
// (gated with the module: wait4)

    test_group_start("getpid");
    getpid::test_getpid();

    test_group_start("sleep_wakeup");
    sleep_wakeup::test_sleep_and_wakeup();

    test_group_start("pid");
// (gated with the module: pid — runs via the SERVICE-TEST SEAM, process)

    // ===== 5. Synchronization =====
    test_group_start("semaphore");
    semaphore::test_semaphore();

// (gated with the module: futex_test — group runs via the
// SERVICE-TEST SEAM, sync::run_unit_tests)

    // ===== 6. Scheduler =====
    test_group_start("smp");
    smp::test_smp();

    test_group_start("smp_schedule");
// (gated with the module: smp_schedule — runs via the SERVICE-TEST SEAM, process)

    test_group_start("preemptive_scheduler");
    preemptive_scheduler::test_preemptive_scheduler();

    // grq-10ns: entity-embedded intrusive CFS timeline (sorted order,
    // shuffled erase, leftmost cache, tie keys, pick/requeue cycles) —
    // MOVED to the sched crate (S1b); runs via the SERVICE-TEST SEAM.

    // ===== 7. Filesystem =====
    test_group_start("file_open");
    file_open::test_file_open();

    test_group_start("dcache");
    dcache::test_dcache();

    test_group_start("icache");
    icache::test_icache();

    test_group_start("fstat");
    fstat::test_fstat();

    test_group_start("fcntl");
    fcntl::test_fcntl();

    test_group_start("mkdir_unlink");
    mkdir_unlink::test_mkdir_unlink();

    test_group_start("link");
    link::test_link();

    // SERVICE-TEST SEAM: the crate-flipped leaves' guest tests run in
    // registration order with the same counters: pipe2 (fs), net_hdrs
    // (net), fair_pins + fair_timeline (sched) — bin
    // registered the entries at boot.
    run_registered_service_tests();





    // ===== 8. IPC =====
    // (signal_procmask: MOVED to the signal crate — runs via the
    // SERVICE-TEST SEAM below, same group name/counters)

    test_group_start("ipc_poll");
// (gated with the module: ipc_poll)

    test_group_start("ipc_epoll");
// (gated with the module: ipc_epoll)

    test_group_start("ipc_eventfd");
// (gated with the module: ipc_eventfd)

    // (gated with the module: ipc_sysv — moved to ipc, runs via
    // the SERVICE-TEST SEAM with the same group name/counters)

    // Real-filesystem test suite (kernel/core/src/tests/fs_io.rs): the ext4
    // root is mounted BEFORE the suite runs — drive it (create/write/
    // read-back/fsync-shape/stat/append/getdents/unlink + errno pins).
    fs_io::test_fs_io();

    // FS pins (kernel/core/src/tests/fs_pins.rs): stat/superblock/vt/rootfs/
    // ext4 csum+indirect.
    fs_pins::test_fs_pins();

    // MM + cmdline pins (kernel/core/src/tests/mm_pins.rs): slab classes,
    // VMA, page flags/refcount/mapcount, cmdline parsers. The sched half
    // of the old mm_sched_pins moved to the sched crate with the S1b
    // flip and runs via the SERVICE-TEST SEAM.
    mm_pins::test_mm_pins();

    // LATE SERVICE-TEST SEAM: the ipc family's entries run here —
    // the historical ipc_sysv/sysv_sem_block position (AFTER fs_io/
    // fs_pins; the sysv sem blocking contract's cross-task handoff
    // depends on the post-fs_io machine state).
    run_registered_service_tests_late();

    // Network pins (kernel/core/src/tests/net_pins.rs): skb core, header
    // sizes, UDP/TCP state machines, checksums, ARP, routes, IPv6,
    // loopback. LAST — it plants ARP/route table entries.
    // (gated with the module: net_pins)

    // Binder group (kernel/core/src/tests/binder.rs): alloc recycle, event
    // name table, device contexts. Formerly an in-module test inside
    // ipc/binder.rs — all binder unit tests live under kernel/core/src/tests/.
    binder::test_binder();

    // sysv_sem_block stays an in-framework test (sysv_sem itself
    // shelved framework-side with this rung — see the crate's record).
    test_group_start("sysv_sem_block");
    sysv_sem_block::test_sysv_sem_block();

    // ===== 9. Memory syscalls =====
    test_group_start("mem_mmap");
// (gated with the module: mem_mmap)

    test_group_start("mem_cow");
// (gated with the module: mem_cow)

    // ===== 10. Network =====
    // net_hdrs: MOVED to the net crate (rung 2) — runs via the
    // SERVICE-TEST SEAM (registration order, same counters).

    test_group_start("tcp_handshake");
    // (gated with the module: tcp_handshake)

    test_group_start("tcp_semantics");
    // (gated with the module: tcp_semantics)

    test_group_start("virtio_net");
    virtio_net::test_virtio_net();

    test_group_start("network");
    // (gated with the module: network)

    // ===== 11. Drivers =====
    test_group_start("virtio_queue");
    virtio_queue::test_virtio_queue();

    test_group_start("framebuffer");
    framebuffer::test_framebuffer();

    // ===== 12. System call interface =====
    test_group_start("syscall_file");
// (gated with the module: syscall_file)

    test_group_start("syscall_io");
// (gated with the module: syscall_io)

    test_group_start("loop+flags");
// (gated with the module: loop_flags)

    test_group_start("syscall_process");

    test_group_start("syscall_memory");

    test_group_start("syscall_time");

    test_group_start("timer-wheel");
    timer_wheel::test_timer_wheel();

    test_group_start("syscall_network");

    test_group_start("syscall_sched");
    // aarch64: the group's very first op (sys_sched_yield → schedule())
    // drives the arch context switch, which wedges inside __switch_to
    // (arch/aarch64/context.rs — first exercise of that path on this
    // port; with the unit-test timer off nothing unwedges it). Skip the
    // group until the port lane lands a working context switch; the
    // riscv64/x86_64 runs keep the full pins.
    #[cfg(feature = "aarch64")]
    test_skip(
        "syscall_sched",
        "aarch64: sched_yield wedges in __switch_to (context switch pending)",
    );
    #[cfg(not(feature = "aarch64"))]

    test_group_start("syscall_signal");

    test_group_start("syscall_misc");
// (gated with the module: syscall_misc)

    // ===== VFS create-mode rules (creat09) =====
    test_group_start("sgid_mode");
    sgid_mode::test_sgid_mode();

    // ===== 13. Boundary (destructive, MUST be last) =====
    test_group_start("boundary");
// (gated with the module: boundary — runs via the SERVICE-TEST SEAM, process)

    // Print test summary
    print_test_summary();
}

/// Print test summary
#[cfg(feature = "unit-test")]
pub fn print_test_summary() {
    let passed = TEST_PASSED.load(Ordering::SeqCst);
    let failed = TEST_FAILED.load(Ordering::SeqCst);
    let total = passed + failed;

    test_println!("\n\u{1b}[36m========================================\u{1b}[0m");
    test_println!("\u{1b}[36m             TEST SUMMARY\u{1b}[0m");
    test_println!("\u{1b}[36m========================================\u{1b}[0m");

    if failed == 0 {
        test_println!("\u{1b}[32m  All tests passed!\u{1b}[0m");
    } else {
        test_println!("\u{1b}[31m  Some tests failed!\u{1b}[0m");
    }

    test_println!();
    test_println!("  Total:   {} tests", total);
    test_println!("  \u{1b}[32mPassed:  {}\u{1b}[0m", passed);
    if failed > 0 {
        test_println!("  \u{1b}[31mFailed:  {}\u{1b}[0m", failed);
    } else {
        test_println!("  Failed:  0");
    }

    // Print failed test list
    if failed > 0 {
        let failed_count = FAILED_COUNT.load(Ordering::SeqCst).min(MAX_FAILED_TESTS);
        let failed_tests = FAILED_TESTS.lock();
        test_println!();
        test_println!("\u{1b}[31m  Failed tests:\u{1b}[0m");
        for i in 0..failed_count {
            test_println!("    \u{1b}[31m{}\u{1b}[0m - {}", failed_tests[i].name(), failed_tests[i].reason());
        }
        if failed > MAX_FAILED_TESTS {
            test_println!("    ... and {} more", failed - MAX_FAILED_TESTS);
        }
    }

    test_println!("\u{1b}[36m========================================\u{1b}[0m");

    // If there are failed tests, print obvious failure marker
    if failed > 0 {
        test_println!("\u{1b}[31m!!! TESTS FAILED !!!\u{1b}[0m");
    } else {
        test_println!("\u{1b}[32m*** ALL TESTS PASSED ***\u{1b}[0m");
    }

    // Whole-kernel coverage dump (cov-dump feature, see kernel/core/src/cov.rs):
    // the .profraw is emitted as hex between [cov-begin]/[cov-end] marker
    // lines on this console stream; the host-side report extracts it from
    // the boot log. Runs once, after all test work.
    #[cfg(feature = "cov-dump")]
    {
        match crate::cov::dump_to_console() {
            Ok(n) => test_println!("[cov] profile emitted ({} bytes)", n),
            Err(e) => test_println!("[cov] profile dump failed: errno {}", e),
        }
    }
}

/// Get failed test count
#[cfg(feature = "unit-test")]
pub fn get_failed_count() -> usize {
    TEST_FAILED.load(Ordering::SeqCst)
}
