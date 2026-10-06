#!/usr/bin/env python3
"""TCB ledger: per-subsystem `unsafe` accounting for the Rux kernel.

Funded refactor F1 (docs/development/rust-kernel-best-practices.md §3.2 /
§9): unsafe is a budget, so measure it.  This script walks kernel/src/**.rs
and reports, per subsystem directory:

  - total .rs lines
  - number of `unsafe { ... }` blocks
  - total lines inside those blocks
  - unsafe lines as a percentage of the subsystem's lines

The kernel-wide summary row is the number tracked per release in the
roadmap: it may only go down, or the increase must be justified.

# Method and approximations (documented, deliberate)

* **Line attribution is by brace matching.**  Each `unsafe {` block is
  measured from the line holding the `unsafe {` keyword to the line
  holding its matching `}` (both inclusive).  A line covered by more
  than one block counts once.  This is an approximation of "lines the
  programmer must audit as unsafe": a line inside a block that only
  holds a safe sub-expression still counts, and a one-line block
  (`let x = unsafe { *p };`) counts one line.
* **Only `unsafe { ... }` blocks are counted.**  `unsafe fn` and
  `unsafe impl` bodies are attributed to the whole function/impl, not to
  an unsafe block; counting them would attribute ordinary safe code
  inside such bodies.  The raw-pointer bodies of `unsafe fn`s are
  therefore undercounted — acceptable for a monotone budget metric.
* **Comment and literal stripping.**  Before matching, the source is
  stripped of line/block comments and of string/char/raw-string CONTENT
  (characters are blanked in place, newlines preserved) so that braces
  or `unsafe {` inside literals do not fool the matcher.  Stripping is a
  small hand-written scanner, not rustc: exotic corner cases (e.g. an
  unterminated literal) can still mis-attribute.  Good enough for a
  ledger; regenerate after any doubt.
* **Exclusions.**  `kernel/src/tests/` (the in-kernel unit-test tree)
  is excluded: test-only code does not ship in the kernel image.  There
  are currently no `#[cfg(test)]`-only source files outside it
  detectable by name; if such files appear (e.g. `*_test.rs` outside
  tests/), add them to EXCLUDE_GLOBS.

# Usage

    python3 scripts/tcb_ledger.py            # markdown table to stdout
    python3 scripts/tcb_ledger.py --json     # machine-readable JSON to stdout
    python3 scripts/tcb_ledger.py --out docs/development/tcb-ledger.md
"""

import argparse
import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
KERNEL_SRC = REPO_ROOT / "kernel" / "src"

# Files/directories excluded from the ledger (see module docstring).
EXCLUDE_DIRS = {"tests"}
EXCLUDE_GLOBS = ("*_test.rs", "*_tests.rs")  # none today; kept for symmetry

UNSAFE_BLOCK_RE = re.compile(r"\bunsafe\s*\{")


def strip_comments_and_literals(src: str) -> str:
    """Blank out comment bodies and string/char literal contents.

    Returns text of identical length: every replaced character becomes a
    space, newlines are preserved, so indices map 1:1 onto `src`.
    """
    out = list(src)
    n = len(src)
    i = 0

    def blank(a: int, b: int) -> None:
        for j in range(a, min(b, n)):
            if out[j] != "\n":
                out[j] = " "

    while i < n:
        c = src[i]

        # Line comment: to end of line (newline kept).
        if c == "/" and i + 1 < n and src[i + 1] == "/":
            j = i
            while j < n and src[j] != "\n":
                j += 1
            blank(i, j)
            i = j
            continue

        # Block comment: nested /* ... */.
        if c == "/" and i + 1 < n and src[i + 1] == "*":
            depth = 0
            j = i
            while j < n:
                if src[j] == "/" and j + 1 < n and src[j + 1] == "*":
                    depth += 1
                    j += 2
                elif src[j] == "*" and j + 1 < n and src[j + 1] == "/":
                    depth -= 1
                    j += 2
                    if depth == 0:
                        break
                else:
                    j += 1
            blank(i, j)
            i = j
            continue

        # String / char literals, with optional b/r/br prefixes.
        if c in "\"'br":
            # Try to lex a literal starting at i.
            j = i
            prefix = ""
            while j < n and src[j] in "br":
                prefix += src[j]
                j += 1
            is_prefixed = j > i and prefix in ("b", "r", "br")
            if src[j : j + 1] == '"' or (
                src[i : i + 1] == "'" and not is_prefixed
            ):
                if src[j : j + 1] == '"':
                    # Normal or raw string.
                    if "r" in prefix:
                        # Raw string: r"..." or r#"..."# (n hashes).
                        k = j + 1
                        hashes = 0
                        while k < n and src[k] == "#":
                            hashes += 1
                            k += 1
                        end_pat = '"' + "#" * hashes
                        k = src.find(end_pat, k)
                        j = (k + len(end_pat)) if k != -1 else n
                        blank(i, j)
                        i = j
                        continue
                    # Escaped string.
                    k = j + 1
                    while k < n:
                        if src[k] == "\\":
                            k += 2
                            continue
                        if src[k] == '"' or src[k] == "\n":
                            k += 1
                            break
                        k += 1
                    blank(i, k)
                    i = k
                    continue
                # Char literal: 'x', '{', '\n', '\u{1}' ... but NOT a
                # lifetime ('a, 'static) — those have no closing quote.
                # After this block, k points at the expected closing
                # quote.
                k = j + 1
                if k < n and src[k] == "\\":
                    k += 1  # past the backslash
                    if src[k : k + 1] == "u" and src[k + 1 : k + 2] == "{":
                        k = src.find("}", k) + 1  # past the '}'
                    else:
                        k += 1  # past the escaped char
                else:
                    k += 1  # past the single char
                if src[k : k + 1] == "'":
                    blank(i, k + 1)
                    i = k + 1
                    continue
                # Not a char literal (lifetime): fall through, treat i
                # as ordinary code (the ' stays).

        i += 1

    return "".join(out)


