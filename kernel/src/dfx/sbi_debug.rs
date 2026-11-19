//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! SBI direct print for low-level debugging.
//!
//! These functions bypass all kernel locks and console infrastructure,
//! using raw SBI ecall to output characters. Safe to call from any
//! context: interrupt handlers, spinlock critical sections, early boot.

/// Print a single byte via SBI putchar.
#[inline]
pub fn sbi_putc(c: u8) {
    unsafe {
        core::arch::asm!(
            "li a7, 1",
            "mv a0, {0}",
            "ecall",
            in(reg) c,
            out("a0") _,
            options(nomem, nostack)
        );
    }
}

/// Print a single hex nibble (0–f) via SBI.
pub fn sbi_put_hex(v: u8) {
    let c = if v < 10 { b'0' + v } else { b'a' + v - 10 };
    sbi_putc(c);
}

/// Print a u32 as decimal via SBI.
pub fn sbi_put_dec(mut v: u32) {
    if v == 0 {
        sbi_putc(b'0');
        return;
    }
    let mut buf = [0u8; 10];
    let mut i = 0;
    while v > 0 {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    for j in (0..i).rev() {
        sbi_putc(buf[j]);
    }
}

/// Print a u64 as hexadecimal via SBI (with "0x" prefix).
pub fn sbi_put_hex64(v: u64) {
    sbi_putc(b'0');
    sbi_putc(b'x');
    for i in (0..16).rev() {
        sbi_put_hex(((v >> (i * 4)) & 0xf) as u8);
    }
}

/// Print a string prefixed by the current CPU ID hex nibble.
///
/// Example output on CPU 2: `2<msg>`
pub fn sbi_dbg(s: &str) {
    let cpu = crate::arch::cpu_id() as usize;
    sbi_put_hex((cpu & 0xf) as u8);
    for &b in s.as_bytes() {
        sbi_putc(b);
    }
}

/// Print a string via SBI (no CPU prefix).
pub fn sbi_print(s: &str) {
    for &b in s.as_bytes() {
        sbi_putc(b);
    }
}
