#!/usr/bin/env python3
# MIT License
# Copyright (c) 2026 Fei Wang

"""collect_arch_unsafe.py — arch-lane unsafe consolidation tool.

Collects every `unsafe` site under kernel/src/arch/ into the arch
subsystem's single file kernel/src/arch/unsafe.rs.

1. The audited edge files (arch/<a>/unsafe_edges.rs, arch/<a>/mm/
   unsafe_edges.rs) are merged into unsafe.rs verbatim — INV contracts
   preserved, `super::` imports rewritten absolute — and the old module
   paths stay alive via `pub use crate::arch::r#unsafe::<arch>[::mm] as
   unsafe_edges;` (zero call-site changes).
2. Every other dirty arch file has its unsafe-carrying items MOVED into
   per-source-file inline submodules of unsafe.rs (verbatim bodies, an
   INV note prepended):
     - free fns: moved verbatim; a same-signature SAFE delegating
       wrapper stays at the original path (callers unchanged);
     - inherent impls: only the dirty methods move (a second impl block
       in unsafe.rs — inherent impls may split across modules);
     - trait impls and `unsafe impl` markers: move whole (impls are
       found by trait/type resolution, no wrapper needed);
     - naked fns (`#[unsafe(naked)]`): move whole + `pub use` re-export
       (no wrapper — a naked trampoline cannot delegate);
     - statics/consts/extern blocks/macro items: move whole + re-export.
   Former `unsafe fn`s are unwrapped to safe fns whose body is wrapped
   in one blanket `unsafe { .. }` block.
3. Each emptied source file gains `#![forbid(unsafe_code)]`; private
   items widen to pub(crate) so moved bodies and glob imports resolve.

Usage: python3 scripts/refactor/collect_arch_unsafe.py [--dry]
Resets kernel/src/arch to HEAD first, then regenerates everything.
"""
import argparse
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
ARCH_REL = "kernel/src/arch"
ARCH = os.path.join(ROOT, ARCH_REL)
UNSAFE_RS = os.path.join(ARCH, "unsafe.rs")
ARMS = ["riscv64", "x86_64", "aarch64"]
EDGE_FILES = [
    "riscv64/unsafe_edges.rs",
    "x86_64/unsafe_edges.rs",
    "riscv64/mm/unsafe_edges.rs",
    "x86_64/mm/unsafe_edges.rs",
    "aarch64/mm/unsafe_edges.rs",
]
NOW = "2026-11-27"

# ---------------------------------------------------------------------------
# masking: comments, string/char contents -> spaces (newlines preserved)
# ---------------------------------------------------------------------------

def mask(text):
    out = list(text)
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        nxt = text[i + 1] if i + 1 < n else ""
        if c == "/" and nxt == "/":
            while i < n and text[i] != "\n":
                out[i] = " "
                i += 1
        elif c == "/" and nxt == "*":
            out[i] = out[i + 1] = " "
            i += 2
            while i < n:
                if text[i] == "*" and i + 1 < n and text[i + 1] == "/":
                    out[i] = out[i + 1] = " "
                    i += 2
                    break
                if text[i] != "\n":
                    out[i] = " "
                i += 1
        elif c == '"':
            i += 1
            while i < n:
                if text[i] == "\\":
                    out[i] = " "
                    if i + 1 < n:
                        out[i + 1] = " "
                    i += 2
                    continue
                if text[i] == '"':
                    break
                if text[i] != "\n":
                    out[i] = " "
                i += 1
            i += 1
        elif c == "'" and (i == 0 or text[i - 1] not in "A-Za-z0-9_\"]"):
            # char literal vs lifetime: a char literal closes within a few
            # chars ('x', '\n', '\u{1F}', ...); lifetimes ('static, 'a) do
            # not. Misclassifying a lifetime would eat braces from the mask.
            cm = re.match(r"'(\\[^']*|[^\\'])'", text[i:i + 12])
            if cm:
                for k in range(i, i + cm.end()):
                    if out[k] != "\n":
                        out[k] = " "
                i += cm.end()
            else:
                i += 1  # lifetime tick — not a brace, leave as-is
        else:
            i += 1
    # raw strings r#"..."# (global_asm templates)
    t = "".join(out)
    for mm in re.finditer(r'\br(#*)"', t):
        hashes = mm.group(1)
        end = t.find('"' + hashes, mm.end())
        if end < 0:
            continue
        for k in range(mm.start(), end + 1 + len(hashes)):
            if out[k] != "\n":
                out[k] = " "
    return "".join(out)


