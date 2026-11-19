#!/usr/bin/env python3
"""Validate the kernel exception table (.ex_table) against disassembly.

Every .ex_table entry must point at a memory-access instruction (load/store)
inside uaccess code, and its fixup must point at executable code. This guards
against the label-placement bug where an entry ends up covering the
instruction AFTER the intended fault site (see code-review-2026-09-12.md,
ARCH-C1 / fix-plan task 1.1).

Usage: python3 scripts/verify_exception_table.py [path-to-elf]
"""

import re
import shutil
import subprocess
import sys

DEFAULT_ELF = "target/riscv64gc-unknown-none-elf/debug/rux"


def find_tool(base: str) -> str:
    """Return a tool that understands riscv64 ELF (cross prefix or llvm)."""
    for cand in (
        f"riscv64-linux-gnu-{base}",
        f"riscv64-unknown-elf-{base}",
        f"llvm-{base}",
        base,
    ):
        path = shutil.which(cand)
        if path:
            return path
    sys.exit(f"verify_ex_table: no usable '{base}' found in PATH")

# RISC-V load/store mnemonics (base ISA, RV64 incl. compressed forms)
MEM_OPS = re.compile(
    r"^\s*(lb|lh|lw|ld|lbu|lhu|lwu|sb|sh|sw|sd|flw|fld|fsw|fsd"
    r"|c\.lw|c\.ld|c\.sw|c\.sd|c\.lwsp|c\.ldsp|c\.swsp|c\.sdsp)\b",
    re.IGNORECASE,
)


def read_ex_table(elf: str):
    """Return [(insn_addr, fixup_addr)] from the raw .ex_table section."""
    import tempfile

    with tempfile.NamedTemporaryFile(suffix=".bin") as f:
        subprocess.run(
            [find_tool("objcopy"), "-O", "binary", "--only-section=.ex_table", elf, f.name],
            check=True,
        )
        data = f.read()
    if len(data) % 16 != 0:
        sys.exit(f"verify_ex_table: .ex_table size {len(data)} not a multiple of 16")
    entries = []
    for i in range(0, len(data), 16):
        insn = int.from_bytes(data[i : i + 8], "little")
        fixup = int.from_bytes(data[i + 8 : i + 16], "little")
        entries.append((insn, fixup))
    return entries


def disassemble(elf: str):
    """Return {addr: mnemonic} for all executable sections."""
    out = subprocess.run(
        [find_tool("objdump"), "-d", "--no-show-raw-insn", elf],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    insns = {}
    for line in out.splitlines():
        m = re.match(r"^\s*([0-9a-f]+):\s+(\S+)", line)
        if m:
            insns[int(m.group(1), 16)] = m.group(2)
    return insns


def main():
    elf = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_ELF
    entries = read_ex_table(elf)
    insns = disassemble(elf)
    if not entries:
        sys.exit("verify_ex_table: no .ex_table entries found (section missing?)")

    errors = 0
    for insn, fixup in entries:
        mnem = insns.get(insn)
        if mnem is None:
            print(f"ERROR: ex_table insn {insn:#x} does not match any instruction")
            errors += 1
        elif not MEM_OPS.match(mnem):
            print(
                f"ERROR: ex_table insn {insn:#x} points at '{mnem}', "
                "not a load/store — label misplacement?"
            )
            errors += 1
        if fixup not in insns:
            print(f"ERROR: ex_table fixup {fixup:#x} does not match any instruction")
            errors += 1

    print(f"verify_ex_table: {len(entries)} entries checked, {errors} errors")
    sys.exit(1 if errors else 0)


if __name__ == "__main__":
    main()
