#!/usr/bin/env python3
"""pass2.py — the shared tool of seed-core pass 2 (docs/architecture/
seed-core-design.md §2c.3 and §2c.6).

Pass 2 files each subsystem directory of the core into `unsafe.rs`,
`types.rs` and `hotpath.rs`. After the merge, every reference to the
subsystem's old modules takes a new path:

    crate::<sub>::<old module>::<x>  ->  crate::<sub>::types::<x>   when x is defined in types.rs
                                     ->  crate::<sub>::hotpath::<x>  otherwise
    crate::<sub>::<root item>        ->  the same two rules
    rux::<sub>::...                  ->  the same, written from a service crate

The rule is a function of one small file per subsystem,
scripts/refactor/pass2/<sub>.json:

    {"modules": ["<old module>", ...],                   the modules merged away
     "renames": {"<old module>::<name>": "<new name>"},  a rename the single file forced
     "keep":    ["<module that stays>"],                 optional
     "inline":  ["<directory kept as one inline module of hotpath.rs>"],
                                                         a directory whose per-architecture
                                                         modules are chosen by #[cfg] on the module
     "root_items": ["<name reachable at the subsystem's root before the merge>"],
     "exports": {"<old module>": ["<the names it exported>"]}}

so the rewrite is deterministic and can be replayed. Lanes run in
parallel and each rewrites the whole tree; when two of them are merged
one after the other, the earlier one's rewrite is run again over the
files the later one brought in, and the result is what the later lane
would have produced on top of the earlier one.

A `use` statement keeps its shape: only the segments that name an old
module are replaced, so the diff stays inside the lines that change.

How a service crate reaches the subsystem decides what is rewritten there:

  - its root re-exports the subsystem (`pub use rux::{.., mm, ..}`):
    `crate::mm::..` is the core's path and keeps its head;
  - its root re-exports old modules by name (`pub use rux::process::{task, ..}`):
    nothing at the root denotes them after the merge, so the statement
    becomes a re-export of `types` and `hotpath`, and every `crate::task::x`
    or bare `task::x` in the crate is written in full (`rux::process::..`);
  - a glob over the whole subsystem (`pub use rux::fs::*`) becomes one over
    hotpath and one over types, and the module names it brought into scope
    are written in full wherever they are used;
  - a glob over one old module (`pub use rux::ipc::util::*`) becomes the
    list of the names that module exported (`exports` in the spec);
  - otherwise `crate::` in a service crate is the crate's own and is left
    alone.

Subcommands
  spec     --sub S [--base REV] [--inline a,b]     write the spec file from the directory before the merge
  rewrite  --sub S [--root DIR] [--dry]            rewrite every .rs under kernel/
  check    --sub S [--root DIR]                    what is left to do
  pair     --sub S[,S..] --base ELF --cur ELF      function symbols of two builds
  verbatim --sub S --base REV [--cur REV]          code lines of the old files against the new
  selftest

`pair` and `verbatim` are the acceptance of a step: the compiled kernel
is unchanged apart from symbol names, and every code line of the old
files is in the new ones except `use` lines, inner attributes, module
declarations and lines whose only change is a path.
"""
import argparse
import bisect
import collections
import contextlib
import copy
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

SEG = r"(?:r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*)"
SEG_RE = re.compile(SEG)
FINAL = ("types", "hotpath", "r#unsafe", "unsafe")
# an item a file declares: at column 0 (the root of a crate), or at any depth
ITEM_DECL = re.compile(r'^(?:pub(?:\([^)]*\))? )?(?:unsafe\s+)?(?:extern\s+"C"\s+)?(?:mod|fn|const|static|type|struct|enum|trait|union|macro_rules!)\s+((?:r#)?[A-Za-z_]\w*)', re.M)
is_core_flag = [False]                # the file rewrite_text is working on is in the core
at_root_flag = [False]                # … and is the root file of its crate
ITEM_ANY = re.compile(r'^\s*(?:pub(?:\([^)]*\))? )?(?:unsafe\s+)?(?:extern\s+"C"\s+)?(?:mod|fn|const|static|type|struct|enum|trait|union)\s+((?:r#)?[A-Za-z_]\w*)', re.M)
# a path that starts at a root: crate::, rux::, self::, super:: (super::super:: ...)
HEAD = r"(?<![A-Za-z0-9_:])((?:super::)+|self::|crate::|rux::)"
PATH_RE = re.compile(HEAD + "(" + SEG + "(?:::" + SEG + ")*)")
USE_RE = re.compile(r"^[ \t]*(?:pub(?:\([a-z]+\))? )?use\b[^;]*;", re.M | re.S)
USE_HEAD = re.compile(r"^([ \t]*)(?:pub(?:\([a-z]+\))? )?use\b")
AS_ALIAS = re.compile(r"\s+as\s+(" + SEG + ")")
TOKEN = re.compile(r"""["'/]|(?<![A-Za-z0-9_])br?#*"|(?<![A-Za-z0-9_])r#*" """)
CHAR = re.compile(r"'(?:\\u\{[0-9a-fA-F]+\}|\\.|[^\\'\n])'")
# a type definition, at any indentation (inside a macro invocation such as bitflags!)
TYPE_DEF = re.compile(r"^\s*(?:pub(?:\([a-z]+\))? )?(?:struct|enum|union|type|trait)\s+([A-Za-z_]\w*)", re.M)
ROOT_CONST = re.compile(r"^(?:pub(?:\([a-z]+\))? )?(?:const|static)\s+([A-Za-z_]\w*)", re.M)
INLINE_MOD = re.compile(r"^\s*(?:pub(?:\([^)]*\))? )?mod +([A-Za-z_][A-Za-z0-9_]*) *\{")
GENERATED = "ksyms_generated.rs"


def norm(s):
    return s[2:] if s.startswith("r#") else s


# ------------------------------------------------------------------ scanner

def scan(text):
    """One byte per character: 0 code, 1 comment, 2 string or char literal.
    Indexes are preserved, so a position in the result is the same position
    in the text."""
    n = len(text)
    kind = bytearray(n)
    i = 0
    while True:
        m = TOKEN.search(text, i)
        if not m:
            break
        s = m.start()
        if m.lastgroup == "raw":
            term = '"' + "#" * m.group("raw").count("#")
            e = text.find(term, m.end())
            e = n if e < 0 else e + len(term)
            kind[s:e] = b"\x02" * (e - s)
            i = e
            continue
        ch = text[s]
        if ch == "/":
            c2 = text[s + 1:s + 2]
            if c2 == "/":
                e = text.find("\n", s)
                e = n if e < 0 else e
                kind[s:e] = b"\x01" * (e - s)
                i = e
                continue
            if c2 == "*":
                depth, j = 0, s
                while j < n:
                    if text.startswith("/*", j):
                        depth += 1
                        j += 2
                    elif text.startswith("*/", j):
                        depth -= 1
                        j += 2
                        if depth == 0:
                            break
                    else:
                        j += 1
                kind[s:j] = b"\x01" * (j - s)
                i = j
                continue
            i = s + 1
            continue
        if ch == '"':
            j = s + 1
            while j < n and text[j] != '"':
                if text[j] == "\\":
                    j += 1
                j += 1
            j = min(j + 1, n)
            kind[s:j] = b"\x02" * (j - s)
            i = j
            continue
        mc = CHAR.match(text, s)             # a char literal; a lifetime does not match
        if mc:
            kind[s:mc.end()] = b"\x02" * (mc.end() - s)
            i = mc.end()
            continue
        i = s + 1
    return kind


def mask(text, kind):
    """comments and literals blanked, newlines kept"""
    return "".join(c if kind[i] == 0 else ("\n" if c == "\n" else " ") for i, c in enumerate(text))


def inline_rows(code):
    """Per line: the inline modules (`mod x { .. }` written in this file)
    that are open at the start of the line. Inside one of them a `super::`
    does not reach the file's parent module."""
    depth, stack, rows = 0, [], []
    for line in code.split("\n"):
        rows.append([name for d, name in stack if d < depth])
        m = INLINE_MOD.match(line)
        if m:
            stack.append((depth, m.group(1)))
        depth += line.count("{") - line.count("}")
        while stack and stack[-1][0] >= depth:
            stack.pop()
    return rows


def lcp(lists):
    out = []
    for parts in zip(*lists):
        if all(p == parts[0] for p in parts):
            out.append(parts[0])
        else:
            break
    return out


# ------------------------------------------------------------------ the rule

class Node:
    __slots__ = ("own", "span", "base", "full", "kids", "alias", "alias_end", "ntail", "nfull", "dual", "via_root")

    def __init__(self):
        self.own, self.span, self.base, self.full = [], (0, 0), None, None
        self.kids, self.alias, self.alias_end, self.ntail, self.nfull = None, None, None, None, None
        self.dual, self.via_root = False, False