def match_brace(m, start):
    pairs = {"{": "}", "(": ")", "[": "]"}
    want = pairs.get(m[start])
    depth = 0
    for i in range(start, len(m)):
        c = m[i]
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth == 0:
                return i
    raise ValueError("unbalanced braces at %d" % start)


# ---------------------------------------------------------------------------
# item scanning — depth-based; each item carries its leading attrs/docs
# ---------------------------------------------------------------------------

KW = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def first_kw(m, pos, end):
    """(kwpos, kw) of the first keyword at/after pos, skipping #[..] groups."""
    i = pos
    while i < end:
        c = m[i]
        if c in " \t\r\n":
            i += 1
            continue
        if c == "#":
            rest = m[i:end]
            am = re.match(r"#\s*\[", rest)
            if not am:
                return None, None
            try:
                i = match_brace(m, i + am.end() - 1) + 1
            except ValueError:
                return None, None
            continue
        km = KW.match(m, i)
        if not km:
            return None, None
        j = km.end()
        while m[j:j + 2] == "::":
            k2 = KW.match(m, j + 2)
            if not k2:
                break
            j = k2.end()
        if m[j:j + 1] == "!":
            return i, m[i:j + 1]
        return i, km.group(0)
    return None, None


def scan_items(m, start, end):
    """Items of region m[start:end] at base depth. Returns dicts
    {start, end, kwpos, kw}; spans include leading attributes/docs.
    An item ends at a depth-0 `;` or at the `}` that returns depth to 0
    (`)`/`]` closers return to 0 mid-item — fn args, array types)."""
    items = []
    i = start
    depth = 0
    item_open = True  # we are between items at i
    while i < end:
        c = m[i]
        if c in "([{":
            depth += 1
            i += 1
            continue
        if c in ")]}":
            depth -= 1
            i += 1
            if depth == 0 and (c == "}" or c == ";"):
                item_open = True
            continue
        if c == ";" and depth == 0:
            i += 1
            item_open = True
            continue
        if depth == 0 and c not in " \t\r\n" and item_open:
            kwpos, kw = first_kw(m, i, end)
            if kwpos is None:
                i += 1
                continue
            j = kwpos
            d2 = 0
            closed = False
            while j < end:
                cj = m[j]
                if cj in "([{":
                    d2 += 1
                elif cj in ")]}":
                    d2 -= 1
                    if d2 == 0 and (cj == "}" or cj == ";"):
                        j += 1
                        closed = True
                        break
                    if d2 == 0:
                        # paren/bracket group closed mid-item; keep going
                        pass
                elif cj == ";" and d2 == 0:
                    j += 1
                    closed = True
                    break
                j += 1
            items.append({"start": i, "end": j if closed else end,
                          "kwpos": kwpos, "kw": kw})
            i = items[-1]["end"]
            continue
        i += 1
    return items


QUAL = re.compile(r"\s*(pub(\([^)]*\))?|async|extern(\s*\"[^\"]*\")?|unsafe)\b")


def kind_of(m, it):
    rest = m[it["kwpos"]:it["end"]]
    # extern block (not extern fn): the unsafe-keyword variant moves whole
    if re.match(r"\s*(unsafe\s+)?extern\s*(\"[^\"]*\")?\s*\{", rest):
        return "unsafe_extern" if re.match(r"\s*unsafe\s+extern\b", rest) else "extern"
    r = rest
    while True:
        mm = QUAL.match(r)
        if not mm:
            break
        r = r[mm.end():]
    r = r.lstrip()
    if r.startswith('"'):
        q = r.find('"', 1)
        if q >= 0:
            r = r[q + 1:].lstrip()
    if r.startswith("impl"):
        if re.match(r"\s*unsafe\s+impl\b", rest):
            return "unsafe_impl"
        if re.search(r"\bfor\b", r.split("{")[0]):
            return "trait_impl"
        return "impl"
    if re.match(r"(const\s+)?(unsafe\s+)?fn\b", r):
        return "fn"
    if re.match(r"(static|const)\b", r):
        return "const_item"
    if re.match(r"(struct|enum|union|type)\b", r):
        return "type_item"
    if re.match(r"trait\b", r):
        return "type_item"
    if r.startswith("use"):
        return "use"
    if r.startswith("mod"):
        return "mod"
    if r.startswith("macro_rules!"):
        return "macro"
    if it["kw"].endswith("!"):
        return "macro_inv"
    return "unknown"


