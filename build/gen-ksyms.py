#!/usr/bin/env python3
"""Generate the kernel symbol table used for runtime symbolization.

The kernel is built with Rust's v0 mangling, so the raw ELF symbol
names are unreadable ("_RNvMs3_NtNtCslPv9FFGBN4x_3rux..."). This script
therefore resolves the demangled names from the DWARF info with
addr2line and writes them next to the addresses.

Only TEXT symbols are kept: the table lives in .rodata, so its own size
does not move .text — that is what makes the two-pass build converge
(the addresses recorded here stay valid after the table is embedded).

Usage: gen-ksyms.py <kernel-elf> <target-triple> <output.rs>

The output file is written ONLY when its content changes, so the
second build pass is not retriggered forever.
"""

import subprocess
import sys
import os

# Symbol types that live in the text segment.
TEXT_TYPES = ("T", "t", "W", "w")


def tool_prefix(target: str) -> str:
    """The cross-toolchain prefix for a target triple."""
    if target.startswith("riscv64"):
        return "riscv64-linux-gnu-"
    if target.startswith("x86_64"):
        return "x86_64-linux-gnu-"
    return ""


def read_text_symbols(nm: str, elf: str):
    """(addr, size, mangled name) for every text symbol, sorted by addr."""
    out = subprocess.run(
        [nm, "-n", "--defined-only", "-S", elf],
        check=True, capture_output=True, text=True,
    ).stdout

    syms = []
    for line in out.splitlines():
        parts = line.split()
        if len(parts) < 3:
            continue
        # "addr size type name" or "addr type name"
        if len(parts) >= 4 and len(parts[1]) == 16:
            addr_s, size_s, typ, name = parts[0], parts[1], parts[2], parts[3]
        else:
            addr_s, size_s, typ, name = parts[0], "0", parts[1], parts[2]
        if typ not in TEXT_TYPES:
            continue
        # Rust's internal and LLVM's bookkeeping symbols are noise.
        if name.startswith("$") or ".llvm." in name:
            continue
        try:
            addr = int(addr_s, 16)
            size = int(size_s, 16)
        except ValueError:
            continue
        syms.append((addr, size, name, typ))

    syms.sort(key=lambda s: s[0])
    return syms


def demangle(addr2line: str, elf: str, addrs):
    """Demangled names for a list of addresses, in the same order."""
    if not addrs:
        return []
    # -C demangles: the kernel is built with Rust's v0 mangling, and
    # llvm's demangler understands it (the raw ELF names start with
    # "_RNv..." and are unreadable).
    out = subprocess.run(
        [addr2line, "-f", "-C", "-e", elf] + ["0x%x" % a for a in addrs],
        check=True, capture_output=True, text=True,
    ).stdout.splitlines()
    names = []
    # addr2line -f prints "function\nfile:line" per address.
    for i in range(0, len(out), 2):
        fn = out[i].strip()
        names.append(fn if fn and fn != "??" else None)
    while len(names) < len(addrs):
        names.append(None)
    return names[: len(addrs)]


