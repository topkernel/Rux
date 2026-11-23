#!/usr/bin/env python3
# MIT License
# Copyright (c) 2026 Fei Wang

"""Unsafe-code census for the kernel.

Extracts every unsafe region from kernel/src/**/*.rs with a
comment/string-aware scanner:

  * unsafe fn / unsafe impl / unsafe trait / unsafe extern blocks
  * unsafe { ... } blocks (brace-counted, nested-unsafe aware)

Each region gets an id (sha1 of file:line:snippet), its enclosing
function, and category tags from regex heuristics. Word occurrences of
"unsafe" inside comments, doc comments, or string literals are ignored.
The id is POSITION-based: a line shift above the region or a path change
renumbers it, so an id is not a cross-version identity. For that, each
region also carries a content fingerprint (owner item, ordinal inside
the owner, comment-stripped text hashes) that is stable under comment
edits, line shifts and file moves; the ledger re-bank uses it.

Outputs:
  * run/unsafe-census.json        raw census (machine truth)
  * summary to stdout (incl. the pattern-shape histogram)
  * run/unsafe-census-histogram.txt  one CI-diffable histogram line
    (Layer 4.2): counts per pattern category over all regions, the
    layer-split variant, and the untagged remainder — shape drift
    between releases is a plain diff of this file.

Ledger re-bank (--ledger) regenerates
docs/development/unsafe-verification.md and the artifact sidecar
docs/development/unsafe-verification-artifacts.tsv (id -> artifact).
Artifacts reach the new ids in this order:

  1. by id — rows whose id still exists are preserved verbatim;
  2. from REFERENCE VERSIONS of the ledger — commits that hold a sidecar
     plus the tree it was written against. A region inherits an artifact
     only where a reference annotated the same code: same owner item,
     same kind, identical comment-stripped text (an edited site needs the
     same owner and ordinal plus a text-similarity tier), paired
     one-to-one and order-preserving. The same pairing runs across file
     moves (git rename records) and for items that moved between files
     (a whole-item fingerprint that is unique on both sides).

Without --reference the references are exactly the versions a text
merge silently merges across: HEAD, an in-progress MERGE_HEAD, and the
parents of a merge commit. --reference REV adds explicit versions and
--reference-history takes every commit that changed the sidecar.

A region is recovered only when EVERY reference that annotates this code
agrees on the artifact. Disagreeing references mean one version
re-audited the site or a re-bank corrupted it, and the reference list
cannot tell which: the region stays GAP and is listed as contested.
Reviewed decisions go in a decisions file (--decide; one row per group
of regions, with the value it replaces, an exact count the tool verifies
against the tree, and the reason). By-id rows are never rewritten
without such a row.

The re-bank prints, per file, the annotated-region count before (the
maximum over the baselines) and after, and a citation check (does the
site's own comment name the ledger's contract?).

Two limits to keep in mind. The drop check and the gate's monotonicity
check count ANNOTATED regions; they cannot see a wrong artifact, so a
green gate is not proof that the annotations are right — only the
self-check line (by-id rows the references annotate differently) and the
citation check speak to that. And a reference is evidence about the code
it was written against, nothing more: an annotation is only as good as
the audit that produced it.

Usage:
    python3 scripts/unsafe_census.py                 # summary only
    python3 scripts/unsafe_census.py --json out.json # write census JSON
    python3 scripts/unsafe_census.py --ledger        # regenerate ledger
    python3 scripts/unsafe_census.py --ledger --reference MERGE_HEAD
    python3 scripts/unsafe_census.py --ledger --reference-history --dry-run
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
KERNEL_SRC = ROOT / "kernel" / "src"
CENSUS_JSON = ROOT / "run" / "unsafe-census.json"
HIST_FILE = ROOT / "run" / "unsafe-census-histogram.txt"
REMAP_TSV = ROOT / "run" / "unsafe-ledger-remap.tsv"
LEDGER_MD = ROOT / "docs" / "development" / "unsafe-verification.md"
ARTIFACTS_TSV = ROOT / "docs" / "development" / "unsafe-verification-artifacts.tsv"
SIDE_CAR_REL = "docs/development/unsafe-verification-artifacts.tsv"


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


def _norm(s):
    """Collapse whitespace runs — the form of text that is hashed."""
    return re.sub(r"\s+", " ", s).strip()


def _hash(s):
    return hashlib.sha1(s.encode("utf-8", "replace")).hexdigest()[:16]


def extract_regions(fname, text, masked):
    """Return a list of region dicts for one file.

    Besides the ledger columns, every region carries the content
    fingerprint the reference remap matches on (none of it feeds the id):

      owner      the item the region belongs to — the enclosing function
                 for a block, the declared name for an unsafe fn, the
                 header text for impl/trait/extern items
      ordinal    position among the file's regions with that owner+kind
      text_hash  the region's comment/string-stripped, whitespace-
                 collapsed text
      ctx_hash   the same for the whole owner item ("" at module level)

    The raw normalized text travels under "text"; scan_file lifts it out
    into _REGION_TEXT so it never reaches the census JSON.
    """
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

    def enclosing_span(pos):
        best = None
        for start, end, name in fn_spans:
            if start <= pos <= end:
                best = (start, end, name)
            elif start > pos:
                break
        return best

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
            header = ""
            item_kw = -1
        else:
            item_kw = m.end() + nm.start(1)
            brace = find_body_brace(masked, item_kw)
            if brace < 0:
                continue
            end = match_brace(masked, brace)
            span = (kw, end)
            kind_name = {"fn": "unsafe-fn", "impl": "unsafe-impl",
                         "trait": "unsafe-trait", "extern": "unsafe-extern"}[kind]
            header = _norm(masked[kw:brace])

        line = text.count("\n", 0, span[0]) + 1
        end_line = text.count("\n", 0, span[1]) + 1
        snippet = re.sub(r"\s+", " ", text[span[0]:span[0] + 90]).strip()
        rid = hashlib.sha1(f"{fname}:{line}:{snippet[:70]}".encode()).hexdigest()[:12]

        # --- content fingerprint ---
        body_text = _norm(masked[span[0]:span[1] + 1])
        owner_span = enclosing_span(span[0])
        if kind_name == "block":
            owner = owner_span[2] if owner_span else "<module>"
            ctx_text = _norm(masked[owner_span[0]:owner_span[1] + 1]) if owner_span else ""
        else:
            ctx_text = body_text
            if kind_name == "unsafe-fn":
                declared = FN_RE.match(masked, item_kw)
                owner = declared.group(1) if declared else header
            else:
                owner = header

        regions.append({
            "id": rid,
            "file": fname,
            "line": line,
            "end_line": end_line,
            "kind": kind_name,
            "fn": (owner_span[2] if owner_span else None) or "<module>",
            "snippet": snippet[:70],
            "owner": owner,
            "owner_line": text.count("\n", 0, owner_span[0]) + 1 if owner_span else 0,
            "text_hash": _hash(body_text),
            "ctx_hash": _hash(ctx_text) if ctx_text else "",
            "text": body_text,
        })

    seen = Counter()
    for r in regions:
        key = (r["owner"], r["kind"])
        r["ordinal"] = seen[key]
        seen[key] += 1

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


SAFETY_MARKERS = ("SAFETY:", "safety:")


def has_safety_note(region_text: str) -> bool:
    """Whether the region carries a SAFETY justification comment.

    The convention in this tree is a `// SAFETY: ...` line on (or just
    above) the unsafe block. Measuring it turns "the comments are not
    everywhere" from a review impression into a tracked number.
    """
    return any(m in region_text for m in SAFETY_MARKERS)


def categorize(region_text):
    cats = []
    for name, rx in CATEGORIES:
        if rx.search(region_text):
            cats.append(name)
    return cats


# ============================================================
# Census
# ============================================================

_SCAN_CACHE = {}      # (relpath, text) -> the file's region dicts
_REGION_TEXT = {}     # (relpath, line, text_hash) -> normalized region text
CUR_TEXTS = {}        # relpath -> text of the last on-disk census


def scan_file(rel, text):
    """Regions of one file with categories and the SAFETY-note flag.

    Cached by (path, text): the census of a git tree that shares most of
    its blobs with the working tree (or with the previous tree examined)
    scans only what differs. Callers get fresh dicts."""
    key = (rel, text)
    hit = _SCAN_CACHE.get(key)
    if hit is None:
        orig_lines = None
        masked = mask_comments_and_strings(text)
        hit = []
        for r in extract_regions(rel, text, masked):
            start = line_start(masked, r["line"])
            end = line_end(masked, r["end_line"])
            cats = categorize(masked[start:end])
            # every unsafe-extern block is an FFI boundary by definition
            if r["kind"] == "unsafe-extern" and "ffi" not in cats:
                cats.append("ffi")
            r["categories"] = cats
            # SAFETY-note coverage: the note conventionally sits on the
            # line ABOVE the unsafe block, so look at the preceding
            # lines as well as the region body.
            # The SAFETY note is a COMMENT — the masked text has it
            # blanked out, so the detection must run on the ORIGINAL
            # source lines (the mask preserves line numbering).
            if orig_lines is None:
                orig_lines = text.split("\n")
            lo = max(0, r["line"] - 6)
            window = "\n".join(orig_lines[lo:r["end_line"] + 1])
            r["safety_note"] = has_safety_note(window)
            body = r.pop("text", "")
            _REGION_TEXT[(rel, r["line"], r["text_hash"])] = body
            hit.append(r)
        _SCAN_CACHE[key] = hit
    return [dict(r) for r in hit]


def scan_texts(texts):
    """Scan {relpath: text} into regions. Shared by the on-disk census and
    the gate's git-tree diff mode."""
    regions = []
    for rel, text in sorted(texts.items()):
        regions.extend(scan_file(rel, text))
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
    CUR_TEXTS.clear()
    CUR_TEXTS.update(texts)
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

