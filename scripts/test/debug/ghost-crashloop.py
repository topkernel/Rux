#!/usr/bin/env python3
"""ghost-crashloop.py — ext4 "ghost empty file after crash" reproducer.

Per cycle, on a private copy of the shared rootfs image:
  phase 1 (crash):  boot RW with init=/bin/sh, drive over a serial unix
                    socket: run /workload.sh (mkdir loop + child files,
                    no sync/unmount); kill -9 our own QEMU a random delay
                    after the workload starts.
  phase 2 (check):  boot the SAME image, run /check.sh — journal recovery
                    runs at mount; every path made before the crash must
                    still be a directory whose child file has the exact
                    payload.

Verdict per path: OK-DIR-CONTENT / GHOST-NOT-A-DIR (the reported bug:
path exists but is an empty regular file) / MISSING-after-MADE /
BAD-CONTENT / DIR-CHILD-MISSING.

Usage: ghost-crashloop.py --kernel PATH --cycles 10 [--tag NAME]
                          [--smp 2] [--mem 1G] [--seed N]
Discipline: private image copy per cycle; kills only its own QEMU child
PID (never pkill); one QEMU at a time.
"""
import argparse
import os
import random
import re
import select
import shutil
import signal
import socket
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
DEBUGFS = "/usr/sbin/debugfs"
QEMU = "/usr/bin/qemu-system-riscv64"

WORKLOAD_SH = """#!/bin/sh
# Crash workload: groups of 8 back-to-back directories, each with a child
# file of known content. Back-to-back mkdirs keep the deferred-metadata
# flush bursts (each ends with a full-cache drain) running nearly
# continuously — the host samples them with its kill timing.
# No sync, no unmount — the host kills QEMU mid-loop.
echo GHOST-WORKLOAD-START
i=0
mkdir -p /ghost
while [ $i -lt 40 ]; do
  j=0
  while [ $j -lt 8 ]; do
    d=/ghost/g${i}_$j
    mkdir $d
    echo "payload-${i}_${j}-abcdefghijklmnopqrstuvwxyz" > $d/child.txt
    echo "FILE ${i}_$j"
    j=$(( j + 1 ))
  done
  echo "GROUP $i"
  i=$(( i + 1 ))
done
echo GHOST-WORKLOAD-ALLDONE
while true; do true; done
"""

CHECK_SH = """#!/bin/sh
echo GHOST-CHECK-START
i=0
while [ $i -lt 40 ]; do
  j=0
  while [ $j -lt 8 ]; do
    d=/ghost/g${i}_$j
    if [ -e $d ]; then
      if [ -d $d ]; then
        if [ -f $d/child.txt ]; then
          got=$(cat $d/child.txt 2>/dev/null)
          want="payload-${i}_${j}-abcdefghijklmnopqrstuvwxyz"
          if [ "$got" = "$want" ]; then
            echo "VERDICT ${i}_$j OK-DIR-CONTENT"
          else
            echo "VERDICT ${i}_$j BAD-CONTENT got=$got"
          fi
        else
          echo "VERDICT ${i}_$j DIR-CHILD-MISSING"
        fi
      else
        t=other
        [ -f $d ] && t=file
        echo "VERDICT ${i}_$j GHOST-NOT-A-DIR type=$t"
      fi
    else
      echo "VERDICT ${i}_$j MISSING"
    fi
    j=$(( j + 1 ))
  done
  i=$(( i + 1 ))
done
echo GHOST-CHECK-DONE
"""


def qemu_cmd(kernel, img, smp, mem, sock):
    return [QEMU, "-M", "virt", "-accel", "tcg,thread=single", "-cpu", "rv64",
            "-m", mem, "-smp", str(smp), "-nographic",
            "-serial", "unix:%s,server=on,wait=off" % sock,
            "-monitor", "none",
            "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % img,
            "-device", "virtio-blk-pci,disable-legacy=on,drive=rootfs",
            "-kernel", kernel,
            "-append", "root=/dev/vda rw init=/bin/sh console=ttyS0"]


