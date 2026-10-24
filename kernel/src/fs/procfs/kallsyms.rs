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

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Generate /proc/kallsyms content.
pub fn generate() -> Vec<u8> {
    let mut out = String::new();
    if crate::dfx::ksyms::table_valid() {
        for (idx, (_off, addr, _size, typ)) in crate::dfx::ksyms::INDEX.iter().enumerate() {
            let name = crate::dfx::ksyms::symbol_name(idx);
            out.push_str(&format!("{:016x} {} rux::{}\n", addr, *typ as char, name));
        }
    }
    out.into_bytes()
}