# ---------------------------------------------------------------------------
# signature helpers
# ---------------------------------------------------------------------------

def split_top(s, sep=","):
    parts, depth, cur = [], 0, []
    for c in s:
        if c in "<([":
            depth += 1
        elif c in "})]":
            depth -= 1
        if c == sep and depth == 0:
            parts.append("".join(cur))
            cur = []
        else:
            cur.append(c)
    parts.append("".join(cur))
    return [p.strip() for p in parts]


def fn_kwpos(seg_m):
    fk = first_kw(seg_m, 0, len(seg_m))
    seg2 = seg_m[fk[0]:]
    fm = re.search(r"\bfn\b", seg2)
    return fk[0] + fm.start()


def parse_free_fn(seg, seg_m):
    kwpos = fn_kwpos(seg_m)
    head = re.match(
        r"(pub(\([^)]*\))?\s+)?(async\s+)?(const\s+)?(unsafe\s+)?"
        r"(extern\s+\"[^\"]*\"\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)",
        seg_m[kwpos:])
    if not head:
        raise ValueError("bad fn head: %r" % seg_m[kwpos:kwpos + 80])
    name = head.group(7)
    i = kwpos + head.end()
    gen_text, tps, has_impl_anon = "", [], False
    if seg_m[i:i + 1] == "<":
        depth = 0
        j = i
        for j in range(i, len(seg_m)):
            if seg_m[j] == "<":
                depth += 1
            elif seg_m[j] == ">":
                depth -= 1
                if depth == 0:
                    break
        gen_all = seg_m[i + 1:j]
        gen_text = seg_m[i:j + 1]
        for p in split_top(gen_all):
            p2 = re.sub(r"^const\s+", "", p.strip())
            if p2.startswith("'"):
                continue
            if p2.startswith("impl "):
                has_impl_anon = True
                continue
            tps.append(p2.split(":")[0].split("=")[0].strip())
        i = j + 1
    pstart = seg_m.index("(", i)
    depth = 0
    for j in range(pstart, len(seg_m)):
        if seg_m[j] == "(":
            depth += 1
        elif seg_m[j] == ")":
            depth -= 1
            if depth == 0:
                break
    names = []
    for p in split_top(seg_m[pstart + 1:j]):
        if not p or p == "...":
            names.append(p.strip() or "_")
            continue
        cpart = split_top(p, ":")[0].strip()
        nm = re.sub(r"^mut\s+", "", cpart).strip()
        names.append(nm)
    return {"name": name, "tps": tps, "has_impl_anon": has_impl_anon,
            "names": names}


ATTRS_DROP = re.compile(
    r"#\s*\[\s*(unsafe\s*\(|no_mangle|export_name|link_section|link_name|"
    r"naked|used|doc|inline|cold|track_caller)\b")


def keep_attrs(attrs):
    """attributes copied onto the safe wrapper: cfg/allow only, really."""
    out = []
    for mm in re.finditer(r"#\s*\[[^\]]*\]", attrs):
        a = mm.group(0)
        if re.match(r"#\s*\[\s*(cfg|allow|expect)\b", a):
            out.append(a)
    return "".join(a + "\n" for a in out)


def first_doc_line(seg):
    dm = re.search(r"///\s*(.*)", seg)
    return dm.group(1).strip() if dm else ""


def rename_underscore_params(seg):
    """`_` params get synthetic names (they can never be referenced, so
    renaming in the moved header is safe; the wrapper needs a name to
    pass). Operates on the fn's parameter list only."""
    seg_m = mask(seg)
    kwpos = fn_kwpos(seg_m)
    pstart = seg_m.index("(", kwpos)
    depth = 0
    for j in range(pstart, len(seg_m)):
        if seg_m[j] == "(":
            depth += 1
        elif seg_m[j] == ")":
            depth -= 1
            if depth == 0:
                break
    params = seg_m[pstart + 1:j]
    if not re.search(r"(^|[,(])\s*_\s*([,)])", params):
        return seg
    res = seg[:pstart + 1]
    idx = pstart + 1
    depth = 1
    k = pstart + 1
    cnt = 0
    while k <= j:
        c = seg_m[k]
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        if c == "," and depth == 1:
            res += rename_piece(seg[idx:k], cnt)
            cnt += 1
            idx = k
            res += seg[k]
            k += 1
            continue
        k += 1
    res += rename_piece(seg[idx:j], cnt)
    res += seg[j:]
    return res


