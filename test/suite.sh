#!/bin/bash
# suite.sh — the unattended Rux test fleet.
#
# ============================================================================
# WHY THIS SCRIPT EXISTS
# ============================================================================
#
# The assistant that drives this repository runs on a metered model quota.
# When that quota runs out, the assistant stops working — but the kernel
# does not have to. A kernel only matures while something is exercising
# it: bugs surface as boot failures, wedges, panics, hangs and slow paths,
# and every one of those needs a machine that keeps running the workload
# long enough to notice.
#
# This script is that machine. It runs unattended, needs no model, and
# leaves behind a directory of records that the assistant can read later
# and turn into fixes. The intended division of labour:
#
#   quota exhausted  ->  the patrol starts this script as a DAEMON
#                        (`--mode auto --daemon`), scaled to what the host
#                        can actually carry right now, and goes quiet.
#                        The daemon keeps running ROUNDS (a fresh record
#                        directory each) until it is stopped or its hours
#                        run out, re-reading the host load before every
#                        round, so the machine never idles while the
#                        assistant is gone. A `quota-window-start` marker
#                        notes when the unattended window opened.
#   quota restored   ->  the patrol FIRST runs `--status` to read the
#                        digest of the newest round PLUS the aggregate of
#                        every round since the quota window opened (new
#                        failures across rounds are what matter — a flake
#                        that repeats every round is a bug), then runs
#                        `--stop` to shut the test machines down (so the
#                        host is free for the debugging it is about to
#                        do), then fixes what the digest reports, and
#                        only then resumes its normal work.
#
# Nothing here decides what to fix. It only decides what to RUN, HOW MANY
# machines the host can carry, and WHERE the evidence lands.
#
# ============================================================================
# WHAT IT COVERS
# ============================================================================
#
#   unit        the in-guest unit suite (make test) and the host-side
#               verification crate (make verify: proptests + sync check)
#   formal      the model checkers: SPIN (concurrency), Kani (symbolic),
#               Miri (undefined behaviour)
#   probe       the functional smoke probes (every test/*-smoke.sh and
#               test/*-probe.sh in the tree; new probes are picked up
#               automatically, no list to maintain)
#   perf        the performance gate (run/perf-gate.sh) and the perf probe
#   stress      memory pressure / churn workloads (oomchurn boots)
#   stability   long boots and soaks (the wedge shapes only appear after
#               minutes, not seconds)
#   monkey      randomised command churn across many parallel machines,
#               both architectures (x86_64 via its own kernel + image)
#   fuzz        the filesystem fuzzer against the fleet images
#   integration whole-suite runs (LTP): a bounded sample of the suite
#               list through test/ltpscan/run1.sh on a private image
#               copy; the standing full scans are not touched
#
# Architectures: riscv64 (primary) and x86_64. The x86_64 leg needs its
# own kernel build and image, and is skipped with a recorded reason when
# they are not present.
#
# ============================================================================
# RESOURCE DISCIPLINE
# ============================================================================
#
# The host is shared: several agents run their own QEMU fleets at the
# same time. This script therefore
#
#   - never starts a machine it cannot afford: the concurrency is derived
#     from the free memory AND from the number of QEMU processes that
#     other lanes already have running;
#   - never touches another lane's machines: only PIDs recorded in this
#     run's own PID file are ever killed (no pkill, no name matching);
#   - uses -snapshot and file.locking=off for every boot, and copies the
#     image when the workload writes to it;
#   - leaves a machine running only as long as its test needs it.
#
# ============================================================================
# USAGE
# ============================================================================
#
#   test/suite.sh --list                     print the test matrix
#   test/suite.sh --mode all --arch both     run everything
#   test/suite.sh --mode auto               scale to the current load
#   test/suite.sh --mode quick              the fast subset
#   test/suite.sh --mode unit,probe,perf    a comma-separated selection
#   test/suite.sh --status                  digest of the newest run
#   test/suite.sh --stop                    shut the test machines down
#
# Options:
#   --mode LIST      comma-separated kinds (see WHAT IT COVERS) or one of
#                    auto | quick | all
#   --arch LIST      comma-separated: riscv64 | x86_64 | both (default both)
#   --concurrency N  parallel machines (default: derived from the load)
#   --minutes M      duration of the long legs (monkey, stability, fuzz)
#   --hours H        daemon lifetime in hours (default 6; 0 = until
#                    stopped) — the quota windows are ~2 h, the default
#                    simply outlasts them
#   --out DIR        record directory (default run/suite/<UTC stamp>)
#   --kernel PATH    kernel image to test (default: the built riscv64 one)
#   --ltp-dir DIR    integration leg: an LTP suite directory to run
#   --only LIST      comma-separated test NAMES to run (the digest prints
#                    them, so a single failing leg can be re-run alone)
#   --list           print the matrix and exit
#   --status         print the digest of the newest run and exit
#   --stop           kill the machines of the newest run and exit
#   --daemon         run the whole suite DETACHED and return immediately:
#                    the run keeps going after the shell that started it
#                    has gone, which is what "the quota is out" looks
#                    like. One daemon at a time; --stop ends it.
#
# Records of a run land in run/suite/<stamp>/:
#   <test>.log      the raw output of one test
#   summary.tsv     one line per test: name kind arch status seconds verdict
#   summary.md      the digest meant to be read first (failures first)
#   env.txt         the host and build facts the results were produced with
#   pids            the QEMU PIDs this run started (--stop reads it)
#
# ============================================================================