def parse_sidecar(text):
    """id -> row (the tab-separated columns) from sidecar text."""
    rows = {}
    for line in text.splitlines():
        if not line or line.startswith("#"):
            continue
        parts = line.split("\t")
        if len(parts) >= 6:
            rows[parts[0]] = parts
    return rows


def load_sidecar_rows():
    """id -> row of the on-disk sidecar."""
    if ARTIFACTS_TSV.exists():
        return parse_sidecar(ARTIFACTS_TSV.read_text())
    return {}


def load_artifacts():
    """id -> artifact string from the sidecar (column 6: id, file, fn,
    line, categories, artifact)."""
    return {rid: row[5] for rid, row in load_sidecar_rows().items()}


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


# ============================================================
# Git trees — batched: one process per tree, not one per file
# ============================================================

def _git(*args, check=True, stdin=None):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True,
                          text=True, check=check, input=stdin)


def git_rev(rev):
    """Full hash of a commit-ish, or None when it does not resolve."""
    try:
        out = _git("rev-parse", "--verify", "--quiet", f"{rev}^{{commit}}").stdout
    except (OSError, subprocess.CalledProcessError):
        return None
    return out.strip() or None


def git_show(rev, rel):
    """Text of <rev>:<rel>, or None when it does not exist there."""
    try:
        return _git("show", f"{rev}:{rel}").stdout
    except (OSError, subprocess.CalledProcessError):
        return None


def git_tree_blobs(rev, prefix="kernel/src"):
    """{relpath: blob oid} for the .rs files under <prefix> at <rev>."""
    try:
        out = _git("ls-tree", "-r", "-z", rev, "--", prefix).stdout
    except (OSError, subprocess.CalledProcessError):
        return {}
    blobs = {}
    for entry in out.split("\0"):
        if not entry:
            continue
        meta, _, path = entry.partition("\t")
        fields = meta.split()
        if path.endswith(".rs") and len(fields) >= 3 and fields[1] == "blob":
            blobs[path] = fields[2]
    return blobs


_BLOBS = {}