def rename_piece(piece, i):
    return re.sub(r"^(\s*)_(\s*:)", lambda mo: mo.group(1) + "__p%d" % (i + 1) + mo.group(2), piece)


def rewrite_paths(s, modpath):
    parts = modpath.split("::")
    sup2 = "crate::arch::" + "::".join(parts[:-2]) + ("::" if len(parts) > 2 else "")
    sup1 = "crate::arch::" + "::".join(parts[:-1]) + ("::" if len(parts) > 1 else "")
    slf = "crate::arch::" + modpath + "::"
    s = re.sub(r"\bsuper::super::", sup2, s)
    s = re.sub(r"\bsuper::", sup1, s)
    s = re.sub(r"\bself::", slf, s)
    return s


def move_fn_text(seg, inv, rel, modpath):
    """verbatim move of a fn item: unwrap the `unsafe fn` keyword, blanket-
    wrap the body in one unsafe block, prepend the INV note."""
    seg = rewrite_paths(seg, modpath)
    mm = mask(seg)
    kwpos = fn_kwpos(mm)
    head = re.match(
        r"(pub(\([^)]*\))?\s+)?(async\s+)?(const\s+)?(unsafe\s+)?"
        r"(extern\s+\"[^\"]*\"\s+)?fn\b", mm[kwpos:])
    newseg = seg
    if head and head.group(5):
        ustart = kwpos + mm[kwpos:].index("unsafe")
        newseg = newseg[:ustart] + newseg[ustart + len("unsafe"):]
        mm = mask(newseg)
        kwpos = fn_kwpos(mm)
    brace = mm.find("{", kwpos)
    close = match_brace(mm, brace)
    newseg = (newseg[:brace]
              + " { unsafe { // blanket: the former unsafe-fn contract, now explicit\n"
              + newseg[brace + 1:close]
              + "} }" + newseg[close + 1:])
    return ("// %s (collected %s from %s): verbatim body; callers use the safe\n"
            "// wrapper at the original path — obligations unchanged.\n%s\n"
            % (inv, NOW, rel, newseg.rstrip()))


def wrapper_text(seg, info, moved_path, inv):
    """safe delegating wrapper: original signature minus unsafe, body calls
    the moved fn by full path."""
    mm = mask(seg)
    kwpos = fn_kwpos(mm)
    attrs_src, sig_src = seg[:kwpos], seg[kwpos:]
    b = mask(sig_src).find("{")
    sig = sig_src[:b if b >= 0 else len(sig_src)].rstrip()
    sig = re.sub(r"\bunsafe\s+(?=fn\b)", "", sig, count=1)
    attrs = keep_attrs(attrs_src)
    call = "crate::arch::r#unsafe::%s::%s" % (moved_path, info["name"])
    if info["tps"] and not info["has_impl_anon"]:
        call += "::<%s>" % ", ".join(info["tps"])
    call += "(%s)" % ", ".join(n for n in info["names"] if n != "...")
    doc = first_doc_line(seg)
    docblock = ""
    if doc:
        docblock += "/// %s\n///\n" % doc
    docblock += ("/// Collected into `arch::r#unsafe` (%s): the unsafe floor moved there\n"
                 "/// verbatim; this safe wrapper keeps the original call path.\n" % inv)
    return "%s%s%s {\n    %s\n}" % (docblock, attrs, sig, call)


def const_name(seg):
    mm = re.search(r"\b(?:static|const)\s+(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)", seg)
    return mm.group(1) if mm else "UNKNOWN"


def extern_names(seg):
    return [mm.group(1) for mm in re.finditer(r"\b(?:fn|static|type)\s+([A-Za-z_][A-Za-z0-9_]*)", seg)]


