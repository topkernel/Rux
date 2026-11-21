#!/usr/bin/env python3
# MIT License
# Copyright (c) 2026 Fei Wang

"""Unsafe-code census for the kernel.

Extracts every unsafe region from kernel/src/**/*.rs with a
comment/string-aware scanner:

  * unsafe fn / unsafe impl / unsafe trait / unsafe extern blocks
  * unsafe { ... } blocks (brace-counted, nested-unsafe aware)

Each region gets a stable id (sha1 of file:line:snippet), its enclosing
function, and category tags from regex heuristics. Word occurrences of
"unsafe" inside comments, doc comments, or string literals are ignored.

Outputs:
  * run/unsafe-census.json        raw census (machine truth)
  * summary to stdout (incl. the pattern-shape histogram)
  * run/unsafe-census-histogram.txt  one CI-diffable histogram line
    (Layer 4.2): counts per pattern category over all regions, the
    layer-split variant, and the untagged remainder — shape drift
    between releases is a plain diff of this file.

With --ledger, regenerates docs/development/unsafe-verification.md from
the census plus the artifact sidecar
docs/development/unsafe-verification-artifacts.tsv (id -> artifact).
Artifacts of ids that still exist are preserved verbatim; new ids enter
as GAP; vanished ids are dropped (counted in the report).

Usage:
    python3 scripts/unsafe_census.py                 # summary only
    python3 scripts/unsafe_census.py --json out.json # write census JSON
    python3 scripts/unsafe_census.py --ledger        # regenerate ledger
"""

import argparse
import hashlib
import json
import re
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
KERNEL_SRC = ROOT / "kernel" / "src"
CENSUS_JSON = ROOT / "run" / "unsafe-census.json"
HIST_FILE = ROOT / "run" / "unsafe-census-histogram.txt"
LEDGER_MD = ROOT / "docs" / "development" / "unsafe-verification.md"
ARTIFACTS_TSV = ROOT / "docs" / "development" / "unsafe-verification-artifacts.tsv"


# ============================================================
# Comment/string masking
# ============================================================

def mask_comments_and_strings(text):
    """Replace comments and string/char literals with spaces.

    Length and newlines are preserved so masked text indexes line up with
    the original. Handles nested block comments (Rust nests them), raw
    strings r"..." / r#"..."# / br#"..."#, byte strings, char literals,
    and distinguishes lifetimes ('a) from char literals ('a').
    """
    out = []  # list of chars, same length as text
    n = len(text)
    i = 0

    def blank(idx):
        out.append(text[idx] if text[idx] == "\n" else " ")

    # Pre-fill lazily: we append as we go.
    while i < n:
        c = text[i]
        nxt = text[i + 1] if i + 1 < n else ""

        # Line comment
        if c == "/" and nxt == "/":
            while i < n and text[i] != "\n":
                blank(i)
                i += 1
            continue

        # Block comment (Rust nests)
        if c == "/" and nxt == "*":
            depth = 0
            while i < n:
                if text[i] == "/" and i + 1 < n and text[i + 1] == "*":
                    depth += 1
                    blank(i)
                    blank(i + 1)
                    i += 2
                elif text[i] == "*" and i + 1 < n and text[i + 1] == "/":
                    depth -= 1
                    blank(i)
                    blank(i + 1)
                    i += 2
                    if depth == 0:
                        break
                else:
                    blank(i)
                    i += 1
            continue

        # Raw / byte raw strings: r", r#", br", b"
        raw_prefix_len = 0
        is_raw = False
        j = i
        if text[j:j + 1] in ("b",) and text[j + 1:j + 2] == "r":
            raw_prefix_len = 2
            is_raw = True
        elif text[j:j + 1] == "r":
            raw_prefix_len = 1
            is_raw = True
        if is_raw:
            # r followed by zero or more # then a quote
            k = j + raw_prefix_len
            hashes = 0
            while k < n and text[k] == "#":
                hashes += 1
                k += 1
            if hashes > 0 and k < n and text[k] == '"':
                term = '"' + "#" * hashes
                # blank the prefix
                for p in range(i, k + 1):
                    blank(p)
                i = k + 1
                while i < n:
                    if text.startswith(term, i):
                        for p in range(i, i + len(term)):
                            blank(p)
                        i += len(term)
                        break
                    blank(i)
                    i += 1
                continue

        # Byte string b"..." / byte char b'...'
        if c == "b" and nxt in ('"', "'"):
            q = nxt
            blank(i)
            blank(i + 1)
            i += 2
            while i < n:
                blank(i)
                if text[i] == "\\" and i + 1 < n:
                    blank(i + 1)
                    i += 2
                    continue
                if text[i] == q:
                    i += 1
                    break
                if text[i] == "\n":
                    i += 1
                    break
                i += 1
            continue

        # Ordinary string literal
        if c == '"':
            blank(i)
            i += 1
            while i < n:
                blank(i)
                if text[i] == "\\" and i + 1 < n:
                    blank(i + 1)
                    i += 2
                    continue
                if text[i] == '"':
                    i += 1
                    break
                if text[i] == "\n":
                    i += 1
                    break
                i += 1
            continue

        # Char literal vs lifetime: try to match a char literal here.
        if c == "'":
            m = re.match(r"'(\\.|[^'\\])'", text[i:])
            if m:
                for p in range(i, i + m.end()):
                    blank(p)
                i += m.end()
                continue
            # Lifetime: leave the quote and identifier as code.
            out.append(text[i])
            i += 1
            continue

        # Ordinary code character: keep verbatim.
        out.append(text[i])
        i += 1

    return "".join(out)