def git_blobs(oids):
    """{oid: text} (None for a missing object) — one `cat-file --batch`
    process for whichever oids are not cached yet. Newlines are
    normalized the way Path.read_text does it, so a blob and the same
    file on disk census identically."""
    need = [o for o in dict.fromkeys(oids) if o not in _BLOBS]
    if need:
        raw = subprocess.run(
            ["git", "cat-file", "--batch"], cwd=ROOT, capture_output=True,
            check=False, input=("\n".join(need) + "\n").encode()).stdout
        pos = 0
        for oid in need:
            nl = raw.find(b"\n", pos)
            if nl < 0:
                break
            fields = raw[pos:nl].decode("utf-8", "replace").split()
            pos = nl + 1
            if len(fields) >= 3 and fields[1] != "missing":
                size = int(fields[2])
                _BLOBS[oid] = (raw[pos:pos + size].decode("utf-8", "replace")
                               .replace("\r\n", "\n").replace("\r", "\n"))
                pos += size + 1
            else:
                _BLOBS[oid] = None
    return {o: _BLOBS.get(o) for o in oids}


def git_tree_texts(rev, prefix="kernel/src"):
    """{relpath: text} for the .rs files under <prefix> at <rev>."""
    blobs = git_tree_blobs(rev, prefix)
    raw = git_blobs(blobs.values())
    return {rel: raw[oid] for rel, oid in blobs.items() if raw.get(oid) is not None}


class Snapshot:
    """The ledger as it stood in one place: sidecar rows plus the census
    of the tree they were written against.

    `art` keeps only rows that are evidence for THIS tree: non-GAP, and
    with an id that is a region of the tree. A row whose id the tree no
    longer has was written against some other tree — the normal state of
    a sidecar after a text merge — and is read from the commit whose tree
    it does describe instead."""

    def __init__(self, label, rev, rows, regions, texts=None):
        self.label = label
        self.rev = rev
        self.rows = rows
        self.regions = regions
        self.texts = texts if texts is not None else CUR_TEXTS
        self.by_id = {r["id"]: r for r in regions}
        self.by_file = defaultdict(list)
        for r in regions:
            self.by_file[r["file"]].append(r)
        self.art = {rid: row[5] for rid, row in rows.items()
                    if row[5] != "GAP" and rid in self.by_id}

    @classmethod
    def at(cls, label, rev, tsv_rel=SIDE_CAR_REL):
        text = git_show(rev, tsv_rel)
        texts = git_tree_texts(rev)
        return cls(label, rev, parse_sidecar(text) if text is not None else {},
                   scan_texts(texts), texts)

    @classmethod
    def working(cls, regions):
        return cls("worktree", None, load_sidecar_rows(), regions, CUR_TEXTS)

    def annotated_by_file(self):
        counts = Counter()
        for r in self.regions:
            if r["id"] in self.art:
                counts[r["file"]] += 1
        return counts


def git_parents(rev):
    try:
        return _git("rev-list", "--parents", "-n", "1", rev).stdout.split()[1:]
    except (OSError, subprocess.CalledProcessError):
        return []


def merge_heads():
    """The heads recorded in MERGE_HEAD while a merge is in progress."""
    try:
        path = _git("rev-parse", "--git-path", "MERGE_HEAD").stdout.strip()
        if path and os.path.isfile(os.path.join(ROOT, path)):
            with open(os.path.join(ROOT, path)) as fh:
                return [line.strip() for line in fh if line.strip()]
    except (OSError, subprocess.CalledProcessError):
        pass
    return []


def ledger_baselines(base_args, no_base=False, previous=False):
    """[(label, full-rev)]: the versions of the ledger whose per-file
    annotated counts the new sidecar must not fall below.

    By default these are exactly the versions a text merge merges the
    sidecar across: HEAD, the heads of an in-progress merge, and the
    parents when HEAD is itself a merge commit. With `previous` the
    parent of an ordinary commit is included too (the gate's history-step
    check; the re-bank leaves it out — its job is to not write something
    worse than what it replaces). --base replaces the defaults."""
    if no_base:
        return []
    if base_args:
        out = []
        for rev in base_args:
            full = git_rev(rev)
            if full is None:
                sys.exit(f"unsafe census: --base {rev!r} is not a commit")
            out.append((rev, full))
        return out
    head = git_rev("HEAD")
    if head is None:
        return []
    out = [("HEAD", head)]
    heads = merge_heads()
    if heads:
        out.extend(("MERGE_HEAD" if len(heads) == 1 else f"MERGE_HEAD[{i}]", h)
                   for i, h in enumerate(heads, 1))
    else:
        parents = git_parents(head)
        if len(parents) > 1 or previous:
            out.extend((f"HEAD^{i}", p) for i, p in enumerate(parents, 1))
    return out


def ledger_references(args, baselines):
    """[(label, full-rev, sidecar path)] to take annotations from, newest
    first. The baselines come first, then the explicit --reference
    versions, then (with --reference-history) every commit that changed
    the sidecar. Two versions with the same sidecar text and the same
    kernel/src tree are the same evidence and appear once."""
    refs = [(label, rev, SIDE_CAR_REL) for label, rev in baselines]
    for spec in args.reference or []:
        rev, _, rel = spec.partition(":")
        full = git_rev(rev)
        if full is None:
            sys.exit(f"unsafe census: --reference {spec!r} is not a commit")
        refs.append((spec, full, rel or SIDE_CAR_REL))
    if args.reference_history is not None:
        start = baselines[0][1] if baselines else git_rev("HEAD")
        history = sidecar_history(start) if start else []
        limit = int(args.reference_history or 0)
        if limit:
            history = history[:limit]
        have = {rev for _, rev, _ in refs}
        refs.extend((f"history:{rev[:9]}", rev, SIDE_CAR_REL)
                    for rev in history if rev not in have)
    out, seen = [], set()
    for label, rev, rel in refs:
        try:
            key = _git("ls-tree", rev, "--", rel, "kernel/src").stdout
        except (OSError, subprocess.CalledProcessError):
            key = ""
        if key and key not in seen:
            seen.add(key)
            out.append((label, rev, rel))
    return out


def sidecar_history(start):
    """Commits reachable from <start> that changed the sidecar, newest first."""
    try:
        return _git("log", "--format=%H", start, "--", SIDE_CAR_REL).stdout.split()
    except (OSError, subprocess.CalledProcessError):
        return []


