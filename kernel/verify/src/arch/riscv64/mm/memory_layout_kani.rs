//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for VirtAddr operations (Sv39).
//!
//! Types copied from: kernel/src/arch/riscv64/mm/memory_layout.rs

#![cfg(kani)]

pub const PAGE_SIZE: u64 = 4096;
pub const PAGE_SHIFT: u64 = 12;
pub const PAGE_OFFSET_MASK: u64 = (1 << PAGE_SHIFT) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct VirtAddr(pub u64);

impl VirtAddr {
    pub const fn new(addr: u64) -> Self {
        let bit38 = (addr >> 38) & 1;
        if bit38 == 1 {
            Self(addr | 0xFFFFFFC0_00000000)
        } else {
            Self(addr & 0x0000007F_FFFFFFFF)
        }
    }

    pub fn bits(&self) -> u64 { self.0 }

    pub fn is_aligned(&self) -> bool { self.0 & PAGE_OFFSET_MASK == 0 }

    pub fn floor(&self) -> Self { Self(self.0 & !PAGE_OFFSET_MASK) }

    pub fn ceil(&self) -> Self { Self((self.0 + PAGE_SIZE - 1) & !PAGE_OFFSET_MASK) }

    pub fn page_offset(&self) -> u64 { self.0 & PAGE_OFFSET_MASK }

    pub fn vpn(&self, level: u8) -> u64 {
        (self.0 >> (PAGE_SHIFT + 9 * level as u64)) & 0x1FF
    }
}

/// INV-VA-K1: user address (bit 38 = 0) clears upper bits.
#[kani::proof]
fn verify_user_sign_extend() {
    let addr: u64 = kani::any();
    kani::assume(addr < 0x3FFFFFFFFF_u64); // bit 38 = 0
    let va = VirtAddr::new(addr);
    assert_eq!(va.bits() & 0xFFFFFFC0_00000000, 0);
}

/// INV-VA-K2: kernel address (bit 38 = 1) sets upper bits.
#[kani::proof]
fn verify_kernel_sign_extend() {
    let low: u64 = kani::any();
    kani::assume(low < 0x4000000000_u64);
    let addr = 0x4000000000_u64 + low; // bit 38 = 1
    let va = VirtAddr::new(addr);
    assert_eq!(va.bits() & 0xFFFFFFC0_00000000, 0xFFFFFFC0_00000000);
}

/// INV-VA-K3: VPN always returns 9-bit value (0..511).
#[kani::proof]
fn verify_vpn_9bit() {
    let addr: u64 = kani::any();
    let level: u8 = kani::any();
    kani::assume(level < 3);
    let va = VirtAddr::new(addr);
    let vpn = va.vpn(level);
    assert!(vpn < 512);
}

/// INV-VA-K4: floor(addr) <= addr for all addresses.
#[kani::proof]
fn verify_floor_le() {
    let addr: u64 = kani::any();
    let va = VirtAddr::new(addr);
    assert!(va.floor().bits() <= addr);
}

/// INV-VA-K5: ceil(addr) >= addr for all addresses.
#[kani::proof]
fn verify_ceil_ge() {
    let addr: u64 = kani::any();
    let va = VirtAddr::new(addr);
    assert!(va.ceil().bits() >= addr);
}

/// INV-VA-K6: page_offset extracts low 12 bits.
#[kani::proof]
fn verify_page_offset() {
    let addr: u64 = kani::any();
    kani::assume(addr < 0x100000_u64);
    let va = VirtAddr::new(addr);
    assert_eq!(va.page_offset(), addr & 0xFFF);
}

/// INV-VA-K7: floor of page-aligned address is itself.
#[kani::proof]
fn verify_floor_aligned() {
    let frame: u64 = kani::any();
    kani::assume(frame < 1000);
    let addr = frame << PAGE_SHIFT;
    let va = VirtAddr::new(addr);
    assert_eq!(va.floor(), va);
}
