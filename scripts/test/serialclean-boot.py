#!/usr/bin/env python3
"""serialclean-boot.py — scripted serial-log boot check (serial-clean agent).

Boots the worktree kernel on the minimal rootfs with a fixed shell command
sequence (ls /proc, cat /proc/loadavg, sync) and captures the serial log,
so the structured boot table and the /proc pid listing can be audited
without an interactive console.

Host-discipline: -smp 2 -m 1G, -snapshot + file.locking=off on the shared
rootfs image; only the QEMU process started here is terminated.

Environment:
  RUX_KERNEL     kernel ELF (default: worktree target/.../debug/rux)
  RUX_ROOTFS     rootfs image (default: test/rootfs.img if present, else
                 the main repo's shared copy)
  RUX_TRANSPORT  "pci" (default) or "mmio" virtio-blk transport
  RUX_APPEND     extra kernel cmdline (e.g. "dfx=schedtrace")

Usage: serialclean-boot.py <log-path> [extra-append-args...]
"""
import os
import subprocess
import sys
import time

WORKTREE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KERNEL = os.environ.get(
    "RUX_KERNEL",
    os.path.join(WORKTREE, "target/riscv64gc-unknown-none-elf/debug/rux"),
)

MAIN_REPO_ROOTFS = "/home/william/workspace/Rux/test/rootfs.img"


def _default_rootfs():
    local = os.path.join(WORKTREE, "test/rootfs.img")
    if os.path.exists(local):
        return local
    return MAIN_REPO_ROOTFS


ROOTFS = os.environ.get("RUX_ROOTFS") or _default_rootfs()

# Shell commands fed to the booted mrsh, one per interval.
SCRIPT = [b"ls /proc\n", b"cat /proc/loadavg\n", b"sync\n"]


def main():
    log_path = sys.argv[1]
    extra = " ".join([os.environ.get("RUX_APPEND", "")] + sys.argv[2:]).strip()
    append = f"root=/dev/vda rw init=/bin/sh console=ttyS0 {extra}".strip()

    transport = os.environ.get("RUX_TRANSPORT", "pci")
    if transport == "mmio":
        blk_arg = "virtio-blk-device,drive=rootfs"
    else:
        blk_arg = "virtio-blk-pci,disable-legacy=on,drive=rootfs"

    cmd = [
        "qemu-system-riscv64",
        "-M", "virt",
        "-accel", "tcg,thread=multi",
        "-cpu", "rv64,zbb=true,zba=true,zbs=true",
        "-m", "1G",
        "-smp", "2",
        "-display", "none",
        "-serial", "stdio",
        "-monitor", "none",
        "-snapshot",
        "-drive", f"file={ROOTFS},if=none,id=rootfs,format=raw,file.locking=off",
        "-device", blk_arg,
        "-kernel", KERNEL,
        "-append", append,
    ]
    print("CMD:", " ".join(cmd), flush=True)
    with open(log_path, "wb") as log:
        proc = subprocess.Popen(
            cmd, stdin=subprocess.PIPE, stdout=log, stderr=subprocess.STDOUT,
            cwd=WORKTREE,
        )
        # Wait for the shell banner, then settle before feeding commands.
        deadline = time.time() + 120
        while time.time() < deadline:
            if proc.poll() is not None:
                print("qemu exited early", flush=True)
                break
            try:
                with open(log_path, "rb") as f:
                    data = f.read()
            except OSError:
                data = b""
            if b"mrsh" in data[-600:] and b"POSIX shell" in data:
                break
            time.sleep(2)
        time.sleep(8)
        for s in SCRIPT:
            try:
                proc.stdin.write(s)
                proc.stdin.flush()
            except BrokenPipeError:
                break
            time.sleep(6)
        time.sleep(4)
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
    print("log written to", log_path, flush=True)


if __name__ == "__main__":
    main()
