#!/usr/bin/env python3
# MIT License
# Copyright (c) 2026 Fei Wang

"""pass3.py — the seed-core pass-3 tool (docs/architecture/
seed-core-design.md §2c.5): collect a subsystem's scattered unsafe into
its `unsafe.rs`.

Pass 2 filed a subsystem into unsafe.rs / types.rs / hotpath.rs but left
the inline unsafe blocks of hotpath.rs (and, where present, types.rs and
mod.rs) in place. Pass 3 moves each one: every inline block becomes a
function in the subsystem's `unsafe.rs` carrying an INV contract, and
the call site uses that function ("a `hotpath.rs` function that used an
inline `unsafe { *p }` now calls an `unsafe.rs` function instead").
Content already in unsafe.rs is NOT touched — new sections are appended.

Like pass 2, the move is spec-driven so every hand replays the same
edit: scripts/refactor/pass3/<sub>.json lists one directive per region —

    {"op": "wrap" | "move-fn" | "move-item" | "inline-call",
     "file": <path inside the repo>, "owner": <census owner>,
     "kind": "block"|"unsafe-impl", "ordinal": <n-th region of that owner+kind>,
     "base_id": <census id at the spec's base — audit trail only>,

     "op": "wrap":        the block becomes a fn in unsafe.rs: "name",
       "params", "ret", "attr", "vis"; "call" = the call-site text (a
       whole line when the block owns its line(s), or — with
       "call_inline": true — a within-line template for blocks embedded
       in a larger expression); "body_suffix" = text appended after the
       block inside the fn body (the ` as *mut T` tails); "body_edits" =
       [{"find", "replace"}] exact, must-hit-once edits on the cut body
       (e.g. a `return` in the caller's scope translated to a returned
       bool — every edit lands in the manifest);
     "op": "move-fn":     the whole enclosing fn (its doc comment and
       attributes included) moves; "vis" overrides the declared
       visibility, "attr_add" adds an attribute line above it, "callmap"
       = [{"find", "replace"}] call-site line rewrites in the source
       file (each must hit exactly once outside every cut span);
       "impl_owner" wraps the moved fn in `impl <Type> { .. }` — for
       methods relocated out of their source impl block;
     "op": "move-item":   a whole item (an unsafe impl) with its
       preceding comment lines moves;
     "op": "inline-call": the block is replaced by a call to an ALREADY
       audited unsafe.rs function — no new fn ("call_inline": true,
       "call": the template);
     "section": <unsafe.rs banner>, "inv": <INV id>, "inv_note": <one
       doc line naming the contract>, "doc"/"contract": doc lines for
       generated wrappers}

and the spec's file-level fields carry the rest:
    "imports_rs":   use-lines appended after unsafe.rs's use block (so
                    the moved bodies stay verbatim),
    "source_edits": [{"file", "find", "replace", "why"}] — exact,
                    must-occur-once edits (visibility widenings the
                    moves force, statics the moved body reads),
    "forbid":       files gaining #![forbid(unsafe_code)],
    "registry":     paths appended to docs/development/forbid-registry.txt.

apply() writes a manifest (run/pass3-<sub>-moves.json): every cut span's
lines, every inserted line, every source edit. verify() recomputes the
AGGREGATE line multiset over the subsystem between a base rev and the
working tree, independently of the manifest, and closes the diff against
it: a verbatim move cancels in the aggregate (the line leaves
hotpath.rs and appears in unsafe.rs), so what remains is exactly what
the collection changed — cut boilerplate (`unsafe {` braces, a
caller-scoped `return`), wrapper signatures, call-site lines — and
every one of those must be claimed by the manifest. A diff line without
a claim is an edit the move did not document: fail.

Blank lines and full-line comments are not audited (pass 3 adds contract
doc comments by design; the census and the ledger gate cover those).

Subcommands
  apply --sub S [--spec PATH] [--dry]    execute the spec; write the manifest
  verify --sub S --base REV [--manifest] manifest-closed line-multiset audit
  plan --sub S [--spec PATH]             census of the subsystem's inline regions
  selftest                               fixture tree: wrap/move/inline + gates

Exit codes: 0 clean, 1 failure.
"""

import argparse
import collections
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from unsafe_census import (  # noqa: E402
    ROOT, mask_comments_and_strings, match_brace, find_body_brace, scan_file)