# ============================================================
# Structure extraction
# ============================================================

def match_brace(masked, open_idx):
    """Return the index of the brace matching masked[open_idx] == '{'."""
    assert masked[open_idx] == "{"
    depth = 0
    n = len(masked)
    i = open_idx
    while i < n:
        c = masked[i]
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return n - 1  # unbalanced (should not happen)


def find_body_brace(masked, kw_idx):
    """Given the index of 'fn'/'impl'/'trait'/'extern' keyword, find the
    index of the opening brace of the item body (first '{' at
    paren-depth 0 — generic parameter lists contain no braces)."""
    n = len(masked)
    depth = 0
    i = kw_idx
    while i < n:
        c = masked[i]
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
        elif c == "{" and depth == 0:
            return i
        i += 1
    return -1


FN_RE = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)")
UNSAFE_RE = re.compile(r"\bunsafe\b")


def extract_regions(fname, text, masked):
    """Return a list of region dicts for one file."""
    regions = []
    n = len(masked)

    # --- all functions (for enclosing-fn attribution) ---
    fn_spans = []  # (start, end, name)
    for m in FN_RE.finditer(masked):
        brace = find_body_brace(masked, m.start())
        if brace < 0:
            continue
        end = match_brace(masked, brace)
        fn_spans.append((m.start(), end, m.group(1)))
    fn_spans.sort()

    def enclosing_fn(pos):
        best = None
        for start, end, name in fn_spans:
            if start <= pos <= end:
                best = name
            elif start > pos:
                break
        return best or "<module>"

    # --- unsafe regions ---
    for m in UNSAFE_RE.finditer(masked):
        kw = m.start()
        # classify by the next word
        after = masked[m.end():]
        nm = re.match(r"\s*(fn|impl|trait|extern|\{)", after)
        if not nm:
            continue  # `unsafe` used as part of an attribute path etc.
        kind = nm.group(1)
        if kind == "{":
            body = m.end() + nm.start(1)
            end = match_brace(masked, body)
            span = (kw, end)
            kind_name = "block"
        else:
            kw2 = m.end() + nm.start(1)
            brace = find_body_brace(masked, kw2)
            if brace < 0:
                continue
            end = match_brace(masked, brace)
            span = (kw, end)
            kind_name = {"fn": "unsafe-fn", "impl": "unsafe-impl",
                         "trait": "unsafe-trait", "extern": "unsafe-extern"}[kind]

        line = text.count("\n", 0, span[0]) + 1
        end_line = text.count("\n", 0, span[1]) + 1
        snippet = re.sub(r"\s+", " ", text[span[0]:span[0] + 90]).strip()
        rid = hashlib.sha1(f"{fname}:{line}:{snippet[:70]}".encode()).hexdigest()[:12]
        regions.append({
            "id": rid,
            "file": fname,
            "line": line,
            "end_line": end_line,
            "kind": kind_name,
            "fn": enclosing_fn(span[0]),
            "snippet": snippet[:70],
        })

    return regions