class Rewriter:
    def __init__(self, sub, modules, types, renames=None, keep=(), root_items=(), inline=(), exports=None):
        self.sub = sub
        self.modules = set(modules)
        self.inline = set(inline)
        self.types = set(types)
        self.renames = {tuple(k.split("::")): v for k, v in (renames or {}).items()}
        self.keep = set(keep or ())
        self.root_items = set(root_items or ())
        self.exports = exports or {}          # old module path -> the names it exported
        self.mods_top = set(modules)          # set by build_rewriter: the first segments of the old modules
        self.has_unsafe = False               # … the subsystem has an unsafe.rs
        self.crate = None                     # … the root of the crate the file being rewritten is in
        self.roots = {}                       # … the roots of every service crate

    def from_root(self, cr, segs):
        """`segs`: the segments after a service crate's name. The subsystem-relative
        path when the path reaches the subsystem through that crate's root: a
        module the root re-exports from the subsystem by name, or — after the
        merge, what such a name was — or the root of a crate it re-exports in
        turn. None otherwise."""
        if cr is None:
            return None
        d, hops = ("crate", cr.name), 0
        while d is not None and d[0] == "crate" and hops < 3 and len(segs) > hops:
            hops += 1
            other = self.roots.get(d[1])
            d = other.lookup(segs[hops - 1], self.mods_top) if other else None
        if d is not None and d[0] == "core":
            return list(d[1]) + segs[hops:]
        return None

    def via_path(self, segs):
        """`segs`: the segments after `crate::` in a service crate. The subsystem-relative
        path when the first name is another crate that the crate's root re-exports
        (`pub use ::fs;`) and the path reaches the subsystem through that crate's
        root; None otherwise."""
        if self.crate is None:
            return None
        d = self.crate.expl.get(segs[0])
        if d is None or d[0] != "crate":
            # not a re-export of another crate: the crate's own name, or the
            # subsystem itself under its name (which the caller handles)
            return None
        # the subsystem's own crate carries the subsystem's name: `crate::process` in a
        # crate whose root says `pub use ::process;` is that crate, not the core's module
        return self.from_root(self.roots.get(d[1]), segs[1:])

    def crate_name_path(self, segs, outright=True):
        """`segs`: the segments after a service crate's own name, written as
        `::process::task::x` or bare `process::task::x` in a file of another
        crate. The subsystem-relative path when it reaches the subsystem
        through that crate's root; None otherwise. A leading `::` names the
        crate whatever else is in scope; a bare name is the crate only where
        the file's own crate root does not put that name in scope."""
        if is_core_flag[0] or len(segs) < 2 or segs[0] not in self.roots:
            return None
        if self.crate is not None and (segs[0] == self.crate.name or (not outright and at_root_flag[0] and segs[0] in self.crate.expl)):
            return None
        return self.from_root(self.roots[segs[0]], segs[1:])

    def root_tail(self, segs, root_binds):
        """`segs`: the segments after `crate::` in a service crate. The subsystem-relative
        path when the first name is one the crate's root makes visible from the
        subsystem (re-exported by name, brought in by a glob, or reached through
        the root of another crate the root re-exports); None otherwise."""
        if segs[0] in root_binds:
            return list(root_binds[segs[0]]) + segs[1:]
        return self.via_path(segs)

    def through(self, tail):
        """written from the core's name, when the path goes through an old module;
        a path that stops at a root item of the subsystem keeps resolving through
        the crate's root and is left alone"""
        if tail is None or tail[0] not in self.mods_top:
            return None
        new = self.map_tail(tail)
        return None if new is None else "rux::" + self.sub + "::" + "::".join(new)

    def declared(self, code):
        """every name a file binds itself: items it declares at any depth, and the
        names its `use` statements import. A name the crate's root brings in by a
        glob is shadowed where the file declares it."""
        names = set(norm(x) for x in ITEM_ANY.findall(code))
        for m in USE_RE.finditer(code):
            stmt = m.group(0)
            i = USE_HEAD.match(stmt).end()
            while i < len(stmt) and stmt[i] in " \t\r\n":
                i += 1
            while stmt.startswith("::", i):
                i += 2
            try:
                node, _ = self.parse_item(stmt, i, [], [], False, {})
            except (AttributeError, IndexError):
                continue
            for nd in self.leaves(node):
                names.add(nd.alias or norm(nd.own[-1]))
        return names

    def map_tail(self, tail, in_use=False):
        """`tail`: the segments after the subsystem's own segment. The new
        segments, or None when the path already has its final form."""
        if not tail or tail[0] in FINAL or tail[0] in self.keep:
            return None
        if tail[0] in self.inline:
            return ["hotpath"] + list(tail)   # a directory kept as one inline module of hotpath.rs
        k = 0
        for n in range(len(tail), 0, -1):          # the longest prefix that was a module
            if "::".join(tail[:n]) in self.modules:
                k = n
                break
        rest = tail[k:]
        if not rest:
            # The path stops on a name that was a module. In a use tree that is
            # the module itself (an item of the same name is added by the
            # caller when one was reachable from the subsystem's root: `dual`).
            # Anywhere else a module cannot stand alone, so the name is that item.
            return ["hotpath"] if in_use else ["types" if tail[-1] in self.types else "hotpath", tail[-1]]
        item = rest[0]
        new_item = self.renames.get(tuple(tail[:k]) + (item,), item)
        return ["types" if item in self.types else "hotpath", new_item] + rest[1:]

    def root_index(self, full):
        """the index of the subsystem's segment when the path starts at the
        subsystem root; -1 otherwise"""
        return 1 if full and len(full) >= 2 and full[0] == "crate" and full[1] == self.sub else -1

    def old_tail(self, nd):
        r = self.root_index(nd.full) if nd.full else -1
        return nd.full[r + 1:] if r >= 0 else None

    def is_module_leaf(self, nd):
        """a leaf that names one of the old modules itself: a plain name, or
        `self` under it"""
        if nd.kids is not None or "*" in nd.own or nd.ntail is None or nd.full is None:
            return False
        return self.old_tail(nd) == [nd.full[-1]] and nd.full[-1] in self.modules

    # ---------------------------------------------------------- a whole file
    def rewrite_text(self, text, modpath, is_core, crate_alias=False, root_binds=None, at_root=False):
        """Every reference to the old modules in one source file.
        `modpath`: the file's module path in its crate. `is_core`: the file is
        in the core. `crate_alias`: the file is in a service crate whose root
        re-exports the subsystem itself. `root_binds`: {a module name the
        crate's root makes visible from the subsystem: its old module path} —
        reached from every file of the crate as `crate::name::x`, as
        `super::name::x` from a child module, or bare, unless the file declares
        a name of its own. `at_root`: this file is the crate's root. A path of
        a service crate may also reach the subsystem through the root of
        ANOTHER crate the root re-exports (`pub use ::fs;`): via_path."""
        kind = scan(text)
        code = mask(text, kind)
        rows = inline_rows(code)
        line_at = [0] + [m.end() for m in re.finditer("\n", text)]
        edits, globs, in_string, use_ranges, unread_uses, notes = [], [], [], [], [], []
        is_core_flag[0] = is_core
        at_root_flag[0] = at_root
        binds = {}                                  # a name THIS file imports -> what it denotes now
        root_binds = root_binds or {}
        head_text = "crate::" if is_core else "rux::"
        hot_imports = []                            # statements through which the file imports an old module by name

        def in_use(pos):
            return any(a <= pos < b for a, b in use_ranges)

        def inline_names(pos):
            return rows[bisect.bisect_right(line_at, pos) - 1]

        # ---- use statements
        for m in USE_RE.finditer(code):
            use_ranges.append((m.start(), m.end()))
            cur = list(modpath) + inline_names(m.start())
            top = code[m.start()] not in " \t" and not inline_names(m.start())
            local, new_binds, wglobs, unread = self.rewrite_use(
                m.group(0), cur, is_core, crate_alias, root_binds, at_root and top)
            for name, val in new_binds.items():
                binds.setdefault(name, val)
            edits += [(m.start() + a, m.start() + b, r) for a, b, r in local]
            globs += wglobs
            unread_uses += unread
            if any(v[0] == "hotpath" and v[1] for v in new_binds.values()):
                hot_imports.append((m.start(), m.end(), m.group(0), top))
        # two old modules imported by name now both read `use …::hotpath;`: with one
        # such import at the file's top level every other one is a duplicate
        tops = [h for h in hot_imports if h[3]]
        if tops:
            for a, b, stmt, _ in hot_imports:
                if (a, b) == (tops[0][0], tops[0][1]):
                    continue
                if "{" not in stmt and " as " not in stmt:
                    edits.append((a, b + 1 if b < len(text) and text[b] == "\n" else b, ""))
                    notes.append("a second `use …::hotpath;` of this file removed: " + stmt.strip())
                else:
                    unread_uses.append(stmt.strip() + "   (this file imports hotpath twice: delete one)")
        elif len(hot_imports) > 1:
            unread_uses.append(hot_imports[1][2].strip() + "   (imported inside a block, and so is another `use …::hotpath;` in this file: read them)")
        # the core's root file declares the module itself, so it names it bare
        if is_core and modpath == ["crate"]:
            binds.setdefault(self.sub, (self.sub, []))

        # ---- every other path
        def replace(m):
            if kind[m.start()] == 2:              # inside a string literal
                in_string.append(m.group(0))
                return m.group(0)
            head, segs = m.group(1), [norm(x) for x in m.group(2).split("::")]
            if head == "crate::" and not is_core:
                new = self.through(self.root_tail(segs, root_binds))
                if new is not None:
                    return new
            if head in ("crate::", "rux::"):
                if segs[:1] != [self.sub] or (head == "crate::" and not (is_core or crate_alias)):
                    return m.group(0)
                prefix, tail = segs[:1], segs[1:]
            else:
                cur = list(modpath) + inline_names(m.start())
                if head == "self::":
                    base = cur
                else:
                    k = head.count("super")
                    if k >= len(cur):
                        return m.group(0)
                    base = cur[:len(cur) - k]
                if not is_core:
                    # a service crate: only a path that starts at the crate's root
                    # can leave the crate, and only for a name the root makes
                    # visible from the subsystem
                    if base != ["crate"]:
                        return m.group(0)
                    new = self.through(self.root_tail(segs, root_binds))
                    return m.group(0) if new is None else new
                if base == ["crate"] and segs[:1] == [self.sub]:
                    prefix, tail = segs[:1], segs[1:]
                elif base == ["crate", self.sub]:
                    prefix, tail = [], segs
                else:
                    return m.group(0)
            new = self.map_tail(tail)
            return m.group(0) if new is None else head + "::".join(prefix + new)

        def via(m):
            """a path written through a name in scope: one this file imported
            (the subsystem's module, or an old module) or one the crate's root
            makes visible"""
            if kind[m.start()] == 2:
                in_string.append(m.group(0))
                return m.group(0)
            name, segs = m.group(1), [norm(x) for x in m.group(2).split("::")]
            if name in binds:
                new_name, old_module = binds[name]
                new = self.map_tail(list(old_module) + segs)
                if new is None:
                    return m.group(0)
                if old_module and new[0] == "hotpath":    # the imported name now IS the merged module
                    return new_name + "::" + "::".join(new[1:])
                if old_module:                            # an item that is not in hotpath: written in full
                    return head_text + self.sub + "::" + "::".join(new)
                return new_name + "::" + "::".join(new)
            new = self.map_tail(list(root_binds[name]) + segs)
            return m.group(0) if new is None else "rux::" + self.sub + "::" + "::".join(new)

        def named(m):
            """another service crate named outright, reaching the subsystem through its root"""
            if kind[m.start()] == 2:
                in_string.append(m.group(0))
                return m.group(0)
            new = self.through(self.crate_name_path([norm(m.group(1))] + [norm(x) for x in m.group(2).split("::")], m.group(0).startswith("::")))
            return m.group(0) if new is None else new

        # a name the root brings in by a glob is shadowed where the file declares it
        shadowed = self.declared(code) if root_binds and not is_core else set()
        names = sorted(set(binds) | (set(root_binds) - shadowed), key=len, reverse=True)
        rxs = [(PATH_RE, replace)]
        if names:
            rxs.append((re.compile(r"(?<![A-Za-z0-9_:])(" + "|".join(re.escape(n) for n in names) + r")::(" + SEG + "(?:::" + SEG + ")*)"), via))
        if not is_core and self.roots:
            # another service crate named outright: `::process::task::x`, or bare
            crates = sorted((set(self.roots) - set(binds) - shadowed), key=len, reverse=True)
            if crates:
                lead = re.compile(r"(?<![A-Za-z0-9_:])::(" + "|".join(re.escape(c) for c in crates) + r")::((?:" + SEG + r"::)*" + SEG + r")")
                bare = re.compile(r"(?<![A-Za-z0-9_:])(" + "|".join(re.escape(c) for c in crates) + r")::((?:" + SEG + r"::)*" + SEG + r")")
                rxs += [(lead, named), (bare, lambda m: m.group(0) if text[max(0, m.start() - 2):m.start()] == "::" else named(m))]
        for rx, fn in rxs:
            for m in rx.finditer(text):
                if not in_use(m.start()):
                    edits.append((m.start(), m.end(), fn(m)))

        out, pos, count = [], 0, 0
        for a, b, r in sorted(edits):
            if a < pos:                           # inside the previous edit
                continue
            if r != text[a:b]:
                count += 1
            out.append(text[pos:a])
            out.append(r)
            pos = b
        out.append(text[pos:])
        return "".join(out), {"replacements": count, "in_string": in_string, "globs": globs,
                              "aliases": sorted(binds), "unread": unread_uses, "notes": notes}

    # ------------------------------------------------------- one use tree
    def rewrite_use(self, stmt, cur, is_core, crate_alias=False, root_binds=None, at_top_of_crate=False):
        """`stmt`: one `use` statement. Returns ([(start, end, replacement)]
        within the statement, {a name it binds: (the name now, the old module
        path it denoted)}, [globs left as they are], [statements left alone])."""
        root_binds = root_binds or {}
        s = mask(stmt, scan(stmt))
        head = USE_HEAD.match(s)
        pub = re.match(r"^[ \t]*pub\b", s) is not None and re.match(r"^[ \t]*pub\(", s) is None
        indent = stmt[:len(stmt) - len(stmt.lstrip())]
        i = head.end()
        while i < len(s) and s[i] in " \t\r\n":
            i += 1
        outright = False
        while s.startswith("::", i):             # `use ::crate::..`: a leading colon names the crate outright
            outright = True
            i += 2
        try:
            node, _ = self.parse_item(s, i, [], cur, is_core or crate_alias, root_binds, False, outright)
        except (AttributeError, IndexError):      # not a tree this parser reads: leave it, say so
            return [], {}, [], [stmt.strip()]
        self.process(node)
        self.lift(node)
        self.process(node)
        if self.stray(node, True):
            return [], {}, [], [stmt.strip()]
        edits, globs, binds = [], [], {}
        root_pub = at_top_of_crate and pub and not is_core

        def rel_head(nd, own_new):
            """a top-level path keeps the head it was written with: only the
            part after a `self::`/`super::` head is rewritten, `rux::` is not
            turned into `crate::`, and a name reached through the crate's root
            is written from the core's name"""
            if nd.via_root:
                return ["rux", self.sub] + own_new[2:]
            h = 0
            while h < len(nd.own) and norm(nd.own[h]) in ("self", "super"):
                h += 1
            if h:
                return nd.own[:h] + own_new[len(nd.base):]
            if nd.own and own_new and nd.base == ["crate"]:
                return [nd.own[0]] + own_new[1:]
            return own_new

        def braces(items):
            return items[0] if len(items) == 1 else "{" + ", ".join(items) + "}"

        def expand(nd, own_new):
            """a glob: over the whole subsystem -> one over hotpath and one over
            types; over an old module -> the list of the names that module
            exported (None when the spec does not hold the list)"""
            tail = self.old_tail(nd)
            base = "::".join(own_new[:-2])              # own_new ends with hotpath, *
            lead = base + "::" if base else ""
            if tail == ["*"]:
                # the glob re-exported the subsystem's whole root: its modules that stay
                # (unsafe.rs, a directory kept under hotpath) stay exported beside the two
                parts = (["r#unsafe"] if self.has_unsafe else [])
                if self.inline:
                    parts.append("hotpath::{" + ", ".join(sorted(self.inline)) + "}")
                return lead + "{" + ", ".join(parts + ["hotpath::*", "types::*"]) + "}"
            module = tail[:-1]
            names = self.exports.get("::".join(module))
            if names is None:
                return None
            names = list(dict.fromkeys(names))
            if len(names) > 24:
                # a module of hundreds of names: the list would be pages long. The
                # glob stays, over the whole of hotpath now, and is reported; the
                # compiler names an ambiguity it causes.
                return None
            hot = [self.renames.get((tuple(module), n), n) for n in names if n not in self.types]
            typ = [self.renames.get((tuple(module), n), n) for n in names if n in self.types]
            if len(names) <= 6:
                parts = [t + "::" + braces(x) for t, x in (("hotpath", hot), ("types", typ)) if x]
                return lead + "{" + ", ".join(parts) + "}"
            lines = [lead + "{"]
            for t, x in (("hotpath", hot), ("types", typ)):
                if not x:
                    continue
                lines.append(indent + "    " + t + "::{")
                lines += [indent + "        " + n + "," for n in x]
                lines.append(indent + "    },")
            lines.append(indent + "}")
            return "\n".join(lines)

        reexported = []

        def leaf(nd, own_new):
            """(start, end, text) of a leaf; text None when it does not change"""
            a, b = nd.span
            old = "::".join(norm(x) for x in nd.own)
            if pub and not root_pub and self.is_module_leaf(nd) and nd.own != ["self"] and not nd.dual and nd.ntail is not None:
                # `pub use <old module>;` inside a module keeps that module's name for the
                # paths that go through it; its items are in two places now, and no one
                # statement gives them all: left as written, and reported
                reexported.append(stmt.strip())
                return a, b, None
            if "*" in nd.own:
                new = expand(nd, own_new)
                if new is None:                         # no export list: the glob stays a glob
                    new = "::".join(own_new)
                    if new.endswith("hotpath::*"):      # … and now spans the whole of hotpath
                        globs.append(stmt.strip())
                return a, b, (new if new != old else None)
            if nd.own == ["self"] and norm(nd.full[-1]) in self.root_items:
                # `mod::{self, ..}` where an item shares the module's name: the
                # name keeps denoting both — the module through an alias, the item directly
                M = norm(nd.full[-1])
                lead = "::".join(own_new) + "::" if own_new else ""
                return a, b, ("self" if not own_new else "::".join(own_new)) + " as " + M + ", " + lead + M
            if not own_new:
                return a, b, "self"
            if nd.dual:                                 # the module and the item of the same name
                name = norm(nd.own[-1])
                inner = ("self as %(x)s, %(n)s as %(x)s" % {"x": nd.alias, "n": name}) if nd.alias else ("self, " + name)
                return a, nd.alias_end or b, "::".join(own_new) + "::{" + inner + "}"
            new = "::".join(own_new)
            return a, b, (new if new != old else None)

        def erase(leaves):
            """remove leaves of a group together with one separator each. Marked
            by character first, so two neighbours that take the same comma do
            not overlap; a run that reaches the closing brace takes the
            separator before it instead of leaving a trailing one."""
            dead = set()
            for nd, a, b, t in leaves:
                j = b
                while j < len(s) and s[j] in " \t\r\n":
                    j += 1
                if j < len(s) and s[j] == ",":
                    k = j + 1
                    while k < len(s) and s[k] in " \t\r\n":
                        k += 1
                    dead.update(range(a, k))
                else:
                    k = a
                    while k > 0 and s[k - 1] in " \t\r\n":
                        k -= 1
                    dead.update(range(k - 1 if k > 0 and s[k - 1] == "," else a, b))
            runs = []                              # [start, end) of each stretch of marked characters
            for x in sorted(dead):
                if runs and x == runs[-1][1]:
                    runs[-1][1] = x + 1
                else:
                    runs.append([x, x + 1])
            for a, b in runs:
                if b < len(s) and s[b] == "}":
                    k = a
                    while k > 0 and s[k - 1] in " \t\r\n":
                        k -= 1
                    if k > 0 and s[k - 1] == ",":
                        a = k - 1
                edits.append((a, b, ""))

        def settle(entries):
            """the leaves of one group: duplicates of a module that collapsed onto
            the same name are removed; at the root of a service crate, in a
            `pub use`, `types` is re-exported beside the first old module, since
            the types that module held live there now"""
            seen, has_types = set(), any(t and t.split("::")[-1] == "types" for _, _, _, t in entries)
            dropped = []
            for nd, a, b, t in entries:
                self.bind(nd, binds)
                if t is None:
                    continue
                module_gone = self.is_module_leaf(nd) and not nd.alias and not nd.dual and nd.own != ["self"]
                if module_gone and t in seen:
                    dropped.append((nd, a, b, t))
                    continue
                seen.add(t)
                if module_gone and root_pub and not nd.via_root and not has_types and t.endswith("hotpath"):
                    edits.append((a, b, t[:-len("hotpath")] + "types, hotpath"))
                    has_types = True
                else:
                    edits.append((a, b, t))
            erase(dropped)

        def emit(nd, pnew, top):
            """the node's own path is rendered against `pnew`, the new path of
            its parent group ([] at the top)"""
            if nd.kids is None:
                self.bind(nd, binds)
            if nd.ntail is None:                   # nothing from here on changes
                for k in nd.kids or []:
                    emit(k, nd.full, False)
                return
            own_new = nd.nfull[len(pnew):]
            if top:
                own_new = rel_head(nd, own_new)
            a, b = nd.span
            if nd.kids is None:
                e = leaf(nd, own_new)
                if top and root_pub and self.is_module_leaf(nd) and not nd.alias and not nd.dual and not nd.via_root:
                    lead = "::".join(own_new[:-1])
                    e = (e[0], e[1], lead + "::{types, hotpath}")   # a single old module, re-exported alone
                if e[2] is not None:
                    edits.append(e)
            else:
                if not own_new:                    # the group moves up: its path and one `::` go
                    edits.append((a, b + 2, ""))
                else:
                    new = "::".join(own_new)
                    if new != "::".join(norm(x) for x in nd.own):
                        edits.append((a, b, new))
                entries = []
                for k in nd.kids:
                    if k.ntail is None:
                        emit(k, nd.full, False)
                    elif k.kids is None:
                        entries.append((k,) + leaf(k, k.nfull[len(nd.nfull):]))
                    else:
                        emit(k, nd.nfull, False)
                settle(entries)

        emit(node, [], True)
        return edits, binds, globs, [r + "   (a module re-exported under its old name inside a module: rewrite it and the paths through it by hand)" for r in reexported]

    def bind(self, nd, binds):
        """what a leaf makes visible under its name in the file that holds it"""
        if nd.kids is not None or nd.full is None:
            return
        if nd.full == ["crate", self.sub]:
            binds[nd.alias or self.sub] = (nd.alias or self.sub, [])    # the subsystem's module itself
        elif self.is_module_leaf(nd) and not nd.dual:
            # An old module imported by name: every `name::x` in this file follows it.
            # (A dual import needs nothing: the name still denotes the module.)
            if nd.alias:
                binds[nd.alias] = (nd.alias, nd.full[2:])
            elif nd.own != ["self"]:
                binds[norm(nd.own[-1])] = ("hotpath", nd.full[2:])
            elif norm(nd.full[-1]) not in self.root_items:
                binds[norm(nd.full[-1])] = ("hotpath", nd.full[2:])

    def lift(self, nd):
        """a group every child of which reaches the subsystem through another
        crate's root (`use crate::fs::{vfs::a, inode::b}`) is written from the
        core's name as a whole (`use rux::fs::{a, b}`)"""
        for k in nd.kids or []:
            self.lift(k)
        if nd.kids and all(k.via_root for k in nd.kids):
            nd.via_root, nd.base, nd.full = True, ["crate"], ["crate", self.sub]

    def stray(self, nd, parent_ok):
        """a name reached through a crate's root inside a group that is not
        reached that way as a whole: the statement cannot be edited in place"""
        if nd.via_root and not parent_ok:
            return True
        return any(self.stray(k, nd.via_root) for k in nd.kids or [])

    def leaves(self, node):
        if node.kids is None:
            yield node
        else:
            for k in node.kids:
                yield from self.leaves(k)

    def parse_item(self, s, i, prefix, cur, is_core, root_binds=None, core_head=False, outright=False):
        """one item of a use tree at s[i:]: a path, then a group or an alias.
        `core_head`: the group the item is in starts at the core's name.
        `outright`: the statement starts with `::`, which names a crate whatever
        else is in scope"""
        root_binds = root_binds or {}
        nd = Node()
        start = i
        own = []
        m = SEG_RE.match(s, i)
        own.append(m.group())
        i = m.end()
        group = False
        while s.startswith("::", i):
            if s.startswith("{", i + 2):
                group = True
                i += 3
                break
            i += 2
            if s.startswith("*", i):
                own.append("*")
                i += 1
                break
            m = SEG_RE.match(s, i)
            own.append(m.group())
            i = m.end()
        nd.own, nd.span = own, (start, i - (3 if group else 0))   # the span stops before `::{`
        nd.base, nd.full, nd.via_root = self.absolute(prefix, own, cur, is_core, root_binds, core_head, outright)
        if group:
            nd.kids = []
            while True:
                while i < len(s) and s[i] in " \t\r\n":
                    i += 1
                if i >= len(s) or s[i] == "}":
                    i += 1
                    break
                kid, i = self.parse_item(s, i, nd.full, cur, is_core, root_binds, nd.own[0] == "rux")
                nd.kids.append(kid)
                while i < len(s) and s[i] in " \t\r\n":
                    i += 1
                if i < len(s) and s[i] == ",":
                    i += 1
        else:
            ma = AS_ALIAS.match(s, i)
            if ma:
                nd.alias = ma.group(1)
                nd.alias_end = ma.end()
                i = ma.end()
        return nd, i

    def absolute(self, prefix, own, cur, is_core, root_binds, core_head=False, outright=False):
        """the absolute path of an item: (the module a relative head starts from,
        the full path, reached through a crate's root). (None, None, False) for
        a path that does not start at a root — a name that is in scope, which
        this tool cannot follow. In a service crate `crate::` is the crate
        itself; it reaches the core's subsystem only through a name the crate's
        root makes visible from it — re-exported by name, brought in by a glob,
        or reached through the root of another crate the root re-exports. The
        same for `self::`/`super::` heads that start at the crate's root, and
        for another service crate named outright (`::process::task::x`)."""
        o = [norm(x) for x in own]
        if o[0] == "rux" or (o[0] == "crate" and is_core):
            return ["crate"], ["crate"] + o[1:], False
        if core_head:
            return ["crate"], (list(prefix) if o == ["self"] else list(prefix) + o), False
        if o[0] == "crate" or prefix:
            base, full = ["crate"], (["crate"] + o[1:]) if o[0] == "crate" else (
                list(prefix) if o == ["self"] else list(prefix) + o)
        elif o[0] == "self":
            base, full = list(cur), list(cur) + o[1:]
        elif o[0] == "super":
            k = 1
            while k < len(o) and o[k] == "super":
                k += 1
            if k >= len(cur):
                return None, None, False
            base, full = cur[:len(cur) - k], cur[:len(cur) - k] + o[k:]
        else:
            # a service crate named outright (`use ::process::task::x;`)
            tail = self.crate_name_path(o, outright) if not is_core and not prefix else None
            if tail is not None and tail[0] in self.mods_top:
                return ["crate"], ["crate", self.sub] + tail, True
            return None, None, False
        if not is_core:
            # a group that already reached the subsystem: its items are under it
            if prefix is not None and len(prefix) >= 2 and prefix[1] == self.sub:
                return list(prefix), full, False
            if len(full) >= 2 and full[0] == "crate":
                tail = self.root_tail(full[1:], root_binds)
                if tail is not None and tail[0] in self.mods_top:
                    return ["crate"], ["crate", self.sub] + tail, True
            return None, None, False         # the crate's own path: nothing of the subsystem in it
        return base, full, False

    def process(self, node):
        """Bottom-up. Sets `ntail` (the item's path after the subsystem
        segment, in its new form; None when nothing from here on changes),
        `nfull` (the new absolute path) and `dual` (a leaf that named a module
        while an item of the same name was reachable from the subsystem's
        root: the import now brings both)."""
        if node.full is None:
            node.ntail = None
        else:
            r = self.root_index(node.full)
            old_tail = node.full[r + 1:] if r >= 0 else None
            if node.kids is None:
                node.ntail = self.map_tail(old_tail, in_use=True) if r >= 0 else None
                node.dual = (node.ntail == ["hotpath"] and norm(node.own[-1]) != "self"
                             and node.full[-1] in self.modules and node.full[-1] in self.root_items)
            elif old_tail and self.map_tail(old_tail) is None:
                node.ntail = None                # a module that stays: nothing under it moves
            else:
                res = [self.process(k) for k in node.kids]
                if old_tail and any(x is not None for x in res):
                    node.ntail = lcp([x for x in res if x is not None])   # the children decide where the group goes
                    # the group's path takes the module part only; an item
                    # keeps its own name below the brace
                    caps = [len(x) - 1 for k, x in zip(node.kids, res)
                            if x is not None and [norm(o) for o in k.own] != ["self"]]
                    if caps:
                        node.ntail = node.ntail[:max(0, min(caps))]
                else:
                    node.ntail = old_tail        # the group's own path does not change
        node.nfull = node.full
        if node.ntail is not None and node.full is not None:
            r = self.root_index(node.full)
            node.nfull = node.full[:r + 1] + node.ntail
        return node.ntail