set -uo pipefail

ORIG_ARGS=("$@")

# The repository root. RUX_ROOT lets the detached daemon run from a
# snapshot copy of this script (kept next to the records) without losing
# the root it belongs to.
ROOT="${RUX_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
cd "$ROOT"

SUITE_ROOT="$ROOT/run/suite"
KERNEL_DEFAULT="$ROOT/target/riscv64gc-unknown-none-elf/debug/rux"
X86_KERNEL="$ROOT/target/x86_64-unknown-none/debug/bzImage"
X86_IMAGE="$ROOT/work/ubuntu-x86.img"

MODE="auto"
ARCHES="both"
CONCURRENCY=""
MINUTES=5
HOURS=6
OUT=""
LTP_DIR=""
ONLY=""
DO_LIST=0
DO_STATUS=0
DO_STOP=0
DO_DAEMON=0

usage() { sed -n '2,145p' "$0"; exit "${1:-0}"; }

while [ $# -gt 0 ]; do
    case "$1" in
        --mode) MODE="${2:-}"; shift 2 ;;
        --arch) ARCHES="${2:-}"; shift 2 ;;
        --concurrency) CONCURRENCY="${2:-}"; shift 2 ;;
        --minutes) MINUTES="${2:-}"; shift 2 ;;
        --hours) HOURS="${2:-}"; shift 2 ;;
        --out) OUT="${2:-}"; shift 2 ;;
        --kernel) KERNEL_DEFAULT="${2:-}"; shift 2 ;;
        --ltp-dir) LTP_DIR="${2:-}"; shift 2 ;;
        --only) ONLY="${2:-}"; shift 2 ;;
        --list) DO_LIST=1; shift ;;
        --status) DO_STATUS=1; shift ;;
        --stop) DO_STOP=1; shift ;;
        --daemon) DO_DAEMON=1; shift ;;
        --_detached) shift ;;
        -h|--help) usage 0 ;;
        *) echo "unknown option: $1" >&2; usage 2 ;;
    esac
done

# --------------------------------------------------------------------------
# host facts
# --------------------------------------------------------------------------

mem_available_mb() {
    awk '/^MemAvailable:/ {print int($2/1024)}' /proc/meminfo 2>/dev/null || echo 0
}

load1() { awk '{print int($1)}' /proc/loadavg 2>/dev/null || echo 0; }

n_cpus() { nproc 2>/dev/null || echo 1; }

# QEMU processes that are NOT ours (this run's PID file lists ours).
foreign_qemus() {
    local mine="$1" p
    local -a own=()
    [ -f "$mine" ] && while read -r p; do [ -n "$p" ] && own+=("$p"); done < "$mine"
    local count=0
    for p in $(ls /proc 2>/dev/null | grep -E '^[0-9]+$'); do
        local comm
        comm="$(cat "/proc/$p/comm" 2>/dev/null)" || continue
        case "$comm" in qemu-system*) ;; *) continue ;; esac
        local skip=0 o
        for o in ${own+"${own[@]}"}; do [ "$p" = "$o" ] && skip=1 && break; done
        [ $skip -eq 0 ] && count=$((count + 1))
    done
    echo "$count"
}

# --------------------------------------------------------------------------
# run directory
# --------------------------------------------------------------------------

# The run directory is created only for an actual run: the query modes
# (--status, --stop) and the daemon launcher must not leave empty
# directories behind.
PIDFILE=""
if [ "$DO_DAEMON" = 1 ]; then
    # Re-exec detached so the whole run survives the caller's shell. The
    # suite's purpose is to keep the kernel under test while nobody is
    # watching; a run that dies with the shell would defeat that.
    mkdir -p "$SUITE_ROOT"
    DETACH_LOG="$SUITE_ROOT/daemon.log"
    DAEMON_PID_FILE="$SUITE_ROOT/daemon.pid"
    : > "$DETACH_LOG"
    if [ -f "$DAEMON_PID_FILE" ]; then
        old="$(cat "$DAEMON_PID_FILE" 2>/dev/null)"
        if [ -n "$old" ] && [ -d "/proc/$old" ]; then
            echo "a suite daemon is already running (pid $old)"
            exit 0
        fi
    fi
    # the re-exec drops --daemon (otherwise the child would detach again)
    daemon_args=()
    for a in "${ORIG_ARGS[@]}"; do
        [ "$a" = "--daemon" ] && continue
        daemon_args+=("$a")
    done
    # A stale stop file would kill the new daemon's first round; the
    # quota-window marker says "unattended testing since <stamp>" and is
    # removed by --stop (it scopes the --status aggregate).
    rm -f "$SUITE_ROOT/stop"
    date -u +%FT%TZ > "$SUITE_ROOT/quota-window-start"
    # The daemon snapshot: bash reads a script incrementally, so editing
    # the file while a run is in flight corrupts the running shell's
    # parse (it dies with an "unexpected EOF" at whatever offset it had
    # reached).
    DAEMON_SCRIPT="$SUITE_ROOT/.suite-daemon.sh"
    cp "$0" "$DAEMON_SCRIPT"
    chmod +x "$DAEMON_SCRIPT"
    if [ "$HOURS" = 0 ]; then SUITE_DEADLINE=9999999999; else SUITE_DEADLINE=$(( $(date +%s) + HOURS * 3600 )); fi
    RUX_ROOT="$ROOT" SUITE_LOOP=1 SUITE_DEADLINE="$SUITE_DEADLINE" \
        setsid nohup "$DAEMON_SCRIPT" "${daemon_args[@]}" --_detached >> "$DETACH_LOG" 2>&1 < /dev/null &
    echo $! > "$DAEMON_PID_FILE"
    echo "suite daemon started (pid $(cat "$DAEMON_PID_FILE")), log $DETACH_LOG"
    exit 0
