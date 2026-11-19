//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for IP checksum (RFC 1071).
//!
//! Types copied from: kernel/src/net/ipv4/checksum.rs

#![cfg(kani)]

pub fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i < data.len() {
        if i + 1 == data.len() {
            sum += (data[i] as u32) << 8;
        } else {
            let word = u16::from_be_bytes([data[i], data[i + 1]]) as u32;
            sum += word;
        }
        i += 2;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !sum as u16
}

pub fn verify_ip_checksum(data: &[u8]) -> bool {
    ip_checksum(data) == 0
}

/// INV-CSUM-K1: checksum of empty data is 0xFFFF.
#[kani::proof]
fn verify_zero_length() {
    let data: [u8; 0] = [];
    assert_eq!(ip_checksum(&data), 0xFFFF);
}

/// INV-CSUM-K2: single byte checksum formula.
#[kani::proof]
fn verify_single_byte() {
    let val: u8 = kani::any();
    let data = [val];
    let csum = ip_checksum(&data);
    assert_eq!(csum, !((val as u32) << 8) as u16);
}

/// INV-CSUM-K3: even-length 2-byte data checksum.
#[kani::proof]
fn verify_even_two_bytes() {
    let hi: u8 = kani::any();
    let lo: u8 = kani::any();
    let data = [hi, lo];
    let csum = ip_checksum(&data);
    assert_eq!(csum, !(u16::from_be_bytes([hi, lo]) as u32) as u16);
}

/// INV-CSUM-K4: verify property — appending checksum yields 0 for 3 words.
#[kani::proof]
fn verify_verify_property() {
    let w0: u16 = kani::any();
    let w1: u16 = kani::any();
    let w2: u16 = kani::any();
    let mut data = Vec::with_capacity(8);
    data.extend_from_slice(&w0.to_be_bytes());
    data.extend_from_slice(&w1.to_be_bytes());
    data.extend_from_slice(&w2.to_be_bytes());
    let csum = ip_checksum(&data);
    data.extend_from_slice(&csum.to_be_bytes());
    assert!(verify_ip_checksum(&data));
}

/// INV-CSUM-K5: 4-word verify property (carry folding test).
#[kani::proof]
fn verify_carry_fold() {
    let w0: u16 = kani::any();
    let w1: u16 = kani::any();
    let w2: u16 = kani::any();
    let w3: u16 = kani::any();
    let mut data = Vec::new();
    data.extend_from_slice(&w0.to_be_bytes());
    data.extend_from_slice(&w1.to_be_bytes());
    data.extend_from_slice(&w2.to_be_bytes());
    data.extend_from_slice(&w3.to_be_bytes());
    let csum = ip_checksum(&data);
    data.extend_from_slice(&csum.to_be_bytes());
    assert!(verify_ip_checksum(&data));
}

/// INV-CSUM-K6: all-zeros checksum is 0xFFFF.
#[kani::proof]
fn verify_all_zeros() {
    let len: usize = kani::any();
    kani::assume(len >= 2 && len <= 64 && len % 2 == 0);
    let data = vec![0u8; len];
    assert_eq!(ip_checksum(&data), 0xFFFF);
}