# ------------------------------------------------------------------ the tree

def git(root, *a):
    return subprocess.run(["git", "-C", root, *a], capture_output=True, text=True, errors="replace").stdout


ITEM_DEF = re.compile(r"^(?:pub(?:\([a-z]+\))? )?(?:struct|enum|union|type|const|static|fn|trait) +((?:r#)?[A-Za-z_][A-Za-z0-9_]*)", re.M)
MOD_PUB = re.compile(r"^pub(?:\([a-z]+\))? mod +((?:r#)?[A-Za-z_][A-Za-z0-9_]*)\s*;", re.M)
EXPORT_DEF = re.compile(r"^pub(?:\(crate\))? (?:struct|enum|union|type|const|static|fn|trait|mod) +([A-Za-z_]\w*)", re.M)
EXPORT_TYPE = re.compile(r"^\s*pub(?:\(crate\))? (?:struct|enum|union|type|trait)\s+([A-Za-z_]\w*)", re.M)
EXPORT_USE = re.compile(r"^pub(?:\(crate\))? use ([^;]+);", re.S)
EXPORT_USE_START = re.compile(r"\s*pub(?:\(crate\))? use\b")
MOD_DECL = re.compile(r"^\s*(?:pub(?:\([^)]*\))? )?mod +([A-Za-z_][A-Za-z0-9_]*)\s*;", re.M)
REEXPORT = re.compile(r"^\s*pub(?:\([a-z]+\))? use ([A-Za-z_][A-Za-z0-9_]*)::([^;]+);", re.M | re.S)
TOPIC_FILES = ("mod.rs", "unsafe.rs", "types.rs", "hotpath.rs")


