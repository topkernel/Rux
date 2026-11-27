//! Fixture with known unsafe content for the meter self-test.
//! Parsed by the meter, never compiled. Line numbers are load-bearing:
//! tests/selftest.rs and the `--selftest` flag assert exact line sets.

use core::arch::asm;

bitflags! {
    struct F: u32 { const A = 1; }
}

unsafe fn plain() {
    asm!("nop");
}

fn outer() {
    unsafe {
        let x = 1;
        unsafe {
            asm!("nop");
        }
        let _ = x;
    }
}

unsafe trait T {
    fn m(&self);
}

unsafe impl T for u32 {
    fn m(&self) {}
}

extern "C" {
    fn ext(x: u32);
}

unsafe extern "C" {
    fn ext2(x: u32);
}

macro_rules! m {
    () => {
        unsafe { 1 }
    };
}

fn uses_macro_and_block() {
    let y = unsafe { 2 };
    let _ = m!();
    let _ = y;
}
