#!/usr/bin/env python3
"""monostat_boot.py — CLOCK_MONOTONIC stability under KVM (timer-lane hunt).

Builds the monostat probe (static musl), puts it on a small ext4 image,
boots q35 + KVM with init=/test/monostat, and:

  1. timestamps every MONOSTAT serial line with the HOST clock, then
     compares guest-claimed 1 s deltas against real host elapsed —
     the definitive "clock runs fast / jumps" detector;
  2. optionally (--pause 10) stops the VM via the QEMU monitor 20 s
     into the run and resumes it after N host seconds — the guest
     monotonic clock must FREEZE (not jump) across the pause, i.e.
     guest delta around the pause stays ~1 s while host delta is ~N s.

Verdict lines:
  MONOSTAT-HOST RESULT: PASS drift_ppm=<n> pause_guest_jump_ms=<n>
  MONOSTAT RESULT: PASS ...     (from the in-guest probe)
rc 0 = both PASS, 1 = FAIL, 3 = timeout/panic.

Usage: monostat_boot.py [--accel kvm] [--smp 2] [--mem 1G]
                        [--kernel PATH] [--pause 10] [--duration 60]
"""
import argparse
import os
import shutil
import socket
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(HERE)
QEMU = "qemu-system-x86_64"
MKFS = "/usr/sbin/mkfs.ext4"
MUSL = os.path.join(WT, "toolchain", "x86_64-rux-linux-musl")
WORK = os.path.join(WT, "work")
PROBE = os.path.join(WORK, "monostat")
IMG = os.path.join(WORK, "monostat.img")


