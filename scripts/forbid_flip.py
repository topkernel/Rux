#!/usr/bin/env python3
# MIT License
# Copyright (c) 2026 Fei Wang
"""Forbid-flip scanner: find kernel/core/src .rs modules that carry zero
`unsafe` tokens (outside comments) and do not yet carry
`#![forbid(unsafe_code)]`, then insert the attribute at the module-head
position (after leading // and //! comments and existing inner attrs).

Compiler-verified downstream: any file the attribute breaks (cfg-gated
unsafe on another arch, macro-generated unsafe) is dropped by the driver
loop; this scan only picks candidates.

Usage: python3 run/flip.py [--dry]
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "kernel" / "core" / "src"
ATTR = "#![forbid(unsafe_code)]"

# Generated / non-module files; the tests harness is cfg(test)-gated and
# include-shaped — leave it alone this round.
EXCLUDE_PARTS = ("tests",)
EXCLUDE_NAMES = {"config.rs"} | {p.name for p in SRC.glob("*generated*")}


def strip_comments(text: str) -> str:
    """Remove // line comments and /* */ block comments (nesting-aware).
    Not string-literal aware; conservative for candidate SELECTION only —
    the compiler is the authority."""
    out = []
    i, n = 0, len(text)
    depth = 0
    while i < n:
        if depth == 0 and text.startswith("//", i):
            j = text.find("\n", i)
            i = n if j < 0 else j
        elif text.startswith("/*", i):
            depth += 1
            i += 2
        elif depth and text.startswith("*/", i):
            depth -= 1
            i += 2
        else:
            if depth == 0:
                out.append(text[i])
            i += 1
    return "".join(out)


def insert_pos(lines: list[str]) -> int:
    """First line index after the leading prologue: blank lines, plain //
    and //! comments, /* */ blocks (incl. continuation lines), and inner
    attributes (#!). STOPS at outer doc comments (///, /**), outer
    attributes (#[), and any item — an inner attribute must precede all
    of those (E0753)."""
    i = 0
    while i < len(lines):
        s = lines[i].strip()
        if not s or s.startswith("#!") or (s.startswith("//") and not s.startswith("///")):
            i += 1
            continue
        if s.startswith("/*") and not s.startswith("/**"):
            i += 1
            while i < len(lines) and "*/" not in lines[i]:
                i += 1
            i += 1  # past the closing */
            continue
        if s.startswith("*"):
            i += 1  # inside a /* */ block's continuation
            continue
        break
    return i


def main() -> None:
    dry = "--dry" in sys.argv
    candidates, flipped = [], 0
    for p in sorted(SRC.rglob("*.rs")):
        rel = p.relative_to(SRC)
        if any(part in EXCLUDE_PARTS for part in rel.parts[:-1]):
            continue
        if p.name in EXCLUDE_NAMES or p.name.endswith("_generated.rs"):
            continue
        text = p.read_text(encoding="utf-8", errors="replace")
        if ATTR in text:
            continue
        # Existing allow(unsafe_code) markers conflict with forbid (E0453)
        # and signal intent to re-add unsafe — excluded this round; the
        # owning lane cleans them up.
        if "allow(unsafe_code)" in text:
            continue
        # An inner attribute in mod.rs covers the WHOLE subtree (child
        # modules are items of that module) — only flip a mod.rs when
        # every .rs file under its directory is zero-unsafe too.
        if p.name == "mod.rs":
            subtree_clean = True
            for q in p.parent.rglob("*.rs"):
                if q == p:
                    continue
                qt = q.read_text(encoding="utf-8", errors="replace")
                if ATTR in qt or "allow(unsafe_code)" in qt:
                    subtree_clean = False
                    break
                if re.search(r"\bunsafe\b", strip_comments(qt)):
                    subtree_clean = False
                    break
            if not subtree_clean:
                continue
        body = strip_comments(text)
        if re.search(r"\bunsafe\b", body):
            continue
        candidates.append(rel)
        if not dry:
            lines = text.splitlines(keepends=True)
            k = insert_pos(lines)
            lines.insert(k, ATTR + "\n")
            p.write_text("".join(lines), encoding="utf-8")
            flipped += 1
    for rel in candidates:
        print(rel)
    print(f"-- {len(candidates)} candidates, {flipped} flipped")


if __name__ == "__main__":
    main()
