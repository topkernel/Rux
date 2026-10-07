# Agent x86-exec (worktree /home/william/rux-agents/x86-exec, branch feature/x86-exec)
Read /home/william/rux-agents/prompts/COMMON-X86.md first.

Mission: finish the x86_64 exec path until a static userspace binary runs
(init64 hello -> x86_64 static toybox shell). You own: kernel/src/** on this
branch, EXCEPT files the other two agents own (S005 agent owns
process/coredump.rs + signal/wait paths; ubuntu agent owns only
test/run-x86-ub.sh + userspace-side scripts).

Current frontier (all earlier milestones work): boot chain through long
mode, PML4 switch, subsystem table, virtio-blk + ext4 root mount,
/sbin/init READ from disk. The blocker is in exec's page-table phase:

1. exec reads a VMA-shaped PTE (0xffffffff8024e083 = a kernel VMA | 0x83
   flags) through get_page_table_virt -> phys_to_virt overflow (diagnostic
   prints bogus phys + rbp-chain callers; phys_to_virt now fails loud).
2. Kernel PML4 top level dumped CLEAN at copy_kernel_mappings time (entries
   273/467/511/0 all valid small-phys links).
3. A garbage "page table" hangs under the identity subtree:
   PT(pml4=0,pdpt=0,pmd=498) entries = 0xf000ff53f000ff53-style data
   (also under pdpt=1,pmd=30). boot_pd0 lives at phys 0x104000 (boot tables
   region 0x101000-0x107000, part of the .boot.pt section).

Next steps (documented for you, verify yourself):
- Dump boot_pd0[498] and boot_pd1[30] raw values; check whether the PMD
  entry itself is corrupted (points into 0xf000-style data) or whether a
  non-leaf entry descends into a data page.
- Find the writer: suspects are (a) the 16-bit trampoline e820 store at
  linear 0x10660 (setup+0x660) overlapping nothing — verify; (b) the early
  page-table pool (mm agent placed a 96-table static BSS pool — find its
  address range and check overlap with boot tables at 0x101000-0x107000);
  (c) setup_linear_mapping's low-map construction writing 4K links.
- GDB techniques that worked: -S -s + `break *ADDR if $rdi > N` conditionals,
  rbp-chain reads (`x/2gx $rbp`), stack scans symbolized via nm+bisect,
  monitor unix socket memory dumps, `xp/.. ` physical reads.
- Once exec completes: boot with ROOTFS=/tmp/rootfs-x86.img (contains
  /tmp/init64, a freestanding x86_64 raw-syscall hello at /sbin/init —
  expect "[init64] hello from x86_64 userspace on Rux!").
- Then build x86_64 static toybox (host gcc works: it's x86 native) into a
  minimal rootfs with /bin/sh + coreutils, get an interactive shell over
  serial. Reuse the debugfs-injection flow from earlier sessions.

Gates per step: `cargo build --target riscv64gc-unknown-none-elf` stays exit 0
(your changes must be x86-scoped or cfg-gated), x86 build green, QEMU boot
progress photos into the report. NEVER merge to main or feature/x86-64
yourself — commit on feature/x86-exec and report.
