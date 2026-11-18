//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 pt_regs ABI pins — ported from the dead #[cfg(test)] block in
//! kernel/src/arch/x86_64/pt_regs.rs per the test-layout policy. Only
//! compiled (and run) for x86_64 kernels: `make test PLATFORM=x86_64`.

use crate::arch::x86_64::pt_regs::{offsets, PtRegs};

pub fn test_pt_regs() {
    crate::tests::test_group_start("pt_regs");

    crate::test_assert_eq!(core::mem::size_of::<PtRegs>(), 168, "PtRegs == 168 bytes");
    crate::test_assert_eq!(offsets::R15, 0x00, "R15 offset");
    crate::test_assert_eq!(offsets::RDI, 0x70, "RDI offset");
    crate::test_assert_eq!(offsets::ORIG_RAX, 0x78, "ORIG_RAX offset");
    crate::test_assert_eq!(offsets::RIP, 0x80, "RIP offset");
    crate::test_assert_eq!(offsets::SS, 0xa0, "SS offset");
}
