#!/usr/bin/env python3
"""Verify sync checker — compare kernel/verify copies against kernel source.

Detects when kernel algorithm changes haven't been propagated to the verify
test copies in kernel/verify/src/. Extracts function bodies from both sides,
normalizes platform differences (core/std/alloc, pr_warn, etc.), and reports
divergences.

Usage:
    python3 scripts/verify_sync_check.py           # check all
    python3 scripts/verify_sync_check.py -v        # verbose (show diff context)
    python3 scripts/verify_sync_check.py mm/page   # filter to specific module

Exit code: 0 = all in sync, 1 = divergences found.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


# ============================================================
# Extraction helpers
# ============================================================

def extract_fn(text, name, impl_type=None):
    """Extract a function definition (signature + body) by name.

    If impl_type is given, only match functions within `impl TypeName { ... }`.
    Uses brace counting to find the full extent of the function.
    """
    pattern = re.compile(rf'\bfn\s+{re.escape(name)}\s*\(')
    lines = text.split('\n')

    # If impl_type specified, find impl block boundaries first
    impl_ranges = []  # list of (start_line, end_line)
    if impl_type:
        i = 0
        while i < len(lines):
            stripped = lines[i].strip()
            if re.match(rf'\bimpl\s+{re.escape(impl_type)}\b', stripped):
                block_start = i
                depth = 0
                j = i
                found_open = False
                while j < len(lines):
                    for ch in lines[j]:
                        if ch == '{':
                            depth += 1
                            found_open = True
                        elif ch == '}':
                            depth -= 1
                    if found_open and depth <= 0:
                        impl_ranges.append((block_start, j))
                        break
                    j += 1
            i += 1

    def in_impl_range(line_idx):
        if not impl_ranges:
            return True  # no scoping
        return any(s <= line_idx <= e for s, e in impl_ranges)

    for i, line in enumerate(lines):
        if pattern.search(line) and in_impl_range(i):
            start = i
            depth = 0
            found_open = False
            j = i
            while j < len(lines):
                for ch in lines[j]:
                    if ch == '{':
                        depth += 1
                        found_open = True
                    elif ch == '}':
                        depth -= 1
                if found_open and depth <= 0:
                    return '\n'.join(lines[start:j + 1])
                j += 1
    return None


def extract_impl_pub_methods(text, type_name):
    """Extract all pub fn names from `impl TypeName { ... }` blocks.

    Skips trait impls (e.g. `impl Default for TypeName`).
    Returns a list of method names.
    """
    lines = text.split('\n')
    methods = []
    in_impl = False
    depth = 0

    i = 0
    while i < len(lines):
        stripped = lines[i].strip()
        if not in_impl:
            # Match `impl TypeName {` or `impl TypeName<...> {`
            if re.match(rf'\bimpl\s+{re.escape(type_name)}\b', stripped):
                if '{' in stripped:
                    depth = stripped.count('{') - stripped.count('}')
                    in_impl = depth > 0
                else:
                    # Opening brace on next line(s)
                    i += 1
                    while i < len(lines) and '{' not in lines[i]:
                        i += 1
                    if i < len(lines):
                        depth = lines[i].count('{') - lines[i].count('}')
                        in_impl = depth > 0
        elif in_impl:
            depth += stripped.count('{') - stripped.count('}')
            m = re.match(r'\bpub\s+(const\s+)?fn\s+(\w+)', stripped)
            if m:
                methods.append(m.group(2))
            if depth <= 0:
                in_impl = False
        i += 1

    return methods


# ============================================================
# Normalization
# ============================================================

def normalize(text):
    """Normalize platform differences between kernel (no_std) and verify (std)."""
    # core:: / std:: / alloc:: -> canonical __NS__
    text = re.sub(r'\bcore::', '__NS__', text)
    text = re.sub(r'\bstd::', '__NS__', text)
    text = re.sub(r'\balloc::', '__NS__', text)

    # Remove crate::pr_warn!(...) calls (kernel-only logging)
    text = re.sub(r'\s*crate::pr_warn!.*?;\s*\n?', '\n', text, flags=re.DOTALL)

    # Remove #[inline], #[inline(never)], #[inline(always)]
    text = re.sub(r'#\[inline[^\]]*\]\s*\n?', '', text)

    # Remove all comments (doc comments /// and regular //)
    text = re.sub(r'\s*//[^\n]*\n', '\n', text)

    # pub(crate) -> pub
    text = re.sub(r'pub\(crate\)\s+', 'pub ', text)

    # Collapse multiple blank lines
    text = re.sub(r'\n{2,}', '\n', text)

    # Strip trailing whitespace
    text = re.sub(r'[ \t]+$', '', text, flags=re.MULTILINE)

    return text.strip()


# ============================================================
# Mappings: verify file <-> kernel source + functions to compare
# ============================================================

MAPPINGS = [
    {
        "name": "mm/page_flags",
        "verify": "kernel/verify/src/mm/page_flags_test.rs",
        "kernel": "kernel/src/mm/page_desc.rs",
        "type": "PageFlags",
        "compare": [
            "new", "from_raw", "raw", "test", "set", "clear",
            "test_and_set", "test_and_clear", "clear_all",
        ],
        "skip": [],
    },
    {
        "name": "mm/refcount",
        "verify": "kernel/verify/src/mm/refcount_test.rs",
        "kernel": "kernel/src/mm/page_desc.rs",
        "type": "Page",
        "compare": ["get_page", "put_page", "try_get_page"],
        "skip": ["new"],  # struct layout intentionally simplified
        "skip_diff": ["put_page"],  # kernel has pr_warn + extra brace nesting
        "check_new": False,  # many intentionally uncopied methods
    },
    {
        "name": "mm/vma",
        "verify": "kernel/verify/src/mm/vma_test.rs",
        "kernel": "kernel/src/mm/vma.rs",
        "type": "Vma",
        "compare": [
            "contains", "overlaps", "split", "can_merge",
        ],
        "skip": ["new"],  # struct fields differ
        "check_new": False,  # many intentionally uncopied methods
    },
    {
        "name": "mm/vma_manager",
        "verify": "kernel/verify/src/mm/vma_test.rs",
        "kernel": "kernel/src/mm/vma.rs",
        "type": "VmaManager",
        "compare": ["add", "find", "remove"],
        "skip": ["new"],  # struct fields differ
        "check_new": False,  # many intentionally uncopied methods
    },
    {
        "name": "sync/spinlock",
        "verify": "kernel/verify/src/sync/spinlock_test.rs",
        "kernel": "kernel/src/sync/spinlock.rs",
        "type": "RawSpinlock",
        "compare": ["try_lock", "unlock", "is_locked"],
        "skip": ["lock", "new"],  # lock has deadlock warn stripped by design
    },
    {
        "name": "arch/riscv64/pagetable",
        "verify": "kernel/verify/src/arch/riscv64/mm/pagetable_test.rs",
        "kernel": "kernel/src/arch/riscv64/mm/pagetable.rs",
        "type": "PageTableEntry",
        "compare": [
            "from_bits", "bits", "is_valid", "is_readable", "is_writable",
            "is_executable", "is_user", "is_leaf", "ppn",
            "new_table", "new_page_kernel", "new_page_user", "new_page_ro",
        ],
        "skip": [],  # new() is const fn, same body
    },
    {
        "name": "arch/riscv64/satp",
        "verify": "kernel/verify/src/arch/riscv64/mm/pagetable_test.rs",
        "kernel": "kernel/src/arch/riscv64/mm/pagetable.rs",
        "type": "Satp",
        "compare": [
            "bits", "mode", "asid", "ppn", "is_bare", "is_sv39",
        ],
        "skip": ["new", "sv39"],  # const fn, same body but skip for noise
    },
]


# ============================================================
# Check logic
# ============================================================

def check_mapping(m, verbose=False):
    """Check one mapping. Returns list of result tuples."""
    name = m["name"]
    vpath = ROOT / m["verify"]
    kpath = ROOT / m["kernel"]
    type_name = m.get("type", "")

    if not vpath.exists():
        return [("ERROR", f"verify file not found: {m['verify']}")]
    if not kpath.exists():
        return [("ERROR", f"kernel file not found: {m['kernel']}")]

    vtext = vpath.read_text()
    ktext = kpath.read_text()
    skip = set(m.get("skip", []))
    skip_diff = set(m.get("skip_diff", []))
    results = []

    # 1. Compare listed functions (scoped to impl type)
    for fn_name in m["compare"]:
        vfn = extract_fn(vtext, fn_name, impl_type=type_name)
        kfn = extract_fn(ktext, fn_name, impl_type=type_name)

        if vfn is None:
            results.append(("ERROR", f"'{fn_name}' not found in verify"))
            continue
        if kfn is None:
            results.append(("WARN", f"'{fn_name}' not found in kernel (removed?)"))
            continue

        vnorm = normalize(vfn)
        knorm = normalize(kfn)

        if vnorm == knorm:
            if verbose:
                results.append(("OK", fn_name))
        elif fn_name in skip_diff:
            if verbose:
                results.append(("SKIP", fn_name))
        else:
            # Find first differing line
            vlines = vnorm.split('\n')
            klines = knorm.split('\n')
            diff_line = 0
            for idx, (a, b) in enumerate(zip(vlines, klines)):
                if a != b:
                    diff_line = idx
                    break
            else:
                diff_line = min(len(vlines), len(klines))

            results.append(("DIFF", fn_name, diff_line, vlines, klines))

    # 2. Check for new pub methods in kernel not covered by verify
    if type_name and m.get("check_new", True):
        k_methods = set(extract_impl_pub_methods(ktext, type_name))
        v_methods = set(extract_impl_pub_methods(vtext, type_name))
        covered = set(m["compare"]) | skip

        for method in sorted(k_methods):
            if method not in v_methods and method not in covered:
                results.append(("NEW", f"kernel has new method '{method}' on {type_name} not in verify"))

    return results


def format_diff(fn_name, diff_line, vlines, klines):
    """Format a diff result for display."""
    ctx = 3
    start = max(0, diff_line - ctx)
    end = min(max(len(vlines), len(klines)), diff_line + ctx + 1)
    lines = []
    for i in range(start, end):
        vl = vlines[i] if i < len(vlines) else "<EOF>"
        kl = klines[i] if i < len(klines) else "<EOF>"
        marker = ">>>" if i == diff_line else "   "
        if vl == kl:
            lines.append(f"    {marker} {i + 1:3d}| {vl}")
        else:
            lines.append(f"    {marker} {i + 1:3d}- verify: {vl}")
            lines.append(f"    {marker} {i + 1:3d}+ kernel: {kl}")
    return '\n'.join(lines)


# ============================================================
# Main
# ============================================================

def main():
    args = sys.argv[1:]
    verbose = '-v' in args or '--verbose' in args
    filter_mod = None
    for arg in args:
        if not arg.startswith('-'):
            filter_mod = arg

    all_diffs = 0
    all_errors = 0

    for m in MAPPINGS:
        if filter_mod and not m["name"].startswith(filter_mod):
            continue

        print(f"Checking {m['name']}...")
        issues = check_mapping(m, verbose)

        for item in issues:
            severity = item[0]
            if severity == "OK":
                print(f"  [OK] {item[1]}")
            elif severity == "SKIP":
                print(f"  [SKIP] {item[1]} (known intentional diff)")
            elif severity == "DIFF":
                all_diffs += 1
                fn_name = item[1]
                diff_line = item[2]
                vlines = item[3]
                klines = item[4]
                print(f"  [DIFF] {fn_name} (first diff at line {diff_line + 1})")
                if verbose:
                    print(format_diff(fn_name, diff_line, vlines, klines))
            elif severity == "ERROR":
                all_errors += 1
                print(f"  [ERROR] {item[1]}")
            elif severity == "WARN":
                print(f"  [WARN] {item[1]}")
            elif severity == "NEW":
                print(f"  [NEW]  {item[1]}")

    print()
    if all_diffs == 0 and all_errors == 0:
        if filter_mod:
            print(f"Module '{filter_mod}' is in sync.")
        else:
            print("All verify copies are in sync with kernel source.")
        return 0
    else:
        parts = []
        if all_diffs:
            parts.append(f"{all_diffs} divergence(s)")
        if all_errors:
            parts.append(f"{all_errors} error(s)")
        print(f"Found {', '.join(parts)}. Review and update verify copies.")
        return 1


if __name__ == "__main__":
    sys.exit(main())
