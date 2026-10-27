#!/bin/bash
# pa46-bootwall.sh — F4-R2 boot-wall comparison from two stamped census
# logs (leg-34). Compares wall-clock at fixed progress markers:
#   - each SYSCOST dump boundary (t=60s..720s kernel time): the WALL
#     time the boot needed to reach that kernel-time checkpoint
#     (wall-lag = host pace; wall-delta at EQUAL kernel t is the
#     per-boot wall saving)
#   - syscall volume at equal kernel t (progress shape)
#   - first occurrence of stable kmsg milestones
# Usage: pa46-bootwall.sh <stamped-A> <stamped-B>   (A=main, B=f4r2)
set -u
A="$1"; B="$2"

py() { python3 - "$A" "$B" "$1"; }

echo "== wall-clock at SYSCOST dump boundaries (kernel t -> wall sec) =="
python3 - "$A" "$B" <<'EOF'
import re, sys

def walls(path):
    # stamped lines: [+00061.301] SYSCOST t=60s ...
    out = {}
    for line in open(path, errors="replace"):
        m = re.search(r"\[\+(\d+)\.(\d+)\] .*SYSCOST t=(\d+)s syscalls:", line)
        if m:
            out[int(m.group(3))] = int(m.group(1)) + int(m.group(2)) / 1000
    return out

def vols(path):
    out = {}
    for line in open(path, errors="replace"):
        m = re.search(r"SYSCOST t=(\d+)s syscalls: (\d+) kinds (\d+) calls", line)
        if m:
            out[int(m.group(1))] = int(m.group(3))
    return out

wa, wb = walls(sys.argv[1]), walls(sys.argv[2])
va, vb = vols(sys.argv[1]), vols(sys.argv[2])
print(f"{'k t':>6} {'A wall':>10} {'B wall':>10} {'B-A s':>8} {'A calls':>12} {'B calls':>12} {'vol B-A':>10}")
tot = 0.0
n = 0
for t in sorted(set(wa) & set(wb)):
    d = wb[t] - wa[t]
    tot += d
    n += 1
    da = va.get(t, 0); db = vb.get(t, 0)
    print(f"{t:>6} {wa[t]:>10.1f} {wb[t]:>10.1f} {d:>+8.1f} {da:>12} {db:>12} {db-da:>+10}")
if n:
    print(f"mean wall delta over {n} checkpoints: {tot/n:+.1f}s (negative = F4-R2 ahead)")
EOF

echo
echo "== milestone first-seen wall (sec) =="
python3 - "$A" "$B" <<'EOF'
import re, sys
PATS = [
    ("foundation exit", r"foundation\(pid \d+\) exit"),
    ("bootevent.appfwk.ready", r"bootevent\.appfwk\.ready not set|ohos\.boot\.time\.appfwk\.ready"),
    ("appspawn fork", r"APPSPAWN.*fork|fork.*appspawn"),
    ("first SIGKILL from init", r"SIGKILL-SEND from pid=1"),
]
def marks(path):
    out = {}
    for line in open(path, errors="replace"):
        m = re.search(r"\[\+(\d+)\.", line)
        if not m:
            continue
        t = int(m.group(1))
        for name, pat in PATS:
            if name not in out and re.search(pat, line):
                out[name] = t
    return out
ma, mb = marks(sys.argv[1]), marks(sys.argv[2])
print(f"{'milestone':>28} {'A':>8} {'B':>8} {'B-A':>7}")
for name, _ in PATS:
    a, b = ma.get(name), mb.get(name)
    d = f"{b-a:+d}" if a is not None and b is not None else "-"
    print(f"{name:>28} {str(a):>8} {str(b):>8} {d:>7}")
EOF
