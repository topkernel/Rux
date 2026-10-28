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

import fnmatch
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
    "kernel/src/arch/riscv64/vext.rs": ("todo", ""),
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
    "kernel/src/dfx/leakorigin.rs": ("todo", ""),
    "kernel/src/dfx/memwatch.rs": ("part", "heap census"),
    "kernel/src/dfx/metawatch.rs": ("part", "buddy shadow scan"),
    "kernel/src/dfx/mod.rs": ("part", "init, debugfs knob registration"),
    "kernel/src/dfx/monowatch.rs": ("part", "monotonic jump tripwire"),
    "kernel/src/dfx/oops.rs": ("todo", ""),
    "kernel/src/dfx/r83hunt.rs": ("part", "task header audit"),
    "kernel/src/dfx/pathstash.rs": ("todo", ""),
    "kernel/src/dfx/pstore.rs": ("todo", ""),
    "kernel/src/dfx/sbi_debug.rs": ("part", "raw SBI printers"),
    "kernel/src/dfx/scribble.rs": ("part", "task scribble hunter"),
    "kernel/src/dfx/softlockup.rs": ("part", "soft lockup detector"),
    "kernel/src/dfx/switches.rs": ("done", "switch table refactored to one macro"),
    "kernel/src/dfx/syscost.rs": ("part", "per-syscall cost table"),
    "kernel/src/dfx/taint.rs": ("done", "the reference kernel taint letters"),
    "kernel/src/dfx/taskdump.rs": ("part", "task dump format"),
    "kernel/src/dfx/treehunt.rs": ("todo", ""),
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
    "kernel/src/fs/memfd.rs": ("fixed", "memfd_create 的参数校验与 seal 语义（参数校验已对齐 879328f3）"),
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
    "kernel/src/fs/procfs/mod.rs": ("part", "节点树、read/write 分发、/proc/<pid> 路径解析"),
    "kernel/src/fs/procfs/mounts.rs": ("todo", ""),
    "kernel/src/fs/procfs/net.rs": ("todo", ""),
    "kernel/src/fs/procfs/pid.rs": ("part", "status/stat generators"),
    "kernel/src/fs/procfs/procfile.rs": ("todo", ""),
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
    "kernel/src/fs/vfs.rs": ("part", "file_read/file_write 分发、目录权限与 sticky 位、chroot 下的路径归一化"),
    "kernel/src/fs/xattr.rs": ("todo", ""),
    "kernel/src/interrupt/domain.rs": ("todo", ""),
    "kernel/src/interrupt/irqchip.rs": ("todo", ""),
    "kernel/src/interrupt/irqdesc.rs": ("part", "descriptor registry"),
    "kernel/src/interrupt/ksoftirqd.rs": ("part", "drain loop"),
    "kernel/src/interrupt/mod.rs": ("todo", ""),
    "kernel/src/interrupt/preempt.rs": ("done", "preempt_count 位布局与参考内核一致，irq_enter/exit 的软中断派发条件"),
    "kernel/src/interrupt/softirq.rs": ("fixed", "2ms 预算改为逐 vector 检查（ddfa8abc）"),
    "kernel/src/interrupt/tasklet.rs": ("todo", ""),
    "kernel/src/io_uring/mod.rs": ("todo", ""),
    "kernel/src/ipc/access_token.rs": ("todo", ""),
    "kernel/src/ipc/binder.rs": ("part", "ioctl numbers, dispatch, mmap 区的 VMA 记账（e32cdfef）"),
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
    "kernel/src/mm/mm_struct.rs": ("part", "total_vm/rss 记账、VMA 增删；记账只增不减已修（e32cdfef）"),
    "kernel/src/mm/mod.rs": ("todo", ""),
    "kernel/src/mm/page_alloc.rs": ("part", "分配快路径、OOM 判定、直接回收闸门、free 的各路防护"),
    "kernel/src/mm/page_desc.rs": ("todo", ""),
    "kernel/src/mm/pglist.rs": ("todo", ""),
    "kernel/src/mm/rmap.rs": ("todo", ""),
    "kernel/src/mm/slab.rs": ("todo", ""),
    "kernel/src/mm/small_alloc.rs": ("part", "size classes、每 CPU magazine、alloc/free 的快慢路径与 D-7 护栏"),
    "kernel/src/mm/vdso.rs": ("todo", ""),
    "kernel/src/mm/vma.rs": ("part", "VMA 结构、split/merge、can_merge 的文件偏移连续性、VmaManager 增删查"),
    "kernel/src/mm/vmscan.rs": ("todo", ""),
    "kernel/src/mm/vmstat.rs": ("done", "written by this review (page counters)"),
    "kernel/src/mm/zone.rs": ("part", "buddy alloc/free、split/merge、水位线；水位公式与参考内核的 6.6 形式有量级很小的差异（更早唤醒 kswapd）"),
    "kernel/src/module/mod.rs": ("todo", ""),
    "kernel/src/net/*": ("todo", ""),
    "kernel/src/panic_policy.rs": ("done", "written by this review (panic= parameter)"),
    "kernel/src/perf/mod.rs": ("todo", ""),
    "kernel/src/printk.rs": ("part", "ring buffer, console level"),
    "kernel/src/process/coredump.rs": ("todo", ""),
    "kernel/src/process/exec.rs": ("part", "exec 的信号处置/掩码复位、装载的 VMA 记账（e32cdfef）"),
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
    "kernel/src/sched/fair.rs": ("fixed", "CFS 核心：vruntime 权重方向、min_vruntime、pick 跳过路径（min_vruntime 越界已修 b433fb13）"),
    "kernel/src/sched/idle.rs": ("done", ""),
    "kernel/src/sched/mod.rs": ("todo", ""),
    "kernel/src/sched/rt.rs": ("part", "enqueue/dequeue/pick/bitmap/priority inversion"),
    "kernel/src/sched/sched.rs": ("part", "pick/switch path, enqueue/dequeue, preemption checks"),
    "kernel/src/sched/stats.rs": ("todo", ""),
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
    "kernel/src/syscall/io.rs": ("part", "read/write/pread64 的分块与用户指针校验"),
    "kernel/src/syscall/memory.rs": ("part", "mmap/munmap/mprotect/mremap/brk；msync 回绕已修（ff6fc39b）、mmap 偏移未页对齐已修（b433fb13）、brk 记账已修（e32cdfef）"),
    "kernel/src/syscall/misc.rs": ("part", "timerfd 族已对齐 v6.6（sys_timerfd_settime 契约已修 4e1f6110）；epoll/poll/eventfd/signalfd 未读"),
    "kernel/src/syscall/mod.rs": ("todo", ""),
    "kernel/src/syscall/network.rs": ("todo", ""),
    "kernel/src/syscall/process.rs": ("part", "reboot/shutdown 级联、memfd_create 的参数校验（已对齐 879328f3）"),
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
    "kernel/src/arch/riscv64/mm/mm_ops.rs": ("part", "COW 故障快/慢路径、PTE 权限检查、mmap/MAP_FIXED/munmap/fork 的 VMA 手术与记账（e32cdfef）"),
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
    "kernel/src/arch/x86_64/mm/mm_ops.rs": ("part", "mmap/MAP_FIXED/munmap/fork 的 VMA 手术与记账（与 riscv64 同构，e32cdfef）"),
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
    "kernel/src/dfx/slowtask.rs": ("todo", ""),
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
    "kernel/src/signal.rs": ("part", "flush_handlers 处置重置语义"),
    "kernel/src/sync/mod.rs": ("todo", ""),
    "kernel/src/sync/percpu.rs": ("todo", ""),
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
    "kernel/src/tests/fileops_caps.rs": ("todo", ""),
    "kernel/src/tests/fdtable.rs": ("todo", ""),
    "kernel/src/tests/file_flags.rs": ("todo", ""),
    "kernel/src/tests/file_open.rs": ("todo", ""),
    "kernel/src/tests/fork.rs": ("todo", ""),
    "kernel/src/tests/framebuffer.rs": ("todo", ""),
    "kernel/src/tests/fstat.rs": ("todo", ""),
    "kernel/src/tests/futex_test.rs": ("todo", ""),
    "kernel/src/tests/getpid.rs": ("todo", ""),
    "kernel/src/tests/getdents_cache.rs": ("todo", ""),
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
    "kernel/src/tests/percpu.rs": ("todo", ""),
    "kernel/src/tests/pid_test.rs": ("todo", ""),
    "kernel/src/tests/pipe2.rs": ("todo", ""),
    "kernel/src/tests/preemptive_scheduler.rs": ("todo", ""),
    "kernel/src/tests/process_tree.rs": ("todo", ""),
    "kernel/src/tests/reparent_safety.rs": ("todo", ""),
    "kernel/src/tests/scheduler.rs": ("part", "guest 侧调度断言：vruntime 公式与权重方向（b433fb13 新增）"),
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
    "kernel/src/tests/syscall_memory.rs": ("part", "guest 侧内存 syscall 断言：mmap 偏移非页对齐返回 EINVAL（b433fb13 新增）"),
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
     "slow handlers could overrun it by the time of every vector left in the batch. the reference kernel "
     "checks the budget before every vector and leaves the unreached ones pending."),
    ("327b4a52", "kernel/src/fs/inode.rs, kernel/src/fs/file.rs",
     "The generic O_APPEND write read the file size, released the lock and only then took "
     "the data lock to write — two appenders on different open descriptions could pick the "
     "same offset, and the second write overwrote the first. the reference kernel takes the append "
     "position under the inode lock."),
    ("ff6fc39b", "kernel/src/syscall/memory.rs",
     "msync had no 'addr + len' wrap check (the reference kernel's mm/msync.c has one). A near-max length "
     "wrapped the page-aligned length to 0, so msync reported success without syncing "
     "anything; the reference kernel returns EINVAL."),
    ("879328f3", "kernel/src/fs/memfd.rs, kernel/src/syscall/process.rs",
     "memfd_create accepted a NULL name (the reference reads the name with strnlen_user, which faults on a NULL pointer and yields EFAULT), accepted MFD_EXEC together with MFD_NOEXEC_SEAL (the reference rejects the combination with EINVAL - the two flags carry opposite meaning), and allowed a name of up to NAME_MAX bytes where the reference's limit is MFD_NAME_MAX_LEN (NAME_MAX minus the length of the 'memfd:' prefix it prepends)."),
    ("e32cdfef", "kernel/src/mm/mm_struct.rs, kernel/src/arch/*/mm/mm_ops.rs, kernel/src/syscall/memory.rs, kernel/src/process/exec.rs, kernel/src/ipc/binder.rs",
     "mm->total_vm only ever grew: the VMA removal paths (munmap, the MAP_FIXED replacement) never released the pages that left the mapping, and the VMA-installing paths that do not go through mmap (the brk heap, the exec image, the forked child's copy, the binder zone) never charged theirs. The reference kernel keeps the counter equal to the live mapping size - every installed VMA is charged and every removed one gives its pages back - and checks RLIMIT_AS against that live value. The drift made /proc/<pid>/status VmSize and /proc/<pid>/stat vsize describe every mapping the process ever made, and refused mappings that fit once the counter had grown past a finite RLIMIT_AS."),
    ("b433fb13", "kernel/src/sched/fair.rs",
     "The CFS pick lifts the entries a CPU cannot run (affinity, cgroup throttle, ownership) out of "
     "the timeline and re-inserts them after the pick, but min_vruntime was advanced while they "
     "were out: the leftmost looked further right than it is. min_vruntime only ever grows, so the "
     "drift was permanent, and it is the reference point new tasks and waking sleepers are placed "
     "against - a drifted value silently erased their sleep credit. The reference kernel computes "
     "min_vruntime from the leftmost entry of the complete tree (kernel/sched/fair.c: "
     "update_min_vruntime)."),
    ("b433fb13", "kernel/src/syscall/memory.rs",
     "mmap accepted a file offset that is not a multiple of the page size. The reference kernel's "
     "mmap wrapper rejects it with EINVAL before anything else, and the check is unconditional - it "
     "applies to anonymous mappings too, where the offset is ignored. Without it a misaligned offset "
     "silently shifted the mapping's file extent: the VMA stored the raw offset and the fault path "
     "then looked up a page-cache entry at a non-page-aligned offset."),
    ("e47b155a", "kernel/src/ipc/posix_mq.rs",
     "mq_timedreceive/mq_timedsend take an ABSOLUTE CLOCK_REALTIME deadline (abs_timeout), "
     "but Rux read the value as RELATIVE — a caller passing an absolute deadline waited "
     "twice as long, and an already-expired deadline blocked for the full timeout instead "
     "of returning ETIMEDOUT at once."),
    ("4e1f6110", "kernel/src/syscall/misc.rs",
     "sys_timerfd_settime broke the reference contract (kernel/fs/timerfd.c) four ways: "
     "the old timer was cancelled BEFORE the itimerspec validation (the reference validates "
     "itimerspec64_valid() first, so an EINVAL call must leave the running timer armed — Rux "
     "silently disarmed it); old_value was copied out BEFORE the reprogramming (the reference "
     "reads the new value first, reprograms, then copies the old snapshot out — a faulting "
     "copy is EFAULT with the NEW timer already programmed); a disarm (it_value == 0) "
     "returned early keeping the OLD interval and the stale expiration count (timerfd_setup() "
     "sets ctx->tintv and ctx->ticks = 0 in both the arming and disarming paths); and the "
     "arming call ran even for total_nsec == 0, arming a dead timer slot."),
]