def mod_decl_order(mod_text):
    """the `mod x;` declarations of a mod.rs, in order (inline modules excluded)"""
    return MOD_DECL.findall(mask(mod_text, scan(mod_text)))


def imported(part):
    """the name a `use` item binds: its alias, else the last segment of its path"""
    part = part.strip()
    return part.split(" as ")[-1].strip() if " as " in part else part.split("::")[-1].strip()


def root_items_of(root, sub, mod_text, read=None):
    """the names reachable at the subsystem's root BEFORE the merge: the items
    mod.rs declares itself and what its `pub use <topic>::..` lines re-export"""
    code = mask(mod_text, scan(mod_text))
    names = set(norm(x) for x in ITEM_DEF.findall(code)) - set(FINAL)
    for topic, body in REEXPORT.findall(code):
        if topic in ("super", "crate", "self", "rux"):
            continue
        body = body.strip()
        if body == "*":
            t = read(os.path.join("kernel/core/src", sub, topic + ".rs")) if read else ""
            if t:
                t = mask(t, scan(t))
                # a module the topic declares is reached as `root::topic::x`, not as a root item
                names |= set(module_exports_of(t, topic, set(), {})) - {norm(x) for x in MOD_PUB.findall(t)}
        elif "{" in body:
            for part in body[body.index("{") + 1:body.rindex("}")].split(","):
                if part.strip() and part.strip() not in ("self", "*"):
                    names.add(imported(part))
        elif body not in ("self", "*"):
            names.add(imported(body))
    return names


BLOCK = re.compile(r"^\s*(?:pub(?:\([^)]*\))? (mod)\b|([A-Za-z_][A-Za-z0-9_]*)!\b)")


def module_exports_of(code, module, mods, done):
    """the names `crate::<sub>::<module>::<name>` reached before the merge, read
    from the module's own text: its public definitions at module level (one
    written inside a macro invocation such as bitflags! { .. } is among them —
    a method inside an `impl` block is not) and the names its `pub use` lines
    re-export (`pub use child::*;` takes the child's list, `done`)."""
    names, stack = [], []                    # stack: the kind of each block that is open
    lines = code.split("\n")
    n = 0
    while n < len(lines):
        line = lines[n]
        in_mod = "mod" in stack
        in_macro = bool(stack) and stack[-1] == "macro" and not in_mod
        m = re.match(r"^\s*pub(?:\(crate\))? (?:struct|enum|union|type|const|static|fn|trait|mod)\s+([A-Za-z_]\w*)", line)
        if m and (not stack or in_macro):
            names.append(m.group(1))
        elif not stack and EXPORT_USE_START.match(line):
            # a re-export at module level; it may run over several lines
            stmt = line
            while ";" not in stmt and n < len(lines) - 1:
                n += 1
                stmt += "\n" + lines[n]
            u = EXPORT_USE.match(stmt.strip())
            if u:
                body = u.group(1).strip()
                if body.endswith("::*"):
                    names += done.get(module + "::" + body[:-3], [])
                else:
                    inner = body[body.index("{") + 1:body.rindex("}")] if "{" in body else body
                    for part in inner.split(","):
                        if part.strip() and part.strip() not in ("self", "*"):
                            names.append(imported(part))
            opens = stmt.count("{") - stmt.count("}")
        else:
            opens = line.count("{") - line.count("}")
        if opens > 0:
            head = BLOCK.match(line)
            stack += ["mod" if head and head.group(1) else ("macro" if head and head.group(2) else "other")] * opens
        elif opens < 0:
            stack = stack[:len(stack) + opens] if len(stack) >= -opens else []
        n += 1
    return list(dict.fromkeys(names))


def module_file(sub, m):
    """the source file of an old module: `<m>.rs`, or `<m>/mod.rs` for a directory"""
    return os.path.join("kernel/core/src", sub, *m.split("::"))


def spec_modules(root, sub, read):
    """the modules merged away, in declaration order, a directory's own modules
    included (drivers/virtio/ declares its children in virtio/mod.rs)"""
    out = []

    def walk(prefix):
        for m in mod_decl_order(read(os.path.join("kernel/core/src", sub, *prefix, "mod.rs"))):
            if norm(m) in ("unsafe", "types", "hotpath"):
                continue
            out.append("::".join(prefix + [m]))
            if read(os.path.join("kernel/core/src", sub, *prefix, m, "mod.rs")):
                walk(prefix + [m])

    walk([])
    return out


def inline_modules(root, sub, mods, read):
    """(module, name) of every inline module written at the top level of a
    topic file. Its items are reached as `<module>::<name>::<item>`, so when
    they move to types.rs the inline module has to be in the spec's
    `modules` too (`<module>::<name>`), or a path to them is mapped wrong."""
    out = []
    for m in mods:
        text = read(os.path.join("kernel/core/src", sub, *m.split("::")) + ".rs")
        for name in INLINE.findall(mask(text, scan(text))):
            out.append((m, name))
    return out


INLINE = re.compile(r"^(?:pub(?:\([^)]*\))? )?mod +([A-Za-z_][A-Za-z0-9_]*) *\{", re.M)


def spec_path(root, sub):
    return os.path.join(root, "scripts", "refactor", "pass2", sub + ".json")


def load_spec(root, sub, spec=None):
    """the spec file (`--spec`, or scripts/refactor/pass2/<sub>.json) as a
    dict. Before the file exists (step 1) the modules and the root items are
    read off the directory as it is before the merge."""
    p = spec or spec_path(root, sub)
    if os.path.exists(p):
        return json.load(open(p))
    d = os.path.join(root, "kernel", "core", "src", sub)
    mods = sorted(f[:-3] for f in os.listdir(d) if f.endswith(".rs") and f not in TOPIC_FILES)
    mod = os.path.join(d, "mod.rs")
    roots = root_items_of(root, sub, open(mod, errors="replace").read()) if os.path.exists(mod) else []
    return {"modules": mods, "renames": {}, "keep": [], "root_items": sorted(roots), "inline": [], "exports": {}}


def types_of(root, sub):
    """the names defined in the subsystem's types.rs: types at any indentation
    (a definition inside bitflags! { .. } is one), constants at module level"""
    p = os.path.join(root, "kernel", "core", "src", sub, "types.rs")
    if not os.path.exists(p):
        return []
    text = open(p, errors="replace").read()
    code = mask(text, scan(text))
    return TYPE_DEF.findall(code) + ROOT_CONST.findall(code)


def build_rewriter(root, sub, spec=None):
    sp = load_spec(root, sub, spec)
    rw = Rewriter(sub, sp["modules"], types_of(root, sub), sp.get("renames", {}), sp.get("keep", []),
                  sp.get("root_items", []), sp.get("inline", []), sp.get("exports", {}))
    kept = set(sp.get("keep", [])) | set(sp.get("inline", []))
    rw.mods_top = {m.split("::")[0] for m in sp["modules"]} - kept
    rw.has_unsafe = os.path.exists(os.path.join(root, "kernel", "core", "src", sub, "unsafe.rs"))
    return rw


