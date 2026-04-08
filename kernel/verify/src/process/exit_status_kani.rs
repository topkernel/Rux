//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for wait status encoding (POSIX ABI).
//!
//! Types copied from: kernel/src/process/exit.rs

#![cfg(kani)]

pub fn encode_exit_status(exit_code: i32) -> i32 {
    (((exit_code as u32) & 0xFF) << 8) as i32
}

pub fn encode_signal_status(signal: i32) -> i32 {
    (signal as u32 & 0x7F) as i32
}

pub fn encode_stopped_status(stop_sig: u32) -> i32 {
    (((stop_sig as u32) << 8) | 0x7F) as i32
}

pub fn wifexited(status: i32) -> bool { (status & 0x7F) == 0 }
pub fn wexitstatus(status: i32) -> i32 { (status >> 8) & 0xFF }
pub fn wifsignaled(status: i32) -> bool {
    ((status & 0x7F) + 1) >> 1 > 0 && !wifstopped(status)
}
pub fn wifstopped(status: i32) -> bool { (status & 0xFF) == 0x7F }
pub fn wtermsig(status: i32) -> i32 { status & 0x7F }
pub fn wstopsig(status: i32) -> i32 { (status >> 8) & 0xFF }

/// INV-EXIT-K1: encode_exit_status + wifexited/wexitstatus roundtrip for 0..255.
#[kani::proof]
fn verify_exit_roundtrip() {
    let exit_code: i32 = kani::any();
    kani::assume(exit_code >= 0 && exit_code < 256);
    let status = encode_exit_status(exit_code);
    assert!(wifexited(status));
    assert_eq!(wexitstatus(status), exit_code);
    assert!(!wifsignaled(status));
    assert!(!wifstopped(status));
}

/// INV-EXIT-K2: encode_signal_status + wifsignaled/wtermsig roundtrip for 1..63.
#[kani::proof]
fn verify_signal_roundtrip() {
    let signal: i32 = kani::any();
    kani::assume(signal >= 1 && signal < 64);
    let status = encode_signal_status(signal);
    assert!(wifsignaled(status));
    assert_eq!(wtermsig(status), signal);
    assert!(!wifexited(status));
    assert!(!wifstopped(status));
}

/// INV-EXIT-K3: encode_stopped_status + wifstopped/wstopsig roundtrip for 1..31.
#[kani::proof]
fn verify_stopped_roundtrip() {
    let stop_sig: u32 = kani::any();
    kani::assume(stop_sig >= 1 && stop_sig < 32);
    let status = encode_stopped_status(stop_sig);
    assert!(wifstopped(status));
    assert_eq!(wstopsig(status), stop_sig as i32);
    assert!(!wifexited(status));
    assert!(!wifsignaled(status));
}

/// INV-EXIT-K4: wifexited and wifsignaled are mutually exclusive for valid encodings.
#[kani::proof]
fn verify_exit_signal_exclusive() {
    let exit_code: i32 = kani::any();
    let signal: i32 = kani::any();
    kani::assume(exit_code >= 0 && exit_code < 256);
    kani::assume(signal >= 1 && signal < 64);

    let exit_st = encode_exit_status(exit_code);
    let sig_st = encode_signal_status(signal);
    assert!(wifexited(exit_st) != wifsignaled(exit_st));
    assert!(wifsignaled(sig_st) != wifexited(sig_st));
}