def main() -> int:
    if len(sys.argv) != 4:
        print(__doc__, file=sys.stderr)
        return 2
    elf, target, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
    if not os.path.exists(elf):
        print("gen-ksyms: %s does not exist" % elf, file=sys.stderr)
        return 1

    prefix = tool_prefix(target)
    nm = prefix + "nm"
    addr2line = prefix + "addr2line"

    try:
        syms = read_text_symbols(nm, elf)
    except (OSError, subprocess.CalledProcessError) as exc:
        print("gen-ksyms: nm failed: %s" % exc, file=sys.stderr)
        return 1

    names = demangle(addr2line, elf, [a for a, _s, _n, _t in syms])

    # The kernel image has a hard budget: the image must end before the
    # heap base (0x80E00000 on riscv64), and the boot-time MMU window is
    # a fixed 12MB. The full table (2.3MB of names alone) does not fit,
    # so the generator embeds as much as the budget allows and says how
    # much was dropped.
    budget = int(os.environ.get("KSYMS_BUDGET_BYTES", str(1024 * 1024)))

    # The table is OPT-IN. Embedding it grows the kernel image by the
    # table size, and the image's address space is budgeted: the heap
    # base sits at a fixed physical address and the vmemmap backing
    # search treats the gap between the image end and the heap base as
    # free space (moving the image end moves that gap — see the
    # RUX-RISCV-LAYOUT notes in main.rs and memory_layout.rs). A full
    # table does not fit in the current budget, so it stays OFF until
    # the image budget is enlarged deliberately.
    enabled = os.environ.get("KSYMS_ENABLE", "0") in ("1", "yes", "true")

    # Compact form: one NUL-separated name blob plus an index of
    # (name offset, address). The symbol SIZE is derived from the next
    # symbol's address, so it costs nothing.
    blob = bytearray()
    index = []
    dropped = 0
    pairs = zip(syms, names) if enabled else ()
    for (addr, size, mangled, typ), demangled in pairs:
        # The FULL demangled name is stored (verbatim, like Linux's
        # /proc/kallsyms prints symbol names) — nothing is added back at
        # runtime. Only the Rust hash suffix ("::h0123...") is dropped;
        # it carries no information for a reader.
        name = demangled if demangled else mangled
        import re as _re
        name = _re.sub(r"::h[0-9a-f]{16}$", "", name)
        entry_cost = len(name) + 1 + 12
        if len(blob) + entry_cost > budget:
            dropped += 1
            continue
        index.append((len(blob), addr, size, typ))
        blob += name.encode("utf-8", "replace") + b"\0"

    lines = [
        "//! Generated by build/gen-ksyms.py — do not edit.",
        "//!",
        "//! The kernel symbol table for runtime symbolization",
        "//! (/proc/kallsyms, symbolized stack traces).",
        "//!",
        "//! The names live in the sibling binary file (ksyms.bin) and are",
        "//! embedded with include_bytes! — a byte-string literal would be",
        "//! emitted TWICE into .rodata (once for the promoted constant, once",
        "//! for the unsizing coercion), and the kernel image has no room for",
        "//! that.",
        "",
        "/// The target triple this table was generated for.",
        'pub const TARGET: &str = "%s";' % target,
        "",
        "/// (name offset into ksyms.bin, address, size, ELF type) per symbol,",
        "/// sorted by address.",
        "pub static INDEX: &[(u32, u64, u64, u8)] = &[",
    ]
    for (off, addr, size, typ) in index:
        lines.append("    (%d, 0x%x, 0x%x, b'%s')," % (off, addr, size, typ))
    lines.append("];")
    lines.append("")
    content = "\n".join(lines)

    blob_path = out_path.replace("ksyms_generated.rs", "ksyms.bin")

    # Write only on a change: an unchanged file keeps its mtime, so cargo
    # does not rebuild forever.
    def unchanged(path, data):
        try:
            with open(path, "rb") as handle:
                return handle.read() == data
        except OSError:
            return False

    if unchanged(out_path, content.encode()) and unchanged(blob_path, bytes(blob)):
        print("gen-ksyms: %d symbols, unchanged" % len(index))
        return 0

    with open(out_path, "w") as handle:
        handle.write(content)
    with open(blob_path, "wb") as handle:
        handle.write(bytes(blob))
    if enabled:
        print(
            "gen-ksyms: %d/%d symbols (%.0f KiB names, budget %d KiB%s) -> %s"
            % (
                len(index),
                len(syms),
                len(blob) / 1024.0,
                budget / 1024,
                ", %d dropped" % dropped if dropped else "",
                out_path,
            )
        )
    else:
        print(
            "gen-ksyms: table DISABLED (KSYMS_ENABLE=1 to embed it) -> %s" % out_path
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
