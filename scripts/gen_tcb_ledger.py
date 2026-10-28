#!/usr/bin/env python3
"""TCB ledger generator for the framekernel effort.

Counts unsafe usage per module across kernel/src, classifies every file into
framekernel layers, and reports the headline metric: unsafe tokens OUTSIDE the
Framework layer (i.e. in Service modules that must eventually carry
#![forbid(unsafe_code)]).

Layers:
  framework    Hardware/privilege TCB. unsafe is permitted here, but every
               block is expected to carry an auditable SAFETY invariant
               comment (coverage is tracked below).
  service      Kernel services (fs, net, ipc, syscall wrappers, sched policy,
               driver logic, process management). Target state: 100% forbid.
  diagnostics  dfx/ debug tooling that deliberately reads raw memory.
               Permitted-unsafe like framework, but reported separately so
               tool churn does not hide in the framework bucket.
  test         kernel/src/tests (unit-test feature only, not shipped).
  unbuilt      cfg'd-off on both gate targets (aarch64 skeleton).

Caveat (honesty rule): Rust's unsafe discipline covers accidental memory
unsafety only. It is not a proof against logic bugs, spec violations, or
compromised build tooling. The ledger measures language-level TCB shape.

Usage:
  python3 scripts/gen_tcb_ledger.py [--json out.json] [--markdown]
"""

import argparse
import json
import os
import re
import sys
from collections import defaultdict

SRC_DEFAULT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                           os.pardir, "kernel", "src")

# ---------------------------------------------------------------------------
# Layer classification. Longest matching prefix wins. Unmatched -> service.
# Keep in sync with docs/architecture/framekernel-layers.md.
# ---------------------------------------------------------------------------
RULES = [
    ("arch/aarch64/", "unbuilt"),
    ("arch/", "framework"),
    ("mm/", "framework"),
    # sync: raw primitives are framework, syscall-facing logic is service
    ("sync/spinlock.rs", "framework"),
    ("sync/rwlock.rs", "framework"),
    ("sync/seqlock.rs", "framework"),
    ("sync/percpu.rs", "framework"),
    ("sync/rcu.rs", "framework"),
    ("sync/lockkey.rs", "framework"),
    ("sync/", "service"),
    ("interrupt/", "framework"),
    # drivers: MMIO/DMA cores are framework, device logic is service
    ("drivers/virtio/", "framework"),
    ("drivers/intc/", "framework"),
    ("drivers/timer/", "framework"),
    ("drivers/pci/", "framework"),
    ("drivers/", "service"),
    ("dfx/", "diagnostics"),
    ("tests/", "test"),
    # root files tied to early boot / raw hardware
    ("console.rs", "framework"),
    ("printk.rs", "framework"),
    ("sbi.rs", "framework"),
    ("main.rs", "framework"),
    ("init.rs", "framework"),
    ("initrd.rs", "framework"),
    ("cmdline.rs", "framework"),
    ("list.rs", "framework"),
    ("timer.rs", "framework"),
    ("module/", "framework"),
    ("perf/", "framework"),
    ("trace/", "framework"),
]
DEFAULT_LAYER = "service"
LAYER_ORDER = ["framework", "service", "diagnostics", "test", "unbuilt"]

UNSAFE_TOKEN = re.compile(r"\bunsafe\b")
UNSAFE_CONSTRUCT = re.compile(r"\bunsafe\s*(?:\{|fn|impl|extern|trait)")
FORBID = re.compile(r"forbid\s*\(\s*unsafe_code\s*\)")
SAFETY = re.compile(r"SAFETY", re.IGNORECASE)
SAFETY_LOOKBACK_LINES = 4


