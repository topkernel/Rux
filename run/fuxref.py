#!/usr/bin/env python3
# fuxref.py — 34th successor: reference-kernel futex wait-site census.
# Boots the REFERENCE kernel (OHOS 6.6.101 bzImage) on the SAME substrate
# as our poproll wedge boots (system-lb5 + bms-ref-derived userdata, all
# drives snapshot=on so nothing persists), with slirp hostfwd to the
# guest hdcd TCP port (the oh-robot qemu_run_user.sh flow). Once hdc
# connects we run the guest-side census that mirrors our kernel's
# dfx=fuxstuck map: every FUTEX park >= ~sweep-time is reported with
# (comm, tgid, tid, uaddr, op, val); /proc maps snapshots give the
# VMA-relative offset so the A/B against our uvma=+0x800xxx map works
# across ASLR. ftrace (if tracefs mounts) adds exact wait/wake pairing.
import os, socket, subprocess, sys, time

TAG = sys.argv[1] if len(sys.argv) > 1 else "fx1"
SECS = int(sys.argv[2]) if len(sys.argv) > 2 else 240
HDCPORT = int(os.environ.get("HDCPORT", "55601"))
POLLSTART = int(os.environ.get("POLLSTART", "25"))
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.chdir(ROOT)

REF = "/home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images"
HDC = "/home/william/workspace/oh-robot/hdc.sh"
SER = f"run/oh-{TAG}-serial.log"
OUTDIR = f"run/fuxref-{TAG}"
os.makedirs(OUTDIR, exist_ok=True)
for f in (SER,):
    try: os.unlink(f)
    except FileNotFoundError: pass

def drive(path, i):
    if not os.path.exists(path):
        return ""
    return (f" -drive file={path},if=none,id=ohd{i},format=raw,"
            f"file.locking=off,snapshot=on -device virtio-blk-pci,drive=ohd{i}")

DISKS = (drive(f"{REF}/updater.img", 0)
         + drive(os.environ.get("SYSIMG", "run/system-lb5.img"), 1)
         + drive(os.environ.get("VENDOR", "run/oh-img/vendor.img"), 2)
         + drive(f"{REF}/sys_prod.img", 3)
         + drive(os.environ.get("CHIP", "run/oh-img/chip_prod.img"), 4)
         + drive(os.environ.get("USERDATA", "run/userdata-fresh2.img"), 5))
APPEND = ("ip=dhcp loglevel=" + os.environ.get("LOGLEVEL", "6") + " console=ttyS0,115200 init=init root=/dev/ram0 rw "
          "ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio "
          "ohos.boot.sn=01234567890 "
          "ohos.required_mount.system=/dev/block/vdb@/usr@ext4@ro,barrier=1@wait,required "
          "ohos.required_mount.vendor=/dev/block/vdc@/vendor@ext4@ro,barrier=1@wait,required")

cmd = ["qemu-system-x86_64", "-M", "q35", "-accel", "kvm", "-cpu", "max",
       "-m", "4G", "-smp", os.environ.get("SMP", "2"),
       "-display", "none", "-vga", "none",
       "-device", "virtio-gpu-pci,id=ohgpu,xres=720,yres=1280",
       "-serial", "stdio", "-monitor", "none",
       "-kernel", f"{REF}/bzImage",
       "-initrd", "run/oh-img/ramdisk.img", "-append", APPEND] + DISKS.split() + [
       "-nic", f"user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:{HDCPORT}-:55555",
       "-device", "virtio-keyboard-pci", "-device", "virtio-tablet-pci"]

def hdc(*args, timeout=15):
    return subprocess.run([HDC, "-t", f"127.0.0.1:{HDCPORT}"] + list(args),
                          capture_output=True, text=True, timeout=timeout)

t0 = time.time()
log = open(SER, "w", buffering=1)
proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

# --- guest-side poller: report every futex(202) park each sweep ---
POLL_SH = r'''
while true; do
  T=$(cut -d. -f1 /proc/uptime)
  for p in /proc/[0-9]*; do
    C=$(cat $p/comm 2>/dev/null) || continue
    for t in $p/task/[0-9]*; do
      R=$(cat $t/syscall 2>/dev/null) || continue
      nr=${R%% *}
      if [ "$nr" = "202" ]; then
        set -- $R
        echo "P $T $C ${p##*/} ${t##*/} $2 $3 $4"
      fi
    done
  done
done
'''

# --- fast targeted poller (fx4+): sub-second sweeps over the wedge-
# critical comms ONLY, so parks of 100ms-2s (invisible to the 1s sweep)
# are captured. Pid set refreshed every 5s by the background finder.
FAST_SH = r'''
(while true; do
  for p in /proc/[0-9]*; do
    C=$(cat $p/comm 2>/dev/null) || continue
    case "$C" in
      samgr|foundation|memmgrservice|bluetooth_servi|telephony|\
mmi_uinput_serv|installs|hilogd|el5_filekey_man|time_service|\
huks_service|render_service|inputmethod_ser|resource_schedu|\
storage_daemon|storage_manager|audio_server|media_service) echo $p ;;
    esac
  done > /data/local/tmp/fxpids
  sleep 5
done) &
while true; do
  T=$(cut -d. -f1-2 /proc/uptime)
  for p in $(cat /data/local/tmp/fxpids 2>/dev/null); do
    C=$(cat $p/comm 2>/dev/null) || continue
    for t in $p/task/[0-9]*; do
      R=$(cat $t/syscall 2>/dev/null) || continue
      nr=${R%% *}
      if [ "$nr" = "202" ]; then
        set -- $R
        echo "F $T $C ${p##*/} ${t##*/} $2 $3 $4"
      fi
    done
  done
done
'''