def count_file(path: Path):
    """Return (total_lines, unsafe_blocks, unsafe_lines) for one file."""
    src = path.read_text(encoding="utf-8", errors="replace")
    total = len(src.splitlines())
    if total == 0:
        return 0, 0, 0

    stripped = strip_comments_and_literals(src)
    # line number of index i (1-based)
    line_of = {}
    line = 1
    for i, ch in enumerate(stripped):
        line_of[i] = line
        if ch == "\n":
            line += 1

    covered = set()
    blocks = 0
    for m in UNSAFE_BLOCK_RE.finditer(stripped):
        # Walk brace-matching from the '{' of this `unsafe {`.
        i = m.end() - 1  # position of '{'
        depth = 0
        j = i
        end = i
        while j < len(stripped):
            if stripped[j] == "{":
                depth += 1
            elif stripped[j] == "}":
                depth -= 1
                if depth == 0:
                    end = j
                    break
            j += 1
        else:
            # Unbalanced (scanner corner case): attribute a conservative
            # single line rather than the whole rest of the file.
            end = i
        blocks += 1
        start_line = line_of.get(i, 1)
        end_line = line_of.get(end, start_line)
        covered.update(range(start_line, end_line + 1))

    return total, blocks, len(covered)


def subsystem_of(rel: Path) -> str:
    """Bucket a kernel/src-relative path into a subsystem name."""
    parts = rel.parts
    if len(parts) == 1:
        return "(top-level files)"
    if parts[0] == "arch":
        if len(parts) >= 3:
            return f"arch/{parts[1]}"
        return "arch (shared)"
    return parts[0]


def collect():
    stats = {}  # subsystem -> dict
    for path in sorted(KERNEL_SRC.rglob("*.rs")):
        rel = path.relative_to(KERNEL_SRC)
        if rel.parts[0] in EXCLUDE_DIRS:
            continue
        if any(path.match(g) for g in EXCLUDE_GLOBS):
            continue
        total, blocks, ulines = count_file(path)
        sub = subsystem_of(rel)
        s = stats.setdefault(
            sub,
            {"files": 0, "lines": 0, "unsafe_blocks": 0, "unsafe_lines": 0},
        )
        s["files"] += 1
        s["lines"] += total
        s["unsafe_blocks"] += blocks
        s["unsafe_lines"] += ulines
    return stats


def pct(part: int, whole: int) -> float:
    return (100.0 * part / whole) if whole else 0.0


def markdown_table(stats: dict) -> str:
    rows = sorted(
        stats.items(), key=lambda kv: (-kv[1]["unsafe_lines"], kv[0])
    )
    out = []
    out.append(
        "| Subsystem | .rs files | .rs lines | unsafe blocks | "
        "unsafe lines | unsafe % |"
    )
    out.append(
        "|---|---:|---:|---:|---:|---:|"
    )
    tot_files = tot_lines = tot_blocks = tot_ulines = 0
    for name, s in rows:
        tot_files += s["files"]
        tot_lines += s["lines"]
        tot_blocks += s["unsafe_blocks"]
        tot_ulines += s["unsafe_lines"]
        out.append(
            f"| {name} | {s['files']} | {s['lines']} | "
            f"{s['unsafe_blocks']} | {s['unsafe_lines']} | "
            f"{pct(s['unsafe_lines'], s['lines']):.1f}% |"
        )
    out.append(
        f"| **Whole kernel** | **{tot_files}** | **{tot_lines}** | "
        f"**{tot_blocks}** | **{tot_ulines}** | "
        f"**{pct(tot_ulines, tot_lines):.1f}%** |"
    )
    return "\n".join(out)


def to_json(stats: dict) -> str:
    tot_lines = sum(s["lines"] for s in stats.values())
    tot_blocks = sum(s["unsafe_blocks"] for s in stats.values())
    tot_ulines = sum(s["unsafe_lines"] for s in stats.values())
    return json.dumps(
        {
            "kernel_src": str(KERNEL_SRC),
            "excluded": sorted(EXCLUDE_DIRS),
            "total": {
                "files": sum(s["files"] for s in stats.values()),
                "lines": tot_lines,
                "unsafe_blocks": tot_blocks,
                "unsafe_lines": tot_ulines,
                "unsafe_pct": round(pct(tot_ulines, tot_lines), 2),
            },
            "subsystems": {
                name: {
                    **s,
                    "unsafe_pct": round(pct(s["unsafe_lines"], s["lines"]), 2),
                }
                for name, s in sorted(stats.items())
            },
        },
        indent=2,
    )


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument(
        "--json", action="store_true", help="emit JSON instead of markdown"
    )
    ap.add_argument(
        "--out",
        type=Path,
        default=None,
        help="write the markdown table to FILE instead of stdout",
    )
    args = ap.parse_args()

    if not KERNEL_SRC.is_dir():
        print(f"error: {KERNEL_SRC} not found", file=sys.stderr)
        return 1

    stats = collect()
    if not stats:
        print("error: no .rs files found", file=sys.stderr)
        return 1

    if args.json:
        print(to_json(stats))
    elif args.out:
        args.out.write_text(markdown_table(stats) + "\n", encoding="utf-8")
        print(f"wrote {args.out}")
    else:
        print(markdown_table(stats))
    return 0


if __name__ == "__main__":
    sys.exit(main())