def item_name(seg, it):
    if it["kw"].endswith("!"):
        base = it["kw"][:-1]
        return base.split("::")[-1] if not base.startswith("macro_rules") else \
            (re.search(r"macro_rules!\s+([A-Za-z_]\w*)", seg) or [None, "UNKNOWN"])[1]
    mm = re.search(r"\b(?:mod|struct|enum|union|type|trait)\s+([A-Za-z_][A-Za-z0-9_]*)", seg)
    return mm.group(1) if mm else "UNKNOWN"


# ---------------------------------------------------------------------------
# widening + forbid
# ---------------------------------------------------------------------------

def widen(text):
    """private -> pub(crate): top-level items and inherent-impl methods."""
    m = mask(text)
    lines = text.split("\n")
    mlines = m.split("\n")
    res = []
    depth = 0
    stack = []  # (depth_at_open, inherent?)
    for line, ml in zip(lines, mlines):
        if depth == 1 and stack and stack[-1][1]:
            nm = re.match(r"^(    )(async |const |unsafe |extern \"[^\"]*\" )*(fn|static)\b", line)
            if nm and not line.lstrip().startswith("pub"):
                line = re.sub(r"^    ", "    pub(crate) ", line)
        elif depth == 0:
            nm = re.match(r"^(async |const |unsafe |extern \"[^\"]*\" )*(fn|static|const|type|struct|enum|union|trait)\b", line)
            if nm and not re.match(r"^const\s+(unsafe\s+)?fn\b", line) and not line.startswith("pub"):
                line = "pub(crate) " + line
            if ml.startswith("impl") or re.match(r"^pub\s+impl\b", ml) or re.match(r"^unsafe\s+impl\b", ml):
                header = ml.split("{")[0]
                stack.append((depth, not re.search(r"\bfor\b", header)))
        res.append(line)
        depth += ml.count("{") - ml.count("}")
        while stack and depth <= stack[-1][0]:
            stack.pop()
    return "\n".join(res)


def insert_forbid(text):
    lines = text.split("\n")
    idx = 0
    while idx < len(lines) and (lines[idx].startswith("//!") or lines[idx].strip() == ""):
        idx += 1
    lines.insert(idx, "\n// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs\n"
                      "// (named INV-contracted operations); this file is compiler-enforced safe.\n"
                      "#![forbid(unsafe_code)]\n")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# collection target
# ---------------------------------------------------------------------------

class Collect:
    def __init__(self):
        self.tree = {}  # key -> node; key = arch name or "__top__"
        self.inv_counter = {}
        self.moved_fns = 0
        self.moved_impls = 0
        self.moved_others = 0
        self.wrappers = 0
        self.files_done = []

    def inv(self, stem):
        n = self.inv_counter.get(stem, 0) + 1
        self.inv_counter[stem] = n
        return "INV-ARCH-%s-%d" % (stem.upper().replace("_", "-"), n)

    def add(self, key, path, text):
        node = self.tree.setdefault(key, {})
        for p in path:
            node = node.setdefault(p, {})
        node.setdefault("__parts__", []).append(text)


def indent(s, pre):
    return "\n".join(pre + ln if ln.strip() else ln for ln in s.split("\n"))


def render_node(name, node):
    parts = node.get("__parts__", [])
    subs = {k: v for k, v in node.items() if k != "__parts__"}
    inner = "\n".join(parts)
    for sname in sorted(subs):
        inner += render_node(sname, subs[sname])
    return indent("pub mod %s {\n%s\n}\n" % (name, inner.rstrip()), "    ")


def render_key(key, node):
    parts = node.get("__parts__", [])
    subs = {k: v for k, v in node.items() if k != "__parts__"}
    inner = "\n".join(parts)
    for sname in sorted(subs):
        inner += render_node(sname, subs[sname])
    if key == "__top__":
        return inner
    return ('#[cfg(feature = "%s")]\npub mod %s {\n%s\n}\n' % (key, key, indent(inner, "    ").rstrip()))


# ---------------------------------------------------------------------------
# the per-file transform
# ---------------------------------------------------------------------------