CATEGORIES = [
    ("transmute", re.compile(r"\btransmute\b")),
    # note: no trailing \b after '!' — asm!( is followed by '(' which is a
    # non-word char, so a \b there would never hold.
    ("asm", re.compile(r"\b(?:global_asm|asm)!")),
    ("volatile", re.compile(r"\b(?:read_volatile|write_volatile)\b")),
    ("raw-construct", re.compile(r"\b(?:from_raw_parts|slice_from_raw_parts|from_utf8_unchecked|new_unchecked|from_raw_buf|from_raw_buf_mut)\b")),
    ("ptr-ops", re.compile(r"\b(?:copy_from|copy_to|copy_nonoverlapping|write_bytes|read_bytes|swap_nonoverlapping|&raw\s+(?:const|mut)|as\s+\*(?:const|mut)|\*const\s|\*mut\s|&mut\s\*|&\*)")),
    ("static-mut", re.compile(r"\bstatic\s+mut\b")),
    ("addr-arith", re.compile(r"\b(?:virt_to_phys|phys_to_virt|phys_addr|pfn|PTE[0-9L]*|page_table|VIRT_BASE|virt_addr)\b")),
    ("no-preempt", re.compile(r"\b(?:preempt_count|PREEMPT_OFFSET|no_preempt)\b")),
    ("ffi", re.compile(r"\bextern\s*\{")),  # strings are masked: extern { ... }
    ("layout-meta", re.compile(r"\b(?:offset_of|addr_of!|align_of|size_of_val)\b")),
]


def categorize(region_text):
    cats = []
    for name, rx in CATEGORIES:
        if rx.search(region_text):
            cats.append(name)
    return cats


# ============================================================
# Census
# ============================================================

def scan_texts(texts):
    """Scan {relpath: text} into regions. Shared by the on-disk census and
    the gate's git-tree diff mode."""
    regions = []
    for rel, text in sorted(texts.items()):
        masked = mask_comments_and_strings(text)
        for r in extract_regions(rel, text, masked):
            start = line_start(masked, r["line"])
            end = line_end(masked, r["end_line"])
            cats = categorize(masked[start:end])
            # every unsafe-extern block is an FFI boundary by definition
            if r["kind"] == "unsafe-extern" and "ffi" not in cats:
                cats.append("ffi")
            r["categories"] = cats
            regions.append(r)
    return regions


def tracked_sources():
    """kernel/src sources git tracks, as paths relative to ROOT.

    The census must measure the source tree, not whatever a previous
    build left behind: a stale generated file (kernel/src/debug/
    ksyms_generated.rs, 27,625 lines, written by an older build and
    referenced by no module) used to be picked up by the rglob. Counting
    only tracked files makes a bare checkout and a built tree produce the
    same census by construction. Falls back to the on-disk scan when git
    is unavailable.
    """
    try:
        out = subprocess.run(
            ["git", "ls-files", "-z", "--", "kernel/src"],
            cwd=ROOT, capture_output=True, text=True, check=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError):
        return None
    rels = set()
    for entry in out.split("\0"):
        if entry:
            rels.add(entry)
    return rels


def full_census():
    """Read kernel/src from disk and census it."""
    tracked = tracked_sources()
    texts = {}
    for path in sorted(KERNEL_SRC.rglob("*.rs")):
        rel = str(path.relative_to(ROOT))
        if tracked is not None and rel not in tracked:
            continue
        try:
            texts[rel] = path.read_text(
                encoding="utf-8", errors="replace")
        except OSError as e:
            print(f"  [skip] {path}: {e}", file=sys.stderr)
    return scan_texts(texts)


def line_start(masked, line):
    if line <= 1:
        return 0
    idx = -1
    for _ in range(line - 1):
        idx = masked.find("\n", idx + 1)
        if idx < 0:
            return len(masked) - 1
    return idx + 1


def line_end(masked, line):
    idx = -1
    for _ in range(line):
        idx = masked.find("\n", idx + 1)
        if idx < 0:
            return len(masked)
    return idx


# ============================================================
# Ledger
# ============================================================