poller = None
fast = None
ftrace = None
connected = None
maps_snap = 0
while True:
    el = time.time() - t0
    if el > SECS or proc.poll() is not None:
        break
    try:
        chunk = proc.stdout.read1(65536)
    except Exception:
        break
    if not chunk:
        time.sleep(0.02)
        continue
    log.buffer.write(chunk)
    log.flush()
    # try to connect once past POLLSTART
    if connected is None and el >= POLLSTART:
        r = subprocess.run([HDC, "tconn", f"127.0.0.1:{HDCPORT}"],
                           capture_output=True, text=True, timeout=20)
        if "Connect OK" in r.stdout or "already" in r.stdout:
            connected = el
            print(f"[fuxref] hdc connected at {el:.1f}s", flush=True)
        else:
            time.sleep(0.01)  # no backoff — serial keeps flowing
    if connected is not None:
        if poller is None:
            poller = subprocess.Popen(
                [HDC, "-t", f"127.0.0.1:{HDCPORT}", "shell", POLL_SH],
                stdout=open(f"{OUTDIR}/parks.tsv", "w"), stderr=subprocess.DEVNULL)
            print("[fuxref] poller started", flush=True)
        if fast is None and os.environ.get("FAST", "1") == "1":
            fast = subprocess.Popen(
                [HDC, "-t", f"127.0.0.1:{HDCPORT}", "shell", FAST_SH],
                stdout=open(f"{OUTDIR}/fast.tsv", "w"), stderr=subprocess.DEVNULL)
            print("[fuxref] fast poller started", flush=True)
        if ftrace is None:
            # tracefs route: enumerate the ACTUAL futex events (the fx2
            # lesson: "futex" in the ls ERROR text false-positived and
            # the enable path never existed — buffer stayed empty).
            try:
                r = hdc("shell", "cat /sys/kernel/tracing/available_events | grep futex")
                evs = [l.strip() for l in r.stdout.splitlines() if l.strip() and "futex" in l]
                if evs:
                    print(f"[fuxref] futex events available: {evs}", flush=True)
                    hdc("shell", "echo 0 > /sys/kernel/tracing/tracing_on")
                    hdc("shell", "echo 32768 > /sys/kernel/tracing/buffer_size_kb")
                    for ev in evs:
                        # available_events lines are "futex:futex_wait" etc.
                        system, name = ev.split(":", 1)
                        hdc("shell", f"echo 1 > /sys/kernel/tracing/events/{system}/{name}/enable")
                    hdc("shell", "echo 1 > /sys/kernel/tracing/tracing_on")
                    # VERIFY: entries must actually land in the buffer
                    import time as _t; _t.sleep(3)
                    v = hdc("shell", "grep -c futex /sys/kernel/tracing/trace")
                    n = 0
                    try:
                        n = int(v.stdout.strip().splitlines()[-1])
                    except Exception:
                        pass
                    if n > 0:
                        ftrace = True
                        print(f"[fuxref] ftrace futex events verified live ({n} lines in 3s)", flush=True)
                    else:
                        ftrace = False
                        print("[fuxref] ftrace enabled but buffer empty — disabled", flush=True)
                else:
                    print(f"[fuxref] no futex tracepoints (out={r.stdout[:100]} err={r.stderr[:100]})", flush=True)
                    ftrace = False
            except Exception as e:
                print(f"[fuxref] ftrace unavailable: {e}", flush=True)
                ftrace = False
        if el - maps_snap >= 10:
            maps_snap = el
            subprocess.run([HDC, "-t", f"127.0.0.1:{HDCPORT}", "shell",
                            "for p in /proc/[0-9]*; do echo \"=== ${p##*/} $(cat $p/comm 2>/dev/null)\"; cat $p/maps 2>/dev/null; done"],
                           stdout=open(f"{OUTDIR}/maps-{int(el)}.txt", "w"),
                           stderr=subprocess.DEVNULL, timeout=60)

log.close()
# teardown: stop ftrace + pull trace before kill
if ftrace:
    try:
        hdc("shell", "echo 0 > /sys/kernel/tracing/tracing_on")
        hdc("shell", "cat /sys/kernel/tracing/trace", timeout=60)
        subprocess.run([HDC, "-t", f"127.0.0.1:{HDCPORT}", "shell",
                        "cat /sys/kernel/tracing/trace"],
                       stdout=open(f"{OUTDIR}/ftrace.txt", "w"),
                       stderr=subprocess.DEVNULL, timeout=120)
        print("[fuxref] ftrace trace pulled", flush=True)
    except Exception as e:
        print(f"[fuxref] ftrace pull failed: {e}", flush=True)
if poller or fast:
    for pp in (poller, fast):
        if pp: pp.kill()
try:
    proc.terminate()
    proc.wait(10)
except Exception:
    proc.kill()
print(f"[fuxref] done parks={os.path.getsize(f'{OUTDIR}/parks.tsv') if os.path.exists(f'{OUTDIR}/parks.tsv') else 0}B", flush=True)