# ============================================================
# Reference remap — carry annotations onto renumbered ids
# ============================================================

# Edited-site tier of the pairing: same owner and ordinal, text changed.
EDIT_SHINGLE = 4            # tokens per shingle
EDIT_SIMILARITY = 0.8       # Jaccard overlap of the two shingle sets
# A top-level item (unsafe fn / impl / extern) is its own context. One
# this short — `unsafe impl Send for Handle {}` — collides across types,
# so it is not evidence that an item moved between files.
MOVED_ITEM_MIN_TOKENS = 12

_TOKEN_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*|[0-9]+|[^\sA-Za-z0-9_]")
_SIMILARITY = {}


def fp_key(r):
    """Whole-item fingerprint: two regions with the same key are the same
    code (comments, strings and whitespace aside) at the same position
    inside the same owner item, wherever the item lives."""
    return (r["kind"], r["owner"], r["ordinal"], r["text_hash"], r["ctx_hash"])


def _region_text(r):
    return _REGION_TEXT[(r["file"], r["line"], r["text_hash"])]


def text_similarity(o, n):
    """Jaccard overlap of the EDIT_SHINGLE-token shingles of two region texts."""
    key = (o["text_hash"], n["text_hash"])
    sim = _SIMILARITY.get(key)
    if sim is None:
        sets = []
        for r in (o, n):
            toks = _TOKEN_RE.findall(_region_text(r))
            k = min(EDIT_SHINGLE, len(toks))
            sets.append({tuple(toks[i:i + k]) for i in range(len(toks) - k + 1)})
        sim = len(sets[0] & sets[1]) / max(len(sets[0] | sets[1]), 1)
        _SIMILARITY[key] = sim
    return sim


# How much a paired region counts for; the alignment maximizes the sum.
_WEIGHT = {"exact": 4, "shift": 3, "edited": 2}


def _pair_class(o, n, edited):
    if o["kind"] != n["kind"] or o["owner"] != n["owner"]:
        return None
    if o["text_hash"] == n["text_hash"]:
        return "exact" if o["ordinal"] == n["ordinal"] else "shift"
    if edited and o["ordinal"] == n["ordinal"] and text_similarity(o, n) >= EDIT_SIMILARITY:
        return "edited"
    return None


def align_regions(old, new, edited=True):
    """Order-preserving one-to-one pairing of two source-ordered region
    lists: [(old, new, class)] with

      exact   same owner, kind and comment-stripped text, same ordinal
      shift   same owner, kind and text, ordinal moved — a region was
              added to or removed from the owner
      edited  same owner, kind and ordinal; the text changed but stays
              similar (EDIT_SIMILARITY) — edited=False refuses this tier

    Anything else scores 0. Stable sites pair up first and shifted or
    edited ones fill in around them."""
    if not old or not new:
        return []
    n_old, n_new = len(old), len(new)
    cls = [[_pair_class(o, n, edited) for n in new] for o in old]
    best = [[0] * (n_new + 1) for _ in range(n_old + 1)]
    take = [[False] * (n_new + 1) for _ in range(n_old + 1)]
    for i in range(1, n_old + 1):
        row, prev, ci, ti = best[i], best[i - 1], cls[i - 1], take[i]
        for j in range(1, n_new + 1):
            top, left = prev[j], row[j - 1]
            b = top if top >= left else left
            c = ci[j - 1]
            if c and prev[j - 1] + _WEIGHT[c] > b:
                b = prev[j - 1] + _WEIGHT[c]
                ti[j] = True
            row[j] = b
    pairs = []
    i, j = n_old, n_new
    while i and j:
        if take[i][j]:
            pairs.append((old[i - 1], new[j - 1], cls[i - 1][j - 1]))
            i -= 1
            j -= 1
        elif best[i - 1][j] >= best[i][j - 1]:
            i -= 1
        else:
            j -= 1
    pairs.reverse()
    return pairs


def moved_paths(rev):
    """{new_path: old_path} under kernel/src for files git reports as
    renamed or copied between <rev> and the working tree, plus
    deleted/added pairs that share a basename (git wants 40% similarity
    for a rename; a file that moved and was heavily edited on the way
    shows up as a deletion plus an addition)."""
    try:
        out = _git("diff", "--name-status", "-M", "-C", "--find-renames=40%",
                   "--find-copies=40%", rev, "--", "kernel/src").stdout
    except (OSError, subprocess.CalledProcessError):
        return {}
    pairs, deleted, added = {}, [], []
    for line in out.splitlines():
        f = line.split("\t")
        if f[0][:1] in ("R", "C") and len(f) == 3:
            pairs[f[2]] = f[1]
        elif f[0] == "D":
            deleted.append(f[1])
        elif f[0] == "A":
            added.append(f[1])
    for new in added:
        base = os.path.basename(new)
        old = [d for d in deleted if os.path.basename(d) == base]
        if len(old) == 1 and sum(os.path.basename(a) == base for a in added) == 1:
            pairs.setdefault(new, old[0])
    return pairs


def how_group(how):
    """The report's four buckets for a pairing's `how`."""
    if how.startswith("file-move/"):
        return "file-move"
    return {"moved-item": "moved-item", "edited": "edited-site"}.get(how, "same-file")


class Proposal:
    """One reference version's annotation for one current region."""

    def __init__(self, snap, old, new, how):
        self.ref = snap.label
        self.old = old
        self.new = new
        self.how = how
        self.artifact = snap.art.get(old["id"], "GAP")
        self.ambiguous = False