def load_artifacts():
    """id -> artifact string from the sidecar (column 6: id, file, fn,
    line, categories, artifact)."""
    artifacts = {}
    if ARTIFACTS_TSV.exists():
        for line in ARTIFACTS_TSV.read_text().splitlines():
            if not line or line.startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) >= 6:
                artifacts[parts[0]] = parts[5]
    return artifacts


def write_sidecar(regions, artifacts):
    lines = ["# id\tfile\tfn\tline\tcategories\tartifact — machine truth for the gate; edit the artifact column only"]
    for r in sorted(regions, key=lambda x: (x["file"], x["line"])):
        rid = r["id"]
        art = artifacts.get(rid, "GAP")
        cats = ",".join(r.get("categories", []))
        lines.append(f"{rid}\t{r['file']}\t{r['fn']}\t{r['line']}\t{cats}\t{art}")
    ARTIFACTS_TSV.write_text("\n".join(lines) + "\n")


def artifact_kind(art):
    if art == "GAP":
        return "GAP"
    return art.split(":", 1)[0] if ":" in art else art


def write_ledger(regions, artifacts):
    per_sys = defaultdict(list)
    for r in regions:
        parts = Path(r["file"]).parts
        # kernel/src/<subsys>/...  or kernel/src/<name>.rs
        subsys = parts[2] if len(parts) > 3 else "<top>"
        per_sys[subsys].append(r)

    total = len(regions)
    verified = sum(1 for r in regions if artifacts.get(r["id"], "GAP") != "GAP")
    cat_counter = Counter(c for r in regions for c in r.get("categories", []))
    kind_counter = Counter(r["kind"] for r in regions)

    out = []
    out.append("# Unsafe Verification Ledger")
    out.append("")
    out.append("Generated by `scripts/unsafe_census.py --ledger` from the machine census.")
    out.append("Do not hand-edit region rows; edit the artifact column in the sidecar")
    out.append("`docs/development/unsafe-verification-artifacts.tsv` (id -> artifact) and")
    out.append("regenerate. The gate (`scripts/unsafe_gate.py --check`) enforces that every")
    out.append("census id is accounted for here.")
    out.append("")
    out.append("## Artifact taxonomy")
    out.append("")
    out.append("| kind | meaning |")
    out.append("|------|---------|")
    out.append("| GAP | no verification artifact yet (accounted, unverified) |")
    out.append("| KANI:<harness> | Kani proof harness in kernel/verify (function name) |")
    out.append("| PROPTEST:<path> | proptest coverage in kernel/verify (file path) |")
    out.append("| LOOM:<test> | loom interleaving model (test name) |")
    out.append("| MIRI:<path> | covered by a Miri-executed mirror (file path) |")
    out.append("| INV:<id> | audited invariant (id from docs/architecture/*.md) |")
    out.append("")
    out.append("## Totals")
    out.append("")
    out.append(f"- unsafe regions: **{total}**")
    out.append(f"- with artifact (non-GAP): **{verified}** ({100.0 * verified / max(total, 1):.1f}%)")
    out.append(f"- GAP: **{total - verified}**")
    out.append(f"- by kind: {', '.join(f'{k}={v}' for k, v in sorted(kind_counter.items()))}")
    out.append("- by category (regions carry 0..n tags): "
               + ", ".join(f"{k}={v}" for k, v in sorted(cat_counter.items(), key=lambda x: -x[1])))
    out.append("")
    out.append("## Per-subsystem")
    out.append("")
    out.append("| subsystem | regions | non-GAP | GAP |")
    out.append("|-----------|---------|---------|-----|")
    for subsys in sorted(per_sys, key=lambda s: -len(per_sys[s])):
        rs = per_sys[subsys]
        ng = sum(1 for r in rs if artifacts.get(r["id"], "GAP") != "GAP")
        out.append(f"| {subsys} | {len(rs)} | {ng} | {len(rs) - ng} |")
    out.append("")

    for subsys in sorted(per_sys, key=lambda s: -len(per_sys[s])):
        rs = sorted(per_sys[subsys], key=lambda x: (x["file"], x["line"]))
        out.append(f"## {subsys}")
        out.append("")
        by_file = defaultdict(list)
        for r in rs:
            by_file[r["file"]].append(r)
        for fname in sorted(by_file):
            frs = by_file[fname]
            out.append(f"### {fname} ({len(frs)})")
            out.append("")
            out.append("| line | fn | kind | categories | artifact | id |")
            out.append("|------|----|------|------------|----------|----|")
            for r in frs:
                art = artifacts.get(r["id"], "GAP")
                cats = ",".join(r.get("categories", [])) or "-"
                out.append(f"| {r['line']} | {r['fn']} | {r['kind']} | {cats} | {art} | `{r['id']}` |")
            out.append("")

    LEDGER_MD.write_text("\n".join(out))


