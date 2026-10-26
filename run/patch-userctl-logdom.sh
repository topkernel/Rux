#!/bin/bash
# patch-userctl-logdom.sh — leg-30B v2 visibility patch (follows
# run/patch-abilityms-logdom.sh mechanics).
#
# Leg-29B's domain-only retarget stayed silent because the sites print
# at INFO/DEBUG and hilogd flow-controls those from these domains; this
# pass ALSO raises the level to ERROR(6) at the decisive sites, and
# brings libuser_controller.z.so (domain 0xd001364, never seen on the
# serial) onto the printing domain 0xd001b00:
#
# libuser_controller.z.so (all sites):
#   0x55b1 INFO "foregroundUserId:%d" (inside
#          UserController::IsForegroundUser — prints ONLY on the
#          false/not-foreground path; silence + StartUser entry present
#          == the already-foreground silent bail of Gate 1)
#   0x6769 DEBUG site
#   0x6db4/0x6fd9/0x7021 ERROR sites (CheckUserParam "permission
#          verification failed, not account process" / "userId not
#          exist" family)
# libabilityms.z.so (level raise only; domains already 0xd001b00):
#   0x2c0deb StartUser entry INFO -> ERROR
#   0x340445 SwitchToUser entry INFO -> ERROR
#
# Run from repo root:  bash run/patch-userctl-logdom.sh
# Idempotent: exits 0 when the image already matches.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$ROOT/run/oh-img-p/system.img"

python3 - "$IMG" <<'EOF'
import subprocess, sys
img = sys.argv[1]

def inode_of(path):
    out = subprocess.run(['debugfs','-R',f'stat {path}','--',img],
                         capture_output=True, text=True).stdout
    for line in out.splitlines():
        if line.startswith('Inode:'):
            return int(line.split()[1])
    raise SystemExit(f'no inode for {path}: {out!r}')

UC = '/system/lib64/platformsdk/libuser_controller.z.so'
AMS = '/system/lib64/platformsdk/libabilityms.z.so'

def patch(inode, sites, label):
    subprocess.run(['debugfs','-R',f'dump <{inode}> /tmp/.uc-patch.cur','--',img],
                   capture_output=True)
    cur = open('/tmp/.uc-patch.cur','rb').read()
    d = bytearray(cur)
    n = 0
    for off, old, new, what in sites:
        if d[off:off+len(old)] == new:
            continue
        assert d[off:off+len(old)] == old, (label, what, hex(off), d[off:off+len(old)].hex())
        d[off:off+len(old)] = new; n += 1
    if n == 0:
        print(f'{label}: already patched'); return
    BS = 4096
    def bmap(o):
        blk = o // BS
        out = subprocess.run(['debugfs','-R',f'bmap <{inode}> {blk}','--',img],
                             capture_output=True, text=True).stdout
        for line in out.splitlines():
            if line.strip().isdigit():
                return int(line)
        raise SystemExit(f'bmap failed block {blk}: {out!r}')
    with open(img,'r+b') as fd:
        for off, old, new, _ in sites:
            blk = bmap(off); within = off % BS
            fd.seek(blk*BS+within); fd.write(d[off:off+len(new)])
    print(f'{label}: patched {n} site(s)')

DOM = (bytes.fromhex('ba6413000d'), bytes.fromhex('ba001b000d'))
L4  = (bytes.fromhex('be04000000'), bytes.fromhex('be06000000'))
L3  = (bytes.fromhex('be03000000'), bytes.fromhex('be06000000'))
uc_sites = [
    (0x45b6, *DOM, 'domain IsForegroundUser fgUserId'),
    (0x45b1, *L4,  'level  IsForegroundUser fgUserId INFO->ERR'),
    (0x5764, *L3,  'level  DEBUG site -> ERR'),
    (0x5769, *DOM, 'domain DEBUG site'),
    (0x5db4, *DOM, 'domain ERR site 1'),
    (0x5fd9, *DOM, 'domain ERR site 2'),
    (0x6021, *DOM, 'domain ERR site 3'),
]
ams_sites = [
    (0x2c0deb, *L4, 'level  AMS StartUser entry INFO->ERR'),
    (0x340445, *L4, 'level  AMS SwitchToUser entry INFO->ERR'),
]
patch(inode_of(UC), uc_sites, 'libuser_controller')
patch(inode_of(AMS), ams_sites, 'libabilityms')
EOF

# Verify: dump both back out and assert.
python3 - "$IMG" <<'EOF'
import subprocess, sys
img = sys.argv[1]
def dump(path):
    out = subprocess.run(['debugfs','-R',f'stat {path}','--',img],
                         capture_output=True, text=True).stdout
    ino = None
    for line in out.splitlines():
        if line.startswith('Inode:'):
            ino = int(line.split()[1]); break
    subprocess.run(['debugfs','-R',f'dump <{ino}> /tmp/.uc-patch.vfy','--',img],
                   capture_output=True)
    return open('/tmp/.uc-patch.vfy','rb').read()
uc = dump('/system/lib64/platformsdk/libuser_controller.z.so')
ams = dump('/system/lib64/platformsdk/libabilityms.z.so')
new = bytes.fromhex('ba001b000d'); l6 = bytes.fromhex('be06000000')
for off in (0x45b6, 0x5769, 0x5db4, 0x5fd9, 0x6021):
    assert uc[off:off+5] == new, ('uc dom', hex(off))
assert uc[0x45b1:0x45b6] == l6 and uc[0x5764:0x5769] == l6
assert ams[0x2c0deb:0x2c0df0] == l6 and ams[0x340445:0x34044a] == l6
print('VERIFY OK: userctl 5 dom + 2 lvl, ams 2 lvl')
EOF
