//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for input event constants and InputEvent ABI.
//! Copied from: kernel/src/drivers/input/event.rs

use proptest::prelude::*;
use core::mem::size_of;

// Copied constants
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;
pub const EV_MSC: u16 = 0x04;

pub const REL_X: u16 = 0x00;
pub const REL_Y: u16 = 0x01;
pub const REL_WHEEL: u16 = 0x08;

pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_MT_SLOT: u16 = 0x2f;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;

pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;

pub const KEY_RELEASE: i32 = 0;
pub const KEY_PRESS: i32 = 1;
pub const KEY_REPEAT: i32 = 2;

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

    pub fn rel_event(axis: u16, value: i32) -> Self {
        Self::new(EV_REL, axis, value)
    }

    pub fn abs_event(axis: u16, value: i32) -> Self {
        Self::new(EV_ABS, axis, value)
    }

    pub fn sync_event() -> Self {
        Self::new(EV_SYN, 0, 0)
    }
}

proptest! {
    #[test]
    fn test_struct_size(_v in 0u8..1u8) {
        // Linux input_event is 24 bytes
        assert_eq!(size_of::<InputEvent>(), 24);
    }

    #[test]
    fn test_ev_type_distinct(_v in 0u8..1u8) {
        let types = [EV_SYN, EV_KEY, EV_REL, EV_ABS, EV_MSC];
        for i in 0..types.len() {
            for j in (i+1)..types.len() {
                assert_ne!(types[i], types[j]);
            }
        }
    }

    #[test]
    fn test_ev_type_sequential(_v in 0u8..1u8) {
        assert_eq!(EV_SYN, 0);
        assert_eq!(EV_KEY, 1);
        assert_eq!(EV_REL, 2);
        assert_eq!(EV_ABS, 3);
        assert_eq!(EV_MSC, 4);
    }

    #[test]
    fn test_key_event_press(code in 0u16..256u16) {
        let ev = InputEvent::key_event(code, true);
        assert_eq!(ev.type_, EV_KEY);
        assert_eq!(ev.code, code);
        assert_eq!(ev.value, KEY_PRESS);
    }

    #[test]
    fn test_key_event_release(code in 0u16..256u16) {
        let ev = InputEvent::key_event(code, false);
        assert_eq!(ev.type_, EV_KEY);
        assert_eq!(ev.code, code);
        assert_eq!(ev.value, KEY_RELEASE);
    }

    #[test]
    fn test_rel_event(axis in 0u16..16u16, val in -1000i32..1000i32) {
        let ev = InputEvent::rel_event(axis, val);
        assert_eq!(ev.type_, EV_REL);
        assert_eq!(ev.code, axis);
        assert_eq!(ev.value, val);
    }

    #[test]
    fn test_abs_event(axis in 0u16..64u16, val in -10000i32..10000i32) {
        let ev = InputEvent::abs_event(axis, val);
        assert_eq!(ev.type_, EV_ABS);
        assert_eq!(ev.code, axis);
        assert_eq!(ev.value, val);
    }

    #[test]
    fn test_sync_event(_v in 0u8..1u8) {
        let ev = InputEvent::sync_event();
        assert_eq!(ev.type_, EV_SYN);
        assert_eq!(ev.code, 0);
        assert_eq!(ev.value, 0);
    }

    #[test]
    fn test_btn_codes_distinct(_v in 0u8..1u8) {
        let btns = [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE];
        for i in 0..btns.len() {
            for j in (i+1)..btns.len() {
                assert_ne!(btns[i], btns[j]);
            }
        }
    }

    #[test]
    fn test_btn_range(_v in 0u8..1u8) {
        // Mouse buttons start at 0x110
        assert!(BTN_LEFT >= 0x100);
        assert!(BTN_RIGHT == BTN_LEFT + 1);
        assert!(BTN_MIDDLE == BTN_LEFT + 2);
    }

    #[test]
    fn test_abs_mt_constants(_v in 0u8..1u8) {
        assert!(ABS_MT_SLOT > ABS_Y);
        assert!(ABS_MT_POSITION_X > ABS_MT_SLOT);
        assert!(ABS_MT_POSITION_Y > ABS_MT_POSITION_X);
    }

    #[test]
    fn test_new_roundtrip(type_ in 0u16..16u16, code in 0u16..512u16, value in -10000i32..10000i32) {
        let ev = InputEvent::new(type_, code, value);
        assert_eq!(ev.type_, type_);
        assert_eq!(ev.code, code);
        assert_eq!(ev.value, value);
    }

    #[test]
    fn test_new_zero_timestamp(type_ in 0u16..16u16, code in 0u16..512u16, value in -10000i32..10000i32) {
        let ev = InputEvent::new(type_, code, value);
        assert_eq!(ev.tv_sec, 0);
        assert_eq!(ev.tv_usec, 0);
    }
}