def remap_from_references(cur, refs, edited=True):
    """Collect, for every current region, what each reference version
    annotates for the same code.

    Three passes per reference, strongest first; a reference speaks for
    a region at most once:
      same file    align_regions(old file, new file)
      file move    the same alignment across a git rename record
      moved item   a whole-item fingerprint that is unique in the current
                   tree and left unmatched in the reference

    Returns (proposals, dest, findings, confirmed, retagged):
      proposals  current id -> [Proposal], newest reference first
      dest       (reference label, old id) -> (current region, how)
      findings   sorted human-readable lines (each carries its references)
      confirmed / retagged — of the current by-id annotations that the
      references paired with the same code: how many carry the identical
      artifact, and how many a DIFFERENT one. A reference that holds GAP
      for the code is neither (the annotation was added later) — retagged
      is the matcher's error signal.
    """
    proposals = defaultdict(list)
    dest = {}
    notes = defaultdict(set)
    confirmed = retagged = 0

    for label, rev, tsv_rel in refs:
        snap = Snapshot.at(label, rev, tsv_rel)
        if not snap.rows or not snap.regions:
            notes["sidecar or tree not readable at this commit; skipped"].add(label)
            continue
        proposed_here = set()
        consumed = set()

        def pair(old, new, how, snap=snap, consumed=consumed, proposed_here=proposed_here):
            nonlocal confirmed, retagged
            consumed.add(old["id"])
            dest[(snap.label, old["id"])] = (new, how)
            have = cur.art.get(new["id"], "GAP")
            if have != "GAP":
                was = snap.art.get(old["id"], "GAP")
                if was == have:
                    confirmed += 1
                elif was != "GAP":
                    retagged += 1
                    notes[f"by-id artifact {have} but the reference annotates the same code "
                          f"{was}: {new['file']}:{new['line']} {new['owner']}"].add(label)
                return None
            if new["id"] in proposed_here:
                return None
            proposed_here.add(new["id"])
            p = Proposal(snap, old, new, how)
            proposals[new["id"]].append(p)
            return p

        # --- pass 1: same file ---
        for fname, new_regions in cur.by_file.items():
            old_regions = snap.by_file.get(fname)
            if not old_regions:
                continue
            pairs = align_regions(old_regions, new_regions, edited)
            paired_old = {o["id"] for o, _, _ in pairs}
            for old, new, cls in pairs:
                p = pair(old, new, cls)
                if p and cls == "shift":
                    # The new region is textually the same as every twin of
                    # its partner. One that stayed unpaired with a different
                    # artifact means the ordinal cannot tell them apart.
                    for o2 in old_regions:
                        if (o2["id"] not in paired_old
                                and (o2["owner"], o2["kind"], o2["text_hash"])
                                == (old["owner"], old["kind"], old["text_hash"])
                                and snap.art.get(o2["id"], "GAP") != p.artifact):
                            p.ambiguous = True
                            break
            unpaired_new = {(n["owner"], n["kind"], n["text_hash"]) for n in new_regions
                            if cur.art.get(n["id"], "GAP") == "GAP"}
            for old in old_regions:
                if (old["id"] in snap.art and old["id"] not in paired_old
                        and (old["owner"], old["kind"], old["text_hash"]) in unpaired_new):
                    notes[f"annotated twin left behind: {fname}:{old['line']} {old['owner']} "
                          f"[{snap.art[old['id']]}] is textually identical to an unannotated "
                          f"region of the same owner at another ordinal"].add(label)

        # --- pass 2: file moves ---
        for new_path, old_path in sorted(moved_paths(rev).items()):
            if (old_path == new_path or old_path not in snap.by_file
                    or new_path not in cur.by_file):
                continue
            leftovers = [o for o in snap.by_file[old_path] if o["id"] not in consumed]
            for old, new, cls in align_regions(leftovers, cur.by_file[new_path], edited):
                pair(old, new, f"file-move/{cls}")

        # --- pass 3: items that moved or were reordered ---
        index = defaultdict(list)
        for old in snap.regions:
            if old["ctx_hash"] and old["id"] not in consumed:
                index[fp_key(old)].append(old)
        new_count = Counter(fp_key(n) for n in cur.regions if n["ctx_hash"])
        for new in cur.regions:
            key = fp_key(new)
            cands = index.get(key)
            if (not cands or new_count[key] != 1 or new["id"] in proposed_here
                    or not new["ctx_hash"]):
                continue
            if (new["ctx_hash"] == new["text_hash"]
                    and len(_TOKEN_RE.findall(_region_text(new))) < MOVED_ITEM_MIN_TOKENS):
                continue
            p = pair(cands[0], new, "moved-item")
            if p and any(snap.art.get(c["id"], "GAP") != p.artifact for c in cands[1:]):
                p.ambiguous = True

    findings = [f"{text} [{', '.join(sorted(labels))}]"
                for text, labels in sorted(notes.items())]
    return proposals, dest, findings, confirmed, retagged


# ---- hand decisions -------------------------------------------------------

def apply_decisions(paths, cur, artifacts):
    """Apply reviewed hand decisions and return the applied rows.

    A decisions file is a TSV, one row per group of regions:

        file <TAB> owner <TAB> expect <TAB> artifact <TAB> count <TAB> reason

    `file` is one path or a comma-separated list. The row covers the
    regions of those files whose owner is `owner` (any owner with `*`)
    and whose artifact at this point equals `expect`. `count` (0 =
    unchecked) must equal the number of regions the row changed plus
    those already carrying `artifact`, so a row written against one tree
    cannot silently land on another or on sites nobody read. `expect` is
    the value the decision was reviewed against — the GAP of a contested
    site, or the artifact a corrupted re-bank left behind. `reason` names
    the contract's definition and the code the regions are."""
    applied = []
    for path in paths or []:
        p = Path(path)
        if not p.is_absolute():
            p = ROOT / p
        for n, line in enumerate(p.read_text().splitlines(), 1):
            if not line.strip() or line.startswith("#"):
                continue
            f = line.split("\t")
            if len(f) < 6:
                sys.exit(f"unsafe census: {path}:{n}: need 6 tab-separated columns "
                         "(file, owner, expect, artifact, count, reason)")
            fname, owner, expect, artifact, count, reason = f[:6]
            fnames = [x.strip() for x in fname.split(",")]
            regions = [r for name in fnames for r in cur.by_file.get(name, [])]
            missing = [name for name in fnames if name not in cur.by_file]
            if missing:
                sys.exit(f"unsafe census: {path}:{n}: no unsafe region in {', '.join(missing)}")
            if owner != "*" and not any(r["owner"] == owner for r in regions):
                sys.exit(f"unsafe census: {path}:{n}: no region owned by {owner!r} in {fname}")
            have = artifacts.get
            todo = [r for r in regions if (owner == "*" or r["owner"] == owner)
                    and have(r["id"], "GAP") == expect]
            done = [r for r in regions if (owner == "*" or r["owner"] == owner)
                    and have(r["id"], "GAP") == artifact] if artifact != expect else []
            want = int(count or 0)
            if want and len(todo) + len(done) != want:
                sys.exit(f"unsafe census: {path}:{n}: decision covers {len(todo) + len(done)} "
                         f"region(s) in {fname}, the row says {want} — the tree moved under it")
            for r in todo:
                artifacts[r["id"]] = artifact
            applied.append((p.name, n, fname, owner, expect, artifact, len(todo), reason,
                            [todo, done]))
    return applied