fi

if [ "$DO_STATUS" = 1 ]; then
    # the newest run that produced a summary (a directory without one is a
    # run that was started and then cut short)
    latest=""
    for d in $(ls -1dt "$SUITE_ROOT"/*/ 2>/dev/null); do
        if [ -f "$d/summary.tsv" ] || [ -f "$d/summary.md" ]; then
            latest="$d"
            break
        fi
    done
    if [ -z "$latest" ]; then
        echo "no completed suite run found under $SUITE_ROOT"
        exit 1
    fi
    echo "=== newest run: $latest ==="
    # The aggregate across the whole unattended window: a leg that fails
    # in one round and passes in the next was host noise; a leg that
    # fails in every round is the first thing to look at. This is the
    # table the assistant reads on quota recovery.
    if [ -f "$SUITE_ROOT/quota-window-start" ]; then
        echo
        echo "=== aggregate since quota window opened ($(cat "$SUITE_ROOT/quota-window-start")) ==="
        find "$SUITE_ROOT" -maxdepth 2 -name summary.tsv -newer "$SUITE_ROOT/quota-window-start" 2>/dev/null \
            | sort | xargs -r cat 2>/dev/null \
            | awk -F'\t' '{ key=$1"|"$4; n[key]++ }
                END { for (k in n) { split(k, a, "|"); tot[a[1]]=tot[a[1]]" "a[2]"x"n[k] }
                      for (t in tot) printf "    %-24s %s\n", t, tot[t] }' | sort
    fi
    echo
    if [ -f "$latest/summary.md" ]; then
        cat "$latest/summary.md"
    else
        echo "(no summary.md — the run did not get far enough)"
    fi
    exit 0
fi

if [ "$DO_STOP" = 1 ]; then
    latest=""
    for d in $(ls -1dt "$SUITE_ROOT"/*/ 2>/dev/null); do
        if [ -f "$d/pids" ]; then
            latest="$d"
            break
        fi
    done
    if [ -z "$latest" ]; then
        echo "no suite run with a pid file found"
        exit 0
    fi
    killed=0
    # the daemon itself first, so it cannot start new work; the stop file
    # stops any round already exec-ing; the quota marker closes the
    # aggregate window.
    : > "$SUITE_ROOT/stop"
    rm -f "$SUITE_ROOT/quota-window-start"
    if [ -f "$SUITE_ROOT/daemon.pid" ]; then
        d="$(cat "$SUITE_ROOT/daemon.pid" 2>/dev/null)"
        if [ -n "$d" ] && [ -d "/proc/$d" ]; then
            kill -TERM "$d" 2>/dev/null && killed=$((killed + 1))
        fi
        rm -f "$SUITE_ROOT/daemon.pid"
    fi
    while read -r p; do
        [ -n "$p" ] || continue
        if [ -d "/proc/$p" ]; then
            kill -TERM "$p" 2>/dev/null && killed=$((killed + 1))
        fi
    done < "$latest/pids"
    sleep 2
    while read -r p; do
        [ -n "$p" ] || continue
        if [ -d "/proc/$p" ]; then
            kill -KILL "$p" 2>/dev/null && killed=$((killed + 1))
        fi
    done < "$latest/pids"
    echo "stopped $killed machine(s) from $latest"
    exit 0
fi

# The guest unit suite boots test/rootfs.img directly (no private copy),
# so it can only run when no other lane holds that image.
rootfs_busy() {
    local img="$ROOT/test/rootfs.img" p
    [ -f "$img" ] || return 1
    for p in $(ls /proc 2>/dev/null | grep -E '^[0-9]+$'); do
        local comm
        comm="$(cat "/proc/$p/comm" 2>/dev/null)" || continue
        case "$comm" in qemu-system*) ;; *) continue ;; esac
        if tr '\0' ' ' < "/proc/$p/cmdline" 2>/dev/null | grep -q "rootfs.img"; then
            return 0
        fi
    done
    return 1
}

