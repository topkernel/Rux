//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Runtime kernel symbol table — the backing store for Linux's
//! `/proc/kallsyms` and for symbolized stack traces.
//!
//! The table is produced at BUILD time: `build/gen-ksyms.py` runs `nm`
//! over the linked kernel ELF, keeps the text symbols and resolves their
//! demangled names from the DWARF info (`addr2line`), then writes
//! `ksyms_generated.rs`.
//!
//! Two properties make that work:
//!
//! - Only TEXT symbols go into the table, so the table itself lives in
//!   `.rodata` and the `.text` addresses it records stay identical
//!   across the two-pass build (the table's own size does not move
//!   `.text`).
//! - The generated file records the target it was built for, so a tree
//!   rebuilt for the other architecture cannot symbolize against the
//!   wrong addresses — the table is then simply ignored.
//!
//! The demangled names matter: the kernel is built with Rust's v0
//! mangling, so the raw ELF names (`_RNvMs3_NtNtCslPv9FFGBN4x_3rux...`)
//! are unreadable. The generator resolves them from the debug info.

use alloc::format;
use alloc::string::{String, ToString};

#[path = "ksyms_generated.rs"]
mod generated;

/// The target triple the table was generated for.
pub const TARGET: &str = generated::TARGET;

/// The symbol names, NUL-separated, in index order.
///
/// The names live in the sibling binary file that the build's symbol pass
/// writes; `include_bytes!` embeds them exactly once (a byte-string
/// literal would be emitted twice into .rodata).
pub static NAMES: &[u8] = include_bytes!("ksyms.bin");

/// The symbol index: (name offset into NAMES, address, size, ELF type).
pub static INDEX: &[(u32, u64, u64, u8)] = generated::INDEX;

/// Is the table usable for the running kernel?
///
/// A table generated for the other architecture has addresses that match
/// nothing — worse than no table, because a lookup would land on a
/// neighbouring symbol. The target check rejects it outright.
pub fn table_valid() -> bool {
    if INDEX.is_empty() {
        return false;
    }
    #[cfg(target_arch = "riscv64")]
    const WANT: &str = "riscv64";
    #[cfg(target_arch = "x86_64")]
    const WANT: &str = "x86_64";
    TARGET.starts_with(WANT)
}

/// The name of the symbol at `index` (the name blob entry, NUL ended).
fn name_at(index: usize) -> &'static str {
    let (off, _addr, _size, _typ) = INDEX[index];
    let start = off as usize;
    let rest = &NAMES[start..];
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    core::str::from_utf8(&rest[..end]).unwrap_or("?")
}

/// The name of the symbol at `index`, verbatim.
///
/// The build's symbol pass stores the full demangled name (e.g.
/// "rux::arch::trap::do_trap" for kernel symbols, "memset" for C/asm
/// ones); nothing is added at print time so /proc/kallsyms and
/// backtraces show the symbol's real name, like Linux.
pub fn symbol_name(index: usize) -> &'static str {
    name_at(index)
}

/// Find the symbol that contains `pc`.
///
/// Returns (name, symbol address, symbol size). `None` when the
/// address is outside every known symbol.
pub fn lookup(pc: u64) -> Option<(&'static str, u64, u64)> {
    if !table_valid() {
        return None;
    }
    // Binary search for the last symbol whose address is <= pc.
    let mut lo = 0usize;
    let mut hi = INDEX.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        if INDEX[mid].1 <= pc {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        return None;
    }
    let idx = lo - 1;
    let (_off, addr, size, _typ) = INDEX[idx];
    let name = name_at(idx);
    // A known size that ends before the pc means the address is in the
    // gap between two functions (padding or a symbol we do not have).
    if size > 0 && pc >= addr.saturating_add(size) {
        return None;
    }
    Some((name, addr, size))
}

/// Linux-style symbol rendering: `func+0x1a/0x40`.
///
/// Linux prints the function name, the offset into it and the function
/// size, followed by `?` when the address could not be resolved. The
/// name is the stored symbol name verbatim — no prefix.
pub fn format_symbol(pc: u64) -> String {
    match lookup(pc) {
        Some((name, addr, size)) => {
            let off = pc - addr;
            if size > 0 {
                format!("{}+0x{:x}/0x{:x}", name, off, size)
            } else {
                format!("{}+0x{:x}", name, off)
            }
        }
        None => "?".to_string(),
    }
}