# ============================================================
# Pattern-shape histogram (Layer 4.2)
# ============================================================

# The framekernel layer rules, verbatim from gen_tcb_ledger.classify
# (path prefix -> layer; longest prefix wins; default service). Kept as
# a copy so the census has no cross-script import; when the ledger's
# rules move, move these with them.
_LAYER_RULES = [
    ("arch/aarch64/", "unbuilt"),
    ("arch/", "framework"),
    ("mm/", "framework"),
    ("sync/spinlock.rs", "framework"),
    ("sync/rwlock.rs", "framework"),
    ("sync/seqlock.rs", "framework"),
    ("sync/percpu.rs", "framework"),
    ("sync/rcu.rs", "framework"),
    ("sync/lockkey.rs", "framework"),
    ("sync/", "service"),
    ("interrupt/", "framework"),
    ("drivers/virtio/", "framework"),
    ("drivers/intc/", "framework"),
    ("drivers/timer/", "framework"),
    ("drivers/pci/", "framework"),
    ("drivers/", "service"),
    ("dfx/", "diagnostics"),
    ("tests/", "test"),
    ("console.rs", "framework"),
    ("printk.rs", "framework"),
    ("sbi.rs", "framework"),
    ("main.rs", "framework"),
]


def layer_map():
    """Sorted (prefix, layer) rules; longest-prefix match wins."""
    return sorted(_LAYER_RULES, key=lambda r: -len(r[0]))


def layer_of(relpath, layers):
    # Census paths are repo-rooted (kernel/src/<module>/...); the layer
    # rules are kernel-src-rooted (gen_tcb_ledger's shape). Strip the
    # kernel/src/ prefix so both worlds line up.
    rel = relpath
    if rel.startswith("kernel/src/"):
        rel = rel[len("kernel/src/"):]
    for prefix, layer in layers:
        if rel.startswith(prefix):
            return layer
    return "service"


def pattern_histogram(regions, layers):
    """Counts per pattern category, overall and per framekernel layer.

    Returns (hist_total, hist_by_layer) where hist_total maps category
    -> region count (untagged regions counted under "-") and
    hist_by_layer maps (layer, category) -> count. Every region counts
    once per tag it carries (regions carry 0..n tags — same semantics
    as the ledger's category column); the untagged remainder makes the
    tally balance-checkable: sum(tags) + untagged == len(regions).
    """
    hist_total = Counter()
    hist_by_layer = Counter()
    for r in regions:
        cats = r.get("categories") or []
        if not cats:
            hist_total["-"] += 1
            hist_by_layer[(layer_of(r["file"], layers), "-")] += 1
            continue
        lay = layer_of(r["file"], layers)
        for c in cats:
            hist_total[c] += 1
            hist_by_layer[(lay, c)] += 1
    return hist_total, hist_by_layer


def write_histogram_file(hist_total, hist_by_layer):
    """Emit run/unsafe-census-histogram.txt — the per-release artifact
    CI diffs. Format: one `cat=count` line per category (sorted), the
    layer-split block below it, and the balance check at the end."""
    lines = ["# pattern-shape histogram (Layer 4.2) — diff between "
             "releases to see shape drift", ""]
    lines.append("total:")
    for cat in sorted(hist_total):
        lines.append(f"  {cat}={hist_total[cat]}")
    lines.append("")
    lines.append("by-layer:")
    for (lay, cat) in sorted(hist_by_layer):
        lines.append(f"  {lay}/{cat}={hist_by_layer[(lay, cat)]}")
    try:
        HIST_FILE.parent.mkdir(parents=True, exist_ok=True)
        HIST_FILE.write_text("\n".join(lines) + "\n")
        print(f"histogram: {HIST_FILE}")
    except OSError as e:
        print(f"  [warn] histogram not written: {e}", file=sys.stderr)


