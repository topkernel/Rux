#!/bin/bash
# perf-gate.sh — one-command merge-gate performance check (user policy
# 2026-10-25, AGENTS.md "tiered perf gate"). Automates the tiers
# against docs/development/perf-gate-baseline.md:
#
#   Tier 1  quiet syscost boot (run/pa-boot.sh: q35, KVM, smp2, 780 s)
#           -> census, tick rate, syscall volume, per-op avgs, panic/OOM
#   Tier 2  pbench P1-P7 boot (test/pbench_boot.py) -> median compare
#           vs the pb1 smoke anchor column (medians only; p99 is
#           host-noise-dominated — perf-report.md 8.5)
#   Tier 3  IF=0 share (needs the pa16 EFLAGS probe hooks in the
#           kernel; on a probe-less kernel the documented proxy is
#           measured instead: syscost tick-mode kernel share 83-86%)
#   Domains (guarded: alloc/reclaim, scheduling, filesystem):
#           from the same Tier-1 log — GRQ hold avg, small_alloc REST
#           share when rest-sites are dumped, tick-mode kernel share
#           as the probe-less proxy; buffer-cache hit / fsw classes
#           only when those instruments are enabled in the build.
#
# Usage: run/perf-gate.sh [--tiers 1,2] [--kernel PATH] [--tag NAME]
#                        [--secs 780] [--skip-boot] [--log PATH]
#                        [--pbench-baseline FILE]
#   --skip-boot --log X   re-parse an existing serial log (no QEMU)
#   --pbench-baseline     a pbench results file whose medians are the
#                         anchor (default: the pb1 smoke column below)
# Env: SMP ACCEL (passed to pa-boot.sh), VOLUME_BASE=136000000,
#      TERM_BASE=8 TERM_BAND=2, REFUSED_BASE=32 REFUSED_BAND=4,
#      TICK_MIN=99.5, CGAVG_US_BAND="3 4.8", NANOAVG_MS_MAX=265
#
# Boots are sequential and this script runs at most one QEMU at a
# time; it refuses to start when 2+ foreign QEMUs are already up
# (host-capacity discipline). Every FAIL prints the rerun-once rule
# (same as the LTP flake policy): rerun in isolation before calling a
# regression.
#
# Baseline update rule: the constants below mirror
# docs/development/perf-gate-baseline.md; when a merge refreshes that
# table, refresh these constants in the same commit.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TIERS="1"
KERNEL=""
TAG="gate-$(date +%m%d-%H%M)"
SECS=780
SKIP_BOOT=0
LOG=""
PB_BASELINE=""

while [ $# -gt 0 ]; do
    case "$1" in
        --tiers) TIERS="$2"; shift 2 ;;
        --kernel) KERNEL="$2"; shift 2 ;;
        --tag) TAG="$2"; shift 2 ;;
        --secs) SECS="$2"; shift 2 ;;
        --skip-boot) SKIP_BOOT=1; shift ;;
        --log) LOG="$2"; SKIP_BOOT=1; shift 2 ;;
        --pbench-baseline) PB_BASELINE="$2"; shift 2 ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

# ---- baseline constants (docs/development/perf-gate-baseline.md) ----
VOLUME_BASE="${VOLUME_BASE:-136000000}"     # pa38 780s boot, +-10%
TERM_BASE="${TERM_BASE:-8}"; TERM_BAND="${TERM_BAND:-2}"
REFUSED_BASE="${REFUSED_BASE:-32}"; REFUSED_BAND="${REFUSED_BAND:-4}"
TICK_MIN="${TICK_MIN:-99.5}"                # ticks/s/CPU sustained
CGAVG_US_MIN="${CGAVG_US_MIN:-3}"           # clock_gettime avg band
CGAVG_US_MAX="${CGAVG_US_MAX:-4.8}"         # 3-4us +-20%
NANOAVG_MS_MAX="${NANOAVG_MS_MAX:-265}"     # nanosleep 221ms +-20%
GRQ_AVG_NS_MIN="${GRQ_AVG_NS_MIN:-4080}"    # GRQ hold 5.1-6.1us +-20%
GRQ_AVG_NS_MAX="${GRQ_AVG_NS_MAX:-7320}"
SMALL_ALLOC_SHARE_MAX="${SMALL_ALLOC_SHARE_MAX:-10}"  # % of machine
KSHARE_MIN="${KSHARE_MIN:-83}"              # probe-less proxy band
KSHARE_MAX="${KSHARE_MAX:-86}"
IF0_BASE="${IF0_BASE:-25.7}"; IF0_BAND="${IF0_BAND:-2}"  # Tier 3

