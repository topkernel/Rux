#!/usr/bin/env python3
"""pa-parse.py — offline analysis of a pa-boot serial log (perf-audit line).

Extracts:
  1. SYSCOST dump tables (cumulative): per-syscall {n, avg, max, tot} at
     each 60 s dump; native x86_64 nrs decoded via the nrmap TSV
     (extracted from kernel/src/syscall/x86_compat.rs).
  2. Tick-mode accounting (u/k/i) per dump -> CPU decomposition.
  3. Kernel-rip page histogram per dump (symbolized if the ELF is given
     via PA_ELF, using nm output cached at /tmp/pa-nm-<tag>.txt).
  4. Slow-ring entries (nr, wall us, comm).
  5. Phase milestones from the stamped log ([+SS.sss] prefixes) when the
     file given carries them.

Usage: pa-parse.py <serial-or-stamped-log> [--nm <nm-file>]
Prints a compact report to stdout.
"""
import re
import sys
from collections import defaultdict


def load_nrmap(path='/tmp/pa-nrmap.tsv'):
    m = {}
    try:
        for line in open(path):
            nat, gen, name = line.rstrip('\n').split('\t')
            m[int(nat)] = name
    except OSError:
        pass
    return m


NAMES = load_nrmap()


def main():
    log = sys.argv[1]
    nmfile = None
    if '--nm' in sys.argv:
        nmfile = sys.argv[sys.argv.index('--nm') + 1]

    syms = []
    if nmfile:
        import bisect
        for line in open(nmfile):
            parts = line.split(' ', 2)
            if len(parts) == 3 and parts[1] in 'tT':
                try:
                    syms.append((int(parts[0], 16), parts[2].strip()))
                except ValueError:
                    pass
        syms.sort()
        addrs = [s[0] for s in syms]

        def sym(a):
            import bisect
            i = bisect.bisect_right(addrs, a) - 1
            if i >= 0:
                return syms[i][1][:90] + ' +%#x' % (a - syms[i][0])
            return '?'

    # Latest SYSCOST state
    dumps = defaultdict(lambda: defaultdict(dict))  # t -> nr -> stats
    ticks = {}
    rips = defaultdict(list)
    slow = []
    t_re = re.compile(r'SYSCOST t=(\d+)s (?:nr=(\d+) n=(\d+) avg=(\d+)us max=(\d+)ms tot=(\d+)ms'
                     r'|ticks\(u/k/i\):((?: c\d+=\d+/\d+/\d+)*)'
                     r'|rip page=(0x[0-9a-f]+) n=(\d+) \((\d+)%\)'
                     r'|syscalls: (\d+) kinds (\d+) calls (\d+) tot=(\d+) ms'
                     r'|slow nr=(\d+) cyc=(\d+)us comm=(\S+))')
    last_t = 0
    for line in open(log, errors='replace'):
        m = t_re.search(line)
        if not m:
            continue
        t = int(m.group(1))
        last_t = t
        if m.group(2):
            nr, n, avg, mx, tot = map(int, m.group(2, 3, 4, 5, 6))
            dumps[t][nr] = (n, avg, mx, tot)
        elif m.group(7):
            ticks[t] = m.group(7).strip()
        elif m.group(8):
            rips[t].append((int(m.group(8), 16), int(m.group(9)), int(m.group(10))))
        elif m.group(13):
            slow.append((t, int(m.group(13)), int(m.group(14)), m.group(15)))
        elif m.group(11):
            pass  # summary line

    print("=== last dump t=%ds ===" % last_t)
    if last_t in ticks:
        print("ticks:", ticks[last_t])
        # decompose
        tot = {}
        for m2 in re.finditer(r'c(\d+)=(\d+)/(\d+)/(\d+)', ticks[last_t]):
            c, u, k, i = map(int, m2.groups())
            s = u + k + i
            tot[c] = (u, k, i, s)
            print("cpu%s: user %.1f%% kernel %.1f%% idle %.1f%% (n=%d)"
                  % (c, 100 * u / s, 100 * k / s, 100 * i / s, s))
    d = dumps.get(last_t, {})
    rows = sorted(d.items(), key=lambda kv: -kv[1][3])
    print("\n=== syscall table (top 40 by total wall) ===")
    print("%-6s %-14s %10s %10s %12s %12s" % ("nr", "name", "n", "avg_us", "max_ms", "tot_ms"))
    for nr, (n, avg, mx, tot) in rows[:40]:
        print("%-6d %-14s %10d %10d %12d %12d" % (nr, NAMES.get(nr, '?'), n, avg, mx, tot))

    if last_t in rips:
        print("\n=== kernel-rip pages (top 20) ===")
        for p, n, pct in rips[last_t][:20]:
            s = sym(p) if syms else ''
            print("%#x n=%-6d %3d%% %s" % (p, n, pct, s))

    if slow:
        print("\n=== slow ring tail (last 20, newest last) ===")
        for t, nr, us, comm in slow[-20:]:
            print("t=%ds nr=%d %-14s %8d us comm=%s" % (t, nr, NAMES.get(nr, '?'), us, comm))

    # pa15/pa16 IF=0 zones + REST call-site table (latest dump).
    if0 = {}          # cpu -> fields
    sites = {}        # (file:line) -> [n, tot_ms, avg_ns, max_ms]
    of_site = None
    for line in open(log, errors='replace'):
        m2 = re.search(r'SYSCOST t=\d+s if0 cpu=(\d+): (.*)$', line)
        if m2:
            if0[int(m2.group(1))] = m2.group(2)
            continue
        m2 = re.search(r'SYSCOST t=\d+s rest-site\s+\d+: (\S+?):(\d+) n=(\d+) tot=(\d+)ms avg=(\d+)ns max=(\d+)ms', line)
        if m2:
            f, ln, n, tot, avg, mx = m2.groups()
            sites['%s:%s' % (f, ln)] = [int(n), int(tot), int(avg), int(mx)]
            continue
        m2 = re.search(r'SYSCOST t=\d+s rest-site of: (.*)$', line)
        if m2:
            of_site = m2.group(1)
    if if0:
        print("\n=== IF=0 zones (latest dump) ===")
        for c in sorted(if0):
            print("cpu%d: %s" % (c, if0[c]))
    if sites:
        print("\n=== REST call sites (latest dump, top 20 by tot) ===")
        print("%-46s %10s %10s %10s %10s" % ("file:line", "n", "tot_ms", "avg_ns", "max_ms"))
        for k, v in sorted(sites.items(), key=lambda kv: -kv[1][1])[:20]:
            print("%-46s %10d %10d %10d %10d" % (k, v[0], v[1], v[2], v[3]))
        print("site-table sum: %d ms across %d sites" % (sum(v[1] for v in sites.values()), len(sites)))
    if of_site:
        print("overflow row:", of_site)

    # wedge detection: last stamped timestamp
    ts = None
    try:
        for line in open(log, errors='replace'):
            m2 = re.match(r'\[\+(\d+)\.(\d+)\]', line)
            if m2:
                ts = int(m2.group(1)) + int(m2.group(2)) / 1000
        if ts is not None:
            print("\nlast stamped serial line: t=%.1fs" % ts)
    except OSError:
        pass


if __name__ == '__main__':
    main()