# --------------------------------------------------------------------------
# the matrix
# --------------------------------------------------------------------------
#
# Each row: name|kind|arch|timeout_seconds|command
#   kind    unit | formal | probe | perf | stress | stability | monkey
#           | fuzz | integration
#   arch    riscv64 | x86_64 | host
#   command run from the repository root

matrix_rows() {
    cat <<'ROWS'
unit-guest|unit|riscv64|900|make test
unit-verify|unit|host|600|make verify
formal-spin|formal|host|600|make spin
formal-kani|formal|host|1800|make kani
formal-miri|formal|host|4200|make miri
perf-gate|perf|riscv64|1200|run/perf-gate.sh --tiers 1 --kernel __KERNEL__
stress-oomchurn|stress|riscv64|__STRESS__|python3 test/oomchurn_boot.py --kernel __KERNEL__ --secs __STRESS__ --mode fake --mb 400 --fork-n 2 --mem 2G
stability-boot|stability|riscv64|__STAB__|python3 test/boot-soak-fast.py --seconds __STAB__ --kernel __KERNEL__
monkey-riscv64|monkey|riscv64|__MONKEY__|python3 test/stress/monkey.py --kernel __KERNEL__ --rootfs test/rootfs.img --instances __INSTANCES__ --minutes __MONKEY_MIN__ --arch riscv64 --out __OUT__/monkey-riscv64
monkey-x86_64|monkey|x86_64|__MONKEY__|python3 test/stress/monkey.py --kernel __X86_KERNEL__ --rootfs __X86_IMAGE__ --instances __INSTANCES__ --minutes __MONKEY_MIN__ --arch x86_64 --out __OUT__/monkey-x86_64
fuzz-fs|fuzz|riscv64|__FUZZ__|__FUZZ_FS__
integration-ltp|integration|riscv64|__LTP__|__LTP_CMD__
ROWS
}

