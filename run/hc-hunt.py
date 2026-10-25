#!/usr/bin/env python3
"""hc-hunt.py — libhicollie condvar wildwrite hunt (leg-21).

The launcher crash-loop dies inside the dynamic linker's pthread_cond
waiter walk: libhicollie.z.so's global condvar (module_base+0x3c108,
.bss) has its queue head (_c_head at +0x28) set to module_base+0xa92c —
a RODATA STRING POINTER no static initializer or relocation ever writes
(ruled out offline). The value is computed at runtime by module code
writing through a bad object pointer: a userspace wildwrite.

This hunt names the writer: wait for the kernel's unbudgeted APPMAPSRC
line that names libhicollie's MAP_FIXED load base, attach gdb (TCG
gdbstub), arm one hardware watchpoint on the condvar's _c_head slot, and
continue. Every legit condvar transition rewrites _c_head (enqueue on
empty / drain); the script filters stops by VALUE: only a write whose
result equals base+0xa92c (or lands while the slot already holds it) is
a capture. Capture = rip, regs, stack words, code window, and the gdb
log keeps everything for offline symbolization against the image's .so
files.

Usage: hc-hunt.py <tag> [secs] [gdbport]
Exit: prints HC-HIT lines from the gdb log on capture.
"""

import os
import re
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RUN = os.path.join(ROOT, "run")

tag = sys.argv[1] if len(sys.argv) > 1 else "hc1"
secs = int(sys.argv[2]) if len(sys.argv) > 2 else 3600
port = int(sys.argv[3]) if len(sys.argv) > 3 else 12601

serial = os.path.join(RUN, f"{tag}-serial.log")
gdblog = os.path.join(RUN, f"{tag}-gdb.log")
cmdfile = os.path.join(RUN, f"{tag}-gdb.cmd")

# Wait for libhicollie's MAP_FIXED load line: the first APPMAPSRC line
# whose path contains hicollie and whose flags carry MAP_FIXED (0x10).
# That ret= is the module base (segment 1 loads at base+0).
base_re = re.compile(
    r'APPMAPSRC "[^"]*" ret=(0x[0-9a-f]+) len=\S+ prot=\S+ '
    r'flags=(0x[0-9a-f]+) fd=\d+ accmode=\S+ off=\S+ \S*hicollie'
)

env = dict(os.environ, TAG=tag, GDBPORT=str(port), SECS=str(secs))
env["EXTRA_APPEND"] = (env.get("EXTRA_APPEND", "") + " dfx=schb,sleepsnap").strip()
qemu = subprocess.Popen(
    [os.path.join(RUN, "hc-boot.sh")],
    env=env,
    stdout=subprocess.DEVNULL,
    stderr=subprocess.STDOUT,
)
print(f"[hc] qemu pid={qemu.pid} tag={tag} port={port} budget={secs}s", flush=True)

deadline = time.time() + secs
base = None
try:
    # Poll the serial for the base (dlopens early in appspawn, t<90s even
    # under TCG; keep polling until found or budget ends).
    while time.time() < deadline and base is None:
        if qemu.poll() is not None:
            print(f"[hc] qemu exited early rc={qemu.returncode}", flush=True)
            sys.exit(1)
        if os.path.exists(serial):
            with open(serial, "r", errors="replace") as f:
                for line in f:
                    if "hicollie" not in line:
                        continue
                    m = base_re.search(line)
                    if m:
                        ret_s, flags_s = m.group(1), m.group(2)
                    else:
                        # Interleaved consoles can inject text mid-line;
                        # fall back to independent token extraction.
                        mr = re.search(r"ret=(0x[0-9a-f]+)", line)
                        mf = re.search(r"flags=(0x[0-9a-f]+)", line)
                        if not (mr and mf):
                            continue
                        ret_s, flags_s = mr.group(1), mf.group(1)
                    if int(flags_s, 16) & 0x10:  # MAP_FIXED segment load
                        cand = int(ret_s, 16)
                        # segment 1 (R, off=0) is the lowest MAP_FIXED ret
                        base = cand if base is None else min(base, cand)
        if base is not None:
            break
        time.sleep(2)
    if base is None:
        print("[hc] base never appeared in serial — no hunt", flush=True)
        sys.exit(2)

    condvar = base + 0x3C108
    stale = base + 0xA92C
    print(
        f"[hc] base={base:#x} condvar={condvar:#x} stale_head={stale:#x}",
        flush=True,
    )

    cmds = f"""
set pagination off
set confirm off
set architecture i386:x86-64
target remote :{port}
# signals must not stop the loop; the guest crash-loops SIGSEGVs after
# corruption and we only care about the watchpoint.
handle SIGSEGV nostop noprint pass
handle SIGALRM nostop noprint pass
handle SIGUSR1 nostop noprint pass
handle SIGUSR2 nostop noprint pass
handle SIGCHLD nostop noprint pass
handle SIGPIPE nostop noprint pass
watch -l *(unsigned long *){condvar}
printf "HC-ARMED c={condvar:#x} stale={stale:#x} initial=%#lx\\n", *(unsigned long *){condvar}
python
import gdb
stale = {stale}
condvar = {condvar}
hits = 0
captured = False
while hits < 20000 and not captured:
    hits += 1
    try:
        gdb.execute("continue")
    except gdb.error as e:
        print("HC-ERR continue: %s" % e)
        break
    try:
        v = int(gdb.parse_and_eval("*(unsigned long *)%d" % condvar))
    except gdb.error:
        continue
    if v == stale:
        print("HC-HIT hits=%d value=stale" % hits)
        gdb.execute("printf \\"HC-HIT rip=%p rsp=%p rbp=%p rdi=%p rsi=%p\\n", $rip, $rsp, $rbp, $rdi, $rsi")
        gdb.execute("info registers")
        gdb.execute("x/16gx $rsp")
        gdb.execute("x/24i $rip-48")
        gdb.execute("x/6gx %d" % condvar)
        captured = True
print("HC-DONE hits=%d captured=%s" % (hits, captured))
end
detach
quit
"""
    with open(cmdfile, "w") as f:
        f.write(cmds)
    with open(gdblog, "w") as lf:
        gdb_proc = subprocess.Popen(
            ["gdb-multiarch", "-batch", "-x", cmdfile],
            stdout=lf,
            stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL,
        )
    print(f"[hc] gdb pid={gdb_proc.pid} armed from {cmdfile}", flush=True)

    while time.time() < deadline:
        if qemu.poll() is not None:
            break
        # A capture ends the hunt early.
        try:
            with open(gdblog, "r", errors="replace") as f:
                if "HC-HIT rip=" in f.read():
                    print("[hc] CAPTURE — stopping qemu", flush=True)
                    break
        except FileNotFoundError:
            pass
        time.sleep(5)

    if gdb_proc and gdb_proc.poll() is None:
        gdb_proc.terminate()
        try:
            gdb_proc.wait(10)
        except subprocess.TimeoutExpired:
            gdb_proc.kill()
finally:
    if qemu.poll() is None:
        qemu.terminate()
        try:
            qemu.wait(15)
        except subprocess.TimeoutExpired:
            qemu.kill()

# Report.
try:
    with open(gdblog, "r", errors="replace") as f:
        for line in f:
            if line.startswith("HC-") or "Hardware watchpoint" in line:
                print("[hc] " + line.rstrip(), flush=True)
except FileNotFoundError:
    pass
print("[hc] done", flush=True)