UNSAFE_RE = re.compile(r"\bunsafe\b")
FN_SPAN = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)")
FN_DECL = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))? )?(?:unsafe\s+)?(?:extern\s+\"C\"\s+)?fn\s+")
SPEC_DIR = ROOT / "scripts" / "refactor" / "pass3"


# ---------------------------------------------------------------------------
# line/char helpers
# ---------------------------------------------------------------------------

def char_to_line(text, i):
    """0-based line index holding char offset i."""
    return text.count("\n", 0, i)


def line_start(text, i):
    return text.rfind("\n", 0, i) + 1


def line_end(text, i):
    j = text.find("\n", i)
    return len(text) if j < 0 else j


def is_fill(line):
    """Blank or full-line comment — not audited, not a cut boundary."""
    s = line.strip()
    return (not s) or s.startswith("//")


def attr_line(line):
    s = line.strip()
    return s.startswith("#[") or s.startswith("#![") or s.startswith("#")


def walk_back_fill(lines, i):
    """First line index of the contiguous attr/comment run directly above
    line i. Blank lines stop the walk (a blank separates one item's doc
    run from the item above it), so a move-fn cut never steals the
    previous item's documentation."""
    s = i
    while s > 0 and (lines[s - 1].strip().startswith("//") or attr_line(lines[s - 1])):
        s -= 1
    return s


def region_char_span(text, masked, reg, kind):
    """Char span (start, end_exclusive) of one census region: its own
    `unsafe` keyword on its start line, brace-matched in masked text."""
    pos = 0
    for _ in range(reg["line"] - 1):
        pos = masked.find("\n", pos) + 1
    le = masked.find("\n", pos)
    le = len(masked) if le < 0 else le
    hits = []
    for m in UNSAFE_RE.finditer(masked, pos, le):
        if m.start() > 0 and masked[m.start() - 1] == "#":
            continue  # r#unsafe — the raw identifier, not the keyword
        j = m.end()
        while j < len(masked) and masked[j] in " \t":
            j += 1
        if kind == "block":
            if j < len(masked) and masked[j] == "{":
                hits.append((m.start(), match_brace(masked, j) + 1))
        elif j < len(masked) and (masked[j:j + 5] == "impl "
                                  or masked[j:j + 3] == "fn "
                                  or masked[j:j + 6] == "trait "
                                  or masked[j:j + 7] == "extern"):
            brace = find_body_brace(masked, j)
            if brace >= 0:
                hits.append((m.start(), match_brace(masked, brace) + 1))
    if not hits:
        raise SystemExit(f"pass3: cannot locate region {kind} at {reg['file']}:"
                         f"{reg['line']} ({reg.get('owner')})")
    return hits[0]


def fn_span_from(text, masked, inner_char):
    """Line span (first, last inclusive) of the innermost fn containing
    inner_char, doc comment and attributes included."""
    # an `unsafe fn` region's span starts at the KEYWORD, one token
    # before the `fn` FN_SPAN matches — advance to the fn's own body so
    # the enclosing-fn scan (which requires brace < inner_char) sees the
    # fn itself, not just its parent
    if masked[inner_char:inner_char + 6] == "unsafe":
        j = inner_char + 6
        while j < len(masked) and masked[j] in " \t":
            j += 1
        brace = find_body_brace(masked, j)
        if brace >= 0:
            inner_char = brace + 1
    best = None
    for m in FN_SPAN.finditer(masked):
        brace = find_body_brace(masked, m.start())
        if brace < 0 or brace >= inner_char:
            continue
        end = match_brace(masked, brace)
        if end < inner_char:
            continue
        if best is None or m.start() > best[0]:
            best = (m.start(), end)
    if best is None:
        raise SystemExit(f"pass3: no enclosing fn at char {inner_char}")
    lines = text.split("\n")
    decl_line = char_to_line(text, best[0])
    end_line = char_to_line(text, best[1])
    first = walk_back_fill(lines, decl_line)
    return first, end_line


def item_span_from(text, span_char):
    """Line span of a whole item (the unsafe-impl region) plus its
    preceding comment lines."""
    lines = text.split("\n")
    start = char_to_line(text, span_char[0])
    end = char_to_line(text, span_char[1] - 1)
    first = start
    while first > 0 and lines[first - 1].strip().startswith("//"):
        first -= 1
    return first, end