# ---- citation check -------------------------------------------------------

_INV_CITE_RE = re.compile(r"\bINV-[A-Z][A-Z0-9]*(?:-[A-Z0-9]+)*\b")
_FROW_CITE_RE = re.compile(r"(?<![A-Za-z0-9_])F([0-9]{1,2})(?![A-Za-z0-9_])")


def region_citations(region, lines):
    """Contract ids the source itself cites at a region: the INV ids and
    bare F-row numbers in the region's own lines, in the 10 lines above
    it, and for a block in the comment block above its owner function.

    This is the site's own statement of its contract. The ledger row
    agreeing with it is the strongest check available without a re-audit;
    the source cites nothing at most sites, which is not evidence either
    way."""
    lo = region["line"] - 1                       # 0-based first line of the region
    parts = [lines[max(0, lo - 10):region["end_line"]]]
    owner_line = region.get("owner_line", 0)
    if region["kind"] == "block" and owner_line:
        j = owner_line - 2                        # index of the line above the owner
        while j >= 0 and lines[j].lstrip().startswith("//"):
            j -= 1
        parts.append(lines[j + 1:owner_line])
    text = "\n".join("\n".join(part) for part in parts)
    return set(_INV_CITE_RE.findall(text)), {int(n) for n in _FROW_CITE_RE.findall(text)}


def citation_check(cur, artifacts):
    """(Counter of outcomes, mismatch list) over the annotated regions.

    A mismatch is a site that names contracts of the ledger's own kind
    (an INV id for an INV artifact, an F row for an F artifact) and not
    the ledger's. A site citing an INV id under an F-row artifact is
    counted but not listed: an F row is the coarse family tag and a
    narrower INV citation under it is normal."""
    stat = Counter()
    mismatches = []
    for r in cur.regions:
        art = artifacts.get(r["id"], "GAP")
        if art == "GAP":
            continue
        ref = art.partition(":")[2]
        inv, frows = region_citations(r, cur.texts[r["file"]].split("\n"))
        if ref.startswith("INV-"):
            cited, other, kind = ref in inv, sorted(inv), "INV id"
        else:
            cited = ref[1:].isdigit() and int(ref[1:]) in frows
            other, kind = sorted(frows), "F row"
        if cited:
            stat["cite their artifact"] += 1
        elif other:
            stat[f"cite a different {kind}"] += 1
            mismatches.append((r, art, other))
        elif inv or frows:
            stat["cite a contract of the other kind"] += 1
        else:
            stat["cite no contract"] += 1
    return stat, mismatches


def explain_drop(cur, dest, snap, fname, artifacts):
    """Where the annotated regions of one baseline file went: {reason: count}.
    Regions whose annotation reached the new sidecar are not listed."""
    gone = Counter()
    for old in snap.by_file.get(fname, []):
        if old["id"] not in snap.art:
            continue
        hit = dest.get((snap.label, old["id"]))
        if hit is None:
            gone["no counterpart in kernel/src (deleted, converted, or moved to a crate)"] += 1
        elif hit[0]["file"] != fname:
            gone[f"moved to {hit[0]['file']}, annotation "
                 + ("carried" if artifacts.get(hit[0]["id"], "GAP") != "GAP" else "NOT carried")] += 1
        elif artifacts.get(hit[0]["id"], "GAP") == "GAP":
            gone["counterpart found but still unannotated"] += 1
    return gone


def _proposal_line(props):
    """`artifact [n references, newest <label>]` for each distinct artifact."""
    seen = []
    for art in dict.fromkeys(p.artifact for p in props):
        holders = [p.ref for p in props if p.artifact == art]
        seen.append(f"{art} [{len(holders)} ref(s), newest {holders[0]}]")
    return " vs ".join(seen)


