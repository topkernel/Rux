#!/usr/bin/env python3
"""repro-drive.py — batched-read + exit-storm repro driver.

Copies the shared rootfs image (sparse), injects /root/repro.sh via
debugfs, boots the given kernel with -snapshot, waits for the shell
prompt, sends ONE short launch line over serial (avoids the serial-input
overrun that mangles long piped scripts), streams serial to a log, and
reports a verdict (panic markers / round progression).

Usage: repro-drive.py --kernel PATH --tag NAME [--dur 600] [--rounds 6]
Discipline: -snapshot + file.locking=off; kills only its own QEMU child.
"""
import argparse
import os
import re
import select
import shutil
import signal
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
BASE_IMG = "/home/william/Rux/test/rootfs.img"
DEBUGFS = "/usr/sbin/debugfs"
QEMU = "/usr/bin/qemu-system-riscv64"

REPRO_SH = """#!/bin/sh
# v3 load shape: the head_desc-collision corruption family needs
# CONCURRENT batch submitters on MANY DISTINCT inodes with MIXED chain
# sizes, plus fork/exec/exit churn (kernel-stack recycling while batches
# are in flight) — the GNOME-session shape. The 300MB /tmp/rdata file
# exceeds what stays cached between rounds on a 512M guest, so the dd
# window readers do REAL cold batch reads every round; its random
# content is checksummed per round (a miscompleted read shows up as a
# CKSUM-MISMATCH, not just a panic).
echo REPRO-SCRIPT-START
FILES="/bin/toybox /bin/mrsh /lib/ld-musl-riscv64.so.1 /app/vshell /app/calculator /app/desktop /app/clock /test/smoke_test"
ref=""
r=0
while [ $r -lt {rounds} ]; do
  echo ROUND-$r-BEGIN
  # (1) data-integrity probe: fixed 8MB window of /tmp/rdata, cksum.
  c=$(dd if=/tmp/rdata bs=1048576 skip=7 count=8 2>/dev/null | cksum)
  if [ -z "$ref" ]; then ref="$c"; fi
  if [ "$c" != "$ref" ]; then
    echo "CKSUM-MISMATCH r=$r got=$c want=$ref"
    # Transient vs persistent: re-read immediately. A second read matching
    # the reference means the corruption was a one-shot DELIVERY failure
    # (page-cache/virtio race); a persistent mismatch means the data on
    # the (snapshot) disk image itself was damaged — a write-path/alloc
    # aliasing bug.
    c2=$(dd if=/tmp/rdata bs=1048576 skip=7 count=8 2>/dev/null | cksum)
    if [ "$c2" = "$ref" ]; then echo "CKSUM-REREAD-CLEAN (transient delivery)"; else echo "CKSUM-REREAD-STILL-BAD (persistent on-disk)"; fi
    # Narrow to the damaged 1MB chunk (4 pages granularity for forensics).
    m=0
    while [ $m -lt 8 ]; do
      cm=$(dd if=/tmp/rdata bs=1048576 skip=$(( 7 + m )) count=1 2>/dev/null | cksum)
      echo "CHUNK r=$r mb=$m $cm"
      m=$(( m + 1 ))
    done
  fi
  # (2) fresh /tmp churn: 4 x 16MB writes (dirty writeback = SYNC chains
  #     mixing with the async read batches).
  head -c 16777216 /dev/zero > /tmp/f1
  head -c 16777216 /dev/zero > /tmp/f2
  head -c 16777216 /dev/zero > /tmp/f3
  head -c 16777216 /dev/zero > /tmp/f4
  # (3) concurrent cold window readers on /tmp/rdata (rotating windows,
  #     coalesced multi-block async chains, 8 tasks racing).
  w=$(( (r * 8) % 280 ))
  dd if=/tmp/rdata of=/dev/null bs=1048576 skip=$w count=8 &
  dd if=/tmp/rdata of=/dev/null bs=1048576 skip=$(( w + 12 )) count=8 &
  dd if=/tmp/rdata of=/dev/null bs=524288 skip=$(( (w * 2) + 40 )) count=24 &
  dd if=/tmp/rdata of=/dev/null bs=4096 skip=$(( (w * 512) + 70000 )) count=3000 &
  dd if=/tmp/rdata of=/dev/null bs=1048576 skip=$(( w + 60 )) count=4 &
  dd if=/tmp/rdata of=/dev/null bs=262144 skip=$(( (w * 4) + 900 )) count=80 &
  # (4) concurrent whole-file readers on 8 distinct large on-disk inodes.
  for f in $FILES; do cat $f > /dev/null & done
  # (5) fork/exec/exit storm: short-lived processes interleaved with the
  #     batches; execs walk + read ELF/libc pages, exits recycle stacks.
  e=0
  while [ $e -lt 60 ]; do
    /bin/cat /etc/hostname > /dev/null 2>&1
    /bin/head -c 4096 /bin/toybox > /dev/null 2>&1
    e=$(( e + 1 ))
  done &
  e=0
  while [ $e -lt 60 ]; do
    /bin/ls /usr/bin > /dev/null 2>&1
    /bin/head -c 8192 /bin/mrsh > /dev/null 2>&1
    e=$(( e + 1 ))
  done &
  # (6) small-reader churn on the fresh files.
  j=0
  while [ $j -lt 20 ]; do head -c 65536 /tmp/f2 > /dev/null & j=$(( j + 1 )); done
  wait
  rm -f /tmp/f1 /tmp/f2 /tmp/f3 /tmp/f4
  echo ROUND-$r-END
  r=$(( r + 1 ))
done
echo REPRO-SCRIPT-ALL-DONE
"""

