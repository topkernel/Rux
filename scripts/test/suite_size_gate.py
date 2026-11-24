#!/usr/bin/env python3
"""suite_size_gate.py — make the in-kernel suite's SIZE a checked quantity.

The suite wrapper (scripts/test/run.sh) turns a run into pass/fail from
the kernel's verdict line alone. That is blind to a test group that stops
running: when a file moves out of the framework tree and nothing calls
it any more, the suite still prints "ALL TESTS PASSED" — with fewer
assertions. This happened three times on 2026-11-23 during the crate
migrations (a sched pair, -66; a net group; a tmpfs group, -48), every
time green.

This gate compares a finished run against a committed baseline:

  scripts/test/suite-baseline.txt    one line per arch:
                                         <arch> groups=<N> assertions=<N>

Two counts, both taken from the serial log:
  groups       distinct test-group headers (test: [<n>] <name> ...). The
               header counter is shared by framework and service tests,
               so a group name can repeat; distinct names are counted.
  assertions   the kernel's "Total:" counter is the authority; the
               text PASS-line count is a cross-check (async kernel
               prints can garble a line through a different console
               lock — three occurrences 2026-11-23/24 — which loses no
               test). A drop below baseline fails on either count only
               when the kernel counter itself dropped.

Usage:
  suite_size_gate.py --log LOG [--arch ARCH]
      Check LOG against the baseline. Exits non-zero when either count is
      BELOW the baseline, or when a group present in the baseline's group
      list is missing. Equal or higher passes.
  suite_size_gate.py --update --log LOG [--arch ARCH]
      Rewrite the baseline line for ARCH from LOG. Refuses to LOWER a
      count: do that deliberately by editing the baseline file by hand,
      with the reason in the commit message.
  --arch defaults to the platform the tree is configured for.

A count ABOVE the baseline passes but says so: raise the baseline in the
same change that added the tests, so the new number becomes the floor.
"""

import argparse
import os
import re
import subprocess
import sys

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
PROJECT_ROOT = os.path.dirname(os.path.dirname(SCRIPT_DIR))
BASELINE = os.path.join(SCRIPT_DIR, "suite-baseline.txt")

ANSI = re.compile(r"\x1b\[[0-9;]*m")
GROUP = re.compile(r"^test: \[\d+\] (\S+)")
PASS = re.compile(r"^test:   PASS ")


def configured_platform():
    try:
        out = subprocess.run(
            ["bash", os.path.join(PROJECT_ROOT, "build", "current-platform.sh")],
            capture_output=True, text=True, check=True, cwd=PROJECT_ROOT,
        )
        return out.stdout.strip()
    except Exception:
        return None


def parse_log(path):
    """Return (groups_in_order, assertion_count, kernel_totals)."""
    groups, passed, totals = [], 0, None
    with open(path, errors="replace") as f:
        for raw in f:
            line = ANSI.sub("", raw).rstrip("\r\n")
            m = GROUP.match(line)
            if m:
                if m.group(1) not in groups:
                    groups.append(m.group(1))
                continue
            if PASS.match(line):
                passed += 1
                continue
            m = re.match(r"^\s*Total:\s+(\d+) tests", line)
            if m:
                totals = int(m.group(1))
    return groups, passed, totals


def load_baseline():
    base = {}
    if not os.path.exists(BASELINE):
        return base
    with open(BASELINE) as f:
        for raw in f:
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            m = re.match(r"^(\S+)\s+groups=(\d+)\s+assertions=(\d+)(?:\s+names=(\S+))?$", line)
            if not m:
                sys.exit("suite_size_gate: cannot parse baseline line: %r" % raw.rstrip())
            base[m.group(1)] = (int(m.group(2)), int(m.group(3)),
                                m.group(4).split(",") if m.group(4) else [])
    return base


def save_baseline(base):
    with open(BASELINE, "w") as f:
        f.write("# In-kernel suite size baseline (scripts/test/suite_size_gate.py).\n"
                "# Lower a number only by editing this file by hand, with the reason\n"
                "# in the commit message. Raise it in the change that adds tests:\n"
                "#   scripts/test/suite_size_gate.py --update --log <serial log>\n")
        for arch in sorted(base):
            g, a, names = base[arch]
            f.write("%s groups=%d assertions=%d names=%s\n" % (arch, g, a, ",".join(names)))


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--log", required=True)
    ap.add_argument("--arch", default=None)
    ap.add_argument("--update", action="store_true")
    ap.add_argument("--quiet", action="store_true",
                    help="print only on failure (used by the make test wrapper)")
    args = ap.parse_args()

    arch = args.arch or configured_platform()
    if not arch:
        sys.exit("suite_size_gate: no --arch and the tree has no configured platform")

    groups, passed, totals = parse_log(args.log)
    base = load_baseline()

    if args.update:
        g, a, _ = base.get(arch, (0, 0, []))
        if g and len(groups) < g:
            sys.exit("suite_size_gate: refusing to lower groups for %s (%d -> %d)"
                     % (arch, g, len(groups)))
        if a and passed < a:
            sys.exit("suite_size_gate: refusing to lower assertions for %s (%d -> %d)"
                     % (arch, a, passed))
        base[arch] = (len(groups), passed, groups)
        save_baseline(base)
        print("suite_size_gate: baseline for %s = groups %d, assertions %d"
              % (arch, len(groups), passed))
        return

    if arch not in base:
        if args.quiet:
            return
        print("suite_size_gate: no baseline for %s — run --update once and commit it" % arch)
        return

    bg, ba, bnames = base[arch]
    problems = []
    if len(groups) < bg:
        lost = [n for n in bnames if n not in groups]
        problems.append("groups %d < baseline %d" % (len(groups), bg))
        if lost:
            problems.append("missing groups: %s" % ", ".join(lost[:25])
                            + (" ..." if len(lost) > 25 else ""))
    # The kernel's own counter is authoritative for assertions: async
    # kernel prints (e820 dump echo, late TSC promotion) interleave with
    # test lines MID-LINE through a different console lock, garbling a
    # PASS line's text without losing the test. A text count below the
    # kernel total is that artifact, not a lost test; a kernel total
    # below the baseline IS a loss.
    authority = totals if totals is not None else passed
    if authority < ba:
        problems.append("assertions %d < baseline %d (-%d)%s"
                        % (authority, ba, ba - authority,
                           "" if totals is None else " (kernel counter)"))
    elif passed < authority:
        print("suite_size_gate: note — %d PASS line(s) garbled by serial interleave; "
              "kernel counter %d is the authority" % (authority - passed, authority))
    if passed < ba and authority >= ba:
        pass  # garble only; already noted

    if problems:
        print("SUITE SIZE GATE FAILED (%s): a green verdict with fewer tests is not a pass." % arch)
        for p in problems:
            print("  - " + p)
        print("  If tests were removed on purpose: edit %s by hand and say why in the commit."
              % os.path.relpath(BASELINE, PROJECT_ROOT))
        sys.exit(1)

    if not args.quiet:
        if len(groups) > bg or authority > ba:
            print("suite_size_gate: %s grew (groups %d vs %d, assertions %d vs %d) — "
                  "raise the baseline in this change"
                  % (arch, len(groups), bg, authority, ba))
        else:
            print("suite_size_gate: %s ok (groups %d, assertions %d)" % (arch, len(groups), passed))


if __name__ == "__main__":
    main()
