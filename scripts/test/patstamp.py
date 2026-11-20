#!/usr/bin/env python3
"""patstamp.py — host-side serial-line timestamper (perf-audit line).

Tails a growing serial log written by QEMU (-serial file:), and writes
every NEW COMPLETE line into <out> prefixed with the wall-clock offset
since the first byte arrival:

    [+123.456] <original line>

Only complete lines are stamped (write-torn tails are held back and
re-read on the next poll — the monostat sc-lesson). This is the wall-
clock backbone for boot-phase timing: kernel printk, hilog passthrough
lines, everything that leaves the UART gets an arrival timestamp at
~50 ms poll resolution. UART drain delay (115200, ~87 us/byte) shifts
arrival vs. emit; at the second-granularity phase analysis that is
noise, and the SYSCOST t=<s> guest-side stamps anchor the mapping.

Usage: patstamp.py <serial-log> <out-stamped> [stop-after-sec]
Stops when the marker file <serial-log>.done appears or the optional
deadline passes. Prints a final line count to stdout.
"""
import os
import sys
import time


def main():
    src, out = sys.argv[1], sys.argv[2]
    deadline = float(sys.argv[3]) if len(sys.argv) > 3 else None

    offset = 0
    t0 = None
    n_lines = 0
    with open(out, "w", buffering=1) as fo:
        while True:
            if os.path.exists(src + ".done"):
                # Drain once more, then stop.
                data = b""
                try:
                    with open(src, "rb") as f:
                        f.seek(offset)
                        data = f.read()
                        offset += len(data)
                except FileNotFoundError:
                    pass
                if data:
                    if t0 is None:
                        t0 = time.monotonic()
                    for line in data.split(b"\n")[:-1]:
                        el = time.monotonic() - t0
                        fo.write("[+%09.3f] %s\n" % (el, line.decode("utf-8", "replace")))
                        n_lines += 1
                break
            data = b""
            try:
                with open(src, "rb") as f:
                    f.seek(offset)
                    data = f.read()
                    offset += len(data)
            except FileNotFoundError:
                pass
            if data:
                if t0 is None:
                    t0 = time.monotonic()
                lines = data.split(b"\n")
                if not data.endswith(b"\n"):
                    # Torn tail: hold back the partial line.
                    offset -= len(lines[-1])
                    lines = lines[:-1]
                for line in lines:
                    el = time.monotonic() - t0
                    fo.write("[+%09.3f] %s\n" % (el, line.decode("utf-8", "replace")))
                    n_lines += 1
            if deadline is not None and t0 and time.monotonic() - t0 > deadline:
                break
            time.sleep(0.05)
    print("patstamp: %d lines stamped" % n_lines)


if __name__ == "__main__":
    main()
