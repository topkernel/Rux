#!/usr/bin/env python3
"""analyze-damage.py — post-mortem for the no-snapshot repro run.

Compares the guest-written image's /tmp/rdata blocks against the original
rdata.bin to identify exactly which blocks were damaged and what the
damage content looks like (zeros vs metadata vs foreign data).

Usage: analyze-damage.py [image] [rdata.bin]
"""
import subprocess
import sys
import collections

HERE = "/home/william/rux-agents/virtio-comp"
DEBUGFS = "/usr/sbin/debugfs"

img = sys.argv[1] if len(sys.argv) > 1 else HERE + "/work/repro-forensic.img"
src = sys.argv[2] if len(sys.argv) > 2 else HERE + "/work/rdata.bin"

# Physical extent map of /tmp/rdata (file block -> phys block list)
out = subprocess.run([DEBUGFS, "-R", "blocks /tmp/rdata", img],
                     capture_output=True, text=True).stdout.split()
phys = [int(b) for b in out if b.isdigit()]
print("rdata file blocks: %d, phys span %d..%d" %
      (len(phys), phys[0], phys[-1]) if phys else "no rdata?!")
if not phys:
    sys.exit(1)

orig = open(src, "rb").read()
BS = 4096
imgf = open(img, "rb")

damaged = []
for i, pb in enumerate(phys):
    imgf.seek(pb * BS)
    got = imgf.read(BS)
    want = orig[i * BS:(i + 1) * BS]
    if got != want:
        damaged.append((i, pb, got, want))

print("damaged blocks: %d / %d" % (len(damaged), len(phys)))
if damaged:
    fb = damaged[0][0]
    lb = damaged[-1][0]
    print("file-block range: %d..%d (bytes %.1f..%.1f MiB)" %
          (fb, lb, fb * BS / 1048576, (lb + 1) * BS / 1048576))
    print("phys range: %d..%d" % (damaged[0][1], damaged[-1][1]))
    # Damage content classification
    kinds = collections.Counter()
    for i, pb, got, want in damaged:
        if got == b"\x00" * BS:
            kinds["zeros"] += 1
        elif got == want:
            kinds["ok"] += 1
        else:
            # where does the wrong content come from?
            kinds["foreign"] += 1
    print("content kinds:", dict(kinds))
    # For up to 3 foreign blocks, search the whole image for the content's
    # other location (is it a copy of some other block?)
    shown = 0
    for i, pb, got, want in damaged:
        if shown >= 3:
            break
        if got == b"\x00" * BS:
            continue
        # search original rdata for the content
        pos = orig.find(got[:256])
        hint = "rdata@%s" % (pos // BS if pos >= 0 else "-")
        # search the image itself (first 64MB) for another copy
        imgf.seek(0)
        window = imgf.read(128 * 1024 * 1024)
        wpos = window.find(got[:256])
        srch = "img-first128M@%s" % (wpos // BS if wpos >= 0 else "-")
        nz = sum(1 for b in got if b)
        print("blk file=%d phys=%d nonzero=%d head=%s %s %s" %
              (i, pb, nz, got[:16].hex(), hint, srch))
        shown += 1