def strip_comments_and_strings(text):
    """Remove // and /* */ comments plus string/rstring literals so token
    counts reflect code only (docs mentioning `unsafe` do not inflate)."""
    out = []
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            j = text.find("\n", i)
            i = n if j == -1 else j  # keep the newline
        elif c == "/" and i + 1 < n and text[i + 1] == "*":
            j = text.find("*/", i + 2)
            i = n if j == -1 else j + 2
            out.append(" ")
        elif c == '"':
            # cooked string
            j = i + 1
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == '"':
                    break
                j += 1
            i = j + 1
            out.append('""')
        elif c == "r" and i + 1 < n and text[i + 1] in "#\"":
            # raw string r"..." or r#"..."#
            j = i + 1
            hashes = 0
            while j < n and text[j] == "#":
                hashes += 1
                j += 1
            if j < n and text[j] == '"':
                end = '"' + "#" * hashes
                k = text.find(end, j + 1)
                i = n if k == -1 else k + len(end)
                out.append('""')
            else:
                out.append(c)
                i += 1
        elif c == "'":
            # char literal or lifetime — only skip when it is a char literal
            m = re.match(r"'(\\.|[^'\\])'", text[i:])
            if m:
                out.append("'x'")
                i += m.end()
            else:
                out.append(c)
                i += 1
        else:
            out.append(c)
            i += 1
    return "".join(out)


def classify(relpath):
    best, best_len = DEFAULT_LAYER, -1
    for prefix, layer in RULES:
        if relpath.startswith(prefix) and len(prefix) > best_len:
            best, best_len = layer, len(prefix)
    return best


def analyze(path, relpath):
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        raw_lines = f.readlines()
    text = "".join(raw_lines)
    code = strip_comments_and_strings(text)
    code_lines = code.splitlines()

    tokens = len(UNSAFE_TOKEN.findall(code))
    constructs = []
    for lineno, line in enumerate(code_lines, start=1):
        if UNSAFE_CONSTRUCT.search(line):
            constructs.append(lineno)
    forbidden = bool(FORBID.search(code)) or text.lstrip().startswith(
        "#![forbid(unsafe_code)]")
    # SAFETY comment coverage: construct has a SAFETY comment within
    # SAFETY_LOOKBACK_LINES lines above it.
    safety_lines = set()
    for lineno, line in enumerate(raw_lines, start=1):
        if "//" in line or "/*" in line:
            if SAFETY.search(line):
                safety_lines.add(lineno)
    covered = 0
    for c in constructs:
        for back in range(1, SAFETY_LOOKBACK_LINES + 1):
            if (c - back) in safety_lines:
                covered += 1
                break
    return {
        "path": relpath,
        "layer": classify(relpath),
        "lines": len(raw_lines),
        "unsafe_tokens": tokens,
        "unsafe_constructs": len(constructs),
        "safety_covered": covered,
        "forbid": forbidden,
    }


