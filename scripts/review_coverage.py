#!/usr/bin/env python3
"""Generate the kernel review coverage map.

The map is the bookkeeping for the line-by-line review: every source
file under kernel/src carries a status, so nothing can be silently
skipped.

Status vocabulary (kept in STATUS below):

  done    the file was read end to end, no defect found
  fixed   the file was read end to end, a defect was found and fixed
  part    the file was read in part (the reviewed range is recorded)
  todo    the file has not been read yet

Usage: scripts/review_coverage.py [--check]

  --check   verify the map is in sync with the tree (no new files, no
            status for an unknown file) and exit non-zero on drift
"""

import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "kernel", "src")
OUT = os.path.join(ROOT, "docs", "development", "review-coverage.md")

# --- the status of every file -----------------------------------------
#
# done  = read end to end, nothing found
# fixed = read end to end, a defect was found and fixed (see BUGS)
# part  = read in part; the value is the reviewed line ranges
# todo  = not read yet
STATUS = {
    "kernel/src/arch/riscv64/mm/memory_layout.rs": ("part", "image window constants"),
    "kernel/src/arch/riscv64/mm/mmu_init.rs": ("part", "kernel image mapping"),
    "kernel/src/arch/riscv64/trap.rs": ("part", "tick accounting, trap entry"),
    "kernel/src/arch/x86_64/mm/memory_layout.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/mmu_init.rs": ("part", "kernel image mapping"),
    "kernel/src/arch/x86_64/trap.rs": ("part", "TSS/IST interrupt stacks"),
    "kernel/src/cmdline.rs": ("part", "parameter parsing"),
    "kernel/src/dfx/backtrace.rs": ("part", "symbolized frame printing"),
    "kernel/src/dfx/bug.rs": ("part", "WARN/BUG banners"),
    "kernel/src/dfx/driftwatch.rs": ("part", "release-site attribution"),
    "kernel/src/dfx/hexdump.rs": ("part", "hex dump helper"),
    "kernel/src/dfx/hung_task.rs": ("part", "D-state scan"),
    "kernel/src/dfx/ksyms.rs": ("done", "written by this review (symbol table)"),
    "kernel/src/dfx/ledger.rs": ("part", "page census"),
    "kernel/src/dfx/memwatch.rs": ("part", "heap census"),
    "kernel/src/dfx/metawatch.rs": ("part", "buddy shadow scan"),
    "kernel/src/dfx/mod.rs": ("part", "init, debugfs knob registration"),
    "kernel/src/dfx/monowatch.rs": ("part", "monotonic jump tripwire"),
    "kernel/src/dfx/r83hunt.rs": ("part", "task header audit"),
    "kernel/src/dfx/sbi_debug.rs": ("part", "raw SBI printers"),
    "kernel/src/dfx/scribble.rs": ("part", "task scribble hunter"),
    "kernel/src/dfx/softlockup.rs": ("part", "soft lockup detector"),
    "kernel/src/dfx/switches.rs": ("done", "switch table refactored to one macro"),
    "kernel/src/dfx/syscost.rs": ("part", "per-syscall cost table"),
    "kernel/src/dfx/taint.rs": ("done", "Linux taint letters"),
    "kernel/src/dfx/taskdump.rs": ("part", "task dump format"),
    "kernel/src/dfx/wildwatch.rs": ("part", "RO-guard tripwire"),
    "kernel/src/drivers/blkdev/iostat.rs": ("done", "written by this review"),
    "kernel/src/drivers/blkdev/mod.rs": ("part", "request path, register_disk"),
    "kernel/src/drivers/drm/mod.rs": ("todo", ""),
    "kernel/src/drivers/gpu/*": ("todo", ""),
    "kernel/src/drivers/input/*": ("todo", ""),
    "kernel/src/drivers/intc/*": ("todo", ""),
    "kernel/src/drivers/net/*": ("todo", ""),
    "kernel/src/drivers/pci/*": ("todo", ""),
    "kernel/src/drivers/timer/mod.rs": ("part", "jiffies/monotonic"),
    "kernel/src/drivers/timer/riscv64.rs": ("part", "time CSR timebase"),
    "kernel/src/drivers/timer/x86_64.rs": ("part", "TSC/HPET timebase"),
    "kernel/src/drivers/virtio/mod.rs": ("part", "virtio queue handling"),
    "kernel/src/drivers/virtio/offset.rs": ("todo", ""),
    "kernel/src/drivers/virtio/probe.rs": ("todo", ""),
    "kernel/src/drivers/virtio/queue.rs": ("todo", ""),
    "kernel/src/drivers/virtio/virtio_pci.rs": ("todo", ""),
    "kernel/src/fs/bio.rs": ("part", "sync_buffers, ordered publications"),
    "kernel/src/fs/buffer.rs": ("part", "FileBuffer chunked read/write"),
    "kernel/src/fs/cgroup.rs": ("todo", ""),
    "kernel/src/fs/char_dev.rs": ("todo", ""),
    "kernel/src/fs/debugfs.rs": ("done", "written by this review (debugfs)"),
    "kernel/src/fs/dentry.rs": ("part", "per-directory locking"),
    "kernel/src/fs/devfs/mod.rs": ("todo", ""),
    "kernel/src/fs/ext4/dir.rs": ("part", "directory ops"),
    "kernel/src/fs/ext4/extent.rs": ("todo", ""),
    "kernel/src/fs/ext4/file.rs": ("part", "read/write/fsync paths"),
    "kernel/src/fs/ext4/inode.rs": ("part", "inode table"),
    "kernel/src/fs/ext4/journal.rs": ("part", "jbd2 transaction start/stop"),
    "kernel/src/fs/ext4/mod.rs": ("part", "superblock, mount"),
    "kernel/src/fs/ext4/namei.rs": ("todo", ""),
    "kernel/src/fs/fifo.rs": ("todo", ""),
    "kernel/src/fs/file.rs": ("part", "reg_file_read/write/lseek、f_pos_lock；O_APPEND 原子性已修（327b4a52）"),
    "kernel/src/fs/inode.rs": ("part", "read_data/write_data/append_data；O_APPEND 原子性已修（327b4a52）"),
    "kernel/src/fs/inotify.rs": ("part", "event notification"),
    "kernel/src/fs/jbd2/checkpoint.rs": ("todo", ""),
    "kernel/src/fs/jbd2/commit.rs": ("todo", ""),
    "kernel/src/fs/jbd2/mod.rs": ("todo", ""),
    "kernel/src/fs/jbd2/recovery.rs": ("todo", ""),
    "kernel/src/fs/jbd2/transaction.rs": ("todo", ""),
    "kernel/src/fs/locks.rs": ("part", "flock/POSIX record locks"),
    "kernel/src/fs/memfd.rs": ("part", "memfd seal semantics"),
    "kernel/src/fs/page_cache.rs": ("part", "get_range_into/put_range"),
    "kernel/src/fs/path.rs": ("todo", ""),
    "kernel/src/fs/pipe.rs": ("todo", ""),
    "kernel/src/fs/procfs/cmdline.rs": ("todo", ""),
    "kernel/src/fs/procfs/configgz.rs": ("todo", ""),
    "kernel/src/fs/procfs/cpuinfo.rs": ("todo", ""),
    "kernel/src/fs/procfs/diskstats.rs": ("done", "written by this review"),
    "kernel/src/fs/procfs/interrupts.rs": ("part", "per-IRQ counters"),
    "kernel/src/fs/procfs/kallsyms.rs": ("done", "written by this review"),
    "kernel/src/fs/procfs/loadavg.rs": ("part", "load average"),
    "kernel/src/fs/procfs/locks.rs": ("done", "written by this review"),
    "kernel/src/fs/procfs/meminfo.rs": ("part", "meminfo fields"),
    "kernel/src/fs/procfs/mod.rs": ("part", "node tree, read/write dispatch"),
    "kernel/src/fs/procfs/mounts.rs": ("todo", ""),
    "kernel/src/fs/procfs/net.rs": ("todo", ""),
    "kernel/src/fs/procfs/pid.rs": ("part", "status/stat generators"),
    "kernel/src/fs/procfs/schedstat.rs": ("done", "written by this review"),
    "kernel/src/fs/procfs/self_proc.rs": ("todo", ""),
    "kernel/src/fs/procfs/softirqs.rs": ("done", "written by this review"),
    "kernel/src/fs/procfs/stat.rs": ("done", "written by this review"),
    "kernel/src/fs/procfs/swaps.rs": ("todo", ""),
    "kernel/src/fs/procfs/sysctl.rs": ("part", "sysctl tree"),
    "kernel/src/fs/procfs/sysrq.rs": ("done", "written by this review"),
    "kernel/src/fs/procfs/uptime.rs": ("part", "uptime/idle accounting"),
    "kernel/src/fs/procfs/vmstat.rs": ("done", "written by this review"),
    "kernel/src/fs/pty.rs": ("todo", ""),
    "kernel/src/fs/rootfs.rs": ("part", "symlink resolution, MAX_SYMLINKS"),
    "kernel/src/fs/selinuxfs.rs": ("todo", ""),
    "kernel/src/fs/stat.rs": ("todo", ""),
    "kernel/src/fs/sysfs.rs": ("part", "sysfs tree, writable attributes"),
    "kernel/src/fs/tmpfs.rs": ("part", "tmpfs read/write"),
    "kernel/src/fs/vfs.rs": ("part", "file_read/file_write dispatch"),
    "kernel/src/fs/xattr.rs": ("todo", ""),
    "kernel/src/interrupt/domain.rs": ("todo", ""),
    "kernel/src/interrupt/irqchip.rs": ("todo", ""),
    "kernel/src/interrupt/irqdesc.rs": ("part", "descriptor registry"),
    "kernel/src/interrupt/ksoftirqd.rs": ("part", "drain loop"),
    "kernel/src/interrupt/mod.rs": ("todo", ""),
    "kernel/src/interrupt/preempt.rs": ("todo", ""),
    "kernel/src/interrupt/softirq.rs": ("fixed", "2ms 预算改为逐 vector 检查（ddfa8abc）"),
    "kernel/src/interrupt/tasklet.rs": ("todo", ""),
    "kernel/src/io_uring/mod.rs": ("todo", ""),
    "kernel/src/ipc/access_token.rs": ("todo", ""),
    "kernel/src/ipc/binder.rs": ("part", "ioctl numbers, dispatch"),
    "kernel/src/ipc/mod.rs": ("todo", ""),
    "kernel/src/ipc/posix_mq.rs": ("part", "open/unlink/send/receive；绝对超时已修（e47b155a）"),
    "kernel/src/ipc/sysv_msg.rs": ("part", "msgsnd/msgrcv"),
    "kernel/src/ipc/sysv_sem.rs": ("part", "semtimedop timeout"),
    "kernel/src/ipc/sysv_shm.rs": ("part", "IPC_RMID, attach/detach"),
    "kernel/src/ipc/util.rs": ("done", "idr allocation, permission checks"),
    "kernel/src/main.rs": ("part", "boot sequence, panic handler, layout constants"),
    "kernel/src/mm/buddy_allocator.rs": ("part", "heap allocator, dealloc dispatch"),
    "kernel/src/mm/deferred_free.rs": ("todo", ""),
    "kernel/src/mm/memblock.rs": ("todo", ""),
    "kernel/src/mm/mm_struct.rs": ("todo", ""),
    "kernel/src/mm/mod.rs": ("todo", ""),
    "kernel/src/mm/page_alloc.rs": ("todo", ""),
    "kernel/src/mm/page_desc.rs": ("todo", ""),
    "kernel/src/mm/pglist.rs": ("todo", ""),
    "kernel/src/mm/rmap.rs": ("todo", ""),
    "kernel/src/mm/slab.rs": ("todo", ""),
    "kernel/src/mm/small_alloc.rs": ("part", "size classes, magazine, alloc/free"),
    "kernel/src/mm/vdso.rs": ("todo", ""),
    "kernel/src/mm/vma.rs": ("todo", ""),
    "kernel/src/mm/vmscan.rs": ("todo", ""),
    "kernel/src/mm/vmstat.rs": ("done", "written by this review (page counters)"),
    "kernel/src/mm/zone.rs": ("part", "buddy alloc/free, merge, watermarks"),
    "kernel/src/module/mod.rs": ("todo", ""),
    "kernel/src/net/*": ("todo", ""),
    "kernel/src/panic_policy.rs": ("done", "written by this review (panic= parameter)"),
    "kernel/src/printk.rs": ("part", "ring buffer, console level"),
    "kernel/src/process/coredump.rs": ("todo", ""),
    "kernel/src/process/exec.rs": ("todo", ""),
    "kernel/src/process/exit.rs": ("todo", ""),
    "kernel/src/process/fork.rs": ("part", "task creation paths"),
    "kernel/src/process/kthread.rs": ("todo", ""),
    "kernel/src/process/mod.rs": ("todo", ""),
    "kernel/src/process/pid.rs": ("todo", ""),
    "kernel/src/process/pid_hash.rs": ("part", "task iteration"),
    "kernel/src/process/ptrace.rs": ("todo", ""),
    "kernel/src/process/task.rs": ("part", "task layout, counters, comm"),
    "kernel/src/process/wait.rs": ("todo", ""),
    "kernel/src/sched/cgroup.rs": ("todo", ""),
    "kernel/src/sched/class.rs": ("todo", ""),
    "kernel/src/sched/deadline.rs": ("todo", ""),
    "kernel/src/sched/fair.rs": ("todo", ""),
    "kernel/src/sched/idle.rs": ("done", ""),
    "kernel/src/sched/mod.rs": ("todo", ""),
    "kernel/src/sched/rt.rs": ("part", "enqueue/dequeue/pick/bitmap/priority inversion"),
    "kernel/src/sched/sched.rs": ("part", "pick/switch path, enqueue/dequeue, preemption checks"),
    "kernel/src/sched/stop_task.rs": ("todo", ""),
    "kernel/src/sched/wakewatch.rs": ("todo", ""),
    "kernel/src/security/*": ("todo", ""),
    "kernel/src/sync/condvar.rs": ("todo", ""),
    "kernel/src/sync/futex.rs": ("part", "wait/wake/wake_op"),
    "kernel/src/sync/rcu.rs": ("todo", ""),
    "kernel/src/sync/rwlock.rs": ("todo", ""),
    "kernel/src/sync/semaphore.rs": ("todo", ""),
    "kernel/src/sync/seqlock.rs": ("todo", ""),
    "kernel/src/sync/spinlock.rs": ("todo", ""),
    "kernel/src/syscall/dispatch.rs": ("todo", ""),
    "kernel/src/syscall/file.rs": ("part", "futex2 wrappers, fsync"),
    "kernel/src/syscall/io.rs": ("todo", ""),
    "kernel/src/syscall/memory.rs": ("part", "msync/mprotect/madvise 长度处理；msync 回绕已修（ff6fc39b）"),
    "kernel/src/syscall/misc.rs": ("todo", ""),
    "kernel/src/syscall/mod.rs": ("todo", ""),
    "kernel/src/syscall/network.rs": ("todo", ""),
    "kernel/src/syscall/process.rs": ("part", "reboot/shutdown cascade"),
    "kernel/src/syscall/time.rs": ("part", "nanosleep, realtime clock"),
    "kernel/src/syscall/x86_compat.rs": ("todo", ""),
    "kernel/src/tests/*": ("todo", ""),
    "kernel/src/trace/files.rs": ("done", "written by this review (tracefs files)"),
    "kernel/src/trace/mod.rs": ("done", "written by this review (tracepoint core)"),
    "kernel/src/arch/aarch64/boot.rs": ("todo", ""),
    "kernel/src/arch/aarch64/context.rs": ("todo", ""),
    "kernel/src/arch/aarch64/cpu.rs": ("todo", ""),
    "kernel/src/arch/aarch64/ipi.rs": ("todo", ""),
    "kernel/src/arch/aarch64/mm/asid.rs": ("todo", ""),
    "kernel/src/arch/aarch64/mm/exception.rs": ("todo", ""),
    "kernel/src/arch/aarch64/mm/fixmap.rs": ("todo", ""),
    "kernel/src/arch/aarch64/mm/memory_layout.rs": ("todo", ""),
    "kernel/src/arch/aarch64/mm/mm_ops.rs": ("todo", ""),
    "kernel/src/arch/aarch64/mm/mmu_init.rs": ("todo", ""),
    "kernel/src/arch/aarch64/mm/mod.rs": ("todo", ""),
    "kernel/src/arch/aarch64/mm/page_fault.rs": ("todo", ""),
    "kernel/src/arch/aarch64/mm/pagetable.rs": ("todo", ""),
    "kernel/src/arch/aarch64/mod.rs": ("todo", ""),
    "kernel/src/arch/aarch64/process.rs": ("todo", ""),
    "kernel/src/arch/aarch64/pt_regs.rs": ("todo", ""),
    "kernel/src/arch/aarch64/smp.rs": ("todo", ""),
    "kernel/src/arch/aarch64/thread.rs": ("todo", ""),
    "kernel/src/arch/aarch64/trap.rs": ("todo", ""),
    "kernel/src/arch/aarch64/uaccess.rs": ("todo", ""),
    "kernel/src/arch/mod.rs": ("todo", ""),
    "kernel/src/arch/riscv64/boot.S": ("todo", ""),
    "kernel/src/arch/riscv64/boot.rs": ("todo", ""),
    "kernel/src/arch/riscv64/context.rs": ("todo", ""),
    "kernel/src/arch/riscv64/cpu.rs": ("todo", ""),
    "kernel/src/arch/riscv64/ipi.rs": ("todo", ""),
    "kernel/src/arch/riscv64/mm/asid.rs": ("todo", ""),
    "kernel/src/arch/riscv64/mm/exception.rs": ("todo", ""),
    "kernel/src/arch/riscv64/mm/fixmap.rs": ("todo", ""),
    "kernel/src/arch/riscv64/mm/mm_ops.rs": ("todo", ""),
    "kernel/src/arch/riscv64/mm/mod.rs": ("todo", ""),
    "kernel/src/arch/riscv64/mm/page_fault.rs": ("todo", ""),
    "kernel/src/arch/riscv64/mm/pagetable.rs": ("todo", ""),
    "kernel/src/arch/riscv64/mod.rs": ("todo", ""),
    "kernel/src/arch/riscv64/process.rs": ("todo", ""),
    "kernel/src/arch/riscv64/pt_regs.rs": ("todo", ""),
    "kernel/src/arch/riscv64/smp.rs": ("todo", ""),
    "kernel/src/arch/riscv64/thread.rs": ("todo", ""),
    "kernel/src/arch/riscv64/trap.S": ("todo", ""),
    "kernel/src/arch/riscv64/uaccess.S": ("todo", ""),
    "kernel/src/arch/riscv64/uaccess.rs": ("todo", ""),
    "kernel/src/arch/x86_64/boot.S": ("todo", ""),
    "kernel/src/arch/x86_64/boot.rs": ("todo", ""),
    "kernel/src/arch/x86_64/context.rs": ("todo", ""),
    "kernel/src/arch/x86_64/cpu.rs": ("todo", ""),
    "kernel/src/arch/x86_64/ipi.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/asid.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/exception.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/fixmap.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/mm_ops.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/mod.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/page_fault.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/pagetable.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mod.rs": ("todo", ""),
    "kernel/src/arch/x86_64/process.rs": ("todo", ""),
    "kernel/src/arch/x86_64/pt_regs.rs": ("todo", ""),
    "kernel/src/arch/x86_64/setup.S": ("todo", ""),
    "kernel/src/arch/x86_64/smp.rs": ("todo", ""),
    "kernel/src/arch/x86_64/smp_trampoline.S": ("todo", ""),
    "kernel/src/arch/x86_64/thread.rs": ("todo", ""),
    "kernel/src/arch/x86_64/trap.S": ("todo", ""),
    "kernel/src/arch/x86_64/uaccess.rs": ("todo", ""),
    "kernel/src/config.rs": ("todo", ""),
    "kernel/src/console.rs": ("todo", ""),
    "kernel/src/dfx/fsw.rs": ("todo", ""),
    "kernel/src/dfx/ksyms_generated.rs": ("todo", ""),
    "kernel/src/dfx/schb.rs": ("todo", ""),
    "kernel/src/drivers/access_tokenid.rs": ("todo", ""),
    "kernel/src/drivers/ashmem.rs": ("todo", ""),
    "kernel/src/drivers/gpu/fb_simple.rs": ("todo", ""),
    "kernel/src/drivers/gpu/fbdev.rs": ("todo", ""),
    "kernel/src/drivers/gpu/framebuffer.rs": ("todo", ""),
    "kernel/src/drivers/gpu/mod.rs": ("todo", ""),
    "kernel/src/drivers/gpu/virtio_cmd.rs": ("todo", ""),
    "kernel/src/drivers/gpu/virtio_gpu.rs": ("todo", ""),
    "kernel/src/drivers/input/evdev.rs": ("todo", ""),
    "kernel/src/drivers/input/event.rs": ("todo", ""),
    "kernel/src/drivers/input/mod.rs": ("todo", ""),
    "kernel/src/drivers/input/ps2.rs": ("todo", ""),
    "kernel/src/drivers/input/virtio_input.rs": ("todo", ""),
    "kernel/src/drivers/intc/apic.rs": ("todo", ""),
    "kernel/src/drivers/intc/clint.rs": ("todo", ""),
    "kernel/src/drivers/intc/mod.rs": ("todo", ""),
    "kernel/src/drivers/intc/plic.rs": ("todo", ""),
    "kernel/src/drivers/loop_dev.rs": ("todo", ""),
    "kernel/src/drivers/mod.rs": ("todo", ""),
    "kernel/src/drivers/net/loopback.rs": ("todo", ""),
    "kernel/src/drivers/net/mod.rs": ("todo", ""),
    "kernel/src/drivers/net/space.rs": ("todo", ""),
    "kernel/src/drivers/net/virtio_net.rs": ("todo", ""),
    "kernel/src/drivers/pci/mod.rs": ("todo", ""),
    "kernel/src/drivers/rtc.rs": ("todo", ""),
    "kernel/src/drivers/timer/hpet.rs": ("todo", ""),
    "kernel/src/drivers/watchdog.rs": ("todo", ""),
    "kernel/src/errno.rs": ("todo", ""),
    "kernel/src/fs/dev_t.rs": ("todo", ""),
    "kernel/src/fs/devfs/registry.rs": ("todo", ""),
    "kernel/src/fs/elf.rs": ("todo", ""),
    "kernel/src/fs/ext4/allocator.rs": ("todo", ""),
    "kernel/src/fs/ext4/csum.rs": ("todo", ""),
    "kernel/src/fs/ext4/indirect.rs": ("todo", ""),
    "kernel/src/fs/ext4/superblock.rs": ("todo", ""),
    "kernel/src/fs/fs_struct.rs": ("todo", ""),
    "kernel/src/fs/io_completion.rs": ("todo", ""),
    "kernel/src/fs/jbd2/journal.rs": ("todo", ""),
    "kernel/src/fs/jbd2/revoke.rs": ("todo", ""),
    "kernel/src/fs/jbd2/types.rs": ("todo", ""),
    "kernel/src/fs/mod.rs": ("todo", ""),
    "kernel/src/fs/mount.rs": ("todo", ""),
    "kernel/src/fs/permission.rs": ("todo", ""),
    "kernel/src/fs/procfs/timer_list.rs": ("todo", ""),
    "kernel/src/fs/procfs/version.rs": ("todo", ""),
    "kernel/src/fs/readahead.rs": ("todo", ""),
    "kernel/src/fs/superblock.rs": ("todo", ""),
    "kernel/src/fs/tty.rs": ("todo", ""),
    "kernel/src/init.rs": ("todo", ""),
    "kernel/src/initrd.rs": ("todo", ""),
    "kernel/src/list.rs": ("todo", ""),
    "kernel/src/mm/allocator.rs": ("todo", ""),
    "kernel/src/mm/compact.rs": ("todo", ""),
    "kernel/src/mm/hugepage.rs": ("todo", ""),
    "kernel/src/mm/kswapd.rs": ("todo", ""),
    "kernel/src/mm/layout.rs": ("todo", ""),
    "kernel/src/mm/lru.rs": ("todo", ""),
    "kernel/src/mm/meminfo.rs": ("todo", ""),
    "kernel/src/mm/oom_kill.rs": ("todo", ""),
    "kernel/src/mm/page.rs": ("todo", ""),
    "kernel/src/mm/pagemap.rs": ("todo", ""),
    "kernel/src/mm/swap.rs": ("todo", ""),
    "kernel/src/mm/vmemmap.rs": ("todo", ""),
    "kernel/src/net/arp.rs": ("todo", ""),
    "kernel/src/net/buffer.rs": ("todo", ""),
    "kernel/src/net/ethernet.rs": ("todo", ""),
    "kernel/src/net/icmp.rs": ("todo", ""),
    "kernel/src/net/ipv4/checksum.rs": ("todo", ""),
    "kernel/src/net/ipv4/defrag.rs": ("todo", ""),
    "kernel/src/net/ipv4/mod.rs": ("todo", ""),
    "kernel/src/net/ipv4/route.rs": ("todo", ""),
    "kernel/src/net/ipv6/icmpv6.rs": ("todo", ""),
    "kernel/src/net/ipv6/mod.rs": ("todo", ""),
    "kernel/src/net/mod.rs": ("todo", ""),
    "kernel/src/net/netlink.rs": ("todo", ""),
    "kernel/src/net/raw.rs": ("todo", ""),
    "kernel/src/net/socket.rs": ("todo", ""),
    "kernel/src/net/tcp.rs": ("todo", ""),
    "kernel/src/net/tcp_timer.rs": ("todo", ""),
    "kernel/src/net/udp.rs": ("todo", ""),
    "kernel/src/net/unix.rs": ("todo", ""),
    "kernel/src/print.rs": ("todo", ""),
    "kernel/src/process/ns.rs": ("todo", ""),
    "kernel/src/sbi.rs": ("todo", ""),
    "kernel/src/security/cap_lsm.rs": ("todo", ""),
    "kernel/src/security/capability.rs": ("todo", ""),
    "kernel/src/security/lsm.rs": ("todo", ""),
    "kernel/src/security/mod.rs": ("todo", ""),
    "kernel/src/signal.rs": ("todo", ""),
    "kernel/src/sync/mod.rs": ("todo", ""),
    "kernel/src/syscall/sched.rs": ("todo", ""),
    "kernel/src/syscall/signal.rs": ("todo", ""),
    "kernel/src/tests/boundary.rs": ("todo", ""),
    "kernel/src/tests/buffer_state.rs": ("todo", ""),
    "kernel/src/tests/checksum.rs": ("todo", ""),
    "kernel/src/tests/config_test.rs": ("todo", ""),
    "kernel/src/tests/dcache.rs": ("todo", ""),
    "kernel/src/tests/dev_t.rs": ("todo", ""),
    "kernel/src/tests/errno_test.rs": ("todo", ""),
    "kernel/src/tests/execve.rs": ("todo", ""),
    "kernel/src/tests/ext4_allocator.rs": ("todo", ""),
    "kernel/src/tests/ext4_file_write.rs": ("todo", ""),
    "kernel/src/tests/fcntl.rs": ("todo", ""),
    "kernel/src/tests/fdtable.rs": ("todo", ""),
    "kernel/src/tests/file_flags.rs": ("todo", ""),
    "kernel/src/tests/file_open.rs": ("todo", ""),
    "kernel/src/tests/fork.rs": ("todo", ""),
    "kernel/src/tests/framebuffer.rs": ("todo", ""),
    "kernel/src/tests/fstat.rs": ("todo", ""),
    "kernel/src/tests/futex_test.rs": ("todo", ""),
    "kernel/src/tests/getpid.rs": ("todo", ""),
    "kernel/src/tests/heap_allocator.rs": ("todo", ""),
    "kernel/src/tests/icache.rs": ("todo", ""),
    "kernel/src/tests/initrd.rs": ("todo", ""),
    "kernel/src/tests/ipc_epoll.rs": ("todo", ""),
    "kernel/src/tests/ipc_eventfd.rs": ("todo", ""),
    "kernel/src/tests/ipc_poll.rs": ("todo", ""),
    "kernel/src/tests/ipc_sysv.rs": ("todo", ""),
    "kernel/src/tests/link.rs": ("todo", ""),
    "kernel/src/tests/listhead.rs": ("todo", ""),
    "kernel/src/tests/loop_flags.rs": ("todo", ""),
    "kernel/src/tests/mem_cow.rs": ("todo", ""),
    "kernel/src/tests/mem_mmap.rs": ("todo", ""),
    "kernel/src/tests/mkdir_unlink.rs": ("todo", ""),
    "kernel/src/tests/mod.rs": ("todo", ""),
    "kernel/src/tests/mount_flags.rs": ("todo", ""),
    "kernel/src/tests/network.rs": ("todo", ""),
    "kernel/src/tests/page_allocator.rs": ("todo", ""),
    "kernel/src/tests/path.rs": ("todo", ""),
    "kernel/src/tests/pid_test.rs": ("todo", ""),
    "kernel/src/tests/pipe2.rs": ("todo", ""),
    "kernel/src/tests/preemptive_scheduler.rs": ("todo", ""),
    "kernel/src/tests/process_tree.rs": ("todo", ""),
    "kernel/src/tests/reparent_safety.rs": ("todo", ""),
    "kernel/src/tests/scheduler.rs": ("todo", ""),
    "kernel/src/tests/semaphore.rs": ("todo", ""),
    "kernel/src/tests/sgid_mode.rs": ("todo", ""),
    "kernel/src/tests/signal.rs": ("todo", ""),
    "kernel/src/tests/signal_procmask.rs": ("todo", ""),
    "kernel/src/tests/sleep_wakeup.rs": ("todo", ""),
    "kernel/src/tests/small_alloc.rs": ("todo", ""),
    "kernel/src/tests/smp.rs": ("todo", ""),
    "kernel/src/tests/smp_schedule.rs": ("todo", ""),
    "kernel/src/tests/syscall_file.rs": ("todo", ""),
    "kernel/src/tests/syscall_io.rs": ("todo", ""),
    "kernel/src/tests/syscall_memory.rs": ("todo", ""),
    "kernel/src/tests/syscall_misc.rs": ("todo", ""),
    "kernel/src/tests/syscall_network.rs": ("todo", ""),
    "kernel/src/tests/syscall_process.rs": ("todo", ""),
    "kernel/src/tests/syscall_sched.rs": ("todo", ""),
    "kernel/src/tests/syscall_signal.rs": ("todo", ""),
    "kernel/src/tests/syscall_time.rs": ("todo", ""),
    "kernel/src/tests/tcp_handshake.rs": ("todo", ""),
    "kernel/src/tests/tmpfs_mknod.rs": ("todo", ""),
    "kernel/src/tests/virtio_net.rs": ("todo", ""),
    "kernel/src/tests/virtio_queue.rs": ("todo", ""),
    "kernel/src/tests/vma_flags.rs": ("todo", ""),
    "kernel/src/tests/wait4.rs": ("todo", ""),
    "kernel/src/timer.rs": ("todo", ""),}