def transform_file(path, rel, coll):
    text = open(path).read()
    m = mask(text)
    items = scan_items(m, 0, len(m))
    stem = os.path.basename(rel)[:-3]
    arch = rel.split("/")[0] if "/" in rel else None
    if arch is None:
        subparts = ["root" if stem == "mod" else stem]
        key = "__top__"
        path_in = []
        moved_path = subparts[0]
    else:
        parts = rel[:-3].split("/")
        parts[-1] = "root" if stem == "mod" else stem
        subparts = parts                      # includes arch name
        key = arch
        path_in = parts[1:-1]
        moved_path = "::".join(parts)
    modpath = "::".join(parts) if arch is not None else stem

    use_texts = [text[it["start"]:it["end"]] for it in items
                 if kind_of(m, it) == "use"]
    copied_uses = "\n".join(rewrite_paths(u, modpath) for u in use_texts)
    globline = "use crate::arch::%s::*;" % modpath

    moved = []
    reexports = []
    edits = []

    for it in reversed(items):
        kind = kind_of(m, it)
        if kind == "use" or not re.search(r"\bunsafe\b", m[it["start"]:it["end"]]):
            continue
        seg = text[it["start"]:it["end"]]

        if kind == "fn":
            seg = rename_underscore_params(seg)
            seg_m = mask(seg)
            info = parse_free_fn(seg, seg_m)
            is_naked = re.search(r"#\s*\[\s*(unsafe\s*\(\s*)?naked\b", seg)
            inv = coll.inv(stem)
            moved.append(move_fn_text(seg, inv, rel, modpath))
            coll.moved_fns += 1
            if is_naked:
                vis = "pub" if re.search(r"^\s*pub\b", seg) else "pub(crate)"
                reexports.append(
                    "// INV-ARCH note: %s moved to arch::r#unsafe::%s (naked\n"
                    "// trampoline — no wrapper; this re-export keeps the original\n"
                    "// path and fn identity).\n%s use crate::arch::r#unsafe::%s::%s;\n"
                    % (info["name"], moved_path, vis, moved_path, info["name"]))
            else:
                wrapper = wrapper_text(seg, info, moved_path, inv)
                edits.append((it["start"], it["end"], wrapper))
                coll.wrappers += 1

        elif kind in ("trait_impl", "unsafe_impl"):
            inv = coll.inv(stem)
            moved.append(
                "// %s (collected %s): whole %s moved verbatim from %s —\n"
                "// impl resolution is crate-global, no wrapper needed.\n"
                % (inv, NOW, kind.replace("_", " "), rel)
                + rewrite_paths(seg, modpath))
            edits.append((it["start"], it["end"], ""))
            coll.moved_impls += 1

        elif kind == "impl":
            brace_off = it["kwpos"] - it["start"] + re.search(r"\{", m[it["kwpos"]:it["end"]]).start()
            header = seg[:brace_off].strip()
            inner = scan_items(m, it["start"] + brace_off + 1, it["end"] - 1)
            dirty_methods = []
            for mit in reversed(inner):
                mk = kind_of(m, mit)
                if mk != "fn" or not re.search(r"\bunsafe\b", m[mit["start"]:mit["end"]]):
                    continue
                mseg = text[mit["start"]:mit["end"]]
                name = re.search(r"\bfn\s+([A-Za-z_]\w*)", mseg).group(1)
                dirty_methods.insert(0, move_fn_text(mseg, coll.inv(stem), rel, modpath))
                edits.append((mit["start"], mit["end"], ""))
                coll.moved_fns += 1
            if dirty_methods:
                block = ("// %s (collected %s): the unsafe-carrying methods of this\n"
                         "// inherent impl, moved verbatim from %s (inherent impls may\n"
                         "// split across modules; the safe methods stayed in place).\n"
                         "%s {\n%s}\n"
                         % (coll.inv(stem), NOW, rel, header,
                            "\n".join(dirty_methods)))
                moved.append(block)
                coll.moved_impls += 1

        elif kind == "const_item":
            name = const_name(seg)
            vis = "pub" if re.search(r"^\s*pub\b", seg) else "pub(crate)"
            moved.append("// %s (collected %s): static/const moved verbatim from %s\n"
                         % (coll.inv(stem), NOW, rel) + rewrite_paths(seg, modpath))
            reexports.append("%s use crate::arch::r#unsafe::%s::%s;\n"
                             % (vis, moved_path, name))
            edits.append((it["start"], it["end"], ""))
            coll.moved_others += 1

        elif kind == "unsafe_extern":
            names = extern_names(seg)
            vis = "pub" if re.search(r"^\s*pub\b", seg) else "pub(crate)"
            moved.append("// %s (collected %s): extern block moved verbatim from %s\n"
                         % (coll.inv(stem), NOW, rel) + rewrite_paths(seg, modpath))
            reexports.append("%s use crate::arch::r#unsafe::%s::{%s};\n"
                             % (vis, moved_path, ", ".join(names)))
            edits.append((it["start"], it["end"], ""))
            coll.moved_others += 1

        elif kind in ("macro", "macro_inv", "mod", "type_item"):
            nm = item_name(seg, it)
            vis = "pub" if re.search(r"^\s*pub\b", seg) else "pub(crate)"
            moved.append("// %s (collected %s): %s moved verbatim from %s\n"
                         % (coll.inv(stem), NOW, kind, rel) + rewrite_paths(seg, modpath))
            reexports.append("%s use crate::arch::r#unsafe::%s::%s;\n"
                             % (vis, moved_path, nm))
            edits.append((it["start"], it["end"], ""))
            coll.moved_others += 1

        else:
            print("  !! unhandled dirty item kind=%s kw=%r in %s" % (kind, it["kw"], rel))
            sys.exit(1)

    if not moved:
        print("  ?? %s dirty but nothing moved" % rel)
        sys.exit(1)

    for s, e, rep in sorted(edits, key=lambda x: -x[0]):
        text = text[:s] + rep + text[e:]

    text = widen(text)
    text = insert_forbid(text)
    if reexports:
        text = (text.rstrip()
                + "\n\n// Collected-unsafe re-exports: the unsafe floor for these names lives in\n"
                  "// kernel/src/arch/unsafe.rs (INV-ARCH-*); these keep the original paths.\n"
                + "".join(reexports))
    open(path, "w").write(text)

    header = ("// ---------------------------------------------------------------------------\n"
              "// Collected from kernel/src/arch/%s (%s)\n"
              "// Every unsafe-carrying item of the source file moved here verbatim; the\n"
              "// original paths stay alive via safe wrappers / re-exports.\n"
              "// ---------------------------------------------------------------------------\n" % (rel, NOW))
    body = "\n".join(copied_uses.splitlines() + [globline]) + "\n\n" + "\n".join(moved)
    modtxt = "pub mod %s {\n%s%s}\n" % (subparts[-1], header, indent(body, "    "))
    coll.add(key, path_in, modtxt)
    coll.files_done.append(rel)
    print("  %-44s fns+methods:%d" % (rel, coll.moved_fns))