# ---------------------------------------------------------------------------
# apply
# ---------------------------------------------------------------------------

def apply_spec(spec, dry):
    sub = spec["sub"]
    target = ROOT / spec["target"]
    if not target.is_dir():
        raise SystemExit(f"pass3: no such subsystem dir: {spec['target']}")
    unsafe_rs = target / "unsafe.rs"
    if not unsafe_rs.is_file():
        raise SystemExit(f"pass3: no unsafe.rs in {spec['target']}")

    manifests, file_edits = [], []
    sections = collections.OrderedDict()
    section_order = []   # (section, spec_index, item) — written in spec order

    byfile = collections.defaultdict(list)
    for d in spec["moves"]:
        byfile[d["file"]].append(d)

    for rel, dirs in sorted(byfile.items()):
        path = ROOT / rel
        text = path.read_text()
        masked = mask_comments_and_strings(text)
        regions = scan_file(rel, text)
        # cut bottom-up: line indices of earlier cuts stay valid
        for d in sorted(dirs, key=lambda d: -census_line(regions, d)):
            reg = locate_region(regions, d)
            kind = reg["kind"]
            span = region_char_span(text, masked, reg, "block" if kind == "block" else "item")
            rec = {"file": rel, "op": d["op"], "owner": d.get("owner"),
                   "base_id": d.get("base_id"), "inv": d.get("inv"),
                   "section": d.get("section")}
            lines = text.split("\n")

            if d["op"] in ("wrap", "inline-call"):
                # inline-call is by definition a within-line replacement;
                # wrap opts in with "inline": true (a block embedded in a
                # larger expression)
                inline = bool(d.get("inline")) or d["op"] == "inline-call"
                ls, le = line_start(text, span[0]), line_end(text, span[1] - 1)
                first_line, last_line = char_to_line(text, span[0]), char_to_line(text, span[1] - 1)
                fn_lines = []
                if d["op"] == "wrap":
                    cut = "\n".join(lines[first_line:last_line + 1])
                    # the fn body carries the BLOCK's text: the exact char
                    # span when the block is embedded in a larger line, the
                    # whole (indented) lines when it owns them
                    fbody = text[span[0]:span[1]] if inline else cut
                    for e in d.get("body_edits") or []:
                        if fbody.count(e["find"]) != 1:
                            raise SystemExit(f"pass3: body_edit hits {fbody.count(e['find'])} "
                                             f"times (want 1): {e['find'][:70]!r}")
                        fbody = fbody.replace(e["find"], e["replace"])
                    sig = ""
                    if d.get("attr"):
                        sig += d["attr"] + "\n"
                    sig += f"{d.get('vis', 'pub(crate)')} fn {d['name']}({d.get('params', '')})"
                    if d.get("ret"):
                        sig += f" -> {d['ret']}"
                    fn_text = doc_for(d) + sig + " {\n" + fbody + d.get("body_suffix", "") + "\n}\n"
                    section_order.append((d["section"], spec["moves"].index(d), fn_text))
                    fn_lines = fn_text.rstrip("\n").split("\n")
                if inline:
                    # a within-line replacement: the call goes where the
                    # block stood, between the first line's prefix and the
                    # last line's suffix — the block may span many lines
                    # (`let x = unsafe {` ... `};`), so take the prefix and
                    # suffix from their OWN lines and collapse the whole
                    # span into the one call line
                    prefix = lines[first_line][:span[0] - ls]
                    ls_last = line_start(text, span[1] - 1)
                    suffix = text[span[1]:line_end(text, span[1] - 1)]
                    call = prefix + d["call"] + suffix
                    rec["cut_lines"] = lines[first_line:last_line + 1]
                    rec["inserted_lines"] = [call]
                    rec["whole_line"] = False
                    lines[first_line:last_line + 1] = [call]
                else:
                    cut = "\n".join(lines[first_line:last_line + 1])
                    call_lines = d["call"].split("\n")
                    if not inline and indent_of(call_lines[0]) != indent_of(lines[first_line]):
                        raise SystemExit(
                            f"pass3: call template indent {len(indent_of(call_lines[0]))} "
                            f"!= cut indent {len(indent_of(lines[first_line]))} "
                            f"for {d.get('owner')} (write the template at the cut's "
                            f"indentation)")
                    rec["cut_lines"] = cut.split("\n")
                    rec["inserted_lines"] = call_lines
                    rec["whole_line"] = True
                    lines[first_line:last_line + 1] = call_lines
                rec["fn_lines"] = fn_lines
                rec["call_line"] = call_lines[0] if not inline else call
                text = "\n".join(lines)
                masked = mask_comments_and_strings(text)
            elif d["op"] in ("move-fn", "move-item"):
                if d["op"] == "move-fn":
                    first_line, last_line = fn_span_from(text, masked, span[0])
                    fname = d.get("name") or fn_name_in(text, masked, span[0])
                else:
                    first_line, last_line = item_span_from(text, span)
                    fname = d.get("name") or d.get("owner", "item")
                cut = "\n".join(lines[first_line:last_line + 1]) + "\n"
                item = cut.rstrip("\n")
                rec["item_edits"] = []
                if d.get("vis"):
                    new_item, n = rewrite_vis(item, d["vis"])
                    if n != 1:
                        raise SystemExit(f"pass3: vis rewrite hit {n} times (want 1) "
                                         f"in {fname}")
                    old_decl = decl_line_of(item)
                    rec["item_edits"].append({"find": old_decl,
                                              "replace": decl_line_of(new_item)})
                    item = new_item
                if d.get("attr_add"):
                    item = d["attr_add"] + "\n" + item
                    rec["item_edits"].append({"find": None, "replace": d["attr_add"]})
                if d.get("inv_note"):
                    item = append_doc(item, d["inv_note"])
                    rec["item_edits"].append({"find": None, "replace": "/// " + d["inv_note"]})
                if d.get("impl_owner"):
                    # a method relocated out of its source impl block: it
                    # lands inside an impl of the same type in unsafe.rs at
                    # its original (member) indentation — verbatim
                    item = f"impl {d['impl_owner']} {{\n{item}\n}}"
                    rec["item_edits"].append(
                        {"find": None, "replace": f"impl {d['impl_owner']} {{"})
                section_order.append((d["section"], spec["moves"].index(d), item))
                rec["cut_lines"] = cut.rstrip("\n").split("\n")
                rec["inserted_lines"] = item.split("\n")
                rec["moved_fn"] = fname
                lines[first_line:last_line + 1] = []
                text = "\n".join(lines)
                masked = mask_comments_and_strings(text)
            else:
                raise SystemExit(f"pass3: unknown op {d['op']!r}")
            manifests.append(rec)
        for d in dirs:
            for e in d.get("callmap") or []:
                file_edits.append({"file": rel, "find": e["find"],
                                   "replace": e["replace"],
                                   "why": f"callmap for {d.get('owner')}"})
        if not dry:
            path.write_text(text)

    if dry:
        print(f"pass3 apply --dry: {len(manifests)} directive(s) on {sub}; "
              f"no file written")
        for m in manifests:
            print(f"  {m['op']:12s} {Path(m['file']).name}:{m.get('owner')} -> "
                  f"{m.get('section')} ({m.get('inv')})")
        return 0

    # call-site rewrites + general source edits (each must hit exactly once)
    for e in file_edits + list(spec.get("source_edits", [])):
        p = ROOT / e["file"]
        t = p.read_text()
        n = t.count(e["find"])
        if n != 1:
            raise SystemExit(f"pass3: source edit hits {n} times (want 1) in "
                             f"{e['file']}: {e['find'][:70]!r} ({e.get('why')})")
        p.write_text(t.replace(e["find"], e["replace"]))

    insert_imports(unsafe_rs, spec.get("imports_rs") or [])
    rs = unsafe_rs.read_text()
    rs += "\n// ==========================================================================\n"
    rs += f"// pass 3 (seed-core §2c.5): {sub}'s collected unsafe — the inline\n"
    rs += "// blocks of the subsystem's mechanism files, one contract each.\n"
    rs += "// ==========================================================================\n"
    for sec in dict.fromkeys(s for s, _, _ in section_order):
        items = [it for s, _, it in sorted(section_order, key=lambda t: (t[1],))
                 if s == sec]
        rs += f"\n// ------------ {sec} ------------\n\n"
        for it in items:
            rs += it + "\n"
    unsafe_rs.write_text(rs)

    for rel in spec.get("forbid", []):
        p = ROOT / rel
        t = p.read_text()
        ls = t.split("\n")
        already = any(l.strip().startswith("#![forbid(unsafe_code)]") for l in ls)
        if already:
            continue
        i = 0
        while i < len(ls) and (ls[i].strip().startswith("//!") or not ls[i].strip()):
            i += 1
        ls.insert(i, "#![forbid(unsafe_code)]")
        p.write_text("\n".join(ls))

    if spec.get("registry"):
        # the registry is append-ordered (each merge appends its entries);
        # keep that order — no re-sorting
        regp = ROOT / "docs" / "development" / "forbid-registry.txt"
        text = regp.read_text()
        add = [r for r in spec["registry"] if r not in text.split("\n")]
        if add:
            if not text.endswith("\n"):
                text += "\n"
            regp.write_text(text + "\n".join(add) + "\n")

    manifest = {"sub": sub, "moves": manifests,
                "file_edits": file_edits + list(spec.get("source_edits", [])),
                "imports_rs": spec.get("imports_rs") or [],
                "forbid": spec.get("forbid") or []}
    out = ROOT / "run" / f"pass3-{sub}-moves.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps(manifest, indent=1))
    ncut = sum(len(m.get("cut_lines", [])) for m in manifests)
    print(f"pass3 apply: {len(manifests)} directive(s) on {sub}, {ncut} line(s) cut; "
          f"manifest -> {out.relative_to(ROOT)}")
    for m in manifests:
        print(f"  {m['op']:12s} {Path(m['file']).name}:{m.get('owner')} -> "
              f"{m.get('section')} ({m.get('inv')})")
    return 0