def rebank_ledger(args, regions):
    """Regenerate the sidecar and the ledger, carrying artifacts across
    line shifts and file moves from the reference versions.

    Returns the exit code: 0 written (or dry run), 2 a file's annotated
    count would drop and --allow-drop does not name it — nothing written.
    """
    cur = Snapshot.working(regions)
    artifacts = {rid: cur.art[rid] for rid in cur.art}
    by_id_carried = set(cur.art)

    baselines = ledger_baselines(args.base, args.no_base)
    snaps = {}

    def snapshot(label, rev, rel=SIDE_CAR_REL):
        key = (rev, rel)
        if key not in snaps:
            snaps[key] = Snapshot.at(label, rev, rel)
        return snaps[key]

    before, before_src = Counter(), {}
    for label, rev in baselines:
        snap = snapshot(label, rev)
        if not snap.rows or not snap.regions:
            print(f"  [warn] baseline {label}: ledger or tree not readable at {rev[:9]} "
                  f"(shallow clone?) — its counts are not checked", file=sys.stderr)
        for fname, n in snap.annotated_by_file().items():
            if n > before[fname]:
                before[fname], before_src[fname] = n, label

    refs = ledger_references(args, baselines)
    proposals, dest, findings, confirmed, retagged = remap_from_references(
        cur, refs, edited=not args.no_edited)

    # A region takes an annotation only when every reference that
    # annotates this code agrees on it. Disagreement means one of the
    # versions re-audited the site or a re-bank corrupted it — the
    # reference list cannot tell which, so a person decides (--decide).
    recovered, contested, ambiguous = {}, [], []
    for rid, props in proposals.items():
        usable = [p for p in props if p.artifact != "GAP" and not p.ambiguous]
        ambiguous.extend(p for p in props if p.ambiguous and p.artifact != "GAP")
        if not usable:
            continue
        if len({p.artifact for p in usable}) > 1:
            contested.append(rid)
            continue
        recovered[rid] = usable[0]
        artifacts[rid] = usable[0].artifact
    decided = apply_decisions(args.decide, cur, artifacts)

    after = Counter()
    for r in regions:
        if artifacts.get(r["id"], "GAP") != "GAP":
            after[r["file"]] += 1
    files = sorted(set(before) | set(after))
    drops = [f for f in files if after.get(f, 0) < before.get(f, 0)]
    blocked = [f for f in drops if f not in set(args.allow_drop or [])]

    # ---- report ----
    groups = Counter(how_group(p.how) for p in recovered.values())
    t_before = sum(before.values())
    t_after = sum(after.values())
    print(f"ledger re-bank: references {len(refs)}"
          + (f" — newest {refs[0][0]}, oldest {refs[-1][0]}" if refs else ""))
    print(f"  by id: {len(by_id_carried)} kept"
          + (f"; self-check — the references pair {confirmed} of these with the same code and "
             f"the same artifact, {retagged} with a DIFFERENT artifact" if refs else ""))
    print(f"  recovered from references: {len(recovered)}"
          + (f" (same-file {groups['same-file']}, edited-site {groups['edited-site']}, "
             f"file-move {groups['file-move']}, moved-item {groups['moved-item']})"
             if recovered else ""))
    if contested:
        print(f"  contested, left unannotated: {len(contested)}")
    if ambiguous:
        print(f"  ambiguous (identical twins annotated differently), left unannotated: {len(ambiguous)}")
    print(f"  hand decisions applied: {sum(row[6] for row in decided)} region(s) "
          f"from {len(decided)} row(s)")
    old_ids = set(cur.rows)
    new_ids = {r["id"] for r in regions} - old_ids
    vanished = old_ids - {r["id"] for r in regions}
    print(f"  new ids: {len(new_ids)} ({sum(1 for r in regions if r['id'] in new_ids)} regions, "
          f"{sum(1 for rid in recovered if rid in new_ids)} recovered), "
          f"vanished: {len(vanished)} (dropped)")

    print(f"\nper-file annotated regions (before = highest baseline count"
          + (f"; baselines {', '.join(label for label, _ in baselines)}" if baselines else "; no baseline")
          + ")")
    print(f"  {'file':64s} {'before':>6s} {'by-id':>6s} {'after':>6s} {'delta':>6s}")
    t_byid = 0
    for fname in files:
        b = before.get(fname, 0)
        c = sum(1 for r in cur.by_file.get(fname, []) if r["id"] in by_id_carried)
        a = after.get(fname, 0)
        t_byid += c
        mark = ""
        if fname in drops:
            mark = "  DROP" + (" (allowed)" if fname not in blocked else " (NOT allowed)")
        elif a != c:
            mark = f"  {a - c:+d}"
        print(f"  {fname:64s} {b:6d} {c:6d} {a:6d} {a - b:+6d}{mark}")
    print(f"  {'TOTAL':64s} {t_before:6d} {t_byid:6d} {t_after:6d} {t_after - t_before:+6d}")

    if drops:
        print("\nannotated-count drops:")
        base_rev = dict(baselines)
        for fname in drops:
            snap = snaps[(base_rev[before_src[fname]], SIDE_CAR_REL)]
            print(f"  {fname}: {before[fname]} -> {after.get(fname, 0)}"
                  + ("  --allow-drop given" if fname not in blocked else "  needs --allow-drop"))
            for reason, n in sorted(explain_drop(cur, dest, snap, fname, artifacts).items(),
                                    key=lambda x: -x[1]):
                print(f"      {n:4d} {reason}")
    for title, ids in (("contested — the references disagree, left unannotated "
                        "(decide with --decide)", contested),
                       ("ambiguous — identical twins annotated differently, left unannotated",
                        ambiguous)):
        if ids:
            print(f"\n{title} ({len(ids)}):")
            for item in ids[:60]:
                if isinstance(item, Proposal):
                    new = item.new
                    print(f"  {new['file']}:{new['line']} {new['owner']}: {_proposal_line(proposals[new['id']])}")
                else:
                    new = cur.by_id[item]
                    print(f"  {new['file']}:{new['line']} {new['owner']}: {_proposal_line(proposals[item])}")
            if len(ids) > 60:
                print(f"  ... and {len(ids) - 60} more")
    if decided:
        print(f"\nhand decisions ({len(decided)} row(s)):")
        for name, n, fname, owner, expect, artifact, changed, reason, _ in decided:
            print(f"  {name}:{n} {fname} [{owner}] {expect} -> {artifact}: {changed} changed"
                  + (f" — {reason}" if reason else ""))
    citations, mismatches = citation_check(cur, artifacts)
    print("\ncitation check — does the site's own comment name the ledger's contract? "
          + ", ".join(f"{k}: {v}" for k, v in sorted(citations.items(), key=lambda x: -x[1])))
    if findings:
        print(f"\nfindings ({len(findings)}):")
        for line in findings[:40]:
            print(f"  - {line}")
        if len(findings) > 40:
            print(f"  ... and {len(findings) - 40} more")
    if args.verbose:
        print("\nrecovered regions:")
        for rid, p in sorted(recovered.items(), key=lambda kv: (kv[1].new["file"], kv[1].new["line"])):
            new = p.new
            print(f"  {new['file']}:{new['line']} {new['owner']} [{p.artifact}] {p.how} "
                  f"<- {p.ref} {p.old['file']}:{p.old['line']}")
        for name, n, fname, owner, expect, artifact, changed, reason, (todo, done) in decided:
            for r in todo:
                print(f"  DECIDED {name}:{n} {r['file']}:{r['line']} {r['owner']}#{r['ordinal']} "
                      f"{expect} -> {artifact}")
        print("\ncitation mismatches (ledger INV id not cited at the site):")
        for r, art, inv in mismatches:
            print(f"  {r['file']}:{r['line']} {r['owner']} ledger={art} cited={inv}")

    try:
        REMAP_TSV.parent.mkdir(parents=True, exist_ok=True)
        with open(REMAP_TSV, "w") as fh:
            fh.write("# id\tfile\tline\towner\tartifact\thow\treference\told_file\told_line\t"
                     "old_id\treference_artifact\tambiguous\n")
            for rid, props in sorted(proposals.items(),
                                     key=lambda kv: (kv[1][0].new["file"], kv[1][0].new["line"])):
                for p in props:
                    fh.write(f"{rid}\t{p.new['file']}\t{p.new['line']}\t{p.new['owner']}\t"
                             f"{artifacts.get(rid, 'GAP')}\t{p.how}\t{p.ref}\t{p.old['file']}\t"
                             f"{p.old['line']}\t{p.old['id']}\t{p.artifact}\t{int(p.ambiguous)}\n")
        print(f"\nremap detail: {REMAP_TSV}")
    except OSError as e:
        print(f"  [warn] remap detail not written: {e}", file=sys.stderr)

    if blocked:
        print(f"\nUNSAFE LEDGER: {len(blocked)} file(s) would lose annotated regions "
              f"without --allow-drop; nothing written:")
        for fname in blocked:
            print(f"  - {fname}: {before[fname]} -> {after.get(fname, 0)}")
        return 2
    if drops:
        print("\nallowed drops (the gate needs a row for each in "
              f"{ROOT / 'docs' / 'development' / 'unsafe-verification-exemptions.tsv'}):")
        for fname in drops:
            print(f"  - {fname}: {before[fname]} -> {after.get(fname, 0)}")

    provenance = {
        "by_id": len(by_id_carried),
        "recovered": len(recovered),
        "groups": groups,
        "contested": len(contested),
        "decided": sum(row[6] for row in decided),
        "references": [label for label, _, _ in refs],
        "baselined": bool(baselines),
        "before": t_before,
        "after": t_after,
    }
    if args.dry_run:
        print("\ndry run: ledger and sidecar not written")
        return 0
    write_sidecar(regions, artifacts)
    write_ledger(regions, artifacts, provenance)
    print(f"\nledger: {LEDGER_MD}")
    print(f"sidecar: {ARTIFACTS_TSV}")
    return 0