def report_drift(old_json_path, hist_total, hist_by_layer):
    """Diff this run's histogram against a previous census JSON."""
    try:
        old_regions = json.loads(Path(old_json_path).read_text())
    except (OSError, ValueError) as e:
        print(f"drift: cannot read {old_json_path}: {e}", file=sys.stderr)
        return
    layers = layer_map()
    old_total, old_by_layer = pattern_histogram(old_regions, layers)
    deltas = []
    for cat in sorted(set(old_total) | set(hist_total)):
        d = hist_total.get(cat, 0) - old_total.get(cat, 0)
        if d:
            deltas.append((cat, d))
    if not deltas:
        print("drift: NONE (pattern shapes unchanged)")
        return
    print(f"drift vs {old_json_path}:")
    for cat, d in deltas:
        print(f"  {cat}: {'+' if d > 0 else ''}{d} "
              f"({old_total.get(cat, 0)} -> {hist_total.get(cat, 0)})")
    # layer moves for the biggest absolute shifts (the "where did the
    # mass go" half of the drift question)
    layer_deltas = Counter()
    for key in set(old_by_layer) | set(hist_by_layer):
        d = hist_by_layer.get(key, 0) - old_by_layer.get(key, 0)
        if d:
            layer_deltas[key] = d
    if layer_deltas:
        print("  layer moves (top 8):")
        for (lay, cat), d in sorted(layer_deltas.items(),
                                    key=lambda x: -abs(x[1]))[:8]:
            print(f"    {lay}/{cat}: {'+' if d > 0 else ''}{d}")


# ============================================================
# Main
# ============================================================

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", metavar="PATH", help="also write raw census JSON")
    ap.add_argument("--ledger", action="store_true", help="regenerate ledger + sidecar")
    ap.add_argument("--drift", metavar="OLDJSON",
                    help="compare this run's pattern histogram against a "
                         "previous census JSON and print the deltas "
                         "(the CI shape-drift check)")
    args = ap.parse_args()

    regions = full_census()

    if args.json:
        Path(args.json).write_text(json.dumps(regions, indent=1))
    try:
        CENSUS_JSON.parent.mkdir(parents=True, exist_ok=True)
        CENSUS_JSON.write_text(json.dumps(regions, indent=1))
    except OSError as e:
        print(f"  [warn] census JSON not written: {e}", file=sys.stderr)

    per_sys = defaultdict(int)
    for r in regions:
        parts = Path(r["file"]).parts
        per_sys[parts[2] if len(parts) > 3 else "<top>"] += 1
    cat_counter = Counter(c for r in regions for c in r.get("categories", []))

    print(f"unsafe regions: {len(regions)} across "
          f"{len(set(r['file'] for r in regions))} files")
    print("per-subsystem:")
    for s, n in sorted(per_sys.items(), key=lambda x: -x[1]):
        print(f"  {s:12s} {n}")
    print("categories:", ", ".join(f"{k}={v}" for k, v in
                                   sorted(cat_counter.items(), key=lambda x: -x[1])))

    # ---- pattern-shape histogram (Layer 4.2; unsafe-hardening plan §4.2)
    # One stable, CI-diffable line per release: the per-category counts
    # over ALL regions, the same counts split by the framekernel layer
    # (the gen_tcb_ledger path rules), and the untagged remainder — a
    # release that shifts mass between shapes (say ptr-ops growing at
    # the service layer) shows up as a plain diff of the histogram file.
    layers = layer_map()
    hist_total, hist_by_layer = pattern_histogram(regions, layers)
    write_histogram_file(hist_total, hist_by_layer)

    if args.drift:
        report_drift(args.drift, hist_total, hist_by_layer)

    if args.ledger:
        artifacts = load_artifacts()
        old_ids = set(artifacts)
        new_ids = set(r["id"] for r in regions) - old_ids
        vanished = old_ids - set(r["id"] for r in regions)
        write_sidecar(regions, artifacts)
        write_ledger(regions, artifacts)
        print(f"ledger: {LEDGER_MD}")
        print(f"sidecar: {ARTIFACTS_TSV}")
        print(f"new ids: {len(new_ids)}, vanished: {len(vanished)} (dropped)")


if __name__ == "__main__":
    main()