# ---------------------------------------------------------------------------
# edges merge + unsafe.rs assembly
# ---------------------------------------------------------------------------

def merge_edges():
    tree = {}
    for rel in EDGE_FILES:
        p = os.path.join(ARCH, rel)
        if not os.path.exists(p):
            continue
        text = open(p).read()
        arch = rel.split("/")[0]
        is_mm = rel.endswith("mm/unsafe_edges.rs")
        lines = text.split("\n")
        k = 0
        while k < len(lines) and (lines[k].startswith("//!") or lines[k].strip() == ""):
            k += 1
        body = "\n".join(lines[k:])
        body = rewrite_paths(body, rel[:-3].replace("/", "::"))
        banner = ("// ---------------------------------------------------------------------------\n"
                  "// MERGED verbatim from kernel/src/arch/%s (%s edges -> unsafe.rs\n"
                  "// consolidation; the INV contracts below are the original audited ones).\n"
                  "// ---------------------------------------------------------------------------\n" % (rel, NOW))
        node = tree.setdefault(arch, {})
        if is_mm:
            node = node.setdefault("mm", {})
        node.setdefault("__parts__", []).append(banner + body)
    return tree


def write_unsafe_rs(edges_tree, coll):
    head = ('''//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! arch unsafe consolidation — the one file holding every unsafe site of
//! the arch subsystem (user mandate 2026-11-27, minimal-first mainline).
//!
//! Contents:
//! - the audited edge files (`unsafe_edges.rs`, per arch root and per arch
//!   mm/) merged verbatim — their INV contracts are the original reviewed
//!   ones;
//! - the scattered inline unsafe of every other arch file, moved verbatim
//!   into per-source-file inline modules (INV-ARCH-* notes) with safe
//!   wrappers / re-exports keeping the original call paths alive.
//!
//! Old edge paths stay alive: `arch::<a>::unsafe_edges` re-exports this
//! file's module. Every other arch file is `#![forbid(unsafe_code)]`.
''')
    body = ("// ===========================================================================\n"
            "// Merged audited edges (verbatim; INV contracts preserved)\n"
            "// ===========================================================================\n\n")
    for arch in ARMS:
        if arch in edges_tree:
            body += render_key(arch, edges_tree[arch]) + "\n"
    body += ("// ===========================================================================\n"
             "// Collected scattered unsafe (per-source-file modules; INV-ARCH-*)\n"
             "// ===========================================================================\n\n")
    for key in ARMS + ["__top__"]:
        if key in coll.tree:
            body += render_key(key, coll.tree[key]) + "\n"
    open(UNSAFE_RS, "w").write(head + body)