def write_ledger(regions, artifacts, provenance=None):
    per_sys = defaultdict(list)
    for r in regions:
        parts = Path(r["file"]).parts
        # kernel/src/<subsys>/...  or kernel/src/<name>.rs
        subsys = parts[2] if len(parts) > 3 else "<top>"
        per_sys[subsys].append(r)

    total = len(regions)
    verified = sum(1 for r in regions if artifacts.get(r["id"], "GAP") != "GAP")
    cat_counter = Counter(c for r in regions for c in r.get("categories", []))
    noted = sum(1 for r in regions if r.get("safety_note"))
    print(f"SAFETY-note coverage: {noted}/{len(regions)} "
          f"({100.0 * noted / max(len(regions), 1):.1f}%)")
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
    out.append("Region ids are position-based, so a line shift or a file move renumbers a")
    out.append("region. The re-bank carries artifacts across that with content fingerprints,")
    out.append("taking the other side of a merge as a reference (`--reference REV`; the")
    out.append("default is HEAD plus MERGE_HEAD, or the parents of a merge commit). It")
    out.append("refuses to write when a file's annotated count would drop (`--allow-drop")
    out.append("FILE` owns one), and the gate fails on a drop that")
    out.append("`docs/development/unsafe-verification-exemptions.tsv` does not exempt.")
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
    if provenance:
        groups = provenance["groups"]
        out.append("")
        out.append("## Re-bank provenance")
        out.append("")
        out.append(f"- artifacts kept by id: **{provenance['by_id']}**")
        rec = provenance["recovered"]
        out.append(f"- recovered from reference versions: **{rec}**"
                   + (f" (same-file {groups['same-file']}, edited-site {groups['edited-site']}, "
                      f"file-move {groups['file-move']}, moved-item {groups['moved-item']})"
                      if rec else ""))
        refs = provenance["references"]
        out.append(f"- references consulted: {', '.join(refs) if refs else 'none'}")
        if provenance.get("baselined"):
            out.append(f"- annotated regions, baseline -> this bank: "
                       f"**{provenance['before']} -> {provenance['after']}**")
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
    ("debug/", "diagnostics"),
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
    ap.add_argument("--reference", action="append", metavar="REV[:PATH]",
                    help="with --ledger: a commit holding a sidecar (default path) and "
                         "the tree it described, to take annotations from; repeatable")
    ap.add_argument("--reference-history", nargs="?", const="0", default=None, metavar="N",
                    help="with --ledger: also take every commit that changed the sidecar "
                         "(the newest N when given) as a reference")
    ap.add_argument("--base", action="append", metavar="REV",
                    help="with --ledger: the commit(s) whose per-file annotated counts "
                         "the new sidecar must not fall below (default HEAD plus "
                         "MERGE_HEAD, or the parents of a merge commit)")
    ap.add_argument("--no-base", action="store_true",
                    help="with --ledger: skip the per-file drop check")
    ap.add_argument("--no-edited", action="store_true",
                    help="with --ledger: refuse the edited-site tier (text changed)")
    ap.add_argument("--allow-drop", action="append", metavar="FILE",
                    help="with --ledger: permit this file's annotated count to drop; "
                         "repeatable, one path per flag")
    ap.add_argument("--decide", action="append", metavar="TSV",
                    help="with --ledger: a hand-decisions file to apply after the remap "
                         "(columns: file, owner, expect, artifact, count, reason); repeatable")
    ap.add_argument("--dry-run", action="store_true",
                    help="with --ledger: report only, write nothing")
    ap.add_argument("-v", "--verbose", action="store_true",
                    help="with --ledger: list every recovered region")
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
    noted = sum(1 for r in regions if r.get("safety_note"))
    print(f"SAFETY-note coverage: {noted}/{len(regions)} "
          f"({100.0 * noted / max(len(regions), 1):.1f}%)")
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
        sys.exit(rebank_ledger(args, regions))


if __name__ == "__main__":
    main()
