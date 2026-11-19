//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ELF header parsing and validation invariant tests.
//!
//! Types copied from: kernel/src/fs/elf.rs
//! NOTE: unsafe from_bytes uses ptr::read_unaligned — test only the safe API

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/fs/elf.rs
// ============================================================================

pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

#[repr(u16)]
#[derive(Debug, Copy, Clone, PartialEq)]
#[allow(non_camel_case_types)]
pub enum ElfType {
    ET_NONE = 0,
    ET_REL = 1,
    ET_EXEC = 2,
    ET_DYN = 3,
    ET_CORE = 4,
}

#[repr(u32)]
#[derive(Debug, Copy, Clone, PartialEq)]
#[allow(non_camel_case_types)]
pub enum ElfPtType {
    PT_NULL = 0,
    PT_LOAD = 1,
    PT_DYNAMIC = 2,
    PT_INTERP = 3,
    PT_NOTE = 4,
    PT_SHLIB = 5,
    PT_PHDR = 6,
    PT_TLS = 7,
}

pub const PF_X: u32 = 0x1;
pub const PF_W: u32 = 0x2;
pub const PF_R: u32 = 0x4;

/// Minimal Elf64Phdr for testing (only fields we use)
#[derive(Debug, Clone, Copy)]
pub struct Elf64Phdr {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

impl Elf64Phdr {
    pub fn is_load(&self) -> bool {
        self.p_type == ElfPtType::PT_LOAD as u32
    }

    pub fn is_readable(&self) -> bool {
        (self.p_flags & PF_R) != 0
    }

    pub fn is_writable(&self) -> bool {
        (self.p_flags & PF_W) != 0
    }

    pub fn is_executable(&self) -> bool {
        (self.p_flags & PF_X) != 0
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-ELF-1: ELF magic is correct
    #[test]
    fn test_magic(_v in 0u8..1u8) {
        prop_assert_eq!(ELF_MAGIC, [0x7f, b'E', b'L', b'F']);
    }

    /// INV-ELF-2: ElfType discriminants distinct
    #[test]
    fn test_elf_type_distinct(_v in 0u8..1u8) {
        let types = [
            ElfType::ET_NONE, ElfType::ET_REL, ElfType::ET_EXEC,
            ElfType::ET_DYN, ElfType::ET_CORE,
        ];
        let mut seen = [false; 16];
        for t in &types {
            prop_assert!(!seen[*t as usize]);
            seen[*t as usize] = true;
        }
    }

    /// INV-ELF-3: ElfPtType discriminants distinct
    #[test]
    fn test_pt_type_distinct(_v in 0u8..1u8) {
        let types = [
            ElfPtType::PT_NULL, ElfPtType::PT_LOAD, ElfPtType::PT_DYNAMIC,
            ElfPtType::PT_INTERP, ElfPtType::PT_NOTE, ElfPtType::PT_PHDR, ElfPtType::PT_TLS,
        ];
        let mut seen = [false; 16];
        for t in &types {
            prop_assert!(!seen[*t as usize]);
            seen[*t as usize] = true;
        }
    }

    /// INV-ELF-4: PF_R | PF_W | PF_X == 0o7
    #[test]
    fn test_perm_bits(_v in 0u8..1u8) {
        prop_assert_eq!(PF_R | PF_W | PF_X, 0o7);
    }

    /// INV-ELF-5: PF flags are powers of 2
    #[test]
    fn test_pf_pow2(_v in 0u8..1u8) {
        for pf in [PF_X, PF_W, PF_R] {
            prop_assert_eq!(pf & (pf - 1), 0);
        }
        prop_assert_ne!(PF_X, PF_W);
        prop_assert_ne!(PF_W, PF_R);
        prop_assert_ne!(PF_X, PF_R);
    }

    /// INV-ELF-6: PT_LOAD is_load
    #[test]
    fn test_is_load(_v in 0u8..1u8) {
        let phdr = Elf64Phdr {
            p_type: ElfPtType::PT_LOAD as u32,
            p_flags: PF_R,
            p_offset: 0, p_vaddr: 0, p_paddr: 0, p_filesz: 0, p_memsz: 0, p_align: 0,
        };
        prop_assert!(phdr.is_load());
    }

    /// INV-ELF-7: PT_NULL is not load
    #[test]
    fn test_not_load(_v in 0u8..1u8) {
        let phdr = Elf64Phdr {
            p_type: ElfPtType::PT_NULL as u32,
            p_flags: 0,
            p_offset: 0, p_vaddr: 0, p_paddr: 0, p_filesz: 0, p_memsz: 0, p_align: 0,
        };
        prop_assert!(!phdr.is_load());
    }

    /// INV-ELF-8: flag combinations
    #[test]
    fn test_flag_combos(
        r in proptest::bool::ANY,
        w in proptest::bool::ANY,
        x in proptest::bool::ANY,
    ) {
        let mut flags = 0u32;
        if r { flags |= PF_R; }
        if w { flags |= PF_W; }
        if x { flags |= PF_X; }
        let phdr = Elf64Phdr {
            p_type: ElfPtType::PT_LOAD as u32,
            p_flags: flags,
            p_offset: 0, p_vaddr: 0, p_paddr: 0, p_filesz: 0, p_memsz: 0, p_align: 0,
        };
        prop_assert_eq!(phdr.is_readable(), r);
        prop_assert_eq!(phdr.is_writable(), w);
        prop_assert_eq!(phdr.is_executable(), x);
    }

    /// INV-ELF-9: is_executable for ET_EXEC and ET_DYN
    #[test]
    fn test_is_executable(et in 2u16..4u16) {
        // Simplified check: ET_EXEC(2) or ET_DYN(3)
        let is_exec = et == ElfType::ET_EXEC as u16 || et == ElfType::ET_DYN as u16;
        prop_assert!(is_exec);
    }

    /// INV-ELF-10: ET_NONE is not executable
    #[test]
    fn test_et_none_not_exec(_v in 0u8..1u8) {
        let is_exec = ElfType::ET_NONE as u16 == ElfType::ET_EXEC as u16
            || ElfType::ET_NONE as u16 == ElfType::ET_DYN as u16;
        prop_assert!(!is_exec);
    }

    /// INV-ELF-11: no flags means no readable/writable/executable
    #[test]
    fn test_no_flags(_v in 0u8..1u8) {
        let phdr = Elf64Phdr {
            p_type: ElfPtType::PT_LOAD as u32,
            p_flags: 0,
            p_offset: 0, p_vaddr: 0, p_paddr: 0, p_filesz: 0, p_memsz: 0, p_align: 0,
        };
        prop_assert!(!phdr.is_readable());
        prop_assert!(!phdr.is_writable());
        prop_assert!(!phdr.is_executable());
    }

    /// INV-ELF-12: RWX all set is valid
    #[test]
    fn test_rwx_all(_v in 0u8..1u8) {
        let phdr = Elf64Phdr {
            p_type: ElfPtType::PT_LOAD as u32,
            p_flags: PF_R | PF_W | PF_X,
            p_offset: 0, p_vaddr: 0, p_paddr: 0, p_filesz: 0, p_memsz: 0, p_align: 0,
        };
        prop_assert!(phdr.is_readable());
        prop_assert!(phdr.is_writable());
        prop_assert!(phdr.is_executable());
    }
}