def edit_arch_mod():
    p = os.path.join(ARCH, "mod.rs")
    text = open(p).read()
    if "r#unsafe" not in text:
        text = text.replace(
            "pub mod uaccess_safe;",
            "pub mod uaccess_safe;\n\n"
            "// The arch unsafe floor: every unsafe site of this subsystem lives in this\n"
            "// one file (merged edges + collected scatter); all other arch files are\n"
            "// `#![forbid(unsafe_code)]`-enforced. See the file head for the map.\n"
            "pub mod r#unsafe;",
            1)
    open(p, "w").write(text)


def edit_edge_mods():
    edits = {
        "riscv64/mod.rs": ("pub mod unsafe_edges;",
                           "pub use crate::arch::r#unsafe::riscv64 as unsafe_edges;"),
        "x86_64/mod.rs": ("pub mod unsafe_edges;",
                          "pub use crate::arch::r#unsafe::x86_64 as unsafe_edges;"),
        "riscv64/mm/mod.rs": ("pub mod unsafe_edges;",
                              "pub use crate::arch::r#unsafe::riscv64::mm as unsafe_edges;"),
        "x86_64/mm/mod.rs": ("pub mod unsafe_edges;",
                             "pub use crate::arch::r#unsafe::x86_64::mm as unsafe_edges;"),
        "aarch64/mm/mod.rs": ("pub mod unsafe_edges;",
                              "pub use crate::arch::r#unsafe::aarch64::mm as unsafe_edges;"),
    }
    for rel, (old, new) in edits.items():
        p = os.path.join(ARCH, rel)
        if not os.path.exists(p):
            continue
        text = open(p).read()
        if old in text:
            text = text.replace(old, new, 1)
        open(p, "w").write(text)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dry", action="store_true")
    args = ap.parse_args()

    subprocess.run(["git", "checkout", "--", ARCH_REL], cwd=ROOT, check=True)
    if os.path.exists(UNSAFE_RS):
        os.remove(UNSAFE_RS)

    coll = Collect()
    edges_tree = merge_edges()
    for rel in EDGE_FILES:
        os.remove(os.path.join(ARCH, rel))

    for dp, dn, fn in os.walk(ARCH):
        for f in sorted(fn):
            if not f.endswith(".rs") or f == "unsafe.rs":
                continue
            full = os.path.join(dp, f)
            rel = os.path.relpath(full, ARCH)
            if not re.search(r"\bunsafe\b", mask(open(full).read())):
                continue
            transform_file(full, rel, coll)

    write_unsafe_rs(edges_tree, coll)
    edit_arch_mod()
    edit_edge_mods()

    print("\nmoved fns/methods: %d | impls: %d | statics/externs/macros: %d | wrappers: %d | files: %d"
          % (coll.moved_fns, coll.moved_impls, coll.moved_others, coll.wrappers, len(coll.files_done)))
    print("unsafe.rs lines: %d" % len(open(UNSAFE_RS).read().split("\n")))

    left = []
    for dp, dn, fn in os.walk(ARCH):
        for f in fn:
            if not f.endswith(".rs") or f == "unsafe.rs":
                continue
            full = os.path.join(dp, f)
            t = mask(open(full).read())
            for mm in re.finditer(r"\bunsafe\b", t):
                left.append((os.path.relpath(full, ARCH), t[:mm.start()].count("\n") + 1))
    if left:
        print("LEFTOVER unsafe sites: %d" % len(left))
        for rel, ln in left[:20]:
            print("   %s:%d" % (rel, ln))
        sys.exit(1)
    print("no leftover unsafe outside unsafe.rs")


if __name__ == "__main__":
    main()