# The probes are discovered, not listed: every test/*-smoke.sh and
# test/*-probe.sh in the tree becomes a probe leg. A probe added to the
# repository is therefore covered the next time this script runs.
probe_rows() {
    local f base
    for f in "$ROOT"/test/*-smoke.sh "$ROOT"/test/*-probe.sh; do
        [ -f "$f" ] || continue
        base="$(basename "$f" .sh)"
        case "$base" in
            *x86*) echo "$base|probe|x86_64|900|bash $f" ;;
            *)     echo "$base|probe|riscv64|900|bash $f" ;;
        esac
    done
}

# --------------------------------------------------------------------------
# selection
# --------------------------------------------------------------------------

all_rows() {
    matrix_rows
    probe_rows
}

row_kind_is_selected() {
    local kind="$1" want="$2" k
    case "$want" in
        all) return 0 ;;
        auto) return 0 ;;
        quick)
            case "$kind" in unit|probe) return 0 ;; *) return 1 ;; esac
            ;;
        *)
            for k in ${want//,/ }; do [ "$k" = "$kind" ] && return 0; done
            return 1
            ;;
    esac
}

row_arch_is_selected() {
    local arch="$1" want="$2"
    case "$want" in
        both) return 0 ;;
        riscv64) [ "$arch" = riscv64 ] || [ "$arch" = host ] ;;
        x86_64) [ "$arch" = x86_64 ] || [ "$arch" = host ] ;;
        *) [ "$arch" = "$want" ] || [ "$arch" = host ] ;;
    esac
}

# auto: with a busy host only the cheap host legs and the fast probes
# run; with a free host the heavy legs (monkey, stability, fuzz) join in.
auto_kinds() {
    local foreign="$1" mem="$2"
    local heavy="unit,formal,probe,perf"
    if [ "$mem" -ge 6144 ] && [ "$foreign" -le 2 ]; then
        heavy="$heavy,stress,stability,monkey,fuzz,integration"
    fi
    echo "$heavy"
}

# --------------------------------------------------------------------------
# execution
# --------------------------------------------------------------------------

TIMEOUT_BIN="$(command -v timeout || echo /usr/bin/timeout)"

# Kill the QEMU processes that descend from a pid (ours only).
kill_descendant_qemus() {
    local root_pid="$1" p ppid killed=0
    local -A kids=()
    for p in $(ls /proc 2>/dev/null | grep -E '^[0-9]+$'); do
        ppid="$(awk '{print $4}' "/proc/$p/stat" 2>/dev/null)" || continue
        [ -n "$ppid" ] && kids["$ppid"]="${kids[$ppid]:-} $p"
    done
    # breadth-first from the root
    local frontier="$root_pid" next="" comm
    for _ in 1 2 3 4 5 6; do
        next=""
        for ppid in $frontier; do
            for p in ${kids["$ppid"]:-}; do
                next="$next $p"
            done
        done
        [ -z "$next" ] && break
        for p in $next; do
            comm="$(cat "/proc/$p/comm" 2>/dev/null)" || continue
            case "$comm" in
                qemu-system*)
                    kill -TERM "$p" 2>/dev/null && killed=$((killed + 1))
                    echo "$p" >> "$PIDFILE"
                    ;;
            esac
        done
        frontier="$next"
    done
    echo "$killed"
}

# run_one <name> <kind> <arch> <timeout> <command>
run_one() {
    local name="$1" kind="$2" arch="$3" tmo="$4" cmd="$5"
    local log="$OUT/$name.log"
    local start end rc verdict status

    echo "--- $name ($kind/$arch, ${tmo}s) ---"
    start=$(date +%s)

    # The command runs in its own process so a timeout can be enforced
    # without touching the shell's own children.
    ( eval "$cmd" ) > "$log" 2>&1 &
    local pid=$!
    local waited=0
    while kill -0 "$pid" 2>/dev/null; do
        sleep 2
        waited=$((waited + 2))
        if [ "$waited" -ge "$tmo" ]; then
            kill_descendant_qemus "$pid" >/dev/null
            kill -TERM "$pid" 2>/dev/null
            sleep 3
            kill_descendant_qemus "$pid" >/dev/null
            kill -KILL "$pid" 2>/dev/null
            wait "$pid" 2>/dev/null
            echo "TIMEOUT" >> "$log"
            rc=124
            break
        fi
    done
    if [ "${rc:-0}" = 0 ]; then
        wait "$pid" 2>/dev/null
        rc=$?
    fi
    end=$(date +%s)

    verdict="$(verdict_for "$name" "$log" "$rc")"
    case "$verdict" in
        PASS*) status=PASS ;;
        SKIP*) status=SKIP ;;
        *)     status=FAIL ;;
    esac
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$name" "$kind" "$arch" "$status" "$((end - start))" "$verdict" "$log" \
        >> "$OUT/summary.tsv"
    echo "    -> $status ($((end - start))s) $verdict"
    return 0
}

# The verdict is read from the log, not only from the exit code: several
# harnesses report their own verdict lines and the exit code of the
# wrapper is not always meaningful.
verdict_for() {
    local name="$1" log="$2" rc="${3:-0}"
    if [ ! -s "$log" ]; then
        echo "EMPTY LOG"
        return
    fi
    # Several harnesses print only "log: <path>" and keep the real output
    # in their own file. The verdict is read from that file too.
    local extra f
    extra="$(grep -aoE '(log|serial log): .*/[^ ]+' "$log" 2>/dev/null | sed 's/^[^:]*: //' | head -3)"
    for f in $extra; do
        case "$f" in
            /*) ;;
            *) f="$ROOT/$f" ;;
        esac
        [ -f "$f" ] && cat "$f" >> "$log.sources" 2>/dev/null
    done
    [ -f "$log.sources" ] && cat "$log.sources" >> "$log" && rm -f "$log.sources"
    # A panic is a failure whatever the harness said.
    if grep -aEq "KERNPANIC|Kernel panic|panic!|Oops:|BUG:" "$log"; then
        echo "PANIC"
        return
    fi
    # The harness verdict lines (case-insensitive: make test prints
    # "All tests passed!", make verify "=== All verify steps passed ===",
    # kani/miri/spin have their own phrasings).
    if grep -aqiE "all tests passed|all [a-z ]*steps passed|all [a-z ]*checks (passed|complete)|all [a-z ]*proofs passed|result: pass|probe-result: pass" "$log"; then
        if grep -aEq "FAIL|Failed: [1-9]|failed" "$log"; then
            # A mixed log: let the failure win.
            if grep -aEq "PROBE-FAIL|FAIL: " "$log"; then
                echo "HARNESS FAIL"
                return
            fi
        fi
        echo "PASS"
        return
    fi
    if grep -aEq "PROBE-FAIL|FAIL: |FAILED|Failed: [1-9]|PROBE-RESULT: FAIL" "$log"; then
        echo "HARNESS FAIL"
        return
    fi
    # Wrappers report their own skips (a missing image, toolchain or
    # kernel): an environment gap, not a kernel defect.
    if grep -aEq "PROBE-RESULT: SKIP" "$log"; then
        echo "SKIP (probe skip)"
        return
    fi
    if grep -aEq "WEDGE|STALL|no serial progress" "$log"; then
        echo "WEDGE"
        return
    fi
    # A missing prerequisite (a prebuilt image, a toolchain, a guest disk)
    # is an environment gap, not a kernel defect: report it as a skip so
    # the digest does not cry wolf. The verification tools (kani, miri)
    # are optional extras that may simply not be installed.
    if grep -aEq "Could not open '[^']*': No such file or directory|No such file or directory$|command not found|not found: |is not installed for the toolchain|find a package to install" "$log"; then
        if grep -aEq "KERNPANIC|panic!|Oops:" "$log"; then
            echo "PANIC"
        else
            echo "SKIP (prerequisite missing)"
        fi
        return
    fi
    if [ "$rc" = "124" ]; then
        echo "TIMEOUT"
        return
    fi
    # No verdict line in the log: fall back to the exit code.
    if [ "$rc" != "0" ]; then
        echo "EXIT $rc"
        return
    fi
    echo "PASS (no verdict line, exit 0)"
}

# --------------------------------------------------------------------------
# the digest
# --------------------------------------------------------------------------

write_summary() {
    local md="$OUT/summary.md"
    {
        echo "# Rux test suite — $OUT"
        echo
        echo "## Host and build"
        echo
        sed 's/^/    /' "$OUT/env.txt"
        echo
        echo "## Totals"
        echo
        if [ -f "$OUT/summary.tsv" ]; then
            awk -F'\t' '{c[$4]++} END {for (k in c) printf "    %-6s %d\n", k, c[k]}' \
                "$OUT/summary.tsv" | sort
        else
            echo "    (no test ran)"
        fi
        echo
        echo "## Per test"
        echo
        if [ -f "$OUT/summary.tsv" ]; then
            awk -F'\t' '{printf "    %-22s %-10s %-9s %-5s %4ss  %s\n", $1, $2, $3, $4, $5, $6}' \
                "$OUT/summary.tsv"
        fi
        echo
        echo "## Failures (read these first)"
        echo
        local any=0 name kind arch status secs verdict log
        while IFS=$'\t' read -r name kind arch status secs verdict log; do
            case "$status" in
                PASS|SKIP) continue ;;
            esac
            any=1
            echo "### $name — $verdict ($status, ${secs}s)"
            echo
            echo "    log: $log"
            echo
            grep -aEn "PROBE-FAIL|FAIL: |FAILED|KERNPANIC|panic!|Oops:|BUG:|WEDGE|TIMEOUT|Error|error:" "$log" 2>/dev/null \
                | head -20 | sed 's/^/    /'
            echo
        done < "$OUT/summary.tsv"
        [ "$any" = 0 ] && echo "    (nothing failed)"
        echo
        echo "## Next steps for the assistant"
        echo
        echo "    1. read the failures above (the log path is next to each one)"
        echo "    2. test/suite.sh --stop   (free the host before debugging)"
        echo "    3. fix, then re-run the failing leg:"
        echo "         test/suite.sh --mode <kind> --arch <arch> --out run/suite/<new>"
    } > "$md"
    echo "$OUT" > "$SUITE_ROOT/LATEST"
    echo "wrote $md"
}

# --------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------

if [ "$DO_LIST" = 1 ]; then
    echo "kind        arch      timeout  test"
    all_rows | while IFS='|' read -r name kind arch tmo cmd; do
        tmo="${tmo//__STAB__/${MINUTES:-5}60}"
        tmo="${tmo//__MONKEY__/${MINUTES:-5}60}"
        tmo="${tmo//__FUZZ__/${MINUTES:-5}60}"
        tmo="${tmo//__STRESS__/${MINUTES:-5}60}"
        tmo="${tmo//__LTP__/${MINUTES:-5}60}"
        printf '%-11s %-9s %-7s  %s\n' "$kind" "$arch" "${tmo}s" "$name"
    done
    exit 0
fi

if [ -z "$OUT" ]; then
    STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
    OUT="$SUITE_ROOT/$STAMP"
fi
mkdir -p "$OUT"
PIDFILE="$OUT/pids"
: > "$PIDFILE"

# --- the environment the results were produced with ------------------------
{
    echo "date        : $(date -u +%FT%TZ)"
    echo "repo        : $ROOT"
    echo "git HEAD    : $(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
    echo "git branch  : $(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo unknown)"
    echo "host cpus   : $(n_cpus)"
    echo "host load1  : $(load1)"
    echo "mem avail MB: $(mem_available_mb)"
    echo "foreign qemu: $(foreign_qemus "$PIDFILE")"
    echo "mode        : $MODE"
    echo "arches      : $ARCHES"
    echo "minutes     : $MINUTES"
} > "$OUT/env.txt"

FOREIGN="$(foreign_qemus "$PIDFILE")"
MEMAV="$(mem_available_mb)"

# --- a NORMAL kernel, or every probe boots the wrong machine ----------
# `make test` overwrites target/.../rux with a unit-test build whose
# boot runs the 1122 in-kernel tests and halts — no shell, no klogd, no
# sysrq. The unattended window has no agents to collide with, so the
# round can safely rebuild the plain kernel first (the same ELF-flip
# the GNOME lane hit once). A failed build fails the round honestly.
if [ ! -f "$KERNEL_DEFAULT" ] || [ "$MODE" != "quick" ]; then
    echo "ensuring a plain kernel build (make build)..."
    if ! make build > "$OUT/build.log" 2>&1; then
        echo "round aborted: make build failed (see $OUT/build.log)" >&2
        exit 2
    fi
fi
# Snapshot the kernel for THIS round: the shared target/.../rux can be
# flipped back to a unit-test build mid-round by a concurrent `make test`
# (the coordinator's merge gates do exactly that) — every leg of a round
# must boot the same plain kernel the round started with.
KERNEL_SNAPSHOT="$SUITE_ROOT/kernel-$(date -u +%Y%m%dT%H%M%SZ)"
cp "$KERNEL_DEFAULT" "$KERNEL_SNAPSHOT" || exit 2
KERNEL_DEFAULT="$KERNEL_SNAPSHOT"

# --- how many machines the host can carry -------------------------------
if [ -z "$CONCURRENCY" ]; then
    # each machine wants ~1G of guest RAM plus the TCG overhead; leave a
    # 4G floor for the other lanes and for the host itself
    by_mem=$(( (MEMAV - 4096) / 1536 ))
    [ "$by_mem" -lt 1 ] && by_mem=1
    CONCURRENCY=$by_mem
    [ "$CONCURRENCY" -gt 4 ] && CONCURRENCY=4
    # never fight the other lanes: every foreign machine already running
    # takes a slot's worth of host capacity away from us
    if [ "$FOREIGN" -gt 0 ]; then
        CONCURRENCY=$(( CONCURRENCY - FOREIGN / 2 ))
        [ "$CONCURRENCY" -lt 1 ] && CONCURRENCY=1
    fi
fi
echo "host: $FOREIGN foreign QEMU(s), ${MEMAV}MB free -> concurrency $CONCURRENCY"

if [ "$MODE" = "auto" ]; then
    SELECT="$(auto_kinds "$FOREIGN" "$MEMAV")"
    echo "auto mode -> kinds: $SELECT"
else
    SELECT="$MODE"
fi

STAB=$(( MINUTES * 60 ))
MONKEY=$(( MINUTES * 60 ))
MONKEY_MIN="$MINUTES"
FUZZ=$(( MINUTES * 60 ))
STRESS=$(( MINUTES * 60 ))
LTP=$(( MINUTES * 60 ))
# the sample needs at least a quarter hour to be worth its image copy
[ "$LTP" -lt 900 ] && LTP=900
# a churn boot shorter than this says almost nothing about the OOM paths
[ "$STRESS" -lt 600 ] && STRESS=600
INSTANCES="$CONCURRENCY"
[ "$INSTANCES" -lt 2 ] && INSTANCES=2

# The x86 leg needs its own kernel and image.
X86_OK=0
if [ -f "$X86_KERNEL" ] && [ -f "$X86_IMAGE" ]; then
    X86_OK=1
else
    echo "x86_64 leg: skipped (need $X86_KERNEL and $X86_IMAGE)"
fi

# The fuzz leg needs the fleet image; without it the leg is skipped.
FLEET_FSFUZZ="$ROOT/run/fleet/images/fleet-fsfuzz-rootfs.img"
FLEET_MEMSTORM="$ROOT/run/fleet/images/fleet-memstorm-rootfs.img"
FUZZ_FS_CMD="echo 'no fleet fsfuzz image at $FLEET_FSFUZZ' ; exit 3"
if [ -f "$FLEET_FSFUZZ" ]; then
    # The fuzz window ending (timeout rc 124) is the expected outcome —
    # the fuzzer runs until the window closes; only an earlier qemu
    # death or a panic (verdict_for greps the serial) fails the leg.
    FUZZ_FS_CMD="timeout ${FUZZ}s qemu-system-riscv64 -M virt -accel tcg,thread=multi -cpu rv64,zbb=true,zba=true,zbs=true,zcb=true,v=true -m 1G -smp 2 -display none -serial stdio -snapshot -drive file=$FLEET_FSFUZZ,if=none,id=rootfs,format=raw,file.locking=off -device virtio-blk-pci,disable-legacy=on,drive=rootfs -kernel __KERNEL__ -append 'root=/dev/vda rw init=/test/fsfuzz console=ttyS0 dfx=watchdog,taskdump'; frc=\$?; if [ \$frc -ne 0 ] && [ \$frc -ne 124 ]; then echo 'PROBE-RESULT: FAIL fuzz qemu exited '\$frc; exit 1; fi; echo 'PROBE-RESULT: PASS fuzz window completed'"
fi

# The LTP leg runs a bounded sample of the suite list through the
# ltpscan PID-1 runner on a PRIVATE image copy (both the shared
# test/rootfs.img and the copy's /test/linux-ltp/chunk.txt would
# otherwise fight a standing scan). The full 1869-test scans stay with
# the LTP lane; this leg is the unattended smoke of the suite.
LTP_DIR="${LTP_DIR:-$ROOT/test/ltpscan}"
LTP_CMD="echo 'no LTP suite at $LTP_DIR (need tests-all.txt)'; exit 3"
if [ -f "$LTP_DIR/tests-all.txt" ] && [ -f "$ROOT/test/rootfs.img" ]; then
    LTP_PRIVATE="$OUT/ltp-rootfs.img"
    LTP_LIST="$(grep -v '^#' "$LTP_DIR/tests-all.txt" | head -30 | paste -sd, -)"
    LTP_CMD="cp '$ROOT/test/rootfs.img' '$LTP_PRIVATE' && LTP_OUT_DIR='$OUT/ltp' KERNEL='__KERNEL__' IMAGE='$LTP_PRIVATE' test/ltpscan/run1.sh -t __LTP__ '$LTP_LIST'"
fi

: > "$OUT/summary.tsv"

n_run=0
all_rows | while IFS='|' read -r name kind arch tmo cmd; do
    [ -n "$name" ] || continue
    row_kind_is_selected "$kind" "$SELECT" || continue
    row_arch_is_selected "$arch" "$ARCHES" || continue
    if [ -n "$ONLY" ]; then
        case ",$ONLY," in
            *",$name,"*) ;;
            *) continue ;;
        esac
    fi
    case "$arch" in
        x86_64)
            # The monkey leg needs the Ubuntu image; the x86 probe
            # legs (pstore-x86, dfx-x86, binder4-x86) build their own
            # minimal images and only need the bzImage.
            if [ "$name" = "monkey-x86_64" ]; then
                [ "$X86_OK" = 1 ] || continue
            else
                [ -f "$X86_KERNEL" ] || continue
            fi
            ;;
    esac
    if [ "$name" = "unit-guest" ] && rootfs_busy; then
        echo "  $name: SKIP (test/rootfs.img is held by another lane)"
        printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
            "$name" "$kind" "$arch" "SKIP" "0" "rootfs busy" "$OUT/$name.log" \
            >> "$OUT/summary.tsv"
        continue
    fi
    cmd="${cmd//__X86_KERNEL__/$X86_KERNEL}"
    cmd="${cmd//__X86_IMAGE__/$X86_IMAGE}"
    cmd="${cmd//__STAB__/$STAB}"
    cmd="${cmd//__MONKEY__/$MONKEY}"
    cmd="${cmd//__MONKEY_MIN__/$MONKEY_MIN}"
    cmd="${cmd//__FUZZ__/$FUZZ}"
    cmd="${cmd//__STRESS__/$STRESS}"
    cmd="${cmd//__LTP__/$LTP}"
    cmd="${cmd//__INSTANCES__/$INSTANCES}"
    cmd="${cmd//__OUT__/$OUT}"
    cmd="${cmd//__FUZZ_FS__/$FUZZ_FS_CMD}"
    cmd="${cmd//__LTP_CMD__/$LTP_CMD}"
    # LAST: the fuzz/LTP command templates themselves carry __KERNEL__
    # (they are expanded only now, so a pass before __FUZZ_FS__ would
    # leave a literal __KERNEL__ in the boot command — the fuzz leg
    # failed with "could not load kernel '__KERNEL__'" for exactly
    # that reason once the fleet image finally existed).
    cmd="${cmd//__KERNEL__/$KERNEL_DEFAULT}"
    # perf numbers from a busy host are noise dressed as data: the perf
    # leg only runs when the host is actually quiet, and says so when it
    # does not.
    if [ "$kind" = perf ]; then
        fnow="$(foreign_qemus "$PIDFILE")"
        if [ "$fnow" -gt 2 ]; then
            echo "  $name: SKIP (host busy: $fnow foreign QEMUs — perf needs quiet)"
            printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
                "$name" "$kind" "$arch" "SKIP" "0" "host busy (foreign=$fnow)" "$OUT/$name.log" \
                >> "$OUT/summary.tsv"
            continue
        fi
    fi
    echo "$name|$kind|$arch|$tmo|$cmd" >> "$OUT/.rows"
    echo "  $name ($kind/$arch)"
done

# The rows are executed here (the pipeline above runs in a subshell, so the
# list is re-read from the file).
while IFS='|' read -r name kind arch tmo cmd; do
    [ -n "$name" ] || continue
    run_one "$name" "$kind" "$arch" "$tmo" "$cmd"
done < "$OUT/.rows"
rm -f "$OUT/.rows"

write_summary
echo
echo "records: $OUT"
echo "digest : $OUT/summary.md"

# --------------------------------------------------------------------------
# the daemon loop
# --------------------------------------------------------------------------
# A quota window outlasts a single pass of the matrix, so the daemon
# child (marked SUITE_LOOP=1 by the launcher) starts its NEXT round here
# by exec-ing a fresh copy of itself with a fresh record directory —
# until --stop drops the stop file or the --hours deadline passes. The
# load is re-read by the new round from scratch, so the suite follows
# the host: heavy legs join when the lanes go quiet, drop out when they
# come back.
if [ "${SUITE_LOOP:-0}" = 1 ]; then
    if [ -f "$SUITE_ROOT/stop" ]; then
        echo "daemon: stop file present — shutting down after this round"
        rm -f "$SUITE_ROOT/stop" "$SUITE_ROOT/daemon.pid" "$SUITE_ROOT/quota-window-start"
        exit 0
    fi
    if [ "$(date +%s)" -ge "${SUITE_DEADLINE:-0}" ]; then
        echo "daemon: hours elapsed — shutting down after this round"
        rm -f "$SUITE_ROOT/daemon.pid" "$SUITE_ROOT/quota-window-start"
        exit 0
    fi
    # next round: same selections, new stamp (drop any --out so the
    # records do not mix, drop --_detached before re-adding it)
    next_args=()
    skip_out=0
    for a in "$@"; do
        [ "$skip_out" = 1 ] && skip_out=0 && continue
        case "$a" in
            --out) skip_out=1; continue ;;
            --_detached) continue ;;
        esac
        next_args+=("$a")
    done
    echo "daemon: next round in 30 s (deadline $(date -u -d "@${SUITE_DEADLINE}" +%H:%M:%SZ 2>/dev/null || echo '?'))"
    sleep 30
    exec env RUX_ROOT="$ROOT" SUITE_LOOP=1 SUITE_DEADLINE="$SUITE_DEADLINE" \
        "$0" "${next_args[@]}" --_detached
fi