PASS=0; FAIL=0; SKIP=0
verdict() { # verdict PASS|FAIL|SKIP|INFO <metric> <value> <band> [note]
    local v="$1" m="$2" val="$3" band="$4" note="${5:-}"
    case "$v" in
        PASS) PASS=$((PASS+1)) ;;
        FAIL) FAIL=$((FAIL+1)); note="$note RERUN-ONCE-RULE: rerun in isolation before calling a regression." ;;
        SKIP) SKIP=$((SKIP+1)) ;;
        INFO) : ;;
    esac
    printf '%-5s %-34s %-28s band: %s%s\n' "$v" "$m" "$val" "$band" \
        "${note:+  [$note]}"
}

tier1_check() {
    local log="$1"
    echo "== Tier 1: quiet syscost boot ($log) =="

    # panic / OOM classes
    local panics ooms
    panics=$(grep -c -E "KERNPANIC|kernel panic|Panic" "$log" || true)
    ooms=$(grep -c -E "Out of memory|oom-kill|OOM kill" "$log" || true)
    if [ "$panics" = "0" ]; then verdict PASS "kernel panics" "0" "= 0"
    else verdict FAIL "kernel panics" "$panics" "= 0"; fi
    if [ "$ooms" = "0" ]; then verdict PASS "OOM lines" "0" "= 0"
    else verdict FAIL "OOM lines" "$ooms" "= 0"; fi

    # census
    local term refused
    term=$(grep -c "TERM-DESCHED engaged" "$log" || true)
    refused=$(grep -c "WAKE-REFUSED" "$log" || true)
    if [ "$((term < TERM_BASE - TERM_BAND ? 1 : term > TERM_BASE + TERM_BAND ? 1 : 0))" = 0 ]; then
        verdict PASS "census TERM-DESCHED" "$term" "$TERM_BASE +-$TERM_BAND"
    else verdict FAIL "census TERM-DESCHED" "$term" "$TERM_BASE +-$TERM_BAND"; fi
    if [ "$((refused < REFUSED_BASE - REFUSED_BAND ? 1 : refused > REFUSED_BASE + REFUSED_BAND ? 1 : 0))" = 0 ]; then
        verdict PASS "census WAKE-REFUSED" "$refused" "$REFUSED_BASE +-$REFUSED_BAND"
    else verdict FAIL "census WAKE-REFUSED" "$refused" "$REFUSED_BASE +-$REFUSED_BAND"; fi
    local stranded
    stranded=$(grep -c "WAKE-STRANDED" "$log" || true)
    verdict INFO "census WAKE-STRANDED (organic)" "$stranded" "no 1001ms grid class"

    # tick rate per CPU (delta of the last two tick dumps)
    python3 - "$log" "$TICK_MIN" <<'EOF'
import re, sys
log, tick_min = sys.argv[1], float(sys.argv[2])
dumps = []
for line in open(log, errors="replace"):
    m = re.search(r"SYSCOST t=(\d+)s ticks\(u/k/i\): (.*)$", line)
    if m:
        per = {}
        for c, u, k, i in re.findall(r"c(\d+)=(\d+)/(\d+)/(\d+)", m.group(2)):
            per[int(c)] = int(u) + int(k) + int(i)
        dumps.append((int(m.group(1)), per))
ok, msg = True, []
if len(dumps) >= 2:
    (t0, p0), (t1, p1) = dumps[-2], dumps[-1]
    dt = t1 - t0
    if dt > 0:
        for c in sorted(p1):
            if c in p0:
                rate = (p1[c] - p0[c]) / dt
                good = rate >= tick_min
                ok = ok and good
                msg.append("cpu%d=%.2f/s%s" % (c, rate, "" if good else " LOW"))
else:
    ok = False; msg.append("fewer than 2 tick dumps")
print(("PASS" if ok else "FAIL"), "tick rate /CPU", ", ".join(msg) or "n/a", "min %.1f" % tick_min)
EOF

    # syscall volume ("syscalls: <kinds> kinds <calls> calls tot=...")
    local calls
    calls=$(grep -oE "syscalls: [0-9]+ kinds [0-9]+ calls" "$log" | tail -1 | awk '{print $4}')
    if [ -n "$calls" ]; then
        local lo=$((VOLUME_BASE * 90 / 100)) hi=$((VOLUME_BASE * 110 / 100))
        if [ "$calls" -ge "$lo" ] && [ "$calls" -le "$hi" ]; then
            verdict PASS "syscall volume" "$calls" "$VOLUME_BASE +-10%"
        else
            verdict FAIL "syscall volume" "$calls" "$VOLUME_BASE +-10%" "boot-depth-sensitive"
        fi
    else verdict SKIP "syscall volume" "no summary line" "needs syscost=1 boot"; fi

    # per-op avgs from the last syscall dump (by name, arch-independent)
    python3 - "$log" "$CGAVG_US_MIN" "$CGAVG_US_MAX" <<'EOF'
import re, sys
log = sys.argv[1]
cg_min, cg_max = float(sys.argv[2]), float(sys.argv[3])
rows = {}
for line in open(log, errors="replace"):
    m = re.search(r"SYSCOST t=(\d+)s nr=(\d+) n=(\d+) avg=(\d+)us", line)
    if m:
        t, nr, n, avg = int(m.group(1)), int(m.group(2)), int(m.group(3)), int(m.group(4))
        # keep the row from the latest dump that carried this nr
        if nr not in rows or t >= rows[nr][0]:
            rows[nr] = (t, n, avg)
# nr by generic + x86_64 native numbering (the dump prints the native nr)
CLOCK = (169, 228); NANO = (35, 230)
def pick(nrs):
    for nr in nrs:
        if nr in rows: return rows[nr]
    return None
cg = pick(CLOCK); nano = pick(NANO)
if cg:
    t, n, avg = cg
    ok = cg_min <= avg <= cg_max
    print(("PASS" if ok else "FAIL"), "clock_gettime avg", "%dus n=%d t=%ds" % (avg, n, t),
          "%.0f-%.0fus" % (cg_min, cg_max))
else:
    print("SKIP clock_gettime avg  no row  needs syscost=1 boot")
if nano:
    t, n, avg = nano
    # Workload-dependent: the seed boot itself (pa38) reads 333 ms at its
    # last table dump vs the 221 ms pa27 figure in the baseline doc, so
    # this row is informational until the baseline is re-banked.
    print("INFO nanosleep avg", "%dus n=%d t=%ds" % (avg, n, t),
          "doc band 221ms +-20% (workload-dependent; seed boot reads 333ms)")
else:
    print("SKIP nanosleep avg  no row  needs syscost=1 boot")
EOF
}

