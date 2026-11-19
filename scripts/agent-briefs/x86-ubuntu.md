# Agent x86-ubuntu (worktree /home/william/rux-agents/x86-ubuntu, branch feature/x86-ubuntu)
Read /home/william/rux-agents/prompts/COMMON-X86.md first.

Mission: prepare the Ubuntu-amd64-on-Rux bring-up so it can start as soon
as the exec agent lands userspace. You own: test/run-x86-ub.sh, any
userspace/image scripts under test/ or scripts/, and the x86 GPU/input
driver BAR handling in kernel/src/drivers/** (virtio-gpu, virtio-input).
Do NOT touch the exec path or coredump paths (other agents own them).

Tasks:
1. Rootfs: replicate the riscv64 ubuntu-image flow for amd64. Inspect the
   `ubuntu-image` target in the Makefile + its scripts to see how the
   riscv image is assembled (Ubuntu 22.04 base + udesk custom init).
   For amd64: download Ubuntu 22.04 server/cloud amd64 rootfs (or
   debootstrap-style assembly if scripts support it), produce
   work/ubuntu-x86.img with the same init approach. If the image flow is
   too riscv-specific, build a minimal amd64 image with systemd-free init
   (the riscv one used udesk-init from userspace/apps) — the goal is a
   glibc amd64 rootfs that exercises dynamic linking.
2. Boot driver: write test/run-x86-ub.sh modeled on the riscv ubuntu-run:
   -M q35 -m 4G -smp 2(1 until SMP) -device virtio-blk for the image,
   virtio-net, virtio-gpu (xres/yres like riscv), -snapshot
   file.locking=off, bzImage kernel, ROOTFS=env. Include a serial-console
   -append matching what init expects (root=/dev/vda rw init=... console=ttyS0).
3. virtio-gpu on x86: the virtio-blk PCI path got firmware-assigned BARs +
   the 0xc0000000..4GB PCI MMIO hole mapping (see virtio_pci.rs cfg(x86_64)
   blocks). The GPU driver likely needs the same treatment: read how
   drivers/gpu/virtio_gpu.rs gets its register base (it may self-assign
   BARs like blk did) and apply the same cfg(x86_64) firmware-BAR policy
   + verify the ECAM probe finds it (drivers/pci find_ecam_devices is now
   arch-generic). Same for virtio-input if PCI.
4. Sanity boots against the current branch kernel: expect it to stop at
   "init: Failed to load /sbin/init" (exec still broken — that's the other
   agent's frontier). Your deliverable is everything AROUND that point:
   image + script + GPU/input probing logs showing the devices enumerate.

Gates: riscv64 build exit 0; x86 build exit 0; report image path + boot
log excerpt. NEVER merge to main/feature/x86-64 yourself.
