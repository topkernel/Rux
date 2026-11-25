//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 pt_regs ABI pins — ported from the dead #[cfg(test)] block in
//! kernel/core/src/arch/x86_64/pt_regs.rs per the test-layout policy. Only
//! compiled (and run) for x86_64 kernels: `make test PLATFORM=x86_64`.

pub fn test_pt_regs() {
    crate::tests::test_group_start("pt_regs");

    #[cfg(feature = "x86_64")]
    {
        use crate::arch::x86_64::pt_regs::{offsets, PtRegs};
        crate::test_assert_eq!(core::mem::size_of::<PtRegs>(), 168, "PtRegs == 168 bytes");
        crate::test_assert_eq!(offsets::R15, 0x00, "R15 offset");
        crate::test_assert_eq!(offsets::RDI, 0x70, "RDI offset");
        crate::test_assert_eq!(offsets::ORIG_RAX, 0x78, "ORIG_RAX offset");
        crate::test_assert_eq!(offsets::RIP, 0x80, "RIP offset");
        crate::test_assert_eq!(offsets::SS, 0xa0, "SS offset");
    }

    #[cfg(feature = "riscv64")]
    {
        use crate::arch::riscv64::pt_regs::{offsets, PtRegs};
        crate::test_assert_eq!(core::mem::size_of::<PtRegs>(), 288, "PtRegs == 288 bytes");
        crate::test_assert_eq!(offsets::EPC, 0x00, "EPC offset");
        crate::test_assert_eq!(offsets::RA, 0x08, "RA offset");
        crate::test_assert_eq!(offsets::SP, 0x10, "SP offset");
        crate::test_assert_eq!(offsets::A0, 0x50, "A0 offset");
        crate::test_assert_eq!(offsets::STATUS, 0x100, "STATUS offset");
        crate::test_assert_eq!(offsets::ORIG_A0, 0x118, "ORIG_A0 offset");
    }
}