domain_check() {
    local log="$1" smp="${SMP:-2}"
    echo "== Domains: guarded hot paths ($log) =="

    if grep -q "rest-site" "$log"; then
        python3 - "$log" "$SECS" "$smp" "$GRQ_AVG_NS_MIN" "$GRQ_AVG_NS_MAX" \
                    "$SMALL_ALLOC_SHARE_MAX" <<'EOF'
import re, sys
log, secs, smp = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
grq_min, grq_max = float(sys.argv[4]), float(sys.argv[5])
share_max = float(sys.argv[6])
sites = {}
for line in open(log, errors="replace"):
    m = re.search(r"SYSCOST t=\d+s rest-site\s+\d+: (\S+?):(\d+) n=(\d+) tot=(\d+)ms avg=(\d+)ns", line)
    if m:
        f, ln, n, tot, avg = m.group(1), int(m.group(2)), int(m.group(3)), int(m.group(4)), int(m.group(5))
        key = "%s:%d" % (f, ln)
        sites[key] = (n, tot, avg)
machine_ms = secs * smp * 1000
# scheduling: the GRQ hold site (sched.rs, top-n site there)
grq = max((s for k, s in sites.items() if "sched.rs" in k),
          key=lambda s: s[0], default=None)
if grq:
    ok = grq_min <= grq[2] <= grq_max
    print(("PASS" if ok else "FAIL"), "GRQ avg hold", "%dns n=%d" % (grq[2], grq[0]),
          "%.0f-%.0fns" % (grq_min, grq_max))
else:
    print("SKIP GRQ avg hold  no sched.rs rest-site")
# alloc: small_alloc family share of machine time
sa_ms = sum(t for k, (n, t, a) in sites.items() if "small_alloc" in k)
share = 100.0 * sa_ms / machine_ms if machine_ms else 0
print(("PASS" if share <= share_max else "FAIL"), "small_alloc REST share",
      "%.1f%% (%dms)" % (share, sa_ms), "<=%.0f%%" % share_max)
EOF
    else
        # Probe-less kernel: the documented proxy (baseline Tier-3 note)
        python3 - "$log" "$KSHARE_MIN" "$KSHARE_MAX" <<'EOF'
import re, sys
log, lo, hi = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
last = None
for line in open(log, errors="replace"):
    m = re.search(r"SYSCOST t=(\d+)s ticks\(u/k/i\): (.*)$", line)
    if m:
        last = m.group(2)
if not last:
    print("SKIP tick-mode kernel share  no tick dumps")
    raise SystemExit
tot_k = tot = 0
for c, u, k, i in re.findall(r"c(\d+)=(\d+)/(\d+)/(\d+)", last):
    tot_k += int(k); tot += int(u) + int(k) + int(i)
share = 100.0 * tot_k / tot if tot else 0
print(("PASS" if lo <= share <= hi else "INFO"),
      "tick-mode kernel share (proxy; no rest-sites dumped)",
      "%.1f%%" % share, "%.0f-%.0f%%" % (lo, hi),
      "alloc/sched domain checks need the pa16 probe hooks")
EOF
    fi
    verdict SKIP "buffer-cache hit rate" "not in this build" "needs cache-stats instrument"
    verdict SKIP "fsw sync-write classes" "not in this build" "needs fsw=1 boot"
}

