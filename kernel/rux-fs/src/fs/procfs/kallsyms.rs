//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/kallsyms — the kernel symbol table (Linux's
//! fs/proc/kallsyms.c).
//!
//! Linux's format is one symbol per line:
//!
//! ```text
//! ffffffff81000000 T _text
//! ffffffff810001a0 T startup_64
//! ```
//!
//! The address, the ELF symbol type letter and the symbol name. Rux
//! prints the DEMANGLED name: the kernel is built with Rust's v0
//! mangling, so the raw ELF names ("_RNvMs3_NtNtCslPv9FFGBN4x_3rux...")
//! would be unreadable in a file whose whole purpose is to be read.
//!
//! Linux masks the addresses with `kptr_restrict` (all zeros for
//! unprivileged readers) — Rux has no such knob and prints the real
//! addresses, which is what a debugging kernel wants.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Linux's KSYM_NAME_LEN — the maximum symbol-name length the kernel's
/// own kallsyms table can carry (NUL included), and therefore the
/// longest line fs/proc/kallsyms.c can ever emit. Rust's demangled
/// generic names run far past it (593 chars measured, 14,913 of 36,932
/// symbols over the cap on a debug build): emitting them unbounded is
/// an ABI divergence that overflows the standard consumers — LTP's
/// kallsyms test parses names into `char name[128]` with unbounded
/// `%s`, heap-overflows its symbol table, and dies in a segfault storm
/// (kallsyms FAIL->CRASH(137), convicted from the USEGV-FOR forensics
/// + objdump of rip 0x40126d). Cap at 127 visible chars like Linux.
const KSYM_NAME_LEN: usize = 128;

/// Generate /proc/kallsyms content.
pub fn generate() -> Vec<u8> {
    let mut out = String::new();
    if crate::debug::ksyms::table_valid() {
        for (idx, (_off, addr, _size, typ)) in crate::debug::ksyms::INDEX.iter().enumerate() {
            let name = crate::debug::ksyms::symbol_name(idx);
            // Linux prints the symbol name verbatim (fs/proc/kallsyms.c)
            // — the demangled Rust name IS the name; no prefix is added —
            // truncated to KSYM_NAME_LEN-1 visible chars (Linux parity:
            // its kallsyms build cannot carry longer names at all).
            let capped: &str = match name.char_indices().nth(KSYM_NAME_LEN - 1) {
                Some((cut, _)) => &name[..cut],
                None => &name[..],
            };
            out.push_str(&format!("{:016x} {} {}\n", addr, *typ as char, capped));
        }
    }
    out.into_bytes()
}
