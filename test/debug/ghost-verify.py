#!/usr/bin/env python3
"""ghost-verify.py — normal-unmount regression + write-read loop.

On a private image copy:
  boot 1: run /loop.sh — 20 rounds of (create dir + files, overwrite with
          fresh content, read back and compare IN GUEST), then `reboot`
          for a CLEAN shutdown (ext4_shutdown_sync marks the fs clean and
          QEMU exits).
  host:   dumpe2fs -h asserts the image reports "Filesystem state: clean".
  boot 2: run /verify.sh — every path must be a directory and every file
          must carry the LAST written content (clean unmount durability).

Usage: ghost-verify.py --kernel PATH [--tag NAME] [--smp 2] [--mem 1G]
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))


def debugfs(img, *requests):
    for req in requests:
        r = subprocess.run(["/usr/sbin/debugfs", "-w", "-R", req, img],
                           capture_output=True, text=True)
        if r.returncode != 0:
            print("debugfs %r failed: %s %s" % (req, r.stdout, r.stderr))
            sys.exit(1)


LOOP_SH = """#!/bin/sh
echo LOOP-START
r=0
while [ $r -lt 20 ]; do
  mkdir -p /loop/d$r
  echo "round-$r-alpha" > /loop/d$r/f1
  echo "round-$r-beta" > /loop/d$r/f2
  # read back immediately and compare in-guest (write->read roundtrip)
  a=$(cat /loop/d$r/f1)
  b=$(cat /loop/d$r/f2)
  if [ "$a" = "round-$r-alpha" ] && [ "$b" = "round-$r-beta" ]; then
    echo "LOOP-OK $r"
  else
    echo "LOOP-FAIL $r a=$a b=$b"
  fi
  r=$(( r + 1 ))
done
echo LOOP-DONE
sync
echo LOOP-SYNCED
reboot
"""

VERIFY_SH = """#!/bin/sh
echo VERIFY-START
fail=0
r=0
while [ $r -lt 20 ]; do
  d=/loop/d$r
  if [ -d $d ]; then
    a=$(cat $d/f1 2>/dev/null)
    b=$(cat $d/f2 2>/dev/null)
    if [ "$a" = "round-$r-alpha" ] && [ "$b" = "round-$r-beta" ]; then
      echo "VERIFY-OK $r"
    else
      echo "VERIFY-FAIL $r a=$a b=$b"
      fail=1
    fi
  else
    echo "VERIFY-FAIL $r missing-dir"
    fail=1
  fi
  r=$(( r + 1 ))
done
if [ $fail -eq 0 ]; then echo VERIFY-PASS; else echo VERIFY-BAD; fi
"""

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kernel", required=True)
    ap.add_argument("--img-base", default="/home/william/Rux/test/rootfs.img")
    ap.add_argument("--tag", default="umount")
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="1G")
    args = ap.parse_args()

    outdir = os.path.join(HERE, "runs", args.tag)
    os.makedirs(outdir, exist_ok=True)
    img = os.path.join(outdir, "umount.img")
    shutil.copyfile(args.img_base, img)
    with open(os.path.join(outdir, "loop.sh"), "w") as f:
        f.write(LOOP_SH)
    with open(os.path.join(outdir, "verify.sh"), "w") as f:
        f.write(VERIFY_SH)
    for local, remote in (("loop.sh", "loop.sh"), ("verify.sh", "verify.sh")):
        debugfs(img, "rm /%s" % remote,
                "write %s/%s /%s" % (outdir, local, remote),
                "sif /%s mode 0100755" % remote)

    # import the session driver from ghost-crashloop
    src = open(os.path.join(HERE, "ghost-crashloop.py")).read()
    src = src.replace('if __name__ == "__main__":\n    sys.exit(main())', '')
    ns = {"__file__": os.path.join(HERE, "ghost-crashloop.py")}
    exec(compile(src, "ghost-crashloop.py", "exec"), ns)
    run_session = ns["run_session"]

    # boot 1: loop + clean reboot (reboot exits QEMU via SBI shutdown)
    buf, _ = run_session(args.kernel, img, outdir, args.smp, args.mem,
                         "/bin/sh /loop.sh",
                         run_marker="LOOP-START", done_marker="reboot: Power down",
                         log_name="serial-loop.log")
    loops_ok = len(re.findall(rb"LOOP-OK \d+", buf))
    loops_fail = re.findall(rb"LOOP-FAIL \S+", buf)
    synced = b"LOOP-SYNCED" in buf
    print("write-read loop: ok=%d fail=%d synced=%s" % (loops_ok, len(loops_fail), synced))
    if loops_fail:
        print("  loop failures:", loops_fail[:5])

    # host-side: fs must be marked clean after the clean shutdown
    dumpe = subprocess.run(["/usr/sbin/dumpe2fs", "-h", img],
                           capture_output=True, text=True)
    state = "?"
    for line in dumpe.stdout.splitlines():
        if "Filesystem state:" in line:
            state = line.split(":", 1)[1].strip()
    print("post-shutdown dumpe2fs state: %s" % state)

    # boot 2: verify durability
    buf2, _ = run_session(args.kernel, img, outdir, args.smp, args.mem,
                          "/bin/sh /verify.sh",
                          run_marker="VERIFY-START", done_marker="VERIFY-PASS",
                          log_name="serial-verify.log")
    v_ok = len(re.findall(rb"VERIFY-OK \d+", buf2))
    v_fail = re.findall(rb"VERIFY-FAIL \S+", buf2)
    passed = b"VERIFY-PASS" in buf2
    print("post-reboot verify: ok=%d fail=%d pass-marker=%s" % (v_ok, len(v_fail), passed))
    if v_fail:
        print("  verify failures:", v_fail[:5])

    ok = (loops_ok == 20 and not loops_fail and synced and "clean" in state
          and passed)
    print("RESULT:", "PASS" if ok else "FAIL")
    return 0 if ok else 2


if __name__ == "__main__":
    sys.exit(main())