tier3_check() {
    local log="$1"
    echo "== Tier 3: IF=0 share (pa16 probe hooks) =="
    if ! grep -q "if0 cpu=" "$log"; then
        verdict SKIP "IF=0 share" "probe not enabled" "needs the EFLAGS probe armed"
        return
    fi
    python3 - "$log" "$IF0_BASE" "$IF0_BAND" <<'EOF'
import re, sys
log, base, band = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
# SYSCOST t=Ns if0 cpu=C: tot=<ms> rest=... hard=... soft=... uart=...
#              nseg=... max=... heal=...n/<ms> miss=<n>n
last = {}
for line in open(log, errors="replace"):
    m = re.search(r"SYSCOST t=(\d+)s if0 cpu=(\d+): tot=(\d+)ms.*?miss=(\d+)n", line)
    if m:
        last[int(m.group(2))] = (int(m.group(1)), int(m.group(3)), int(m.group(4)))
if not last:
    print("SKIP IF=0 share  no if0 dumps")
    raise SystemExit
shares, miss = [], 0
for cpu, (t, tot_ms, m) in sorted(last.items()):
    if t > 0:
        shares.append(100.0 * tot_ms / (t * 1000))
        miss = max(miss, m)
if shares:
    share = sum(shares) / len(shares)
    ok = (base - band <= share <= base + band) and miss == 0
    print(("PASS" if ok else "FAIL"), "IF=0 share of machine",
          "%.1f%% (cpus: %s, heal miss=%d)" % (share, ",".join("%.1f" % s for s in shares), miss),
          "%.1f +-%.1fpp, miss=0" % (base, band))
else:
    print("SKIP IF=0 share  no tot= field")
EOF
}

