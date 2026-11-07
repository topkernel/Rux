#!/bin/bash
# paint-bisect.sh — unattended paint-regression bisect.
#
# Bisects the x86_64 paint regression (host screendump 99% = good, shell
# up + 0% = bad) with a VALIDITY RULE: a verdict counts only when the
# shell actually started — a boot where the session froze before the
# shell says nothing about painting and is retried/skipped.
#
# Usage: bash run/paint-bisect.sh <worktree> <bad> <good>
set -u
WT="${1:?worktree}"; BAD="${2:?bad}"; GOOD="${3:?good}"
REPO=/home/william/workspace/Rux

cd "$WT" || exit 1
git bisect reset >/dev/null 2>&1
git bisect start "$BAD" "$GOOD" >/dev/null 2>&1

step=0
while :; do
    step=$((step+1))
    [ "$step" -gt 40 ] && { echo "bisect: step budget exhausted"; break; }
    cur=$(git rev-parse HEAD)
    msg=$(git log --oneline -1 "$cur")
    echo "=== step $step: $msg"
    export PATH="$HOME/.cargo/bin:$PATH"
    if ! cargo build --target x86_64-unknown-none --no-default-features --features x86_64 >/tmp/pb-build.log 2>&1; then
        echo "bisect: BUILD FAILED at $cur"; git bisect skip >/dev/null 2>&1; continue
    fi
    python3 build/mkbzimage.py target/x86_64-unknown-none/debug/rux work/bz >/dev/null 2>&1 || {
        echo "bisect: pack failed"; git bisect skip >/dev/null 2>&1; continue; }

    verdict=""
    for attempt in 1 2 3; do
        cd "$REPO"
        python3 scripts/test/gnome-x86/fire.py --tag "pbis" --seconds 95 --smp 8 \
            --accel kvm --bzimage "$WT/work/bz" --dump-every 45 > /tmp/paint-bisect-boot.log 2>&1
        shell=$(grep -ac 'Running GNOME Shell' /tmp/paint-bisect-boot.log)
        nb=$(grep -a nonblack /tmp/paint-bisect-boot.log | tail -1 | grep -oE 'nonblack=[0-9]+' | cut -d= -f2)
        nb=${nb:-0}
        echo "    attempt=$attempt shell=$shell nonblack=$nb"
        if [ "$shell" -gt 0 ]; then
            if [ "$nb" -gt 100000 ]; then verdict=good; else verdict=bad; fi
            break
        fi
    done
    cd "$WT"
    case "$verdict" in
        good) echo "    -> GOOD"; git bisect good >/dev/null 2>&1 ;;
        bad)  echo "    -> BAD";  git bisect bad  >/dev/null 2>&1 ;;
        *)    echo "    -> inconclusive (shell never started 3x): skipping"; git bisect skip >/dev/null 2>&1 ;;
    esac
    # git bisect is done when it prints the culprit
    if git bisect log 2>/dev/null | grep -q "is the first bad commit"; then
        echo "=== CULPRIT: $(git bisect log | grep 'first bad commit' | head -1)"
        git bisect log > /tmp/paint-bisect-log.txt
        break
    fi
done
echo "bisect finished; log: /tmp/paint-bisect-log.txt"