def each_file(root):
    for dp, dns, fns in os.walk(os.path.join(root, "kernel")):
        dns[:] = [d for d in dns if d not in ("target", "run", ".git")]
        for f in sorted(fns):
            if f.endswith(".rs") and f != GENERATED:
                yield os.path.join(dp, f)


def file_role(root, path):
    """(is_core, module path in its crate)"""
    parts = os.path.relpath(path, os.path.join(root, "kernel")).split(os.sep)
    if parts[-1] in ("mod.rs", "lib.rs", "main.rs"):
        parts = parts[:-1]
    else:
        parts = parts[:-1] + [norm(parts[-1][:-3])]
    if parts[0] == "core" and parts[1] == "src":
        return True, ["crate"] + parts[2:]
    return False, ["crate"] + parts[2:]


class CrateRoot:
    """what the root file of a service crate makes visible, by name: an item it
    declares or imports explicitly, the core's subsystem (the whole of it, one of
    its modules, or — through a glob — its whole root), or another crate it
    re-exports (`pub use ::fs;`). Read from the file as it is on disk."""

    def __init__(self, root, rw, name):
        self.name, self.expl = name, {}       # name -> ("own",) | ("core", tail) | ("crate", other)
        self.from_sub = False                 # the root imports something from the core's subsystem
        self.glob_sub = False                 # … by a glob over it
        self.final_sub = False                # … a module of it that the merge leaves (types, hotpath, unsafe.rs)
        for fname in ("lib.rs", "main.rs"):
            p = os.path.join(root, "kernel", name, "src", fname)
            if not os.path.exists(p):
                continue
            text = open(p, errors="replace").read()
            code = mask(text, scan(text))
            for n in ITEM_DECL.findall(code):     # items the root declares
                self.expl.setdefault(norm(n), ("own",))
            for m in USE_RE.finditer(code):
                stmt = m.group(0)
                if code[m.start()] in " \t":      # indented: inside an inline module or a function
                    continue
                i = USE_HEAD.match(stmt).end()
                while i < len(stmt) and stmt[i] in " \t\r\n":
                    i += 1
                while stmt.startswith("::", i):
                    i += 2
                try:
                    node, _ = rw.parse_item(stmt, i, [], [], False, {})
                except (AttributeError, IndexError):
                    continue
                for nd in rw.leaves(node):
                    bound = nd.alias or norm(nd.own[-1])
                    other = nd.own[0] if nd.full is None and nd.own[-1] in (nd.own[0], "self") and "*" not in nd.own else None
                    if other and os.path.isdir(os.path.join(root, "kernel", other, "src")):
                        self.expl[bound] = ("crate", other)     # another crate, re-exported under its own name
                    elif nd.full is not None and nd.full[:2] == ["crate", rw.sub]:
                        self.from_sub = True
                        if "*" in nd.own:                       # a glob's names are implicit: see lookup
                            self.glob_sub = True
                            self.expl.setdefault(bound, ("own",))
                        else:
                            self.expl[bound] = ("core", list(nd.full[2:]))
                            if len(nd.full) > 2 and nd.full[2] in FINAL:
                                self.final_sub = True
                    else:
                        self.expl[bound] = ("own",)

    def lookup(self, name, mods_top):
        """what `name` at this root denotes, for a file of ANOTHER crate that
        reaches it as `crate::<this crate>::name`. A name the root does not
        declare or import explicitly was there before the merge only through
        the root's glob over the subsystem, so it is the subsystem's old module
        of that name."""
        if name in self.expl:
            return self.expl[name]
        # a name the root does not declare or import explicitly was there before
        # the merge only through a glob over the subsystem — or, once such a
        # re-export is rewritten, it is an old module of a root that now
        # re-exports types and hotpath in its place
        if (self.glob_sub or self.final_sub) and name in mods_top:
            return ("core", [name])
        return None


def own_view(cr, rw):
    """for the files of the crate itself: does its root re-export the subsystem,
    and {an old module name the root makes visible by an explicit re-export:
    its old module path}"""
    alias = cr.expl.get(rw.sub) == ("core", [])
    binds = {n: list(d[1]) for n, d in cr.expl.items()
             if d[0] == "core" and d[1] and "::".join(d[1]) in rw.modules}
    return alias, binds


def glob_binds(cr, rw):
    """the old module names a glob over the subsystem brought into the crate's
    root: those the root neither declares nor imports under that name"""
    if not cr.glob_sub:
        return {}
    return {m: [m] for m in sorted(rw.mods_top | set(rw.inline)) if m not in cr.expl}


def rewrite_tree(root, rw, sub, dry, verbose=True):
    rw.roots = {}
    for d in sorted(os.listdir(os.path.join(root, "kernel"))):
        if d != "core" and os.path.isdir(os.path.join(root, "kernel", d, "src")):
            rw.roots[d] = CrateRoot(root, rw, d)
    total, files, strings, globs, unread, notes = 0, 0, [], [], [], []
    for path in each_file(root):
        try:
            text = open(path, errors="replace").read()
        except OSError:
            continue
        is_core, modpath = file_role(root, path)
        parts = os.path.relpath(path, os.path.join(root, "kernel")).split(os.sep)
        cr = rw.roots.get(parts[0]) if not is_core else None
        rw.crate = cr
        if is_core or cr is None:
            args, extra, reaches = (text, modpath, is_core), [], False
        else:
            alias, binds = own_view(cr, rw)
            for n, t in glob_binds(cr, rw).items():
                binds.setdefault(n, t)
            args, extra = (text, modpath, False, alias, binds, parts[-1] in ("lib.rs", "main.rs")), list(binds)
            reaches = any(d[0] == "crate" for d in cr.expl.values())
        if (sub + "::") not in text and not any(n + "::" in text for n in extra) and not reaches:
            continue
        new, st = rw.rewrite_text(*args)
        rel = os.path.relpath(path, root)
        if st["replacements"]:
            files += 1
            total += st["replacements"]
            if verbose:
                print("%5d  %s" % (st["replacements"], rel))
            if not dry:
                open(path, "w").write(new)
        strings += [(rel, x) for x in st["in_string"]]
        globs += [(rel, g) for g in st["globs"]]
        unread += [(rel, u) for u in st["unread"]]
        notes += [(rel, x) for x in st["notes"]]
    print("%s %d files, %d replacements" % ("would change" if dry else "changed", files, total))
    for title, items in (("references inside string literals, not rewritten — read each one", strings),
                         ("globs left as they are (over hotpath: they now span the whole file; read each one)", globs),
                         ("use statements the parser could not read or edit in place (rewrite these by hand)", unread),
                         ("duplicate imports of hotpath the tool removed (read them)", notes)):
        if items:
            print("%s: %d" % (title, len(items)))
            for rel, x in items[:40]:
                print("   %s: %s" % (rel, x))


# ------------------------------------------------------------------ commands

def cmd_rewrite(args):
    root = os.path.abspath(args.root)
    rewrite_tree(root, build_rewriter(root, args.sub, args.spec), args.sub, args.dry)
    return 0


def cmd_check(args):
    root = os.path.abspath(args.root)
    d = os.path.join(root, "kernel", "core", "src", args.sub)
    print("directory kernel/core/src/%s:" % args.sub)
    for f in sorted(os.listdir(d)):
        if f.endswith(".rs"):
            text = open(os.path.join(d, f), errors="replace").read()
            print("   %-12s %6d lines, %4d `unsafe`" % (f, text.count("\n"), len(re.findall(r"\bunsafe\b", mask(text, scan(text))))))
    print("\nrewrite, dry run (a finished lane changes nothing):")
    rewrite_tree(root, build_rewriter(root, args.sub, args.spec), args.sub, True)
    return 0


def funcs(elf, nm):
    """mangled name -> (address, size) of every function"""
    if not nm:
        with open(elf, "rb") as fh:
            machine = int.from_bytes(fh.read(20)[18:20], "little")
        triple = {62: "x86_64-linux-gnu", 243: "riscv64-linux-gnu", 183: "aarch64-linux-gnu"}.get(machine, "")
        nm = (triple + "-nm") if triple and shutil.which(triple + "-nm") else "nm"
    out = subprocess.run([nm, "-S", "--defined-only", elf], capture_output=True, text=True).stdout
    d = {}
    for line in out.splitlines():
        f = line.split()
        if len(f) == 4 and f[2] in "tTwW" and f[3].startswith("_R"):
            d[f[3]] = (int(f[0], 16), int(f[1], 16))
    return d


def demangle(names, syms, elf):
    """Rust v0 names demangled: c++filt for the lot, gdb for the rest."""
    if not names:
        return []
    r = subprocess.run(["c++filt"], input="\n".join(names), capture_output=True, text=True)
    out = r.stdout.split("\n")[:len(names)]
    left = [k for k, n in zip(names, out) if not n or n == k]
    if left and len(left) > len(names) // 4:     # c++filt does not read this format: ask gdb
        with tempfile.NamedTemporaryFile("w", suffix=".gdb", delete=False) as fh:
            script = fh.name
            fh.write("set pagination off\nset confirm off\n")
            for k in left:
                fh.write("info symbol 0x%x\n" % syms[k][0])
        try:
            r = subprocess.run(["gdb", "-batch", "-nx", "-x", script, elf], capture_output=True, text=True)
        finally:
            os.unlink(script)
        got = iter(line.rsplit(" in section ", 1)[0].strip()
                   for line in r.stdout.splitlines() if " in section " in line)
        j = 0
        for i in range(len(out)):
            if j < len(left) and names[i] == left[j]:
                out[i] = next(got, out[i])
                j += 1
    return out


def cmd_pair(args):
    """`--sub` takes one subsystem or several (`mm,sched,fs`): on a tree where
    more than one has been merged, every one of them is normalized, so the
    pairing against a build from before pass 2 still pairs 1:1."""
    root = os.path.abspath(args.root)
    subs = args.sub.split(",")
    strips, ren = [], []
    for sub in subs:
        sp = load_spec(root, sub, args.spec if len(subs) == 1 else None)
        modules, renames, inline = sp["modules"], sp.get("renames", {}), sp.get("inline", [])
        alt = "|".join(sorted((re.escape(m) for m in list(modules) + list(inline) + ["types", "hotpath"]), key=len, reverse=True))
        strips.append(re.compile(r"rux(?:\[[0-9a-f]+\])?::" + re.escape(sub) + r"(?:::(?:" + alt + r")(?![A-Za-z0-9_]))+"))
        ren += [(sub, [q for q in key.split("::") if q], new) for key, new in renames.items()]

    def normalize(name, renamed):
        # c++filt prints the crate's disambiguator on some names; it is the
        # same crate on both sides
        name = re.sub(r"rux\[[0-9a-f]+\]::", "rux::", name)
        if renamed:                              # the base carries the old names
            for sub, parts, new in ren:
                name = re.sub(r"rux::" + re.escape(sub) + "::" + re.escape("::".join(parts)) + r"(?![A-Za-z0-9_])",
                              "rux::" + sub + "::" + "::".join(parts[:-1] + [new]), name)
        for sub, rx in zip(subs, strips):
            name = rx.sub("rux::" + sub, name)
        return name

    b, c = funcs(args.base, args.nm), funcs(args.cur, args.nm)
    if not b or not c:
        print("no function symbols read from %s" % (args.base if not b else args.cur))
        return 2
    same = sum(1 for k in b if k in c and c[k][1] == b[k][1])
    size_diff = sum(1 for k in b if k in c and c[k][1] != b[k][1])
    only_b, only_c = sorted(set(b) - set(c)), sorted(set(c) - set(b))
    print("functions: base %d, current %d; identical name and size %d; same name, size differs %d; "
          "name differs: base %d, current %d" % (len(b), len(c), same, size_diff, len(only_b), len(only_c)))
    db, dc = collections.defaultdict(list), collections.defaultdict(list)
    for k, n in zip(only_b, demangle(only_b, b, args.base)):
        db[normalize(n, True)].append(b[k][1])
    for k, n in zip(only_c, demangle(only_c, c, args.cur)):
        dc[normalize(n, False)].append(c[k][1])
    bad = [(k, db.get(k, []), dc.get(k, [])) for k in sorted(set(db) | set(dc)) if sorted(db.get(k, [])) != sorted(dc.get(k, []))]
    missing = [(k, x) for k, x, y in bad if x and not y]
    extra = [(k, y) for k, x, y in bad if y and not x]
    print("name-differing symbols paired 1:1 with equal size, module path normalized: %d; unpaired: %d"
          % (len(only_b) - sum(len(x[1]) for x in bad), len(bad)))
    if missing or extra or any(x and y for _, x, y in bad):
        print("   of the base build and missing from the current one: %d" % len(missing))
        print("   of the current build only (extra bodies — show that nothing references them): %d" % len(extra))
    for k, x, y in bad[:args.show]:
        print("UNPAIRED %s\n     base sizes %s\n     current sizes %s" % (k[:160], x[:8], y[:8]))
    return 1 if bad or size_diff else 0


