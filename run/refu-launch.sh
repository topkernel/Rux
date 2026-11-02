#!/bin/bash
# refu-launch.sh — clean launcher for reference-kernel OH boots (15th
# successor). Args: TAG SECS [SEED|LB5WARM]. Uses absolute paths; writes
# run/oh-<TAG>-serial.log. Kills only what it starts (driver exits at SECS).
cd /home/william/rux-agents/oh-launcher || exit 1
TAG=$1; SECS=${2:-700}; MODE=${3:-SEED}
S=/home/william/rux-images/oh
SP=/home/william/rux-agents/oh-stockperf/run/oh-img
case "$MODE" in
  SEED)  export SYSIMG=$S/system.img VENDOR=$S/vendor.img CHIP=$S/chip_prod.img USERDATA=$S/userdata.img ;;
  SP)    export SYSIMG=$SP/system.img VENDOR=$SP/vendor.img CHIP=$SP/chip_prod.img USERDATA=/home/william/rux-agents/oh-stockperf/run/userdata-fresh.img ;;
  LB5WARM) export SYSIMG=$PWD/run/system-lb5.img VENDOR=$PWD/run/oh-img/vendor.img CHIP=$PWD/run/oh-img/chip_prod.img USERDATA=$PWD/run/userdata-warm.img ;;
esac
export SMP=${SMP:-2}
setsid python3 run/sp-ref-u.py "$TAG" "$SECS" > "run/${TAG}-driver.out" 2>&1 &
echo "launched $TAG mode=$MODE pid=$!"