def census_line(regions, d):
    hits = [r for r in regions
            if r["kind"] == d.get("kind", "block") and r.get("owner") == d.get("owner")]
    o = d.get("ordinal", 0)
    if not hits or o >= len(hits):
        raise SystemExit(f"pass3: no census region kind={d.get('kind')} "
                         f"owner={d.get('owner')} ordinal={o} in {d['file']}")
    return hits[o]["line"]


def locate_region(regions, d):
    hits = [r for r in regions
            if r["kind"] == d.get("kind", "block") and r.get("owner") == d.get("owner")]
    if not hits:
        raise SystemExit(f"pass3: census region not found: kind={d.get('kind')} "
                         f"owner={d.get('owner')} in {d['file']}")
    o = d.get("ordinal", 0)
    if o >= len(hits):
        raise SystemExit(f"pass3: ordinal {o} out of range for {d.get('owner')} "
                         f"({len(hits)} region(s))")
    return hits[o]


def indent_of(line):
    return line[:len(line) - len(line.lstrip())]


def doc_for(d):
    ls = []
    if d.get("doc"):
        ls.extend(("/// " + x) if x else "///" for x in d["doc"])
    if d.get("inv"):
        ls.append("///")
        ls.append(f"/// # Contract ({d['inv']})")
        for x in d.get("contract", []):
            ls.append("/// " + x if x else "///")
    return ("\n".join(ls) + "\n") if ls else ""