# Bugs found and fixed by this review line.
BUGS = [
    ("ddfa8abc", "kernel/src/interrupt/softirq.rs",
     "The 2ms softirq budget was checked once per batch of pending vectors, so a batch of "
     "slow handlers could overrun it by the time of every vector left in the batch. Linux "
     "checks the budget before every vector and leaves the unreached ones pending."),
    ("327b4a52", "kernel/src/fs/inode.rs, kernel/src/fs/file.rs",
     "The generic O_APPEND write read the file size, released the lock and only then took "
     "the data lock to write — two appenders on different open descriptions could pick the "
     "same offset, and the second write overwrote the first. Linux takes the append "
     "position under the inode lock."),
    ("ff6fc39b", "kernel/src/syscall/memory.rs",
     "msync had no 'addr + len' wrap check (Linux's mm/msync.c has one). A near-max length "
     "wrapped the page-aligned length to 0, so msync reported success without syncing "
     "anything; Linux returns EINVAL."),
    ("e47b155a", "kernel/src/ipc/posix_mq.rs",
     "mq_timedreceive/mq_timedsend take an ABSOLUTE CLOCK_REALTIME deadline (abs_timeout), "
     "but Rux read the value as RELATIVE — a caller passing an absolute deadline waited "
     "twice as long, and an already-expired deadline blocked for the full timeout instead "
     "of returning ETIMEDOUT at once."),
]