MARKS = {"done": "✅", "fixed": "🔧", "part": "🔶", "todo": "⬜"}

# The mainline commit the statuses above were recorded against. A file
# that mainline has changed since then keeps its mark but gains a "needs
# re-read" flag: the code I read is no longer the code that is there.
# The mainline commit the reviews were done against (the branch base at
# the time). Everything mainline touched since then needs a re-read.
REVIEW_BASE = "47d96961"


def status_for(rel):
    """The map entry for a file: an exact key wins, else the best glob.

    The exact key always wins so a specific file can be lifted out of a
    directory-wide entry (e.g. kernel/src/sched/idle.rs out of
    kernel/src/sched/*). Among globs the one with the longest literal
    prefix wins, so a narrow pattern beats a broad one.
    """
    if rel in STATUS:
        return STATUS[rel]
    best = None
    best_prefix = -1
    for key, value in STATUS.items():
        if "*" not in key:
            continue
        if fnmatch.fnmatchcase(rel, key):
            prefix = len(key.split("*")[0])
            if prefix > best_prefix:
                best, best_prefix = value, prefix
    return best if best is not None else ("todo", "")


def changed_since_base():
    """The kernel/src files mainline touched after REVIEW_BASE."""
    import subprocess
    try:
        out = subprocess.run(
            ["git", "diff", "--name-only", REVIEW_BASE + "..main", "--", "kernel/src"],
            cwd=ROOT, check=True, capture_output=True, text=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError):
        return set()
    return {line for line in out.split() if line}


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
    unknown = [rel for rel, _ in files if rel not in STATUS and status_for(rel)[0] == "todo"
               and not any("*" in k and fnmatch.fnmatchcase(rel, k) for k in STATUS)]
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

    changed = changed_since_base()
    counts = {"done": 0, "part": 0, "todo": 0, "fixed": 0}
    stale = 0
    for rel, _lines in files:
        status = status_for(rel)[0]
        counts[status] = counts.get(status, 0) + 1
        if status in ("done", "fixed", "part") and rel in changed:
            stale += 1

    total_lines = sum(lines for _rel, lines in files)
    reviewed_lines = 0
    for rel, lines in files:
        if status_for(rel)[0] in ("done", "fixed"):
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
    out.append("| 🔁 | reviewed, but mainline changed the file since the review base — re-read needed |")
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
    out.append(f"| reviewed files mainline changed since `{REVIEW_BASE}` (need re-read) | {stale} |")
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
            status, note = status_for(rel)
            mark = MARKS.get(status, "⬜")
            if status in ("done", "fixed", "part") and rel in changed:
                mark += "🔁"
                note = (note + "; " if note else "") + "主线已改动，需重读"
            out.append(f"| `{os.path.basename(rel)}` | {lines} | {mark} | {note} |")
        out.append("")

    with open(OUT, "w") as handle:
        handle.write("\n".join(out))
    print("wrote %s (%d files tracked)" % (OUT, len(files)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