def append_doc(item, note):
    ls = item.split("\n")
    i = 0
    while i < len(ls) and ls[i].strip().startswith("//"):
        i += 1
    indent = ls[i][:len(ls[i]) - len(ls[i].lstrip())] if i < len(ls) else ""
    ls.insert(i, f"{indent}/// {note}")
    return "\n".join(ls)


def rewrite_vis(item, vis):
    hits = 0
    out = []
    EXISTING_VIS = re.compile(r"^(?:pub(?:\([^)]*\))?\s+)")
    for l in item.split("\n"):
        if not hits and FN_DECL.match(l):
            hits += 1
            indent = l[:len(l) - len(l.lstrip())]
            # strip an existing visibility before applying the override —
            # the spec's `vis` is the FINAL visibility, not a prefix
            # (an already-`pub` fn with vis "pub" would spell `pub pub`)
            l = indent + vis + " " + EXISTING_VIS.sub("", l.strip())
        out.append(l)
    return "\n".join(out), hits


def decl_line_of(item):
    for l in item.split("\n"):
        if FN_DECL.match(l):
            return l
    return ""


def fn_name_in(text, masked, inner_char):
    lines = text.split("\n")
    decl = char_to_line(text, inner_char)
    for i in range(decl, max(-1, decl - 6), -1):
        m = re.search(r"fn\s+([A-Za-z_][A-Za-z0-9_]*)", lines[i])
        if m:
            return m.group(1)
    return "item"


