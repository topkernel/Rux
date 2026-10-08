# Common preamble for feature-branch agents (x86 program)
Worktree: /home/william/rux-agents/<NAME> on branch feature/<NAME>, based on
feature/x86-64 (30c784f1 + guard). Read CLAUDE.md at the worktree root.
Toolchain: `cargo` on PATH (else export PATH="$HOME/.cargo/bin:$PATH" EVERY
bash command). Nightly default. Builds:
- riscv64 regression (MUST stay exit 0): cargo build --target riscv64gc-unknown-none-elf
- x86: cargo build --target x86_64-unknown-none --no-default-features --features x86_64
  then python3 build/mkbzimage.py target/x86_64-unknown-none/debug/rux target/x86_64-unknown-none/debug/bzImage
- x86 boot probe: timeout 240 qemu-system-x86_64 -M q35 -m 2G -smp 1 -accel tcg,thread=single -cpu max -nic none -nographic -serial mon:stdio -drive file=ROOTFS,if=none,id=rootfs,format=raw,file.locking=off -device virtio-blk-pci,disable-legacy=on,drive=rootfs -kernel target/x86_64-unknown-none/debug/bzImage
  (/tmp/rootfs-x86.img = riscv rootfs + /sbin/init replaced by x86_64 freestanding hello)
- x86 kernel boot chain status: SeaBIOS -> trampoline -> long mode -> banner
  -> PML4 switch -> full subsystem table -> virtio-blk -> ext4 mount ->
  init read; frontier = exec page-table phase.
- riscv smoke (rootfs musl, march-pinned, Zbb CPU):
  (sleep 45; echo 'echo OK'; sleep 6) | timeout 85 qemu-system-riscv64 -M virt -accel tcg,thread=single -cpu rv64,zbb=true,zba=true,zbs=true -m 1G -smp 1 -nographic -snapshot -serial mon:stdio -drive file=test/rootfs.img,if=none,id=r,format=raw,file.locking=off -device virtio-blk-pci,disable-legacy=on,drive=r -kernel target/riscv64gc-unknown-none-elf/debug/rux -append "root=/dev/vda rw init=/bin/sh console=ttyS0"
Branch policy: MAIN must stay riscv64-only and bootable; all x86 work stays
on feature branches; the main session integrates into feature/x86-64. NEVER
merge into main or feature/x86-64 yourself — commit on YOUR branch, report.
Style: English only, conventional commits, plain tone, SAFETY comments on
unsafe (<=5 lines). QEMU discipline: only kill PIDs you started (use
pkill -x qemu-system-... AFTER checking; never broad pkill -f with strings
that match your own shell). Don't push.
