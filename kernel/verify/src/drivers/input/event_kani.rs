//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for input event ABI constants and struct size.
//!
//! Types copied from: kernel/src/drivers/input/event.rs

#![cfg(kani)]

pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;
pub const KEY_RELEASE: i32 = 0;
pub const KEY_PRESS: i32 = 1;
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct InputEvent {
    pub tv_sec: u64,
    pub tv_usec: u64,
    pub type_: u16,
    pub code: u16,
    pub value: i32,
}

impl InputEvent {
    pub fn new(type_: u16, code: u16, value: i32) -> Self {
        Self { tv_sec: 0, tv_usec: 0, type_, code, value }
    }
    pub fn key_event(code: u16, pressed: bool) -> Self {
        Self::new(EV_KEY, code, if pressed { KEY_PRESS } else { KEY_RELEASE })
    }
    pub fn sync_event() -> Self { Self::new(EV_SYN, 0, 0) }
}

/// INV-INPUT-K1: InputEvent is 24 bytes (Linux ABI).
#[kani::proof]
fn verify_struct_size() {
    assert_eq!(core::mem::size_of::<InputEvent>(), 24);
}

/// INV-INPUT-K2: EV types are 0-4 sequential.
#[kani::proof]
fn verify_ev_types() {
    assert_eq!(EV_SYN, 0);
    assert_eq!(EV_KEY, 1);
    assert_eq!(EV_REL, 2);
    assert_eq!(EV_ABS, 3);
}

/// INV-INPUT-K3: key_event constructor preserves code and pressed state.
#[kani::proof]
fn verify_key_event() {
    let code: u16 = kani::any();
    let pressed: bool = kani::any();
    let ev = InputEvent::key_event(code, pressed);
    assert_eq!(ev.type_, EV_KEY);
    assert_eq!(ev.code, code);
    assert_eq!(ev.value, if pressed { KEY_PRESS } else { KEY_RELEASE });
}

/// INV-INPUT-K4: new() roundtrip preserves type, code, value.
#[kani::proof]
fn verify_new_roundtrip() {
    let type_: u16 = kani::any();
    let code: u16 = kani::any();
    let value: i32 = kani::any();
    let ev = InputEvent::new(type_, code, value);
    assert_eq!(ev.type_, type_);
    assert_eq!(ev.code, code);
    assert_eq!(ev.value, value);
}

/// INV-INPUT-K5: sync_event has type=0, code=0, value=0.
#[kani::proof]
fn verify_sync_event() {
    let ev = InputEvent::sync_event();
    assert_eq!(ev.type_, EV_SYN);
    assert_eq!(ev.code, 0);
    assert_eq!(ev.value, 0);
}

/// INV-INPUT-K6: button codes are sequential 0x110, 0x111, 0x112.
#[kani::proof]
fn verify_button_codes() {
    assert_eq!(BTN_RIGHT, BTN_LEFT + 1);
    assert_eq!(BTN_MIDDLE, BTN_LEFT + 2);
    assert!(BTN_LEFT >= 0x100);
}
