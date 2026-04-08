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
pub mod mm {
    pub mod slab_kani;
    pub mod page_flags_kani;
    pub mod buddy_alloc_kani;
    pub mod refcount_kani;
    pub mod vma_kani;
}
#[cfg(kani)]
pub mod sync {
    pub mod spinlock_kani;
}
#[cfg(kani)]
pub mod arch {
    pub mod pt_regs_kani;
    pub mod riscv64 {
        pub mod mm {
            pub mod memory_layout_kani;
            pub mod asid_kani;
        }
    }
}
#[cfg(kani)]
pub mod process {
    pub mod exit_status_kani;
    pub mod pid_kani;
    pub mod task_state_kani;
    pub mod cred_kani;
}
#[cfg(kani)]
pub mod signal {
    pub mod signal_kani;
    pub mod sigpending_kani;
}
#[cfg(kani)]
pub mod drivers {
    pub mod pci_offset_kani;
    pub mod virtio_offset_kani;
    pub mod netdev_kani;
    pub mod input {
        pub mod event_kani;
    }
}
#[cfg(kani)]
pub mod ipc {
    pub mod ipc_id_kani;
}
#[cfg(kani)]
pub mod errno_kani;
#[cfg(kani)]
pub mod fs {
    pub mod dev_t_kani;
    pub mod permission_kani;
    pub mod stat_kani;
    pub mod inode_kani;
}
#[cfg(kani)]
pub mod net {
    pub mod checksum_kani;
    pub mod ethernet_kani;
    pub mod tcp_state_kani;
}
#[cfg(kani)]
pub mod sched {
    pub mod rt_bitmap_kani;
    pub mod class_kani;
}
#[cfg(kani)]
pub mod interrupt {
    pub mod preempt_kani;
    pub mod softirq_kani;
}
#[cfg(kani)]
pub mod security {
    pub mod capability_kani;
}