FN_NAME = re.compile(r"fn\s+([A-Za-z_][A-Za-z0-9_]*)")


def insert_imports(path, imports):
    if not imports:
        return
    ls = path.read_text().split("\n")
    last = 0
    for i, l in enumerate(ls):
        if l.startswith("use ") or l.startswith("pub use "):
            last = i
    ls[last + 1:last + 1] = list(imports)
    path.write_text("\n".join(ls))


# ---------------------------------------------------------------------------
# verify — manifest-closed line multiset audit
# ---------------------------------------------------------------------------

def content_lines(text):
    """Content lines: rstripped, blanks and full-line comments dropped."""
    out = collections.Counter()
    for l in text.split("\n"):
        l = l.rstrip()
        s = l.lstrip()
        if not s or s.startswith("//"):
            continue
        out[l] += 1
    return out


def content_line_p(l):
    s = l.strip()
    return bool(s) and not s.startswith("//")


def verify(sub, base_rev, manifest_path):
    man = json.loads(Path(manifest_path).read_text())
    files = sorted({m["file"] for m in man["moves"]} |
                   {e["file"] for e in man["file_edits"]} |
                   set(man["forbid"]))
    sub_dir = str(Path(files[0]).parent)
    dirpath = ROOT / sub_dir

    # tracked source only: build.rs writes gitignored placeholders
    # (ksyms_generated.rs) INSIDE subsystem dirs; they are build
    # artifacts, not source, and must not enter the audited multiset
    tracked = subprocess.run(
        ["git", "-C", str(ROOT), "ls-files", sub_dir],
        capture_output=True, text=True).stdout.split()
    rels = set(tracked)
    rels |= set(files)
    base, cur = collections.Counter(), collections.Counter()
    for rel in sorted(rels):
        r = subprocess.run(["git", "-C", str(ROOT), "show", f"{base_rev}:{rel}"],
                           capture_output=True, text=True)
        if r.returncode == 0:
            base += content_lines(r.stdout)
        p = ROOT / rel
        if p.is_file():
            cur += content_lines(p.read_text())
    removed, added = base - cur, cur - base

    claimed_removed, claimed_added = collections.Counter(), collections.Counter()
    for m in man["moves"]:
        for l in m.get("cut_lines", []):
            claimed_removed[l.rstrip()] += 1
        for l in m.get("inserted_lines", []):
            claimed_added[l.rstrip()] += 1
        for l in m.get("fn_lines", []):
            claimed_added[l.rstrip()] += 1
    for e in man["file_edits"]:
        # find/replace are exact-substring texts and may span several lines
        # (an insertion in place); the audit's currency is single lines, so
        # split both sides (mm's path-stability re-exports, drivers' const
        # rewrite do this)
        for l in e["find"].split("\n"):
            claimed_removed[l.rstrip()] += 1
        for l in e["replace"].split("\n"):
            claimed_added[l.rstrip()] += 1
    for l in man["imports_rs"]:
        claimed_added[l.rstrip()] += 1
    claimed_added["#![forbid(unsafe_code)]"] += len(man["forbid"])
    # claims live in the same currency as observations: content lines only
    claimed_removed = +collections.Counter({l: n for l, n in claimed_removed.items()
                                            if content_line_p(l)})
    claimed_added = +collections.Counter({l: n for l, n in claimed_added.items()
                                          if content_line_p(l)})

    failures = []
    # The audit's currency is the AGGREGATE line multiset over the
    # subsystem (base rev vs working tree). A verbatim move cancels: the
    # line leaves hotpath.rs and appears in unsafe.rs, so it is in neither
    # removed nor added. What remains is exactly what the move changed in
    # aggregate — cut boilerplate (`unsafe {` braces, a caller-scoped
    # `return`), wrapper signatures, call-site lines — and every one of
    # those must be claimed by the manifest. A diff line without a claim
    # is an edit the move did not document: fail.
    unclaimed_r = removed - claimed_removed
    unclaimed_a = added - claimed_added
    if unclaimed_r or unclaimed_a:
        print(f"verify: {sum(removed.values())} aggregate line change(s) removed, "
              f"{sum(added.values())} added; manifest claims "
              f"{sum(claimed_removed.values())} removal(s), "
              f"{sum(claimed_added.values())} addition(s)")
        for l, n in list(unclaimed_r.items())[:25]:
            failures.append(f"unclaimed REMOVAL x{n}: {l[:130]}")
        for l, n in list(unclaimed_a.items())[:25]:
            failures.append(f"unclaimed ADDITION x{n}: {l[:130]}")
        print("\nPASS3 VERIFY FAILED:")
        for f in failures:
            print(f"  - {f}")
        return 1
    print(f"verify: {sum(removed.values())} line(s) removed, {sum(added.values())} added "
          f"in aggregate; every change claimed by the manifest "
          f"({len(man['moves'])} move(s), {sum(m and len(m.get('cut_lines', [])) or 0 for m in man['moves'])} "
          f"cut line(s) relocated) — audit clean")
    return 0