def lines_of(text):
    """(line, inside a `use` statement) for every non-blank line that is not a comment"""
    kind = scan(text)
    code = mask(text, kind)
    in_use = bytearray(len(text))
    for m in USE_RE.finditer(code):
        in_use[m.start():m.end()] = b"\x01" * (m.end() - m.start())
    pos = 0
    for line in code.split("\n"):
        s_ = line.strip()
        if s_ and not s_.startswith("//"):
            yield s_, bool(in_use[pos])
        pos += len(line) + 1


def cmd_verbatim(args):
    """Every code line of the subsystem's files at --base against the files at
    --cur (the working tree by default). Lines present on both sides cancel.
    A base line and a current line that differ only in a path (module
    segments, the two targets, a relative head) pair up, then lines that
    differ only by a rename the spec records. What is left is sorted by kind;
    a line of no known kind is listed in run/pass2-verbatim-<sub>.txt and
    has to be explained by hand."""
    root = os.path.abspath(args.root)
    sub = args.sub
    sp = load_spec(root, sub, args.spec)
    rw = build_rewriter(root, sub, args.spec)
    d = "kernel/core/src/" + sub
    base_files = [p for p in git(root, "ls-tree", "-r", "--name-only", args.base, "--", d).split() if p.endswith(".rs")]
    if args.cur:
        cur_files = [p for p in git(root, "ls-tree", "-r", "--name-only", args.cur, "--", d).split() if p.endswith(".rs")]
        read_cur = lambda p: git(root, "show", args.cur + ":" + p)
    else:
        cur_files = [os.path.relpath(os.path.join(dp, f), root)
                     for dp, _, fns in os.walk(os.path.join(root, d)) for f in fns if f.endswith(".rs")]
        read_cur = lambda p: open(os.path.join(root, p), errors="replace").read()
    cb, cc = collections.Counter(), collections.Counter()
    for p in base_files:
        cb.update(lines_of(git(root, "show", args.base + ":" + p)))
    for p in cur_files:
        cc.update(lines_of(read_cur(p)))
    stats = collections.OrderedDict()
    stats["identical"] = sum(min(n, cc[item]) for item, n in cb.items())
    rest_base, rest_cur = cb - cc, cc - cb

    mods = sorted(list(rw.modules) + sorted(rw.inline), key=len, reverse=True)
    # the same line in its old and its new form differs only in these
    segs = "|".join(re.escape(m) for m in mods + ["types", "hotpath"])
    canon_rx = re.compile(r"(?:crate|rux)::" + re.escape(sub) + r"::(?:(?:" + segs + r")::)*"
                          r"|(?:self|super)::(?:(?:" + segs + r")::)?"
                          r"|(?<=[\s{,(])(?:" + segs + r")::"
                          r"|(?<=[\s{,(])(?:self|types|hotpath)(?=[,\s})])")
    ren = [(re.compile(r"(?<![A-Za-z0-9_])" + re.escape(k.split("::")[-1]) + r"(?![A-Za-z0-9_])"), v)
           for k, v in sp.get("renames", {}).items()]

    def by_path(item):
        return re.sub(r"\s+", "", canon_rx.sub("", item[0])), item[1]

    def by_rename(item):
        """a recorded rename applied first, then the path rule"""
        t = item[0]
        for rx, v in ren:
            t = rx.sub(v, t)
        return by_path((t, item[1]))

    def pair(key):
        """move every base line that has a current line of the same key out of both sets"""
        groups = collections.defaultdict(list)
        for item, n in rest_cur.items():
            groups[key(item)] += [item] * n
        left_base, matched = collections.Counter(), 0
        for item, n in rest_base.items():
            g = groups.get(key(item), [])
            k = min(n, len(g))
            matched += k
            del g[:k]
            if n - k:
                left_base[item] = n - k
        left_cur = collections.Counter()
        for items in groups.values():
            left_cur.update(items)
        return left_base, left_cur, matched

    rest_base, rest_cur, n = pair(by_path)
    stats["path only"] = n
    rest_base, rest_cur, n = pair(by_rename)
    stats["recorded rename"] = n

    def kind(item, other, fewer):
        line, in_use = item
        if in_use or re.match(r"^(pub(\([a-z]+\))? )?use\b", line):
            return "use line"
        if line.startswith("#!["):
            return "inner attribute"
        if line.startswith("#["):
            return "attribute line"
        if re.match(r"^(pub(\([a-z]+\))? )?mod [a-z_0-9#]+;$", line):
            return "module declaration"
        if other.get((line, True)) or other.get((line, False)):
            return "repeated line, now " + ("fewer" if fewer else "more")
        return None

    listed = []
    for items, other, fewer, side in ((rest_base, cc, True, "MISSING"), (rest_cur, cb, False, "ADDED")):
        for item, n in items.items():
            k = kind(item, other, fewer)
            for _ in range(n):
                if k:
                    stats[k] = stats.get(k, 0) + 1
                else:
                    listed.append("%s: %s" % (side, item[0]))
    out = os.path.join(root, "run", "pass2-verbatim-%s.txt" % sub)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w") as fh:
        fh.write("\n".join(listed) + ("\n" if listed else ""))
    print("code lines: base %d in %d files, current %d in %d files" % (sum(cb.values()), len(base_files), sum(cc.values()), len(cur_files)))
    for k, v in stats.items():
        print("   %-28s %d" % (k, v))
    print("of no known kind (listed in %s): %d" % (os.path.relpath(out, root), len(listed)))
    for line in listed[:args.show]:
        print("   " + line[:150])
    return 1 if listed else 0


def cmd_spec(args):
    """Write the spec from the directory as it is before the merge: run it
    once step 1 is committed and before the merge. `renames`, `keep` and
    `inline` are yours to edit; running this again keeps them and rewrites
    `modules`, `root_items` and `exports` from the tree. It also lists the
    inline modules of the topic files: add one to `modules` as
    `<module>::<name>` when the definitions inside it moved to types.rs.
    `inline` is only for a directory that stays one inline module of
    hotpath.rs (`--inline`); an inline module written inside a topic file is
    never listed there."""
    root = os.path.abspath(args.root)
    sub = args.sub
    base = "kernel/core/src/" + sub

    def read(rel):
        p = os.path.join(root, rel)
        if os.path.isfile(p):
            return open(p, errors="replace").read()
        return git(root, "show", args.base + ":" + rel) if args.base else ""

    def is_merged():
        return os.path.exists(os.path.join(root, base, "hotpath.rs")) or not mod_decl_order(read(base + "/mod.rs"))

    if is_merged():                              # already merged: read the old tree from git
        if not args.base:
            print("the directory is merged and --base is not given")
            return 2
        read = lambda rel: git(root, "show", args.base + ":" + rel)
    mods_all = spec_modules(root, sub, read)
    inline = [x for x in (args.inline.split(",") if args.inline else []) if x]
    old = {}
    out = spec_path(root, sub)
    if os.path.exists(out):
        old = json.load(open(out))
    if not inline:
        inline = old.get("inline", [])
    mods = [m for m in mods_all if m not in inline and not any(m.startswith(x + "::") for x in inline)]
    exports = {}
    for m in reversed(mods_all):                 # children before parents: a parent's `pub use child::*` needs them
        raw = read(module_file(sub, m) + "/mod.rs") or read(module_file(sub, m) + ".rs")
        exports[m] = module_exports_of(mask(raw, scan(raw)), m, set(mods_all), exports)
    spec = {"modules": old.get("modules") or mods,
            "root_items": sorted(root_items_of(root, sub, read(base + "/mod.rs"), read)),
            "renames": old.get("renames", {}), "keep": old.get("keep", []), "inline": inline,
            "exports": exports}
    os.makedirs(os.path.dirname(out), exist_ok=True)
    json.dump(spec, open(out, "w"), indent=1)
    print("%s: %d modules, %d root items, %d renames, %d kept, %d inline, %d export lists" % (
        os.path.relpath(out, root), len(spec["modules"]), len(spec["root_items"]), len(spec["renames"]),
        len(spec["keep"]), len(spec["inline"]), len(spec["exports"])))
    print("   modules in declaration order: %s" % " ".join(spec["modules"]))
    named = sorted(set(spec["root_items"]) & set(spec["modules"]))
    if named:
        print("   a module and a root item share a name (an import of it brings both): %s" % " ".join(named))
    for m, name in inline_modules(root, sub, mods, read):
        listed = ("%s::%s" % (m, name)) in spec["modules"]
        print("   inline module %s::%s%s" % (m, name, "  (in modules)" if listed else
              "  — if its definitions moved to types.rs, add `%s::%s` to the `modules` list of the spec; "
              "`inline` is for a directory kept whole under hotpath.rs, not for this" % (m, name)))
    return 0


# ------------------------------------------------------------------ self test

