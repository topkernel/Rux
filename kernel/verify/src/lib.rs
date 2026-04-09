//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for Rux kernel core data structure invariants.
//!
//! These tests copy pure algorithmic logic from kernel/src/ and verify
//! safety invariants using proptest (randomized input generation).
//!
//! The types and functions tested here are copied directly from the kernel
//! source to avoid a shared-crate dependency chain. When kernel types change,
//! the copies here must be updated accordingly.
//!
//! Run: `cargo test -p rux-verify`

pub mod mm;
pub mod sync;
pub mod arch;
pub mod net;
pub mod fs;
pub mod ipc;
pub mod drivers;
pub mod security;
pub mod signal;
pub mod process;
pub mod sched;
pub mod interrupt;
pub mod errno_test;

// Kani symbolic verification harnesses (only compiled with `cargo kani`)
#[cfg(kani)]
#[path = "mm/slab_kani.rs"]
pub mod slab_kani;
#[cfg(kani)]
#[path = "mm/page_flags_kani.rs"]
pub mod page_flags_kani;
#[cfg(kani)]
#[path = "mm/buddy_alloc_kani.rs"]
pub mod buddy_alloc_kani;
#[cfg(kani)]
#[path = "mm/refcount_kani.rs"]
pub mod refcount_kani;
#[cfg(kani)]
#[path = "mm/vma_kani.rs"]
pub mod vma_kani;

#[cfg(kani)]
#[path = "sync/spinlock_kani.rs"]
pub mod spinlock_kani;

#[cfg(kani)]
#[path = "arch/pt_regs_kani.rs"]
pub mod pt_regs_kani;
#[cfg(kani)]
#[path = "arch/riscv64/mm/memory_layout_kani.rs"]
pub mod memory_layout_kani;
#[cfg(kani)]
#[path = "arch/riscv64/mm/asid_kani.rs"]
pub mod asid_kani;

#[cfg(kani)]
#[path = "process/exit_status_kani.rs"]
pub mod exit_status_kani;
#[cfg(kani)]
#[path = "process/pid_kani.rs"]
pub mod pid_kani;
#[cfg(kani)]
#[path = "process/task_state_kani.rs"]
pub mod task_state_kani;
#[cfg(kani)]
#[path = "process/cred_kani.rs"]
pub mod cred_kani;

#[cfg(kani)]
#[path = "signal/signal_kani.rs"]
pub mod signal_kani;
#[cfg(kani)]
#[path = "signal/sigpending_kani.rs"]
pub mod sigpending_kani;

#[cfg(kani)]
#[path = "drivers/pci_offset_kani.rs"]
pub mod pci_offset_kani;
#[cfg(kani)]
#[path = "drivers/virtio_offset_kani.rs"]
pub mod virtio_offset_kani;
#[cfg(kani)]
#[path = "drivers/netdev_kani.rs"]
pub mod netdev_kani;
#[cfg(kani)]
#[path = "drivers/input/event_kani.rs"]
pub mod event_kani;

#[cfg(kani)]
#[path = "ipc/ipc_id_kani.rs"]
pub mod ipc_id_kani;

#[cfg(kani)]
#[path = "errno_kani.rs"]
pub mod errno_kani;

#[cfg(kani)]
#[path = "fs/dev_t_kani.rs"]
pub mod dev_t_kani;
#[cfg(kani)]
#[path = "fs/permission_kani.rs"]
pub mod permission_kani;
#[cfg(kani)]
#[path = "fs/stat_kani.rs"]
pub mod stat_kani;
#[cfg(kani)]
#[path = "fs/inode_kani.rs"]
pub mod inode_kani;

#[cfg(kani)]
#[path = "net/checksum_kani.rs"]
pub mod checksum_kani;
#[cfg(kani)]
#[path = "net/ethernet_kani.rs"]
pub mod ethernet_kani;
#[cfg(kani)]
#[path = "net/tcp_state_kani.rs"]
pub mod tcp_state_kani;

#[cfg(kani)]
#[path = "sched/rt_bitmap_kani.rs"]
pub mod rt_bitmap_kani;
#[cfg(kani)]
#[path = "sched/class_kani.rs"]
pub mod class_kani;

#[cfg(kani)]
#[path = "interrupt/preempt_kani.rs"]
pub mod preempt_kani;
#[cfg(kani)]
#[path = "interrupt/softirq_kani.rs"]
pub mod softirq_kani;

#[cfg(kani)]
#[path = "security/capability_kani.rs"]
pub mod capability_kani;
