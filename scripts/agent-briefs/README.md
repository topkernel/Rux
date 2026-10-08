# Agent Briefs

Task briefs for the parallel-agent workflow (see CLAUDE.md). These live in
the repo so a new dev machine can relaunch the same program without
hand-copying prompts. Current set:

- `COMMON-X86.md` — shared preamble: toolchain, build/boot commands, branch
  policy (main stays riscv64-only; all x86 on feature branches)
- `x86-exec.md` — finish the x86_64 exec path (page-table corruption
  frontier) until a static userspace shell runs
- `x86-ubuntu.md` — Ubuntu amd64 rootfs + run script + virtio-gpu/input
  firmware-BAR handling
- `s005-coredump.md` — fix BUG-S005 (repeated core-dumping children corrupt
  the parent shell); unlocks the LTP full baseline

To launch on a new machine:
  git worktree add -b feature/<name> /path/to/worktrees/<name> feature/x86-64
  # then hand the brief to your agent runner of choice
