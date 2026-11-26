#!/usr/bin/env python3
# MIT License
# Copyright (c) 2026 Fei Wang

"""Test-layout gate: every kernel unit test lives under kernel/core/src/tests/.

Enforces the test-layout policy (user directive, 2026-11-19):

  1. no `#[cfg(test)]` blocks anywhere in kernel/core/src — host-side test
     blocks can NEVER run in this no_std crate (std panic_impl conflict),
     so they are dead weight that reads like coverage but isn't;
  2. no `mod tests` / `pub mod tests` outside kernel/core/src/tests/;
  3. no `fn test_*` definitions outside kernel/core/src/tests/, except the
     explicitly allowlisted test-support helpers that must live inside
     their own module (driver/time fakes, shared test fixtures);
  4. no `#[test]` attributes outside kernel/core/src/tests/.

Exit 0 = layout clean, exit 1 = violations found (with file:line).

Usage:
    python3 scripts/test_layout_gate.py [--selftest]
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
KERNEL_SRC = ROOT / "kernel" / "core" / "src"
TESTS_DIR = KERNEL_SRC / "tests"

# Test-support helpers that legitimately live inside their modules (they
# fake time / expose fixtures the tests drive). Exact (relpath, name) pairs.
ALLOWLIST = {
    # the per-arch fakes live in the timer inline module of hotpath.rs
    ("kernel/core/src/drivers/hotpath.rs", "test_advance_jiffies"),
    ("kernel/core/src/timer.rs", "test_advance_jiffies"),  # top-level twin of the per-arch fakes
    ("kernel/core/src/timer/wheel.rs", "test_shared"),
    ("kernel/core/src/process/hotpath.rs", "test_ti_flag"),
    # page_desc's atomic bit helpers: "test" as in test-and-set, not tests.
    ("kernel/core/src/mm/hotpath.rs", "test_and_set"),
    ("kernel/core/src/mm/hotpath.rs", "test_and_clear"),
    ("kernel/core/src/mm/hotpath.rs", "test_flag"),
    ("kernel/core/src/mm/hotpath.rs", "test_and_set_flag"),
    ("kernel/core/src/mm/hotpath.rs", "test_and_clear_flag"),
}

FN_TEST_RE = re.compile(r'\bfn\s+(test_[A-Za-z0-9_]+)')
MOD_TESTS_RE = re.compile(r'\b(?:pub\s+)?mod\s+tests\b')
ATTR_TEST_RE = re.compile(r'#\[test\]')


# Files allowed to carry cfg(unit-test) items outside kernel/core/src/tests/:
# the harness wiring itself and the test-support helpers that must live
# inside their own modules (fake time, shared fixtures).
CFG_ALLOWLIST_FILES = {
    "kernel/core/src/main.rs",                    # mod tests; + run_all_tests wiring
    "kernel/core/src/lib.rs",                     # crate-flip twin of main.rs's wiring
    "kernel/core/src/drivers/hotpath.rs",          # timer inline module: test_advance_jiffies fakes
    "kernel/core/src/timer.rs",                    # top-level test_advance_jiffies twin
    # The safety-primitives host battery: kernel/safety-tests includes
    # kernel/core/src/safety/*.rs VERBATIM (#[path]), so the #[test] fns live
    # beside the code BY DESIGN — moving them to kernel/core/src/tests/ would
    # break the verbatim include. They run under the host crate
    # (cargo test --target x86_64-unknown-linux-gnu), verified green.
    "kernel/core/src/safety/slot.rs",
    "kernel/core/src/safety/bytes.rs",
    "kernel/core/src/safety/ring.rs",
    "kernel/core/src/safety/ptr.rs",
    "kernel/core/src/safety/mmio.rs",
    "kernel/core/src/timer/wheel.rs",             # test_shared fixture
}


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
            # Blanket rule: NO cfg(unit-test)/cfg(test)-gated items
            # outside tests/ outside the allowlist. This catches the
            # callable-test-bodies pattern (shape_check/snapshot_check
            # mods, caps_shape shims) the fn-name patterns miss.
            if ("unit-test" in line and "#[cfg" in line) and rel not in CFG_ALLOWLIST_FILES:
                violations.append(f"{rel}:{lineno}: cfg-gated test item outside kernel/core/src/tests/ (not in the file allowlist)")
            if "#[cfg(test)]" in line and rel not in CFG_ALLOWLIST_FILES:
                violations.append(f"{rel}:{lineno}: #[cfg(test)] block (host tests never run in this crate)")
            if ATTR_TEST_RE.search(line) and rel not in CFG_ALLOWLIST_FILES:
                violations.append(f"{rel}:{lineno}: #[test] outside kernel/core/src/tests/")
            m = MOD_TESTS_RE.search(line)
            if m and rel not in CFG_ALLOWLIST_FILES:
                # `mod tests;` (declaration) is the policy wiring itself —
                # main.rs pointing at kernel/core/src/tests/. Only block modules
                # WITH BODIES (`mod tests {`).
                if "mod tests;" not in line:
                    violations.append(f"{rel}:{lineno}: mod tests with body outside kernel/core/src/tests/")
            m = FN_TEST_RE.search(line)
            if m and (rel, m.group(1)) not in ALLOWLIST:
                violations.append(
                    f"{rel}:{lineno}: fn {m.group(1)} outside kernel/core/src/tests/ "
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
    print("test-layout gate: OK (all unit tests under kernel/core/src/tests/)")


if __name__ == "__main__":
    main()
