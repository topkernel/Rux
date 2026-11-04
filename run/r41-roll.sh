#!/bin/bash
# r41-roll.sh — 41st-campaign continuous boot roller (2026-11-05).
# Boots system-lb5 + userdata-fresh2 with the latest main kernel,
# one VM at a time, 900s window, dfx=ams46,txnhunt,fuxstuck unixrd=1 syscost=1.
# Per boot: pixel hunt (md5 vs frozen set; foreign -> PNG + alert file),
# metrics via r41-analyze.py, ledger row in run/r41/ledger.csv.
# Every 5 boots: fetch origin/main; if new commits -> merge (no-edit),
# rebuild x86_64, roll the new kernel.
# Only PIDs/files matching the unique tag prefix r41v are ever killed.
cd /home/william/rux-agents/oh-roll || exit 1
MAX=${1:-100}
START=${3:-1}
KERNEL=${2:-run/r41/bzImage-current}
FROZEN="82f0a25fd7feac19dd266da52b6b9afc 06b933b49b493ea86c5719c597ebedd9"
mkdir -p run/r41
LEDGER=run/r41/ledger.csv
[ -f $LEDGER ] || echo "boot,kernel_sha,sa_ceiling,sa_inserts,sa_last_t,ladder,fux_lines,fux_tids,fux_max_stuck,txn_n,txn_codes,txn_max_exec,bwr_lines,bwr_tids,pixels" >> $LEDGER

KSHA=$(git rev-parse --short HEAD)
echo "[roll] start $(date +%F\ %H:%M:%S) kernel=$KERN SHA=$KSHA" >> run/r41/roller.log