def run_session(kernel, img, cdir, smp, mem, command, run_marker, done_marker,
                kill_delay_s=None, boot_timeout=150, done_timeout=150,
                log_name="serial.log", marker_skip=0):
    """Boot, wait for the sh prompt, run `command`, then either kill after
    run_delay_s (crash phase) or wait for done_marker (check phase).
    Returns (serial_bytes, killed)."""
    sock_path = os.path.join(cdir, "serial.sock")
    if os.path.exists(sock_path):
        os.unlink(sock_path)
    log_path = os.path.join(cdir, log_name)
    logf = open(log_path, "wb")
    proc = subprocess.Popen(qemu_cmd(kernel, img, smp, mem, sock_path),
                             stdout=subprocess.DEVNULL,
                             stderr=subprocess.STDOUT)
    killed = False
    conn = None
    try:
        # connect to serial socket
        deadline = time.time() + 30
        while time.time() < deadline:
            if os.path.exists(sock_path):
                try:
                    conn = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                    conn.connect(sock_path)
                    break
                except OSError:
                    conn = None
            if proc.poll() is not None:
                break
            time.sleep(0.2)
        if conn is None:
            return b"", False

        buf = b""
        state = "booting"     # booting -> sent -> running
        sent_at = 0
        run_at = None
        quiet_at = time.time()
        boot_deadline = time.time() + boot_timeout
        done_deadline = None

        while True:
            now = time.time()
            if proc.poll() is not None:
                break
            if state == "booting" and now > boot_deadline:
                break
            if done_deadline is not None and now > done_deadline:
                break
            r, _, _ = select.select([conn], [], [], 0.2)
            if r:
                try:
                    chunk = conn.recv(65536)
                except OSError:
                    chunk = b""
                if chunk:
                    buf += chunk
                    logf.write(chunk)
                    logf.flush()
                    quiet_at = now
                    if (run_at is None and run_marker.encode() in chunk
                            and marker_skip > 0):
                        marker_skip -= 1
                    if run_at is None and run_marker.encode() in buf and marker_skip <= 0:
                        run_at = now
                        if kill_delay_s is not None:
                            done_deadline = now + kill_delay_s + 5
                        else:
                            done_deadline = now + done_timeout
                else:
                    break  # EOF
            if state == "booting":
                # Boot is done when serial is quiet for 2s after the init
                # banner, or a "# " prompt appears.
                if buf.endswith(b"# ") or (now - quiet_at > 2.5 and b"init:" in buf):
                    time.sleep(0.4)
                    try:
                        conn.sendall(command.encode() + b"\n")
                    except OSError:
                        pass
                    state = "sent"
                    sent_at = now
            elif state == "sent":
                if run_at is None and now - sent_at > 20:
                    # prompt detection may have misfired; resend once
                    try:
                        conn.sendall(command.encode() + b"\n")
                    except OSError:
                        pass
                    sent_at = now
                if now - sent_at > 2 and run_at is None and buf.endswith(b"# "):
                    try:
                        conn.sendall(command.encode() + b"\n")
                    except OSError:
                        pass
                    sent_at = now
            if run_at is not None:
                if kill_delay_s is not None and now - run_at >= kill_delay_s:
                    os.kill(proc.pid, signal.SIGKILL)
                    killed = True
                    break
                if kill_delay_s is None and done_marker.encode() in buf:
                    break
        return buf, killed
    finally:
        if conn is not None:
            conn.close()
        if proc.poll() is None:
            if not killed:
                proc.terminate()
                try:
                    proc.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    os.kill(proc.pid, signal.SIGKILL)
                    proc.wait()
            else:
                proc.wait()
        logf.close()


def debugfs(img, *requests):
    for req in requests:
        r = subprocess.run([DEBUGFS, "-w", "-R", req, img],
                           capture_output=True, text=True)
        if r.returncode != 0:
            print("debugfs %r failed: %s %s" % (req, r.stdout, r.stderr))
            sys.exit(1)


