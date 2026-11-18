#!/usr/bin/env python3
"""Test-layout gate: every kernel unit test lives under kernel/src/tests/.

Enforces the test-layout policy (user directive, 2026-11-19):

  1. no `#[cfg(test)]` blocks anywhere in kernel/src — host-side test
     blocks can NEVER run in this no_std crate (std panic_impl conflict),
     so they are dead weight that reads like coverage but isn't;
  2. no `mod tests` / `pub mod tests` outside kernel/src/tests/;
  3. no `fn test_*` definitions outside kernel/src/tests/, except the
     explicitly allowlisted test-support helpers that must live inside
     their own module (driver/time fakes, shared test fixtures);
  4. no `#[test]` attributes outside kernel/src/tests/.

Exit 0 = layout clean, exit 1 = violations found (with file:line).

Usage:
    python3 scripts/test_layout_gate.py [--selftest]
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
KERNEL_SRC = ROOT / "kernel" / "src"
TESTS_DIR = KERNEL_SRC / "tests"

# Test-support helpers that legitimately live inside their modules (they
# fake time / expose fixtures the tests drive). Exact (relpath, name) pairs.
ALLOWLIST = {
    ("kernel/src/drivers/timer/riscv64.rs", "test_advance_jiffies"),
    ("kernel/src/drivers/timer/x86_64.rs", "test_advance_jiffies"),
    ("kernel/src/drivers/timer/aarch64.rs", "test_advance_jiffies"),
    ("kernel/src/timer/wheel.rs", "test_shared"),
    ("kernel/src/process/task.rs", "test_ti_flag"),
    # page_desc's atomic bit helpers: "test" as in test-and-set, not tests.
    ("kernel/src/mm/page_desc.rs", "test_and_set"),
    ("kernel/src/mm/page_desc.rs", "test_and_clear"),
    ("kernel/src/mm/page_desc.rs", "test_flag"),
    ("kernel/src/mm/page_desc.rs", "test_and_set_flag"),
    ("kernel/src/mm/page_desc.rs", "test_and_clear_flag"),
}

FN_TEST_RE = re.compile(r'\bfn\s+(test_[A-Za-z0-9_]+)')
MOD_TESTS_RE = re.compile(r'\b(?:pub\s+)?mod\s+tests\b')
ATTR_TEST_RE = re.compile(r'#\[test\]')


def scan():
    violations = []
    for path in sorted(KERNEL_SRC.rglob("*.rs")):
        rel = str(path.relative_to(ROOT))
        in_tests = TESTS_DIR in path.parents
        if in_tests:
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        for lineno, raw_line in enumerate(text.splitlines(), 1):
            # Strip line comments (doc /// and plain //) before scanning:
            # doc text legitimately mentions #[test] when explaining the
            # harness contract. Simplification: everything after the first
            # "//" is cut (a "//" inside a string literal would over-cut;
            # the gate only ever reports extra-suspicious lines then).
            line = raw_line
            idx = line.find("//")
            if idx >= 0:
                line = line[:idx]
            if "#[cfg(test)]" in line:
                violations.append(f"{rel}:{lineno}: #[cfg(test)] block (host tests never run in this crate)")
            if ATTR_TEST_RE.search(line):
                violations.append(f"{rel}:{lineno}: #[test] outside kernel/src/tests/")
            m = MOD_TESTS_RE.search(line)
            if m:
                # `mod tests;` (declaration) is the policy wiring itself —
                # main.rs pointing at kernel/src/tests/. Only block modules
                # WITH BODIES (`mod tests {`).
                if "mod tests;" not in line:
                    violations.append(f"{rel}:{lineno}: mod tests with body outside kernel/src/tests/")
            m = FN_TEST_RE.search(line)
            if m and (rel, m.group(1)) not in ALLOWLIST:
                violations.append(
                    f"{rel}:{lineno}: fn {m.group(1)} outside kernel/src/tests/ "
                    f"(not in the helper allowlist)")
    return violations


def selftest():
    """Add a synthetic offender, expect a violation, remove it, expect clean."""
    scratch = KERNEL_SRC / "layout_selftest_scratch.rs"
    scratch.write_text("#[cfg(test)]\nmod tests {\n    #[test]\n    fn test_scratch() {}\n}\n")
    v = scan()
    ok = any("layout_selftest_scratch.rs" in x for x in v)
    print(f"selftest: {'OK' if ok else 'FAILED'} ({len(v)} violations with scratch present)")
    scratch.unlink(missing_ok=True)
    v2 = scan()
    clean = not any("layout_selftest_scratch.rs" in x for x in v2)
    print(f"selftest: clean after removal: {clean}")
    return ok and clean


def main():
    if "--selftest" in sys.argv:
        sys.exit(0 if selftest() else 1)
    violations = scan()
    if violations:
        print(f"TEST-LAYOUT GATE FAILED: {len(violations)} violation(s)")
        for v in violations[:40]:
            print(f"  - {v}")
        if len(violations) > 40:
            print(f"  ... and {len(violations) - 40} more")
        sys.exit(1)
    print("test-layout gate: OK (all unit tests under kernel/src/tests/)")


if __name__ == "__main__":
    main()
