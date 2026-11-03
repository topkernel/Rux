#!/usr/bin/env python3
# hunt2.py — full attribution run: break at bread(280) (the kmsg inode
# read inside persistent_log::resolve_file), break at its return, find
# the freshly populated buffer, arm a watchpoint on its first qword,
# then report the zeroing writer with registers + symbolized stack.
import subprocess, time, sys

WT = "/home/william/rux-agents/backlog-zero"
KERN = "/home/william/rux-agents/review-r3a/target/x86_64-unknown-none/debug/bzImage"
ELF = "/home/william/rux-agents/review-r3a/target/x86_64-unknown-none/debug/rux"
IMG = f"{WT}/run/img/lmbench.img"
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 47133
LOG = f"{WT}/run/logs/hunt2-{PORT}.log"

qemu = subprocess.Popen(["qemu-system-x86_64", "-M", "q35", "-accel", "tcg,thread=multi",
    "-cpu", "max", "-m", "2G", "-smp", "2", "-display", "none", "-vga", "none",
    "-serial", f"file:{LOG}", "-monitor", "none",
    "-gdb", f"tcp:127.0.0.1:{PORT}", "-S",
    "-kernel", KERN,
    "-append", "root=/dev/vda rw init=/test/diag3 console=ttyS0",
    "-drive", f"file={IMG},if=none,id=rootfs,format=raw,snapshot=on,file.locking=off",
    "-device", "virtio-blk-pci,drive=rootfs",
    "-device", "virtio-net-pci,netdev=n0", "-netdev", "user,id=n0"],
    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
print(f"[hunt2] qemu pid={qemu.pid} port={PORT}")
time.sleep(2)

GDBSCRIPT = f'''set pagination off
set confirm off
set height 0
file {ELF}
target remote :{PORT}
# Phase A: catch the first bread(blocknr==280) — rdi=device, rsi=blocknr.
hbreak *0xffffffff805dba20 if $rsi == 280
continue
printf "[A] bread(280) called from:\\n"
x/gx $rsp
set $retaddr = *(unsigned long long*)$rsp
info symbol $retaddr
delete
# Return-site breakpoint: after bread returns, the buffer is populated.
tbreak *$retaddr
continue
printf "[B] bread(280) returned; searching fresh buffer:\\n"
find /b 0xffff888078000000, 0xffff88807d000000, 0xfd, 0x81, 0x00, 0x00, 0x30, 0x52, 0x03, 0x00
printf "[B] pattern addr in $_:\\n"
# arm watchpoint on the first qword of the found buffer
watch *(unsigned long long *)$_
info breakpoints
printf "[C] continuing into the corruption window...\\n"
continue
printf "[D] WATCHPOINT HIT\\n"
info registers rip rax rbx rcx rdx rsi rdi rbp rsp r8 r9 r10 r11 r12 r13 r14 r15 eflags
info symbol $rip
x/24gx $rsp
x/8i $rip-16
'''
open(f"{WT}/run/logs/hunt2-{PORT}.gdb", "w").write(GDBSCRIPT)
r = subprocess.run(["gdb", "-batch", "-x", f"{WT}/run/logs/hunt2-{PORT}.gdb"],
                   capture_output=True, text=True, timeout=1500)
open(f"{WT}/run/logs/hunt2-{PORT}-out.log", "w").write(r.stdout + "\n===STDERR===\n" + r.stderr)
print(r.stdout[-4000:])
qemu.terminate()