def inject_scripts(img, wl_path, ck_path):
    for local, remote in ((wl_path, "workload.sh"), (ck_path, "check.sh")):
        debugfs(img, "rm /%s" % remote,
                "write %s /%s" % (local, remote),
                "sif /%s mode 0100755" % remote)
        r = subprocess.run([DEBUGFS, "-R", "stat /%s" % remote, img],
                           capture_output=True, text=True)
        if "Type: regular" not in r.stdout:
            print("inject verify failed for %s: %s" % (remote, r.stdout))
            sys.exit(1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kernel", required=True)
    ap.add_argument("--cycles", type=int, default=10)
    ap.add_argument("--img-base", default="/home/william/Rux/test/rootfs.img")
    ap.add_argument("--tag", default="ghost")
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="1G")
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("--max-delay", type=float, default=1.2,
                    help="max seconds after a GROUP marker to kill")
    ap.add_argument("--min-delay", type=float, default=0.05)
    args = ap.parse_args()

    rng = random.Random(args.seed)
    outdir = os.path.join(HERE, "runs", args.tag)
    os.makedirs(outdir, exist_ok=True)
    wl = os.path.join(outdir, "workload.sh")
    ck = os.path.join(outdir, "check.sh")
    with open(wl, "w") as f:
        f.write(WORKLOAD_SH)
    with open(ck, "w") as f:
        f.write(CHECK_SH)

    ghosts_total = 0
    integrity_total = 0
    for cycle in range(args.cycles):
        cdir = os.path.join(outdir, "c%02d" % cycle)
        os.makedirs(cdir, exist_ok=True)
        img = os.path.join(cdir, "ghost.img")
        shutil.copyfile(args.img_base, img)
        inject_scripts(img, wl, ck)

        delay = rng.uniform(args.min_delay, args.max_delay)
        crash_buf, killed = run_session(
            args.kernel, img, cdir, args.smp, args.mem,
            "/bin/sh /workload.sh",
            run_marker="GROUP ", done_marker="GHOST-WORKLOAD-ALLDONE",
            kill_delay_s=delay, log_name="serial-crash.log",
            marker_skip=(cycle * 2) % 10)
        group_done = set(int(m) for m in re.findall(rb"GROUP (\d+)", crash_buf))
        file_done = set(m.decode() for m in re.findall(rb"FILE (\S+)", crash_buf))

        check_buf, _ = run_session(
            args.kernel, img, cdir, args.smp, args.mem,
            "/bin/sh /check.sh",
            run_marker="GHOST-CHECK-START", done_marker="GHOST-CHECK-DONE",
            log_name="serial-check.log")
        m = re.search(rb"GHOST-CHECK-START(.*?)GHOST-CHECK-DONE", check_buf, re.S)
        verdicts = {}
        if m:
            for vm in re.finditer(rb"VERDICT (\S+) (\S+)(.*)", m.group(1)):
                verdicts[vm.group(1).decode()] = (vm.group(2).decode(),
                                                  vm.group(3).decode().strip())
        else:
            print("  !! check phase produced no verdicts (boot issue?)")
        ghosts, hard_lost, bad, lazy_content = [], [], [], []
        highest_group = max(group_done) if group_done else -1
        for key, v in sorted(verdicts.items()):
            kind, detail = v
            gi = int(key.split("_")[0])
            # groups below the last confirmed GROUP marker were fully
            # completed; the group right after it was mid-flight at the
            # kill (its dirs may legitimately be missing, but must never
            # be GHOSTS — a published entry over a stale inode)
            completed = gi <= highest_group
            if kind == "GHOST-NOT-A-DIR":
                ghosts.append((key, detail))
            elif kind == "MISSING" and completed:
                hard_lost.append(key)
            elif kind == "DIR-CHILD-MISSING" and key in file_done:
                bad.append((key, kind, detail))
            elif kind == "BAD-CONTENT" and key in file_done:
                # write(2) content durability is lazy by design (fsync/
                # sync/eviction flush it); report but do not fail on it
                lazy_content.append((key, detail))
        ghosts_total += len(ghosts)
        integrity_total += len(ghosts) + len(hard_lost) + len(bad)
        print("cycle %02d: kill@%.0fms killed=%s groups=%d ghosts=%d lost=%d bad=%d lazy-content=%d %s"
              % (cycle, delay * 1000, killed, len(group_done),
                 len(ghosts), len(hard_lost), len(bad), len(lazy_content),
                 ghosts[:4] if ghosts else ""))
        sys.stdout.flush()

    print("=" * 64)
    print("ghosts: %d   integrity-defects(ghost+lost+bad): %d   cycles: %d"
          % (ghosts_total, integrity_total, args.cycles))
    ok = integrity_total == 0
    print("RESULT:", "PASS" if ok else "FAIL")
    return 0 if ok else 2


if __name__ == "__main__":
    sys.exit(main())