def cmd_selftest(args):
    rw = Rewriter("mm", ["vma", "page", "page_desc", "zone", "page_alloc"],
                  {"Vma", "VmaFlags", "Zone", "PAGE_SIZE", "GfpFlags", "VirtAddr"},
                  renames={"page::frame_to_page": "page_frame_to_page"}, root_items={"page_desc", "zone"})
    core = (["crate", "fs", "vfs"], True)          # a file of another subsystem of the core
    inside = (["crate", "mm", "types"], True)      # a file inside the merged subsystem
    crate = (["crate"], False)                     # a service crate
    cases = [
        ("let x = crate::mm::vma::Vma::new();", "let x = crate::mm::types::Vma::new();", core),
        ("let p = crate::mm::page_desc::pfn_to_page(pfn);", "let p = crate::mm::hotpath::pfn_to_page(pfn);", core),
        ("let z = crate::mm::PAGE_SIZE;", "let z = crate::mm::types::PAGE_SIZE;", core),
        ("let a = crate::mm::page::frame_to_page(f);", "let a = crate::mm::hotpath::page_frame_to_page(f);", core),
        ("let b = crate::mm::page_desc::frame_to_page(f);", "let b = crate::mm::hotpath::frame_to_page(f);", core),
        ("let c = rux::mm::zone::Zone::new();", "let c = rux::mm::types::Zone::new();", crate),
        ("let d = $crate::mm::page_desc::x();", "let d = $crate::mm::hotpath::x();", core),
        ("use super::vma::Vma;\nlet e = super::page::PAGE_SIZE;", "use super::types::Vma;\nlet e = super::types::PAGE_SIZE;", inside),
        ("use super::page_desc::pfn_to_page;\nlet f = super::page::frame_to_page(x);",
         "use super::hotpath::pfn_to_page;\nlet f = super::hotpath::page_frame_to_page(x);", inside),
        ("use super::mm::page_desc::pfn_to_page;", "use super::mm::hotpath::pfn_to_page;", (["crate", "printk"], True)),
        ("use crate::mm::{self, page::PAGE_SIZE, vma::{Vma, VmaFlags}, zone::Zone};",
         "use crate::mm::{self, types::PAGE_SIZE, types::{Vma, VmaFlags}, types::Zone};", core),
        ("use crate::mm::{\n    page::PAGE_SIZE,\n    zone::Zone,\n    page_desc::pfn_to_page,\n};",
         "use crate::mm::{\n    types::PAGE_SIZE,\n    types::Zone,\n    hotpath::pfn_to_page,\n};", core),
        ("use crate::mm::vma::{Vma, VmaFlags};", "use crate::mm::types::{Vma, VmaFlags};", core),
        ("use crate::mm::vma::{self, Vma};", "use crate::mm::{hotpath, types::Vma};", core),
        ("use crate::mm::vma::{self};", "use crate::mm::hotpath::{self};", core),
        ("pub(crate) use crate::mm::zone::Zone as Z;", "pub(crate) use crate::mm::types::Zone as Z;", core),
        ("use crate::mm::page_desc::*;", "use crate::mm::hotpath::*;", core),
        ("use crate::{mm::page_desc::pfn_to_page, sched::sched::pick};",
         "use crate::{mm::hotpath::pfn_to_page, sched::sched::pick};", core),
        ("use crate::{mm::vma::{Vma}, fs::vfs::Path};", "use crate::{mm::types::{Vma}, fs::vfs::Path};", core),
        ("use rux::mm::{page::PAGE_SIZE, vma::Vma};", "use rux::mm::{types::PAGE_SIZE, types::Vma};", crate),
        ("use crate::mm;\nlet g = mm::zone::Zone::new();\nlet h = mm::PAGE_SIZE;", "use crate::mm;\nlet g = mm::types::Zone::new();\nlet h = mm::types::PAGE_SIZE;", core),
        # an item of the same name as a module, reachable from the subsystem's root
        ("let p = crate::mm::page_desc(pfn);", "let p = crate::mm::hotpath::page_desc(pfn);", core),
        ("let z = super::zone();", "let z = super::hotpath::zone();", inside),
        ("use crate::mm;\nlet y = mm::page_desc(pfn);", "use crate::mm;\nlet y = mm::hotpath::page_desc(pfn);", core),
        ("use crate::mm::page_desc;", "use crate::mm::hotpath::{self, page_desc};", core),
        ("use crate::mm::page_desc as pd;", "use crate::mm::hotpath::{self as pd, page_desc as pd};", core),
        ("use crate::mm::{page_desc, vma::Vma};", "use crate::mm::{hotpath::{self, page_desc}, types::Vma};", core),
        ("use crate::mm::page_desc;\nlet r = page_desc::pfn_to_page(1);\nlet s = page_desc(2);",
         "use crate::mm::hotpath::{self, page_desc};\nlet r = page_desc::pfn_to_page(1);\nlet s = page_desc(2);", core),
        # an old module imported by its name: the uses through that name follow it
        ("use crate::mm::vma;\nlet v = vma::Vma::new();\nlet w = vma::vma_len(v);",
         "use crate::mm::hotpath;\nlet v = crate::mm::types::Vma::new();\nlet w = hotpath::vma_len(v);", core),
        ("use crate::mm::page_alloc as pa;\nlet q = pa::alloc_pages(1);", "use crate::mm::hotpath as pa;\nlet q = pa::alloc_pages(1);", core),
        ("use rux::mm::vma;\nlet u = vma::vma_len(v);", "use rux::mm::hotpath;\nlet u = hotpath::vma_len(v);", crate),
        ("use ::rux::mm::vma::Vma;", "use ::rux::mm::types::Vma;", crate),
        ("use crate::mm::vma::{self, VmaFlags};\nlet k = vma::vma_len(v);",
         "use crate::mm::{hotpath, types::VmaFlags};\nlet k = hotpath::vma_len(v);", core),
        ("use crate::mm::vma::{self as vm, VmaFlags};\nlet k = vm::vma_len(v);",
         "use crate::mm::{hotpath as vm, types::VmaFlags};\nlet k = vm::vma_len(v);", core),
        ("use crate::mm::page_desc::{self, pfn_to_page};\nlet k = page_desc::x(1);\nlet l = page_desc(2);",
         "use crate::mm::hotpath::{self as page_desc, page_desc, pfn_to_page};\nlet k = page_desc::x(1);\nlet l = page_desc(2);", core),
        ("use crate::mm::vma::{self};\nlet k = vma::vma_len(v);", "use crate::mm::hotpath::{self};\nlet k = hotpath::vma_len(v);", core),
        # unchanged
        ("let i = crate::mm::types::Vma::new();", None, core),
        ("let j = crate::mm::hotpath::pfn_to_page(0);", None, core),
        ("let k = crate::mm::r#unsafe::f();", None, core),
        ("let l = crate::sched::vma::Vma::new();", None, core),
        ("let m = mm::zone::Zone::new();", None, core),                 # no binding of the module name in this file
        ("let s = \"crate::mm::vma::Vma\";", None, core),               # a string literal
        ("let t = crate::mm::vma::Vma::new(); // crate::mm::page::PAGE_SIZE", "let t = crate::mm::types::Vma::new(); // crate::mm::types::PAGE_SIZE", core),
        ("struct S { crate_like: u8 }\nlet u = self.mm::x;", None, core),
        ("impl X { fn f(&self) { self::g(); } }", None, core),
        ("mod inner {\n    use super::page::y;\n    let v = super::vma::Vma::new();\n}", None, inside),
        ("use crate::mm::local::x;\nlet w = crate::mm::vma::Vma::new();", "use crate::mm::local::x;\nlet w = crate::mm::vma::Vma::new();", crate),
    ]
    bad = 0
    for text, want, role in cases:
        got, st = rw.rewrite_text(text, role[0], role[1])
        if want is not None and got != want:
            bad += 1
            print("FAIL\n   in  : %r\n   got : %r\n   want: %r" % (text, got, want))
    # a directory that stays one inline module: per-architecture modules chosen by #[cfg]
    dr = Rewriter("drivers", ["virtio", "virtio::blk", "rtc"], {"Dev"}, inline={"timer", "intc"})
    for text, want in [
        ("let t = crate::drivers::timer::riscv64::read_time();", "let t = crate::drivers::hotpath::timer::riscv64::read_time();"),
        ("use crate::drivers::timer;", "use crate::drivers::hotpath::timer;"),
        ("use crate::drivers::timer;\nlet t = timer::riscv64::read_time();", "use crate::drivers::hotpath::timer;\nlet t = timer::riscv64::read_time();"),
        ("use crate::drivers::timer::{riscv64, x86_64};", "use crate::drivers::hotpath::timer::{riscv64, x86_64};"),
        ("use crate::drivers::timer::{self, hpet};", "use crate::drivers::hotpath::timer::{self, hpet};"),
        ("use crate::drivers::intc::apic::*;", "use crate::drivers::hotpath::intc::apic::*;"),
        ("let d = crate::drivers::virtio::blk::Dev::new();", "let d = crate::drivers::types::Dev::new();"),
        ("use rux::drivers::intc::gicv3::init();", "use rux::drivers::hotpath::intc::gicv3::init();"),
        ("let t = crate::drivers::hotpath::timer::riscv64::read_time();", "let t = crate::drivers::hotpath::timer::riscv64::read_time();"),
    ]:
        got, st = dr.rewrite_text(text, ["crate", "fs", "vfs"], True)
        if got != want:
            bad += 1
            print("FAIL inline\n   in  : %r\n   got : %r\n   want: %r" % (text, got, want))
    # two old modules imported by name in one file: the second `use …::hotpath;` is a duplicate
    for text, want in [
        ("use crate::mm::vma;\nuse crate::mm::page_alloc;\npub fn f() {\n    vma::vma_len();\n    page_alloc::alloc_pages(1);\n}\n",
         "use crate::mm::hotpath;\npub fn f() {\n    hotpath::vma_len();\n    hotpath::alloc_pages(1);\n}\n"),
        ("pub fn f() {\n    use crate::mm::vma;\n    use crate::mm::page_alloc;\n    vma::vma_len();\n}\n",
         "pub fn f() {\n    use crate::mm::hotpath;\n    use crate::mm::hotpath;\n    hotpath::vma_len();\n}\n"),
    ]:
        got, st = rw.rewrite_text(text, ["crate", "fs", "vfs"], True)
        if got != want:
            bad += 1
            print("FAIL duplicate hotpath import\n   in  : %r\n   got : %r\n   want: %r" % (text, got, want))
    # a crate whose root re-exports old modules by name
    proc = Rewriter("process", ["task", "task_flags", "task_ref", "exit", "ns", "exec_ring", "ptrace_core"],
                    {"Task", "TaskFlags"}, root_items={"current"})
    proc.mods_top = {"task", "task_flags", "task_ref", "exit", "ns", "exec_ring", "ptrace_core"}
    binds = {"task": ["task"], "ns": ["ns"]}
    for text, want, kw in [
        ("let t = crate::task::Task::new();", "let t = rux::process::types::Task::new();", {}),
        ("use crate::task::Task;", "use rux::process::types::Task;", {}),
        ("use crate::task::{Task, current};", "use rux::process::{types::Task, hotpath::current};", {}),
        ("use crate::task;\nlet t = task::TaskFlags::A;", "use rux::process::hotpath;\nlet t = rux::process::types::TaskFlags::A;", {}),
        ("use crate::task::{self, TaskFlags};\nlet t = task::current();", "use rux::process::{hotpath, types::TaskFlags};\nlet t = hotpath::current();", {}),
        ("pub fn f() { task::current() }", "pub fn f() { rux::process::hotpath::current() }", {}),
        ("let n = crate::ns::MAX;", "let n = rux::process::hotpath::MAX;", {}),
        ("let x = crate::task::local::y;", "let x = rux::process::hotpath::local::y;", {}),
    ]:
        got, st = proc.rewrite_text(text, ["crate", "p"], False, False, binds, kw.get("root", False))
        if got != want:
            bad += 1
            print("FAIL crate root\n   in  : %r\n   got : %r\n   want: %r" % (text, got, want))
    for text, want in [
        ("pub use rux::process::{task, task_ref, r#unsafe, ns, exec_ring};", "pub use rux::process::{types, hotpath, r#unsafe};"),
        ("pub use rux::process::task;", "pub use rux::process::{types, hotpath};"),
    ]:
        got, st = proc.rewrite_text(text, ["crate"], False, False, {}, True)
        if got != want:
            bad += 1
            print("FAIL crate root re-export\n   in  : %r\n   got : %r\n   want: %r" % (text, got, want))
    print("selftest: %d cases, %d failures" % (len(cases), bad))
    bad += integration()
    return 1 if bad else 0


