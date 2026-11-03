#!/bin/bash
# k10-freeze-hunt-x86.sh — kill10 hunt for BOTH residual faces handed to
# the stress lane by LTP r24 (kill10 lying-slot fix a5dfbe33 now merged):
#   face 1 (common): lost-timer-wake park subset — children never finish,
#     no WAKE-REFUSED prints (kill10 hangs to its 30s SIGTERM TBROK).
#   face 2 (rare): both-CPU interrupt-freeze — serial goes dark, both
#     vCPUs in wfi; the SP-5 r6 TICKARM-RING (this branch) holds the last
#     timer events; this driver dumps it HOST-SIDE via the QEMU monitor
#     the moment serial stalls (nothing in-guest can print).
# Rig: stress lane private image run/rootfs-stress.img (an LTP-rootfs
# copy — kill10 lives at /test/linux-ltp/testcases/bin/), init=/bin/sh.
# Usage: k10-freeze-hunt-x86.sh <tag> <kill10_loops> [budget_secs]
set -u
cd "$(dirname "$0")/../.."

TAG="${1:?tag}"
LOOPS="${2:-6}"
BUDGET="${3:-420}"
IMAGE=run/rootfs-stress.img
KERNEL="${KERNEL:-target/x86_64-unknown-none/debug/bzImage}"
LOG="run/stress-runs/k10-$TAG-serial.log"
MON="run/stress-runs/k10-$TAG.mon"
FIFO="run/stress-runs/k10-$TAG.fifo"
rm -f "$LOG" "$MON" "$FIFO"; mkfifo "$FIFO"

# ---- serial feeder: shell up, then kill10 loop with completion marks ----
(
  sleep 14
  for i in $(seq 1 "$LOOPS"); do
    echo "/test/linux-ltp/testcases/bin/kill10 -d 4 -n 10 -g 2 > /tmp/k10-$i.out 2>&1 </dev/null; echo K10RET-$i=\$? > /dev/console" > "$FIFO"
    sleep 34
    printf 'DUMP!\n' > "$FIFO"
    sleep 6
    printf 'DUMP!\n' > "$FIFO"
    sleep 20
  done
  sleep 30
) &
FEEDER=$!

# ---- freeze watcher: serial stall >10s while QEMU alive -> monitor dump ----
python3 - "$LOG" "$MON" "$BUDGET" <<'PYEOF' &
import os, socket, subprocess, sys, time
log, mon, budget = sys.argv[1], sys.argv[2], int(sys.argv[3])
def monq(*cmds):
    out = ""
    try:
        s = socket.socket(socket.AF_UNIX); s.settimeout(5); s.connect(mon)
        for c in cmds:
            s.sendall((c + "\n").encode()); time.sleep(0.4)
            try: out += s.recv(65536).decode("utf-8", "replace")
            except Exception: pass
        s.close()
    except Exception as e:
        out += f"[monq err {e}]"
    return out
t0 = time.time(); last_size = -1; last_move = time.time(); dumped = 0
while time.time() - t0 < budget:
    time.sleep(2)
    try: size = os.path.getsize(log)
    except FileNotFoundError: continue
    if size != last_size:
        last_size = size; last_move = time.time(); continue
    if time.time() - last_move > 40 and dumped < 3:
        dumped += 1
        ring_phys = None
        with open(log, "rb") as f:
            for ln in f.read().split(b"\n"):
                if b"TICKARM-RING" in ln:
                    for tok in ln.split():
                        if tok.startswith(b"phys="):
                            ring_phys = tok[5:].decode()
                    if ring_phys: break
        stamp = time.strftime("%H%M%S")
        with open(log, "a") as f:
            f.write(f"\n== FREEZE-WATCH stall dump #{dumped} @+{time.time()-t0:.0f}s ==\n")
            f.write(monq("info cpus", "info registers"))
            if ring_phys:
                # 2 CPUs x 16 slots x 6 u64-fields (u32 pairs) = 1536 bytes
                f.write(monq(f"xp/384gx {ring_phys}"))
            f.write("nmi\n")  # does NMI resume it? (k10-rux-nmi1: no)
            time.sleep(2)
            f.write(monq("info cpus"))
            f.write("== FREEZE-WATCH dump end ==\n")
        last_move = time.time()  # the dump lines themselves move the file
print("[freeze-watch done]", file=sys.stderr)
PYEOF
WATCHER=$!

timeout "$BUDGET" qemu-system-x86_64 -M q35 -accel kvm -cpu max \
  -m 2G -smp 2 -display none -serial stdio -monitor unix:"$MON",server,nowait \
  -no-reboot -snapshot \
  -drive file="$IMAGE",if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -kernel "$KERNEL" \
  -append "root=/dev/vda rw init=/bin/sh console=ttyS0 dfx=taskdump,watchdog" \
  < "$FIFO" > "$LOG" 2>&1
kill $FEEDER $WATCHER 2>/dev/null
rm -f "$FIFO"
echo "=== K10 HUNT $TAG verdict input ==="
grep -a 'K10RET-\|TICKARM-RING\|FREEZE-WATCH' "$LOG" | head -20
grep -a -c 'WAKE-REFUSED\|SLOT-LIE-HEALED\|PARKED' "$LOG" || true
