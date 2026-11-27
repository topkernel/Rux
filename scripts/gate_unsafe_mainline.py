#!/usr/bin/env python3
"""Mainline unsafe gate — the minimal-first architecture's checker.

Rule: the unsafe keyword may appear only in kernel/src/<sub>/unsafe.rs
(one file per subsystem) plus kernel/src/tests/ (test code exercises
unsafe paths by design). Everything else must be safe. Comment/string
occurrences do not count; r#unsafe (the safe module identifier) does
not count.
"""
import os, re, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))  # repo root
SRC = os.path.join(ROOT, "kernel", "src")
SAFE = re.compile(r"r#unsafe")
LINE_COMMENT = re.compile(r"//")
GEN = {"ksyms_generated.rs"}

def strip_comments_strings(text):
    out, i, n, in_block, in_line, in_str = [], 0, len(text), False, False, False
    while i < n:
        c = text[i]
        nxt = text[i+1] if i+1 < n else ""
        if in_line:
            if c == "\n": in_line = False; out.append(c)
            i += 1; continue
        if in_str:
            if c == "\\": i += 2; continue
            if c == '"': in_str = False
            out.append(" " if c != "\n" else c); i += 1; continue
        if in_block:
            if c == "*" and nxt == "/": in_block = False; i += 2; out.append("  "); continue
            out.append(" " if c != "\n" else c); i += 1; continue
        if c == "/" and nxt == "/": in_line = True; out.append("  "); i += 2; continue
        if c == "/" and nxt == "*": in_block = True; out.append("  "); i += 2; continue
        if c == '"': in_str = True; out.append(" "); i += 1; continue
        if c == "'" and (i == 0 or text[i-1] not in "A-Za-z0-9_"):
            # lifetime ('a, 'static) vs char literal ('a', '\\n'): a lifetime's
            # quote is followed by an identifier char that is NOT closed by
            # another quote within three chars. Lifetimes pass through —
            # the old crude skip could swallow real unsafe keywords.
            j = i + 1
            if j < n and (text[j].isalnum() or text[j] == "_"):
                if j + 1 < n and text[j + 1] == "'":
                    out.append("   "); i = j + 2; continue   # 'x' char literal
                out.append(c); i += 1; continue              # lifetime — keep
            j = i + 1
            while j < n:
                if text[j] == "\\": j += 2; continue
                if text[j] == "'": break
                j += 1
            out.append(" " * (j - i + 1)); i = j + 1; continue
        out.append(c); i += 1
    return "".join(out)

def keyword_sites(text):
    t = strip_comments_strings(text)
    return [(m.start(), t[:m.start()].count("\n") + 1)
            for m in re.finditer(r"(?<!r)#?\bunsafe\b", t)
            if not SAFE.match(t[m.start():m.start() + 8])]

def main():
    failures = []
    files = {}
    for dp, dn, fn in os.walk(SRC):
        for f in fn:
            if not f.endswith(".rs") or f in GEN: continue
            p = os.path.join(dp, f)
            rel = os.path.relpath(p, SRC)
            files[rel] = p
    unsafe_files = set()
    for rel, p in sorted(files.items()):
        top = rel.split(os.sep)[0] if os.sep in rel else "(root)"
        text = open(p, errors="replace").read()
        sites = keyword_sites(text)
        if not sites: continue
        if rel.endswith("unsafe.rs") and os.sep in rel:
            continue  # the subsystem's one unsafe file
        if top == "tests":
            continue  # test code exercises unsafe paths by design
        unsafe_files.add(rel)
        lines = sorted(set(ln for _, ln in sites))
        failures.append(f"{rel}: {len(sites)} site(s) at lines {lines[:12]}")
    if failures:
        print(f"MAINLINE UNSAFE GATE FAILED: {len(failures)} file(s) hold unsafe outside unsafe.rs:")
        for f in failures: print("  " + f)
        return 1
    print(f"mainline unsafe gate: OK — unsafe only in per-subsystem unsafe.rs files "
          f"({len([r for r in files if r.endswith('unsafe.rs') and os.sep in r])} collected) and tests/")
    return 0

if __name__ == "__main__":
    sys.exit(main())