def module_of(relpath):
    parts = relpath.split("/")
    if len(parts) == 1:
        return "(root)"
    if parts[-1] in ("mod.rs",) or parts[-1] == parts[-2] + ".rs":
        return "/".join(parts[:-1])
    return "/".join(parts[:-1])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", default=SRC_DEFAULT)
    ap.add_argument("--json", help="write per-file records as JSON")
    ap.add_argument("--markdown", action="store_true",
                    help="emit tables in markdown")
    args = ap.parse_args()

    src = os.path.normpath(args.src)
    records = []
    for root, dirs, files in os.walk(src):
        dirs.sort()
        for fn in sorted(files):
            if not fn.endswith(".rs"):
                continue
            full = os.path.join(root, fn)
            rel = os.path.relpath(full, src)
            records.append(analyze(full, rel))
    records.sort(key=lambda r: r["path"])

    def total(key, pred=lambda r: True):
        return sum(r[key] for r in records if pred(r))

    headline = total("unsafe_tokens", lambda r: r["layer"] == "service")
    framework = total("unsafe_tokens", lambda r: r["layer"] == "framework")
    diag = total("unsafe_tokens", lambda r: r["layer"] == "diagnostics")
    test = total("unsafe_tokens", lambda r: r["layer"] == "test")
    unbuilt = total("unsafe_tokens", lambda r: r["layer"] == "unbuilt")
    total_tokens = total("unsafe_tokens")

    svc_files = [r for r in records if r["layer"] == "service"]
    svc_forbidden = sum(1 for r in svc_files if r["forbid"])
    svc_unsafe_free = sum(1 for r in svc_files if r["unsafe_tokens"] == 0)
    constructs = total("unsafe_constructs")
    covered = total("safety_covered")

    md = args.markdown
    def table(*cols):
        print(("| " + " | ".join(cols) + " |") if md else
              " | ".join(cols))

    if md:
        print("# TCB Ledger (generated by scripts/gen_tcb_ledger.py)\n")
        print("Caveat: language-level safety covers accidental memory"
              " unsafety only, not logic bugs or spec violations.\n")
        print("## Headline\n")
        print("- unsafe tokens OUTSIDE Framework (service): **%d**" % headline)
        print("- Framework tokens: %d | diagnostics: %d | test: %d |"
              " unbuilt: %d | total: %d"
              % (framework, diag, test, unbuilt, total_tokens))
        print("- Service files: %d — forbid(unsafe_code): %d | unsafe-free:"
              " %d | unsafe-bearing: %d"
              % (len(svc_files), svc_forbidden, svc_unsafe_free,
                 len(svc_files) - svc_unsafe_free))
        print("- SAFETY-comment coverage over unsafe constructs:"
              " %d/%d (%.0f%%)\n" % (covered, constructs,
                                     100.0 * covered / max(1, constructs)))
        print("## Per-module rollup\n")
        table("module", "layer", "files", "forbid", "unsafe files",
              "tokens", "constructs", "SAFETY cov")
        print("|---|---|---|---|---|---|---|---|")
    else:
        print("TCB ledger — %s" % src)
        print("Caveat: language safety covers accidental memory unsafety"
              " only, not logic bugs.")
        print()
        print("HEADLINE  unsafe tokens outside Framework (service): %d"
              % headline)
        print("          framework=%d diagnostics=%d test=%d unbuilt=%d"
              " total=%d" % (framework, diag, test, unbuilt, total_tokens))
        print("SERVICE   files=%d forbidden=%d unsafe-free=%d"
              " unsafe-bearing=%d"
              % (len(svc_files), svc_forbidden, svc_unsafe_free,
                 len(svc_files) - svc_unsafe_free))
        print("SAFETY    %d/%d constructs covered (%.0f%%)"
              % (covered, constructs, 100.0 * covered / max(1, constructs)))
        print()

    mods = defaultdict(lambda: {"files": 0, "forbid": 0, "uf": 0, "tok": 0,
                                "con": 0, "cov": 0, "layer": None})
    for r in records:
        m = mods[module_of(r["path"])]
        m["files"] += 1
        m["forbid"] += 1 if r["forbid"] else 0
        m["uf"] += 1 if r["unsafe_tokens"] > 0 else 0
        m["tok"] += r["unsafe_tokens"]
        m["con"] += r["unsafe_constructs"]
        m["cov"] += r["safety_covered"]
        m["layer"] = r["layer"]
    rows = sorted(mods.items(), key=lambda kv: (-kv[1]["tok"], kv[0]))
    for name, m in rows:
        table(name, m["layer"], str(m["files"]), str(m["forbid"]),
              str(m["uf"]), str(m["tok"]), str(m["con"]), str(m["cov"]))

    if md:
        print("\n## Top service-layer files by unsafe tokens\n")
        table("file", "tokens", "constructs", "SAFETY cov", "forbid")
        print("|---|---|---|---|---|")
    else:
        print()
        print("TOP SERVICE FILES BY TOKENS")
    top = sorted((r for r in svc_files if r["unsafe_tokens"] > 0),
                 key=lambda r: -r["unsafe_tokens"])[:25]
    for r in top:
        table(r["path"], str(r["unsafe_tokens"]), str(r["unsafe_constructs"]),
              "%d/%d" % (r["safety_covered"], r["unsafe_constructs"]),
              "yes" if r["forbid"] else "no")

    if args.json:
        with open(args.json, "w") as f:
            json.dump({
                "headline_service_unsafe": headline,
                "framework_unsafe": framework,
                "diagnostics_unsafe": diag,
                "test_unsafe": test,
                "unbuilt_unsafe": unbuilt,
                "total_unsafe": total_tokens,
                "service_files": len(svc_files),
                "service_forbidden": svc_forbidden,
                "safety_coverage": [covered, constructs],
                "files": records,
            }, f, indent=1)
        if not md:
            print("\nJSON written to %s" % args.json, file=sys.stderr)


if __name__ == "__main__":
    main()