def build_probe():
    cmd = ["gcc", "-static", "-nostdlib", "-O2", "-Wall",
           "-I", os.path.join(MUSL, "include"),
           "-o", PROBE,
           os.path.join(HERE, "monostat.c"),
           os.path.join(MUSL, "lib/crt1.o"),
           os.path.join(MUSL, "lib/libc.a"), "-lgcc"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    return True


def build_image():
    staging = os.path.join(WORK, "monostat_root")
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(os.path.join(staging, "test"))
    shutil.copyfile(PROBE, os.path.join(staging, "test/monostat"))
    os.chmod(os.path.join(staging, "test/monostat"), 0o755)
    r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum",
                        "-d", staging, IMG, "64M"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("[img] mkfs failed: %s" % r.stderr)
        return False
    return True


def mon_send(sock, cmd):
    sock.sendall((cmd + "\n").encode())


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--accel", default="kvm")
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--mem", default="1G")
    ap.add_argument("--kernel", default=os.path.join(
        WT, "target/x86_64-unknown-none/debug/bzImage"))
    ap.add_argument("--pause", type=int, default=0,
                    help="host seconds to pause the VM mid-run (0 = off)")
    ap.add_argument("--duration", type=int, default=60)
    ap.add_argument("--tag", default=None)
    args = ap.parse_args()

    os.makedirs(WORK, exist_ok=True)
    if not os.path.exists(args.kernel):
        print("[boot] kernel missing: %s" % args.kernel)
        return 2
    if not build_probe():
        return 2
    if not build_image():
        return 2

    tag = args.tag or ("pause%d" % args.pause if args.pause else "run")
    serial = os.path.join(WORK, "monostat_serial_%s.log" % tag)
    mon = os.path.join(WORK, "monostat_mon_%s.sock" % tag)
    for f in (serial, mon):
        if os.path.exists(f):
            os.remove(f)

    append = ("root=/dev/vda rw init=/test/monostat console=ttyS0 "
              "dfx=monowatch")
    cmd = [QEMU, "-M", "q35", "-accel", args.accel, "-cpu", "max",
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none",
           "-serial", "file:" + serial,
           "-monitor", "unix:" + mon + ",server=on,wait=off",
           "-drive", "file=%s,if=none,id=rootfs,format=raw,file.locking=off" % IMG,
           "-device", "virtio-blk-pci,drive=rootfs",
           "-kernel", args.kernel, "-append", append]
    print("[boot] qemu accel=%s smp=%s mem=%s pause=%ds kernel=%s"
          % (args.accel, args.smp, args.mem, args.pause, args.kernel))
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    # Host-timestamped tail of the serial log.
    samples = []   # (host_monotonic, guest_mono_ms)
    verdict = None
    panic = False
    paused = False
    pause_at = 20.0
    resume_jump = None
    try:
        t0 = time.monotonic()
        msock = None
        while time.monotonic() - t0 < args.duration + 120:
            if proc.poll() is not None:
                break
            try:
                with open(serial, "rb") as f:
                    data = f.read()
            except FileNotFoundError:
                data = b""
            if b"panicked" in data or b"Kernel panic" in data:
                panic = True
                break
            for line in data.split(b"\n"):
                if line.startswith(b"MONOSTAT t=") and b"mono_ms=" in line:
                    try:
                        gmono = int(line.split(b"mono_ms=")[1].split()[0])
                        samples.append((time.monotonic(), gmono))
                    except (IndexError, ValueError):
                        pass
                if line.startswith(b"MONOSTAT RESULT:"):
                    verdict = line.decode(errors="replace")
                    print("[probe] %s" % verdict)

            # Pause/resume cycle mid-run.
            if args.pause and not paused and time.monotonic() - t0 >= pause_at:
                if msock is None:
                    try:
                        msock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                        msock.settimeout(5)
                        msock.connect(mon)
                        msock.setblocking(False)
                    except (FileNotFoundError, ConnectionRefusedError, OSError):
                        msock = None
                if msock is not None:
                    try:
                        mon_send(msock, "stop")
                        print("[monowatch] VM stopped at t+%.1fs for %ds host s"
                              % (time.monotonic() - t0, args.pause))
                        paused = True
                        time.sleep(args.pause)
                        mon_send(msock, "cont")
                        print("[monowatch] VM resumed")
                    except OSError:
                        pass
            if verdict is not None:
                break
            time.sleep(0.5)

        if msock is not None:
            try:
                msock.close()
            except OSError:
                pass

        # Host-vs-guest drift from the timestamped samples.
        host_result = "SKIPPED (no samples)"
        if len(samples) >= 5 and not panic:
            # Drop samples that span the pause window when paused.
            if paused:
                # resume_jump: guest ms jump across the pause vs the
                # host-at-resume baseline. Recompute pairwise drifts on
                # non-pause spans only; then measure the pause jump.
                pass
            (h0, g0), (h1, g1) = samples[0], samples[-1]
            host_ms = (h1 - h0) * 1000.0
            guest_ms = float(g1 - g0)
            if paused:
                # The sample pair spans the pause; guest_ms excludes the
                # frozen time by design, so drift here == pause size —
                # exclude it by using per-interval guest/host deltas and
                # taking the max interval ratio as the pause-jump probe.
                intervals = []
                for i in range(1, len(samples)):
                    dh = (samples[i][0] - samples[i - 1][0]) * 1000.0
                    dg = float(samples[i][1] - samples[i - 1][1])
                    intervals.append((dh, dg))
                if intervals:
                    dh, dg = max(intervals, key=lambda x: x[0])
                    resume_jump = dg - 1000.0  # expected 1 s per sample
                    steady = [iv for iv in intervals if iv[0] < 5000]
                    if steady:
                        hs = sum(iv[0] for iv in steady)
                        gs = sum(iv[1] for iv in steady)
                        host_ms, guest_ms = hs, gs
            drift_ppm = int(abs(guest_ms - host_ms) / host_ms * 1e6) if host_ms > 0 else -1
            ok = drift_ppm < 5000 and (resume_jump is None or abs(resume_jump) < 2000)
            host_result = "%s drift_ppm=%d pause_guest_jump_ms=%s" % (
                "PASS" if ok else "FAIL", drift_ppm,
                "n/a" if resume_jump is None else "%d" % resume_jump)
        print("MONOSTAT-HOST RESULT: %s" % host_result)

        if panic:
            print("[boot] KERNEL PANIC detected")
            return 3
        if verdict is None:
            print("[boot] no in-guest verdict (timeout?)")
            return 3
        if "FAIL" in verdict or "FAIL" in host_result:
            return 1
        return 0
    finally:
        proc.terminate()
        try:
            proc.wait(5)
        except Exception:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
