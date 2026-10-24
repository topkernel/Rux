#!/bin/sh
# stress-monkey — randomized app-level churn on the OH surface.
# Replaces wwload.sh on the private fleet-monkey-system.img (the init
# job init.wwload.cfg already starts /system/bin/sh /system/bin/wwload.sh
# at post-fs-data). Beats are picked with $RANDOM (mksh); every beat
# prints MONKEY-BEAT so the sweep sees progress. IMAGE-SIDE NOISE to
# exclude per hunt ledger: usb_host DDK crashes, SAMgr 15/30/89s cost
# family, RS userspace read-lock park.
i=0
while :; do
    r=$((RANDOM % 8))
    case $r in
    0) timeout 2 /system/bin/sdc ;;                                  # StorageDaemon exec cycles (proven)
    1) /system/bin/hilog -b $((RANDOM % 5)) >/dev/null 2>&1 ;;       # log level churn (hilog flood driver)
    2) /system/bin/hilog -r >/dev/null 2>&1 ;;                       # log buffer rotate storm
    3) if [ $((i % 15)) -eq 0 ]; then /system/bin/killall -9 appspawn >/dev/null 2>&1; fi ;;  # paced appspawn restart storm
    4) timeout 3 /system/bin/bm dump -a >/dev/null 2>&1 ;;           # bundle manager walk
    5) timeout 3 /system/bin/hidumper -ls >/dev/null 2>&1 ;;         # system ability dump walk
    6) /system/bin/sync ;;                                           # writeback flush under churn
    7) timeout 2 /system/bin/param list >/dev/null 2>&1 ;;           # system param walk
    esac
    echo "MONKEY-BEAT $i r=$r"
    i=$((i + 1))
    sleep 1
done