# ---------------------------------------------------------------------------
# plan / selftest
# ---------------------------------------------------------------------------

def plan(spec_path):
    spec = json.loads(Path(spec_path).read_text())
    target = ROOT / spec["target"]
    specmoves = spec["moves"]
    for p in sorted(target.rglob("*.rs")):
        if p.name == "unsafe.rs":
            continue
        rel = str(p.relative_to(ROOT))
        regs = scan_file(rel, p.read_text())
        if not regs:
            continue
        print(f"{rel}: {len(regs)} region(s)")
        for r in regs:
            marked = any(d["file"] == rel and d.get("owner") == r["owner"]
                         and d.get("kind", "block") == r["kind"] and
                         d.get("ordinal", 0) == sum(1 for x in regs[:regs.index(r) + 1]
                                                    if x["kind"] == r["kind"] and
                                                    x.get("owner") == r["owner"]) - 1
                         for d in specmoves)
            print(f"  {'SPEC' if marked else '    '} L{r['line']:>6} {r['kind']:12s} "
                  f"{r['owner']} ord={r['ordinal']} id={r['id'][:12]}")
    return 0


def selftest():
    """Exercise wrap/move-fn/move-item/inline-call + the keyword gate on a
    fixture tree, then the manifest-closed verify against a fake base."""
    global ROOT
    old_root, tmp = ROOT, Path(tempfile.mkdtemp(prefix="pass3-selftest-"))
    try:
        ROOT = tmp
        src = "kernel/core/src/fixture"
        (tmp / src).mkdir(parents=True)
        (tmp / src / "unsafe.rs").write_text(
            "//! fixture unsafe\n\npub fn existing() -> u32 { 1 }\n")
        hot = (
            "//! hot\n\nstatic S: core::sync::atomic::AtomicU32 =\n"
            "    core::sync::atomic::AtomicU32::new(0);\n\n"
            "/// Doc of the reader.\n#[inline]\nfn reader(p: *const u32) -> u32 {\n"
            "    // SAFETY: contract\n    unsafe { core::ptr::read_volatile(p) }\n}\n\n"
            "pub fn caller(p: *const u32) -> u32 {\n"
            "    let v = reader(p);\n"
            "    let a = unsafe { core::ptr::read_volatile(p) };\n"
            "    S.store(v + a, core::sync::atomic::Ordering::Relaxed);\n    v + a\n}\n\n"
            "unsafe impl Send for Marker {}\npub struct Marker;\n")
        (tmp / src / "hotpath.rs").write_text(hot)
        spec = {
            "sub": "fixture", "target": src, "imports_rs": [],
            "forbid": [f"{src}/hotpath.rs"],
            "moves": [
                {"op": "move-fn", "file": f"{src}/hotpath.rs", "owner": "reader",
                 "kind": "block", "ordinal": 0, "base_id": "x2", "inv": "INV-X-1",
                 "section": "reads", "vis": "pub(crate)",
                 "inv_note": "INV-X-1: fixture contract.",
                 "callmap": [{"find": "    let v = reader(p);",
                              "replace": "    let v = crate::fixture::r#unsafe::reader(p);"}]},
                {"op": "wrap", "file": f"{src}/hotpath.rs", "owner": "caller",
                 "kind": "block", "ordinal": 0, "base_id": "x3", "inv": "INV-X-1",
                 "section": "reads", "name": "read_once", "params": "p: *const u32",
                 "ret": "u32", "attr": "#[inline(always)]", "vis": "pub(crate)",
                 "call": "    let a = crate::fixture::r#unsafe::read_once(p);",
                 "doc": ["Read one volatile u32."],
                 "contract": ["`p` is a valid readable u32."]},
                {"op": "move-item", "file": f"{src}/hotpath.rs",
                 "owner": "unsafe impl Send for Marker",
                 "kind": "unsafe-impl", "ordinal": 0, "base_id": "x4", "inv": "INV-X-2",
                 "section": "proofs", "inv_note": "INV-X-2: the fixture Send proof."},
            ],
        }
        apply_spec(spec, dry=False)
        hp = (tmp / src / "hotpath.rs").read_text()
        rs = (tmp / src / "unsafe.rs").read_text()
        checks = {
            "no unsafe left in hotpath (keyword)": re.search(r"(?<![A-Za-z0-9_#])unsafe(?![A-Za-z0-9_])",
                                                             mask_comments_and_strings(hp)) is None,
            "forbid attr inserted": "#![forbid(unsafe_code)]" in hp,
            "reader moved": "pub(crate) fn reader(p: *const u32) -> u32" in rs,
            "wrapper generated": "pub(crate) fn read_once(p: *const u32) -> u32" in rs,
            "impl moved": "unsafe impl Send for Marker {}" in rs,
            "call sites rewritten": "r#unsafe::reader(p);" in hp and
                                    "r#unsafe::read_once(p);" in hp,
            "wrapper body verbatim": "unsafe { core::ptr::read_volatile(p) }" in rs,
        }
        bad = [k for k, v in checks.items() if not v]
        for k, v in checks.items():
            print(f"  selftest {k}: {'ok' if v else 'FAIL'}")
        # manifest-closed verify vs the pre-apply content (simulate base rev
        # by stuffing the base text into git-less storage): run the same
        # accounting inline.
        base = content_lines(hot) + content_lines(
            "pub fn existing() -> u32 { 1 }\n")
        cur = content_lines(hp) + content_lines(rs)
        man = json.loads((tmp / "run" / "pass3-fixture-moves.json").read_text())
        cr, ca = collections.Counter(), collections.Counter()
        for m in man["moves"]:
            for l in m.get("cut_lines", []):
                cr[l.rstrip()] += 1
            for l in m.get("inserted_lines", []):
                ca[l.rstrip()] += 1
            for l in m.get("fn_lines", []):
                ca[l.rstrip()] += 1
        for e in man["file_edits"]:
            cr[e["find"].rstrip()] += 1
            ca[e["replace"].rstrip()] += 1
        ca["#![forbid(unsafe_code)]"] += len(man["forbid"])
        cr = +collections.Counter({l: n for l, n in cr.items() if content_line_p(l)})
        ca = +collections.Counter({l: n for l, n in ca.items() if content_line_p(l)})
        rem, add = base - cur, cur - base
        closed = not (rem - cr) and not (add - ca)
        print(f"  selftest manifest-closed accounting: {'ok' if closed else 'FAIL'}")
        ok = not bad and closed
        print("pass3 selftest:", "OK" if ok else "FAILED")
        return 0 if ok else 1
    finally:
        ROOT = old_root


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["apply", "verify", "plan", "selftest"])
    ap.add_argument("--sub", default="sched")
    ap.add_argument("--spec", default=None)
    ap.add_argument("--base", default=None)
    ap.add_argument("--manifest", default=None)
    ap.add_argument("--dry", action="store_true")
    args = ap.parse_args()
    if args.cmd == "selftest":
        return selftest()
    spec_path = args.spec or str(SPEC_DIR / f"{args.sub}.json")
    if args.cmd == "plan":
        return plan(spec_path)
    if args.cmd == "apply":
        return apply_spec(json.loads(Path(spec_path).read_text()), args.dry)
    if args.cmd == "verify":
        if not args.base:
            ap.error("verify needs --base REV")
        manifest = args.manifest or str(ROOT / "run" / f"pass3-{args.sub}-moves.json")
        return verify(args.sub, args.base, manifest)
    return 2


if __name__ == "__main__":
    sys.exit(main())
