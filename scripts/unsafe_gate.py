# MIT License
# Copyright (c) 2026 Fei Wang

#!/usr/bin/env python3
"""Unsafe-verification merge gate.

Enforces the unsafe-code accounting policy:

  * every unsafe region found by the census must have a row in the
    artifact sidecar (docs/development/unsafe-verification-artifacts.tsv)
    — GAP counts as accounted-but-unverified;
  * every non-GAP artifact must resolve (KANI/LOOM harness names exist in
    kernel/verify/src, PROPTEST/MIRI file paths exist, INV ids exist in
    docs/architecture/*.md);
  * --diff <git-rev>: fails when the diff introduces unsafe regions whose
    ids are missing from the sidecar (forward monotonicity: new unsafe
    must come with a ledger row, artifact or explicit GAP).

Exit codes: 0 = accounted, 1 = unaccounted unsafe or dangling artifact.

Usage:
    python3 scripts/unsafe_gate.py --check
    python3 scripts/unsafe_gate.py --check -v          # list GAP ids per file
    python3 scripts/unsafe_gate.py --diff HEAD~1       # vs a git revision
"""

import argparse
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from unsafe_census import (  # noqa: E402
    ARTIFACTS_TSV, KERNEL_SRC, ROOT, full_census, load_artifacts, scan_texts)

ARCH_DOCS = [
    ROOT / "docs" / "architecture" / "lock-ordering.md",
    ROOT / "docs" / "architecture" / "safety-and-performance.md",
    ROOT / "docs" / "architecture" / "design-divergences.md",
    ROOT / "docs" / "architecture" / "rust-native-register.md",
]

VALID_KINDS = {"GAP", "KANI", "PROPTEST", "LOOM", "MIRI", "INV"}


def git_tree_texts(rev):
    """{relpath: text} for kernel/src/**/*.rs at a git revision."""
    out = subprocess.run(
        ["git", "-C", str(ROOT), "ls-tree", "-r", "--name-only", rev, "kernel/src"],
        capture_output=True, text=True, check=True).stdout
    texts = {}
    for rel in out.splitlines():
        if not rel.endswith(".rs"):
            continue
        blob = subprocess.run(
            ["git", "-C", str(ROOT), "show", f"{rev}:{rel}"],
            capture_output=True, text=True, check=False)
        if blob.returncode == 0:
            texts[rel] = blob.stdout
    return texts


def verify_artifact(art):
    """Return None if the artifact reference resolves, else a reason string."""
    kind, _, ref = art.partition(":")
    if kind not in VALID_KINDS:
        return f"unknown artifact kind {kind!r}"
    if kind == "GAP":
        return None
    if not ref:
        return "missing reference after ':'"
    if kind in ("KANI", "LOOM"):
        # harness / test fn name must exist in kernel/verify/src
        hits = subprocess.run(
            ["grep", "-rl", f"fn {ref}", str(ROOT / "kernel" / "verify" / "src")],
            capture_output=True, text=True, check=False)
        if hits.returncode != 0:
            return f"no `fn {ref}` in kernel/verify/src"
        return None
    if kind in ("PROPTEST", "MIRI"):
        p = ROOT / "kernel" / "verify" / "src" / ref
        if not p.exists():
            return f"missing kernel/verify/src/{ref}"
        return None
    if kind == "INV":
        for doc in ARCH_DOCS:
            if doc.exists() and ref in doc.read_text(encoding="utf-8", errors="replace"):
                return None
        names = ",".join(d.name for d in ARCH_DOCS if d.exists())
        return f"invariant id {ref!r} not found in {names}"
    return "unreachable"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--diff", metavar="REV", help="check a git revision against the sidecar")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()
    if not args.check and not args.diff:
        ap.error("one of --check / --diff is required")

    artifacts = load_artifacts()
    failures = []

    if args.check:
        regions = full_census()
        ids = {r["id"] for r in regions}
        missing = ids - set(artifacts)
        dangling = []
        kinds = Counter()
        for rid, art in sorted(artifacts.items()):
            if rid not in ids:
                continue  # stale row (harmless; --ledger prunes)
            reason = verify_artifact(art)
            if reason:
                dangling.append((rid, art, reason))
            kinds[art.partition(":")[0]] += 1

        verified = sum(1 for r in regions if artifacts.get(r["id"], "GAP") != "GAP")
        total = len(regions)
        print(f"unsafe gate: {total} regions, {verified} with artifact "
              f"({100.0 * verified / max(total, 1):.1f}%), {total - verified} GAP")
        print(f"artifact kinds: {dict(sorted(kinds.items()))}")

        if missing:
            failures.append(f"{len(missing)} unsafe region(s) not in the ledger")
            if args.verbose:
                by_file = defaultdict(list)
                for r in regions:
                    if r["id"] in missing:
                        by_file[r["file"]].append(r)
                for f, rs in sorted(by_file.items()):
                    print(f"  MISSING {f}: {len(rs)} region(s)")
                    for r in rs[:5]:
                        print(f"    {r['line']}:{r['fn']} {r['snippet'][:60]}")
        for rid, art, reason in dangling[:50]:
            failures.append(f"dangling artifact {rid} -> {art}: {reason}")
        if len(dangling) > 50:
            failures.append(f"... and {len(dangling) - 50} more dangling artifacts")

    if args.diff:
        rev = args.diff
        base_regions = scan_texts(git_tree_texts(rev))
        base_ids = {r["id"] for r in base_regions}
        cur_regions = full_census()
        new_ids = {r["id"] for r in cur_regions} - base_ids
        unledgered = sorted(new_ids - set(artifacts))
        print(f"diff vs {rev}: {len(new_ids)} new unsafe region(s), "
              f"{len(unledgered)} without a ledger row")
        for r in cur_regions:
            if r["id"] in unledgered:
                failures.append(
                    f"new unsafe without ledger row: {r['file']}:{r['line']} "
                    f"in {r['fn']} ({r['snippet'][:50]})")

    if failures:
        print("\nUNSAFE GATE FAILED:")
        for f in failures[:40]:
            print(f"  - {f}")
        if len(failures) > 40:
            print(f"  ... and {len(failures) - 40} more")
        sys.exit(1)
    print("unsafe gate: OK")


if __name__ == "__main__":
    main()