PANIC_MARKERS = [
    b"KERNPANIC", b"WAKE-WILD-PTR", b"panicked", b"Kernel panic",
    b"soft lockup", b"SOFT-LOCKUP", b"ALLOCTHROW", b"trap: stack:",
    b"lost completion", b"buffer io wait timeout", b"dead bh in chain",
    b"CKSUM-MISMATCH",
]

# Diagnostics that indicate the completion mis-mapping / stranding paths
# (informational: "abandoned"/"leaked timed-out" fire only on 10s I/O
# stalls — rare; "Failed to reserve descriptor window" is window-
# reservation backpressure, expected absent or rare on a healthy walker).
INFO_MARKERS = [
    b"recovered by final kick", b"abandoned",
    b"leaked timed-out", b"leaked failed",
    b"Failed to reserve descriptor window", b"VirtIO request timeout",
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kernel", required=True)
    ap.add_argument("--tag", required=True)
    ap.add_argument("--dur", type=int, default=600)
    ap.add_argument("--rounds", type=int, default=6)
    ap.add_argument("--smp", type=int, default=4)
    ap.add_argument("--img", default=None,
                        help="override work image path (default work/repro.img)")
    ap.add_argument("--no-snapshot", action="store_true",
                        help="write guest changes into the (private) work "
                             "image instead of a discard overlay — lets the "
                             "host inspect damaged blocks post-run")
    ap.add_argument("--mem", default="512M",
                        help="guest RAM; small enough that the 300MB "
                             "rdata file cannot stay fully cached between "
                             "rounds (real cold reads every round)")
    args = ap.parse_args()

    out = os.path.join(HERE, "runs", args.tag)
    os.makedirs(out, exist_ok=True)
    work_img = args.img or os.path.join(HERE, "work", "repro.img")
    rdata_bin = os.path.join(HERE, "work", "rdata.bin")
    os.makedirs(os.path.dirname(work_img), exist_ok=True)

    if not os.path.exists(work_img) or os.path.getmtime(work_img) < os.path.getmtime(BASE_IMG):
        print("[prep] sparse copy of rootfs image")
        shutil.copyfile(BASE_IMG, work_img)

    # 300MB of random data inside the image: cold-read fuel + checksum
    # payload (zeros cannot detect a miscompleted read delivering the
    # wrong page). Injected once; the work image is reused across runs.
    def have_rdata():
        r = subprocess.run([DEBUGFS, "-R", "stat /tmp/rdata", work_img],
                           capture_output=True, text=True)
        m = re.search(r"Project:\s+\d+\s+Size:\s+(\d+)", r.stdout)
        return m and int(m.group(1)) == 300 * 1024 * 1024

    if not have_rdata():
        if not os.path.exists(rdata_bin) or os.path.getsize(rdata_bin) != 300 * 1024 * 1024:
            print("[prep] generating 300MB random payload ...")
            with open(rdata_bin, "wb") as f:
                left = 300 * 1024 * 1024
                while left > 0:
                    n = min(left, 8 * 1024 * 1024)
                    f.write(os.urandom(n))
                    left -= n
        subprocess.run([DEBUGFS, "-w", "-R", "rm /tmp/rdata", work_img],
                       capture_output=True, text=True)
        print("[prep] injecting /tmp/rdata (300MB) — this takes a while")
        r = subprocess.run([DEBUGFS, "-w", "-R", "write %s /tmp/rdata" % rdata_bin, work_img],
                           capture_output=True, text=True)
        if not have_rdata():
            print("[prep] rdata injection failed: %s%s" % (r.stdout.strip(), r.stderr.strip()))
            return 2
    print("[prep] /tmp/rdata present")

    sh_path = os.path.join(out, "repro.sh")
    with open(sh_path, "w") as f:
        f.write(REPRO_SH.format(rounds=args.rounds))
    # NOTE: the rootfs has no /root dir; /tmp exists. debugfs exits rc=0
    # even on failure — check output and verify with stat. The initial rm
    # of a not-yet-existing file is fine to fail.
    subprocess.run([DEBUGFS, "-w", "-R", "rm /tmp/repro.sh", work_img],
                   capture_output=True, text=True)
    r = subprocess.run([DEBUGFS, "-w", "-R", "write %s /tmp/repro.sh" % sh_path, work_img],
                       capture_output=True, text=True)
    if "ext2_lookup" in (r.stdout + r.stderr).lower() or r.returncode != 0:
        print("[prep] debugfs write failed: %s%s" % (r.stdout.strip(), r.stderr.strip()))
        return 2
    r = subprocess.run([DEBUGFS, "-R", "stat /tmp/repro.sh", work_img],
                       capture_output=True, text=True)
    # The stat dump also contains "Fragment: ... Size: 0" — match the real
    # file size on the inode header line only.
    m = re.search(r"Project:\s+\d+\s+Size:\s+(\d+)", r.stdout)
    if not m or int(m.group(1)) == 0:
        print("[prep] verification failed: %s%s" % (r.stdout.strip(), r.stderr.strip()))
        return 2
    print("[prep] repro.sh injected (%d bytes)" % os.path.getsize(sh_path))

    cmd = [QEMU,
           "-M", "virt", "-accel", "tcg,thread=multi", "-cpu", "rv64",
           "-m", args.mem, "-smp", str(args.smp),
           "-nographic", "-serial", "stdio", "-monitor", "none"] + \
          ([] if args.no_snapshot else ["-snapshot"]) + [
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % work_img,
           "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
           "-kernel", args.kernel,
           "-append", "root=/dev/vda rw init=/bin/sh console=ttyS0"]
    print("[boot] tag=%s dur=%ds rounds=%d" % (args.tag, args.dur, args.rounds))
    log = open(os.path.join(out, "serial.log"), "wb")
    proc = subprocess.Popen(cmd, stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

    t0 = time.time()
    launched = False
    verdict = {"panic": None, "rounds_done": 0, "last_round": None, "all_done": False}
    buf = bytearray()

    def kill_own():
        try:
            proc.terminate()
            proc.wait(timeout=10)
        except Exception:
            try:
                proc.kill()
            except Exception:
                pass

    try:
        while time.time() - t0 < args.dur:
            r, _, _ = select.select([proc.stdout], [], [], 1.0)
            if not r:
                continue
            chunk = os.read(proc.stdout.fileno(), 65536)
            if not chunk:
                break
            buf.extend(chunk)
            log.write(chunk)
            log.flush()
            text = bytes(buf[-32768:])
            if not launched and b"root" in text and text.rstrip().endswith(b"#"):
                time.sleep(3.0)  # let boot noise drain
                proc.stdin.write(b"\n/bin/sh /tmp/repro.sh\n")
                proc.stdin.flush()
                launched = True
                print("[boot] repro launched at t=%.0fs" % (time.time() - t0))
                buf.clear()
                continue
            for line in text.split(b"\n"):
                if b"ROUND-" in line and b"-END" in line:
                    verdict["rounds_done"] += 1
                    verdict["last_round"] = line.strip().decode(errors="replace")
                    print("[load] %s (t=%.0fs)" % (verdict["last_round"], time.time() - t0))
                if b"REPRO-SCRIPT-ALL-DONE" in line:
                    verdict["all_done"] = True
            for m in PANIC_MARKERS:
                if m in text:
                    hard = m in (b"KERNPANIC", b"WAKE-WILD-PTR", b"panicked",
                                 b"Kernel panic", b"dead bh in chain")
                    print("[marker] %s at t=%.0fs (hard=%s)" %
                          (m.decode(), time.time() - t0, hard))
                    if hard:
                        verdict["panic"] = m.decode()
                        break
            if verdict["panic"]:
                # capture a bit more context, then stop
                time.sleep(15.0)
                break
            if verdict["all_done"]:
                time.sleep(2.0)
                break
    finally:
        kill_own()
        log.close()

    with open(os.path.join(out, "verdict.txt"), "w") as f:
        f.write("tag=%s kernel=%s dur=%d\n" % (args.tag, args.kernel, args.dur))
        f.write("panic=%s\n" % verdict["panic"])
        f.write("rounds_done=%d last=%s all_done=%s\n" %
                (verdict["rounds_done"], verdict["last_round"], verdict["all_done"]))
        with open(os.path.join(out, "serial.log"), "rb") as lf:
            data = lf.read()
        for m in PANIC_MARKERS:
            f.write("marker %-24s count=%d\n" % (m.decode(), data.count(m)))
        for m in INFO_MARKERS:
            f.write("info   %-24s count=%d\n" % (m.decode(), data.count(m)))
        f.write("serial_bytes=%d\n" % len(data))
    print("[verdict]", verdict)
    return 0 if not verdict["panic"] else 1


if __name__ == "__main__":
    sys.exit(main())