def integration():
    """every command on a small tree, end to end: a subsystem before the
    merge (committed), the same after it (working tree), a file of another
    subsystem of the core, and service crates that reach it in each of the
    ways a crate can."""
    bad = 0
    with tempfile.TemporaryDirectory() as d:
        root = d
        mm = "kernel/core/src/mm/"

        def put(rel, text):
            os.makedirs(os.path.dirname(os.path.join(root, rel)), exist_ok=True)
            open(os.path.join(root, rel), "w").write(text)

        def get(rel):
            return open(os.path.join(root, rel)).read()

        put(mm + "mod.rs", "pub mod page_desc;\npub mod r#unsafe;\npub use page_desc::{page_desc, pfn_to_page};\n")
        put(mm + "page_desc.rs", "#![forbid(unsafe_code)]\npub struct Page {\n    pub pfn: usize,\n}\n\n"
            "pub fn page_desc(pfn: usize) -> Option<usize> {\n    Some(pfn)\n}\n\n"
            "pub fn pfn_to_page(pfn: usize) -> usize {\n    pfn\n}\n")
        put(mm + "unsafe.rs", "pub unsafe fn poke(p: *mut u8) {\n    *p = 1;\n}\n")
        put("kernel/core/src/fs/mod.rs", "pub mod vfs;\n")
        put("kernel/core/src/fs/vfs.rs", "use crate::mm::page_desc::{page_desc, Page};\n\n"
            "pub fn f(pfn: usize) -> Option<usize> {\n    page_desc(pfn)\n}\n\n"
            "pub fn g(p: &Page) -> usize {\n    p.pfn\n}\n")
        put("kernel/core/src/lib.rs", "pub mod fs;\npub mod mm;\n\nuse crate::mm::page_desc;\n\n"
            "pub fn k(pfn: usize) -> Option<usize> {\n    page_desc(pfn)\n}\n")
        # the crate named for the subsystem re-exports it at its root, and one old module by name
        put("kernel/mm/src/lib.rs", "pub use rux::{mm, fs};\npub use rux::mm::page_desc;\n\npub fn h(pfn: usize) -> Option<usize> {\n    crate::mm::page_desc::page_desc(pfn)\n}\n")
        put("kernel/mm/src/other.rs", "use crate::mm::page_desc::pfn_to_page;\n\npub fn j(pfn: usize) -> usize {\n    pfn_to_page(pfn)\n}\n")
        # a crate whose root re-exports old modules by name
        put("kernel/ipc/src/lib.rs", "pub use rux::mm::page_desc;\n")
        put("kernel/ipc/src/x.rs", "pub fn k(p: usize) -> Option<usize> {\n    crate::page_desc::page_desc(p)\n}\n\n"
            "use crate::page_desc;\npub fn m(p: usize) -> usize {\n    page_desc::pfn_to_page(p)\n}\n\n"
            "pub fn q(p: usize) -> usize {\n    page_desc::pfn_to_page(p)\n}\n")
        # a crate whose root holds a glob over the whole subsystem
        put("kernel/net/src/lib.rs", "pub use rux::mm::*;\n")
        put("kernel/net/src/y.rs", "pub fn r(p: usize) -> Option<usize> {\n    crate::page_desc::page_desc(p)\n}\n\n"
            "pub fn s(p: usize) -> usize {\n    page_desc::pfn_to_page(p)\n}\n\n"
            "pub fn t(p: usize) -> Option<usize> {\n    page_desc(p)\n}\n")
        # a crate whose root holds a glob over one old module
        put("kernel/fsx/src/lib.rs", "pub use rux::mm::page_desc::*;\n")
        # a crate that reaches the subsystem through ANOTHER crate's root (`pub use ::net;`):
        # its root glob exported the old module, and the crate's own module of another name shadows the glob
        put("kernel/netx/src/lib.rs", "pub use ::net;\n")
        put("kernel/netx/src/a.rs", "pub fn a(p: usize) -> usize {\n    crate::net::page_desc::pfn_to_page(p)\n}\n\n"
            "pub fn c(p: usize) -> Option<usize> {\n    crate::net::page_desc(p)\n}\n\n"
            "use crate::net::page_desc;\npub fn d(p: usize) -> usize {\n    page_desc::pfn_to_page(p)\n}\n")
        put("kernel/netx/src/b.rs", "use crate::net::page_desc::pfn_to_page;\n\npub fn e(p: usize) -> usize {\n    pfn_to_page(p)\n}\n")
        # the subsystem's own crate, named outright: `::mm::x` and bare `mm::x`
        put("kernel/fsx/src/a.rs", "pub fn f(p: usize) -> usize {\n    ::mm::page_desc::pfn_to_page(p)\n}\n\n"
            "pub fn g(p: usize) -> usize {\n    mm::page_desc::pfn_to_page(p)\n}\n")
        # a name the root brings in by a glob, used under a `super::` head from a child module
        put("kernel/net/src/sub/mod.rs", "pub fn h(p: usize) -> usize {\n    super::page_desc::pfn_to_page(p)\n}\n")
        # a crate that does not reach the subsystem: crate::mm there is not the core's
        put("kernel/io_uring/src/lib.rs", "pub use rux::fs;\n")
        put("kernel/io_uring/src/local.rs", "pub fn i(pfn: usize) -> usize {\n    match pfn {\n        crate::mm::page_desc::pfn_to_page(p) => p,\n        _ => 0,\n    }\n}\n")
        run = lambda *a: subprocess.run(["git", "-C", root, *a], capture_output=True, text=True)
        run("init", "-q")
        run("add", "-A")
        run("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "base")
        os.remove(os.path.join(root, mm, "page_desc.rs"))        # the merged state
        put(mm + "types.rs", "pub struct Page {\n    pub pfn: usize,\n}\n")
        put(mm + "hotpath.rs", "use super::types::Page;\n\npub fn page_desc(pfn: usize) -> Option<usize> {\n    Some(pfn)\n}\n\n"
            "pub fn pfn_to_page(pfn: usize) -> usize {\n    pfn\n}\n")
        put(mm + "mod.rs", "pub mod hotpath;\npub mod r#unsafe;\npub mod types;\n")

        def cmd(*a):
            sys.argv = ["pass2.py", *a, "--root", root]
            return main()

        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rc = [cmd("spec", "--sub", "mm", "--base", "HEAD"), cmd("rewrite", "--sub", "mm"),
                  cmd("rewrite", "--sub", "mm", "--dry"), cmd("check", "--sub", "mm"),
                  cmd("verbatim", "--sub", "mm", "--base", "HEAD")]
        text = out.getvalue()
        want = {
            "kernel/core/src/fs/vfs.rs": "use crate::mm::{hotpath::page_desc, types::Page};\n\n"
                                         "pub fn f(pfn: usize) -> Option<usize> {\n    page_desc(pfn)\n}\n\n"
                                         "pub fn g(p: &Page) -> usize {\n    p.pfn\n}\n",
            "kernel/core/src/lib.rs": "pub mod fs;\npub mod mm;\n\nuse crate::mm::hotpath::{self, page_desc};\n\n"
                                     "pub fn k(pfn: usize) -> Option<usize> {\n    page_desc(pfn)\n}\n",
            "kernel/mm/src/lib.rs": "pub use rux::{mm, fs};\npub use rux::mm::hotpath::{self, page_desc};\n\npub fn h(pfn: usize) -> Option<usize> {\n    crate::mm::hotpath::page_desc(pfn)\n}\n",
            "kernel/mm/src/other.rs": "use crate::mm::hotpath::pfn_to_page;\n\npub fn j(pfn: usize) -> usize {\n    pfn_to_page(pfn)\n}\n",
            # page_desc was both a module and a function at the root: the name keeps both meanings,
            # so a file that imports it keeps using it as a module
            "kernel/ipc/src/lib.rs": "pub use rux::mm::hotpath::{self, page_desc};\n",
            "kernel/ipc/src/x.rs": "pub fn k(p: usize) -> Option<usize> {\n    rux::mm::hotpath::page_desc(p)\n}\n\n"
                                   "use rux::mm::hotpath::{self, page_desc};\npub fn m(p: usize) -> usize {\n    page_desc::pfn_to_page(p)\n}\n\n"
                                   "pub fn q(p: usize) -> usize {\n    page_desc::pfn_to_page(p)\n}\n",
            # a glob over the whole root: what stayed a module stays exported beside the two
            "kernel/net/src/lib.rs": "pub use rux::mm::{r#unsafe, hotpath::*, types::*};\n",
            "kernel/net/src/y.rs": "pub fn r(p: usize) -> Option<usize> {\n    rux::mm::hotpath::page_desc(p)\n}\n\n"
                                   "pub fn s(p: usize) -> usize {\n    rux::mm::hotpath::pfn_to_page(p)\n}\n\n"
                                   "pub fn t(p: usize) -> Option<usize> {\n    page_desc(p)\n}\n",
            "kernel/fsx/src/lib.rs": "pub use rux::mm::{hotpath::{page_desc, pfn_to_page}, types::Page};\n",
            "kernel/io_uring/src/local.rs": "pub fn i(pfn: usize) -> usize {\n    match pfn {\n        crate::mm::page_desc::pfn_to_page(p) => p,\n        _ => 0,\n    }\n}\n",
            "kernel/netx/src/a.rs": "pub fn a(p: usize) -> usize {\n    rux::mm::hotpath::pfn_to_page(p)\n}\n\n"
                                   # page_desc is also an old module's name: written from the core (a root
                                   # item of a name no module had would still come through the rewritten glob)
                                   "pub fn c(p: usize) -> Option<usize> {\n    rux::mm::hotpath::page_desc(p)\n}\n\n"
                                   "use rux::mm::hotpath::{self, page_desc};\npub fn d(p: usize) -> usize {\n    page_desc::pfn_to_page(p)\n}\n",
            "kernel/netx/src/b.rs": "use rux::mm::hotpath::pfn_to_page;\n\npub fn e(p: usize) -> usize {\n    pfn_to_page(p)\n}\n",
            "kernel/fsx/src/a.rs": "pub fn f(p: usize) -> usize {\n    rux::mm::hotpath::pfn_to_page(p)\n}\n\n"
                                   "pub fn g(p: usize) -> usize {\n    rux::mm::hotpath::pfn_to_page(p)\n}\n",
            "kernel/net/src/sub/mod.rs": "pub fn h(p: usize) -> usize {\n    rux::mm::hotpath::pfn_to_page(p)\n}\n",
        }
        for rel, w in want.items():
            got = get(rel)
            if got != w:
                bad += 1
                print("FAIL integration %s\n   got : %r\n   want: %r" % (rel, got, w))
        sp = json.load(open(spec_path(root, "mm")))
        if sp["modules"] != ["page_desc"] or set(sp["root_items"]) != {"page_desc", "pfn_to_page"} or sp["exports"] != {"page_desc": ["Page", "page_desc", "pfn_to_page"]}:
            bad += 1
            print("FAIL integration spec: %s" % sp)
        if rc != [0, 0, 0, 0, 0] or "would change 0 files, 0 replacements" not in text:
            bad += 1
            print("FAIL integration: exit codes %s\\n%s" % (rc, text[-900:]))
    print("integration (spec, rewrite, dry, check, verbatim on a small tree): %s" % ("ok" if not bad else "%d failures" % bad))
    return bad


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sp = ap.add_subparsers(dest="cmd", required=True)
    p = sp.add_parser("spec")
    p.add_argument("--sub", required=True)
    p.add_argument("--root", default=".")
    p.add_argument("--base", help="git revision holding the mod.rs of before the merge, when the merge is done")
    p.add_argument("--inline", help="comma-separated directories that stay one inline module of hotpath.rs")
    for name in ("rewrite", "check", "pair", "verbatim"):
        p = sp.add_parser(name)
        p.add_argument("--sub", required=True)
        p.add_argument("--root", default=".")
        p.add_argument("--show", type=int, default=20)
        p.add_argument("--spec", help="the spec file (default: scripts/refactor/pass2/<sub>.json)")
        if name == "rewrite":
            p.add_argument("--dry", action="store_true")
        if name == "pair":
            p.add_argument("--base", required=True)
            p.add_argument("--cur", required=True)
            p.add_argument("--nm")
        if name == "verbatim":
            p.add_argument("--base", required=True, help="git revision holding the old files")
            p.add_argument("--cur", help="git revision holding the new files (default: the working tree)")
    sp.add_parser("selftest")
    args = ap.parse_args()
    return {"selftest": cmd_selftest, "rewrite": cmd_rewrite, "check": cmd_check, "pair": cmd_pair,
            "verbatim": cmd_verbatim, "spec": cmd_spec}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
