#!/usr/bin/env python3
"""multidisk_boot.py — multi-disk virtio-blk boot test (OH Phase 1b gap 2).

Boots the Rux kernel on a QEMU virt with TWO virtio-blk-pci functions
(vda + vdb), twice:

  run A: root=/dev/vda  — probe mounts /dev/vdb on /mnt, cross-reads,
                          writes + reads back on vdb
  run B: root=/dev/vdb  — kernel must mount the SECOND PCI disk as root;
                          probe cross-mounts /dev/vda

Verdict comes from the probe itself:
  MDPROBE RESULT: PASS n/n  -> rc 0
  MDPROBE RESULT: FAIL p/n  -> rc 1
  kernel panic              -> rc 2

Also greps the serial log for the per-slot registration rows
("virtio-blk: vda (slot 0) ...", "virtio-blk: vdb (slot 1) ...").

Resource discipline: one QEMU at a time, -snapshot + file.locking=off,
killed by this script only.
"""
import argparse
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(HERE)
QEMU = "/usr/bin/qemu-system-riscv64"
MUSL = os.path.join(WT, "toolchain", "riscv64-rux-linux-musl")
CC = "riscv64-linux-gnu-gcc"
MKFS = "/usr/sbin/mkfs.ext4"
DEFAULT_KERNEL = os.path.join(
    WT, "target/riscv64gc-unknown-none-elf/debug/rux")
MARKER = "MDPROBE RESULT:"
BIG_BYTES = 512 * 1024


def build_probe(out):
    cmd = [CC, "-static", "-nostdlib", "-O2", "-Wall",
           "-I", os.path.join(MUSL, "include"),
           "-o", out, os.path.join(HERE, "mdprobe.c"),
           os.path.join(MUSL, "lib", "crt1.o"),
           os.path.join(MUSL, "lib", "libc.a"),
           "-lgcc"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    return True


def stage_image(staging, marker, letter):
    shutil.rmtree(staging, ignore_errors=True)
    os.makedirs(os.path.join(staging, "test"))
    probe = os.path.join(WT, "work", "mdprobe")
    shutil.copyfile(probe, os.path.join(staging, "test", "mdprobe"))
    os.chmod(os.path.join(staging, "test", "mdprobe"), 0o755)
    with open(os.path.join(staging, "marker-%s.txt" % letter), "w") as f:
        f.write(marker)
    # 512K pattern file: byte i == (i*7 + 11) & 0xff (mdprobe.c checks it).
    with open(os.path.join(staging, "bigfile.bin"), "wb") as f:
        chunk = bytes((i * 7 + 11) & 0xFF for i in range(4096))
        for _ in range(BIG_BYTES // 4096):
            f.write(chunk)


def build_images(img_a, img_b):
    if not build_probe(os.path.join(WT, "work", "mdprobe")):
        return False
    os.makedirs(os.path.join(WT, "work"), exist_ok=True)
    for img, marker, letter in ((img_a, "RUX-MD-A\n", "a"),
                                (img_b, "RUX-MD-B\n", "b")):
        staging = os.path.join(WT, "work", "md_root_%s" % letter)
        stage_image(staging, marker, letter)
        r = subprocess.run([MKFS, "-q", "-F", "-O", "^metadata_csum,^flex_bg",
                            "-d", staging, img, "64M"],
                           capture_output=True, text=True)
        if r.returncode != 0:
            print("[img] mkfs failed for %s: %s" % (img, r.stderr))
            return False
    return True


def boot(root, img_a, img_b, args, log):
    append = "root=%s rw init=/test/mdprobe console=ttyS0" % root
    cmd = [QEMU,
           "-M", "virt", "-accel", "tcg,thread=" + args.thread,
           "-cpu", "rv64,zbb=true,zba=true,zbs=true",
           "-m", args.mem, "-smp", str(args.smp),
           "-display", "none", "-serial", "stdio",
           "-snapshot",
           "-drive", "file=%s,if=none,id=diska,format=raw,file.locking=off" % img_a,
           "-device", "virtio-blk-pci,disable-legacy=on,drive=diska",
           "-drive", "file=%s,if=none,id=diskb,format=raw,file.locking=off" % img_b,
           "-device", "virtio-blk-pci,disable-legacy=on,drive=diskb",
           "-kernel", args.kernel,
           "-append", append]
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    buf = bytearray()
    t0 = time.time()
    rc, verdict = 3, None
    with open(log, "wb") as ser:
        while time.time() - t0 < args.timeout:
            chunk = os.read(proc.stdout.fileno(), 65536) if proc.poll() is None else b""
            if chunk == b"" and proc.poll() is not None:
                break
            if chunk:
                buf.extend(chunk)
                ser.write(chunk)
                ser.flush()
                text = bytes(buf[-32768:])
                if b"panicked" in text or b"Kernel panic" in text:
                    rc = 2
                    break
                for line in text.split(b"\n"):
                    if line.startswith(MARKER.encode()):
                        verdict = line.decode(errors="replace").strip()
                        rc = 0 if "RESULT: PASS" in verdict else 1
                if verdict:
                    break
            time.sleep(0.05)
    proc.kill()
    proc.wait()
    print("  probe: %s" % (verdict or "(no verdict)"))
    return rc, verdict


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--thread", default="single", choices=["multi", "single"])
    ap.add_argument("--timeout", type=int, default=180)
    ap.add_argument("--mem", default="1G")
    ap.add_argument("--kernel", default=DEFAULT_KERNEL)
    args = ap.parse_args()

    img_a = os.path.join(WT, "work", "md_a.img")
    img_b = os.path.join(WT, "work", "md_b.img")
    if not build_images(img_a, img_b):
        return 2
    if not os.path.exists(args.kernel):
        print("[boot] kernel missing: %s" % args.kernel)
        return 2

    fails = 0
    for root, name in (("/dev/vda", "A root=vda"),
                       ("/dev/vdb", "B root=vdb")):
        log = os.path.join(WT, "work", "md_serial_%s.log" % name[0])
        print("[run %s] booting %s (log %s)" % (name, root, log))
        rc, _ = boot(root, img_a, img_b, args, log)
        with open(log, "rb") as f:
            text = f.read().decode(errors="replace")
        for m in ("virtio-blk: vda (slot 0)", "virtio-blk: vdb (slot 1)"):
            if m in text:
                print("  ok   kernel: %s" % m)
            else:
                print("  MISS kernel row: %s" % m)
                fails += 1
        if rc != 0:
            fails += 1
        print("  run %s rc=%d" % (name, rc))

    if fails == 0:
        print("MULTIDISK: PASS (both runs)")
        return 0
    print("MULTIDISK: FAIL (%d problem(s))" % fails)
    return 1


if __name__ == "__main__":
    sys.exit(main())
