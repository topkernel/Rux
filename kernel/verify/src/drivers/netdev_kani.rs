//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for network device flags and operations.
//!
//! Types copied from: kernel/src/drivers/net/space.rs

#![cfg(kani)]

pub mod dev_flags {
    pub const IFF_UP: u32 = 0x1;
    pub const IFF_BROADCAST: u32 = 0x2;
    pub const IFF_LOOPBACK: u32 = 0x8;
    pub const IFF_RUNNING: u32 = 0x40;
    pub const IFF_MULTICAST: u32 = 0x1000;
}

pub struct NetDeviceFlags {
    pub flags: u32,
}

impl NetDeviceFlags {
    pub fn new() -> Self { Self { flags: 0 } }
    pub fn up(&mut self) { self.flags |= dev_flags::IFF_UP | dev_flags::IFF_RUNNING; }
    pub fn down(&mut self) { self.flags &= !(dev_flags::IFF_UP | dev_flags::IFF_RUNNING); }
    pub fn is_up(&self) -> bool { (self.flags & dev_flags::IFF_UP) != 0 }
    pub fn is_running(&self) -> bool { (self.flags & dev_flags::IFF_RUNNING) != 0 }
}

/// INV-NETDEV-K1: IFF flags are distinct powers of 2.
#[kani::proof]
fn verify_iff_flags_distinct() {
    let flags = [
        dev_flags::IFF_UP, dev_flags::IFF_BROADCAST, dev_flags::IFF_LOOPBACK,
        dev_flags::IFF_RUNNING, dev_flags::IFF_MULTICAST,
    ];
    let mut seen = 0u32;
    for &f in &flags {
        assert!(f > 0 && (f & (f - 1)) == 0);
        assert_eq!(seen & f, 0);
        seen |= f;
    }
}

/// INV-NETDEV-K2: up() sets both IFF_UP and IFF_RUNNING.
#[kani::proof]
fn verify_up_sets_both() {
    let mut dev = NetDeviceFlags::new();
    dev.up();
    assert!(dev.is_up());
    assert!(dev.is_running());
}

/// INV-NETDEV-K3: down() clears both IFF_UP and IFF_RUNNING.
#[kani::proof]
fn verify_down_clears_both() {
    let mut dev = NetDeviceFlags::new();
    dev.up();
    dev.down();
    assert!(!dev.is_up());
    assert!(!dev.is_running());
}

/// INV-NETDEV-K4: down() preserves other flags.
#[kani::proof]
fn verify_down_preserves_other() {
    let initial: u32 = kani::any();
    kani::assume(initial < 0x2000);
    let mut dev = NetDeviceFlags::new();
    dev.flags = initial;
    let other = initial & !(dev_flags::IFF_UP | dev_flags::IFF_RUNNING);
    dev.up();
    dev.down();
    assert_eq!(dev.flags & !(dev_flags::IFF_UP | dev_flags::IFF_RUNNING), other);
}