for N in $(seq $START $MAX); do
  TAG="r41v$N"
  SER="run/oh-${TAG}-serial.log"
  # belt+braces: kill any leftover from a previous crash of THIS tag only
  pkill -f "oh-${TAG}-" 2>/dev/null; sleep 1

  echo "[roll] boot $N/$MAX tag=$TAG sha=$KSHA $(date +%H:%M:%S)" >> run/r41/roller.log
  USERDATA=run/userdata-fresh2.img SMP=2 LOGLEVEL=7 \
    python3 run/lb-boot.py "$TAG" 900 run/oh-img/system-lb5.img "$KERNEL" \
    " dfx=ams46,txnhunt,fuxstuck unixrd=1 syscost=1" \
    > "run/r41/$TAG-driver.out" 2>&1 &
  DRV=$!

  # live pixel hunt: check each shot as it lands
  CHECKED=""
  while kill -0 "$DRV" 2>/dev/null; do
    sleep 20
    for PPM in run/oh-${TAG}-shot-*.ppm; do
      [ -f "$PPM" ] || continue
      KEY=$(basename "$PPM")
      case " $CHECKED " in *" $KEY "*) continue;; esac
      CHECKED="$CHECKED $KEY"
      M=$(md5sum "$PPM" | cut -d' ' -f1)
      FOUND=0
      for FZ in $FROZEN; do [ "$M" = "$FZ" ] && FOUND=1; done
      if [ $FOUND -eq 0 ]; then
        PNG="${PPM%.ppm}-$M.png"
        python3 run/r41-ppm2png.py "$PPM" "$PNG" >> run/r41/PIXEL-ALERT-$TAG.txt 2>&1
        echo "PIXEL CHANGE: $PPM md5=$M (not in frozen set) $(date +%F\ %H:%M:%S)" >> run/r41/PIXEL-ALERT-$TAG.txt
        cp "$PPM" "run/r41/PIXEL-$TAG-$M.ppm"
        echo "[roll] *** PIXEL CHANGE $TAG $PPM md5=$M ***" >> run/r41/roller.log
      fi
    done
  done
  wait $DRV 2>/dev/null

  # post-boot analysis
  python3 run/r41-analyze.py "$TAG" > "run/r41/$TAG-summary.txt" 2>&1
  SA=$(grep -m1 '^SA:' "run/r41/$TAG-summary.txt" | sed 's/^SA: //')
  LAD=$(grep -m1 '^ladder:' "run/r41/$TAG-summary.txt" | sed 's/^ladder: //')
  FUX=$(grep -m1 '^FUX-STUCK:' "run/r41/$TAG-summary.txt" | sed 's/^FUX-STUCK: lines=//;s/ distinct_tids=/,/;s/ max_stuck_total=/,/')
  TXN=$(grep -m1 '^TXN-HANG:' "run/r41/$TAG-summary.txt" | sed 's/^TXN-HANG: n=//;s/ codes=/,/;s/ max_exec=/,/;s/ ms.*//')
  BWR=$(grep -m1 '^BWR-SNAP:' "run/r41/$TAG-summary.txt" | sed 's/^BWR-SNAP: lines=/,/;s/ distinct_tids=/,/')
  SHOTMD5=$(md5sum run/oh-${TAG}-shot-*.ppm 2>/dev/null | cut -c1-8 | sort -u | tr '\n' ' ')
  SAC=$(echo "$SA" | sed 's/ceiling=\([0-9]*\).*/\1/')
  SAI=$(echo "$SA" | sed 's/.*inserts=\([0-9]*\).*/\1/')
  SAT=$(echo "$SA" | sed 's/.*last_insert_t=\([0-9.]*\)s/\1/')
  FL=$(echo "$FUX" | cut -d, -f1); FT=$(echo "$FUX" | cut -d, -f2); FS=$(echo "$FUX" | cut -d, -f3)
  TN=$(echo "$TXN" | cut -d, -f1); TC=$(echo "$TXN" | cut -d, -f2 | tr ' ' ';'); TE=$(echo "$TXN" | cut -d, -f3)
  BL=$(echo "$BWR" | cut -d, -f1); BT=$(echo "$BWR" | cut -d, -f2)
  echo "$TAG,$KSHA,$SAC,$SAI,$SAT,\"${LAD//,/;}\",$FL,$FT,$FS,$TN,\"${TC//,/;}\",$TE,$BL,$BT,\"$SHOTMD5\"" >> $LEDGER
  echo "[roll] $TAG done: SA=$SAC ladder='${LAD:0:60}' pixels=$SHOTMD5 $(date +%H:%M:%S)" >> run/r41/roller.log
  pkill -f "oh-${TAG}-" 2>/dev/null
  sleep 5

  # every 5 boots: sync origin/main, rebuild if new
  if [ $((N % 5)) -eq 0 ]; then
    echo "[roll] sync check $(date +%H:%M:%S)" >> run/r41/roller.log
    if git fetch origin >> run/r41/sync.log 2>&1; then
      NEW=$(git rev-parse --short origin/main)
      if [ "$NEW" != "$KSHA" ]; then
        echo "[roll] origin/main moved $KSHA -> $NEW, merging+rebuilding" >> run/r41/roller.log
        if git merge --no-edit origin/main >> run/r41/sync.log 2>&1; then
          rm -f build/.config
          if PLATFORM=x86_64 make build >> run/r41/sync.log 2>&1; then
            KSHA=$(git rev-parse --short HEAD)
            cp target/x86_64-unknown-none/debug/bzImage "run/r41/bzImage-$KSHA"
            KERNEL="run/r41/bzImage-$KSHA"
            echo "[roll] new kernel live: $KERNEL ($KSHA)" >> run/r41/roller.log
          else
            echo "[roll] REBUILD FAILED, staying on $KERNEL" >> run/r41/roller.log
          fi
        else
          echo "[roll] MERGE CONFLICT, staying on $KERNEL (agent must reconcile)" >> run/r41/roller.log
          git merge --abort 2>/dev/null
        fi
      fi
    fi
  fi
done
echo "[roll] exhausted $MAX boots $(date +%H:%M:%S)" >> run/r41/roller.log