MARKS = {"done": "✅", "fixed": "🔧", "part": "🔶", "todo": "⬜"}


def collect():
    """Every source file under kernel/src with its line count."""
    out = []
    for dirpath, _dirs, files in os.walk(SRC):
        for name in sorted(files):
            if not (name.endswith(".rs") or name.endswith(".S")):
                continue
            path = os.path.join(dirpath, name)
            rel = os.path.relpath(path, ROOT)
            with open(path, "rb") as handle:
                lines = sum(1 for _ in handle)
            out.append((rel, lines))
    return sorted(out)


def main():
    check = "--check" in sys.argv
    files = collect()
    by_path = {rel: lines for rel, lines in files}

    # Drift detection: a file in the tree that the map does not know.
    unknown = [rel for rel, _ in files if rel not in STATUS and "*" not in rel]
    # A map entry that no longer exists.
    stale = [rel for rel in STATUS if "*" not in rel and rel not in by_path]

    if check:
        if unknown:
            print("map is missing entries for:")
            for rel in unknown:
                print("  " + rel)
        if stale:
            print("map has entries for files that no longer exist:")
            for rel in stale:
                print("  " + rel)
        return 1 if (unknown or stale) else 0

    # Per-directory tables.
    groups = {}
    for rel, lines in files:
        d = os.path.dirname(rel)
        groups.setdefault(d, []).append((rel, lines))

    counts = {"done": 0, "part": 0, "todo": 0, "fixed": 0}
    for rel, _lines in files:
        status = STATUS.get(rel, ("todo", ""))[0]
        counts[status] = counts.get(status, 0) + 1

    total_lines = sum(lines for _rel, lines in files)
    reviewed_lines = 0
    for rel, lines in files:
        if STATUS.get(rel, ("todo", ""))[0] in ("done", "fixed"):
            reviewed_lines += lines

    out = []
    out.append("# Kernel Review Coverage Map")
    out.append("")
    out.append("Bookkeeping for the line-by-line kernel review: every source file")
    out.append("under `kernel/src` has a status, so nothing is silently skipped.")
    out.append("")
    out.append("Generated by `scripts/review_coverage.py` — edit the STATUS table")
    out.append("there, not this file. `scripts/review_coverage.py --check` fails")
    out.append("when the tree gains a file the map does not know about.")
    out.append("")
    out.append("## Legend")
    out.append("")
    out.append("| mark | meaning |")
    out.append("|---|---|")
    out.append("| ✅ done | read end to end, no defect found |")
    out.append("| 🔧 fixed | read end to end, a defect was found and fixed (see below) |")
    out.append("| 🔶 part | read in part; the note says what was covered |")
    out.append("| ⬜ todo | not read yet |")
    out.append("")
    out.append("## Progress")
    out.append("")
    out.append("| metric | value |")
    out.append("|---|---|")
    out.append(f"| source files in kernel/src | {len(files)} |")
    out.append(f"| source lines in kernel/src | {total_lines:,} |")
    out.append(f"| files fully reviewed | {counts.get('done', 0) + counts.get('fixed', 0)} |")
    out.append(f"| files partially reviewed | {counts.get('part', 0)} |")
    out.append(f"| files not yet reviewed | {counts.get('todo', 0)} |")
    out.append(f"| lines in fully reviewed files | {reviewed_lines:,} |")
    out.append("")
    out.append("## Defects found and fixed")
    out.append("")
    out.append("| commit | file | defect |")
    out.append("|---|---|---|")
    for commit, path, text in BUGS:
        out.append(f"| `{commit}` | `{path}` | {text} |")
    out.append("")
    out.append("## Per-file status")
    out.append("")
    for d in sorted(groups):
        out.append(f"### {d} ({sum(l for _r, l in groups[d])} lines)")
        out.append("")
        out.append("| file | lines | status | note |")
        out.append("|---|---:|---|---|")
        for rel, lines in groups[d]:
            status, note = STATUS.get(rel, ("todo", ""))
            mark = MARKS.get(status, "⬜")
            out.append(f"| `{os.path.basename(rel)}` | {lines} | {mark} | {note} |")
        out.append("")

    with open(OUT, "w") as handle:
        handle.write("\n".join(out))
    print("wrote %s (%d files tracked)" % (OUT, len(files)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