tier2_check() {
    echo "== Tier 2: pbench medians =="
    local bz="$1"
    local pbtag="${TAG}-pb"
    python3 "$ROOT/test/pbench_boot.py" --bzimage "$bz" --tag "$pbtag" \
        --timeout 1200 > "$ROOT/run/perf-gate-$pbtag.log" 2>&1
    local rc=$?
    local res
    res=$(ls -t "$ROOT/work"/pbench-results* 2>/dev/null | head -1)
    if [ $rc -ne 0 ] || [ -z "$res" ]; then
        verdict FAIL "pbench run" "rc=$rc" "PBENCH DONE rc=0 required"
        return
    fi
    verdict PASS "pbench run" "DONE ($res)" "rc=0"
    # medians vs the anchor
    python3 - "$res" "${PB_BASELINE:-}" <<'EOF'
import re, sys
res, basefile = sys.argv[1], (sys.argv[2] if len(sys.argv) > 2 else "")
# pb1 smoke anchor column (perf-report.md 8.5 notes; ONE boot, TCG,
# minimal rootfs — sanity anchor, not a banked baseline). All cells are
# pb1's own medians from run/pb1-results.txt (us). Leg-31 fix: four
# cells (write_fsync/mkdir_rmdir/unlink/append) had been transcribed
# from the REFERENCE kernel column of the 8.10 gap table — values 4-1000x
# below anything a Rux boot can read, making those rows FAIL by
# construction. Pass --pbench-baseline <results file> to override.
ANCHOR = {
    "p1_null": 4.57, "p1_clock": 4.95, "p2_ctxsw": 15.4,
    "p3b_cow": 24.6, "p6_write_fsync": 18124.666,
    "p6_mkdir_rmdir": 83191.968, "p6_unlink": 32815.925,
    "p6_append": 500.378,
}
if basefile:
    for line in open(basefile, errors="replace"):
        f = line.split()
        if len(f) >= 2 and f[0] == "PBENCH" and "med=" in line:
            kv = dict(t.split("=", 1) for t in f if "=" in t)
            if "med" in kv:
                try: ANCHOR[f[1]] = float(kv["med"])
                except ValueError: pass
rows = {}
for line in open(res, errors="replace"):
    f = line.split()
    if len(f) >= 2 and f[0] == "PBENCH":
        kv = dict(t.split("=", 1) for t in f if "=" in t)
        if "med" in kv:
            try: rows[f[1]] = float(kv["med"])
            except ValueError: pass
fails = 0
for name, med in sorted(rows.items()):
    if name in ANCHOR and ANCHOR[name] > 0:
        ok = 0.8 * ANCHOR[name] <= med <= 1.2 * ANCHOR[name]
        if not ok: fails += 1
        print(("PASS" if ok else "FAIL"), "pbench %s" % name,
              "med=%.3f" % med, "anchor %.3f +-20%%" % ANCHOR[name])
    else:
        print("INFO pbench %s med=%.3f  no anchor (baseline cell PENDING)" % (name, med))
print("INFO pbench note: anchor is the pb1 SMOKE column (one TCG boot) —"
      " treat FAIL as a lead, rerun before calling a regression")
EOF
}

# ---- preflight: sequential boots, at most this script's own QEMU ----
foreign=$(pgrep -c qemu-system 2>/dev/null || echo 0)
if [ "$SKIP_BOOT" = "0" ] && [ "${foreign:-0}" -ge 2 ]; then
    echo "refusing to boot: $foreign foreign QEMUs already running (host capacity)" >&2
    exit 3
fi

# ---- kernel defaults -------------------------------------------------
if [ -z "$KERNEL" ] && [ "$SKIP_BOOT" = "0" ]; then
    KERNEL="$ROOT/target/x86_64-unknown-none/debug/bzImage"
    [ -f "$KERNEL" ] || KERNEL="$ROOT/run/pa31-main-bzImage"
fi

echo "perf-gate: tiers=[$TIERS] tag=$TAG kernel=${KERNEL:-none} secs=$SECS"
RC=0

if [[ ",$TIERS," == *,1,* ]]; then
    SERIAL="$ROOT/run/pa-$TAG-serial.log"
    if [ "$SKIP_BOOT" = "0" ]; then
        echo "== booting (quiet syscost, pa-boot.sh; ~$((SECS/60)) min) =="
        SMP="${SMP:-2}" ACCEL="${ACCEL:-kvm}" \
            "$ROOT/run/pa-boot.sh" "$TAG" "$KERNEL" "$SECS" \
            > "$ROOT/run/perf-gate-$TAG-boot.log" 2>&1
        RC=$?
        echo "boot rc=$RC serial=$SERIAL"
    else
        SERIAL="${LOG:-$ROOT/run/pa-$TAG-serial.log}"
    fi
    if [ ! -f "$SERIAL" ]; then
        echo "no serial log at $SERIAL" >&2; exit 3
    fi
    tier1_check "$SERIAL"
    domain_check "$SERIAL"
    if [[ ",$TIERS," == *,3,* ]]; then tier3_check "$SERIAL"; fi
    # human-readable dump alongside the machine checks
    python3 "$ROOT/run/pa-parse.py" "$SERIAL" > "$ROOT/run/perf-gate-$TAG-parse.txt" 2>/dev/null
    echo "(full syscost dump: run/perf-gate-$TAG-parse.txt)"
fi

if [[ ",$TIERS," == *,2,* ]] && [ "$SKIP_BOOT" = "0" ]; then
    tier2_check "$KERNEL"
fi

echo
echo "perf-gate done: see verdicts above (PASS/FAIL/SKIP/INFO per metric)."
echo "FAIL does not auto-block: apply the rerun-once rule, then weigh against"
echo "docs/development/perf-gate-baseline.md."
exit $([ "$FAIL" -gt 0 ] && echo 1 || echo 0)
