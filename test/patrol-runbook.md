# Agent Coordination Patrol — Operational Runbook

This document is the canonical specification of the recurring coordination
patrol for the Rux kernel project. The patrol prompt deployed to the
scheduler MUST stay in sync with section 9 of this document; any change to
the prompt is made here first, then mirrored to the scheduler entry.

## 1. Purpose and scope

The project runs multiple long-lived development agents in parallel, each
in its own git worktree under `/home/william/rux-agents/<name>/`. A single
coordination session (the main session) adjudicates agent health, merges
verified work into main, enforces the merge gate, and manages host
resources. The patrol is the recurring heartbeat of that coordination:
every 30 minutes it verifies agent liveness, cleans up orphaned virtual
machines, monitors milestones, advances the merge queue, and enforces the
model-quota policy (unattended full test execution while the quota is
exhausted; continuous agent work while the quota is available).

## 2. Agent roster (8 concurrent agents, subject to phase adjustments)

| # | Role | Worktree / branch | Responsibility |
|---|------|-------------------|----------------|
| 1 | OpenHarmony launcher bring-up agent | `samgr-callback` / `feature/samgr-callback` | Drive the OpenHarmony image to a visible launcher UI; image-side mitigations and kernel-side escalation |
| 2 | GNOME desktop integration agent | `gnome-bringup` / `feature/gnome-bringup` | GNOME desktop interactivity, README evidence assets, gdm login chain |
| 3 | LTP conformance agent | `ltp-fix-b` / `feature/ltp-fix-b` | Full LTP scans, per-case isolation adjudication, syscall conformance fixes |
| 4 | Long-run stress agent | `wwh-main-kernel` (+ `sf2-*` worktrees) / stress branches | Continuous stress rounds (LTP churn, filesystem fuzzing, memory storms, randomized command workloads), regression adjudication |
| 5 | Performance optimization agent | `perf-audit` / `feature/perf-audit` | Measured optimizations against the performance merge gate; baseline maintenance |
| 6 | Observability tooling agent | `dfx-align` / `feature/dfx-align` | In-kernel diagnostics aligned with upstream interfaces (perf, trace, pstore, kallsyms) |
| 7 | Test completeness agent | `test-coverage` / `feature/test-coverage` | Test-coverage ledger; wiring unwired probes; architecture coverage; LTP TCONF triage |
| 8 | Code review and conformance agent | `abi-review` / `feature/abi-review` | Thesis-driven review against the reference kernel, POSIX and the ABI; fixes functional, compatibility and performance findings; target: every metric at ≤0.5x the reference median |

Two scheduler entries support the patrol and are specified here for
completeness: the coordination patrol itself (every 30 minutes) and the
line-by-line review automation (every 20 minutes, worktree
`rux-agents/code-review`, driven by `docs/development/review-coverage.md`).

## 3. Liveness adjudication

1. For each agent, verify liveness with TaskOutput on its current task
   identifier. Identifiers rotate between batches; the live set is
   authoritative, never a list carried in the prompt.
2. Death classification:
   - Model-quota exhaustion (5-hour usage cap): record silently; do NOT
     restart agents inside the window; restart after the reset time.
   - Provider timeout: restart immediately with full context.
3. A finished agent (completed status) is resumed for its next mission by
   SendMessage when resumable, otherwise relaunched with a self-contained
   brief; its worktree and session notes carry the context.

## 4. Orphaned virtual machine cleanup

- Kill only virtual machines whose owning agent is dead AND that have no
  terminating timeout of their own; identify processes by exact PID with
  command-line verification. `pkill` is forbidden.
- Virtual machines of dead agents that still produce wanted data and have
  bounded timeouts are left to finish naturally and their output is
  harvested.

## 5. Milestone monitoring

- OpenHarmony: `bootevent.launcher.ready` set, ability OnForeground, first
  launcher frames (probe color gradients do not count).
- GNOME: interactive desktop already achieved; monitor the README evidence
  asset and ≥2-boot interactive reproduction; gdm login chain progress.
- LTP: current scan pass counts (adjudicated baseline r19 = 1018/1869).
- Performance: merge-gate tier verdicts, baseline refreshes, and the
  cold-start gap (reference 16 s vs current >20 min to the same point).

## 6. Merge queue and gate

The coordinator merges branch work into main only after: dual-architecture
build rc=0 (riscv64 and `PLATFORM=x86_64` with `config.rs` restored), the
in-kernel unit suite all green (private image copy via `ROOTFS_IMG` when
the shared image is contended), and the performance tiers of the merge
gate (quiet-window measurements may be recorded as explicit debt with an
armed watcher). `git push origin main` is executed separately, only after
a green verdict line is observed.

## 7. Host resource discipline

- Report QEMU process count and available memory each round; the floor is
  10 GB available (MemAvailable, not free).
- Shared images boot with `-snapshot` and `file.locking=off`; default test
  shape `-smp 2 -m 1G`.
- Repository root stays clean: no scratch scripts, drivers, logs or images
  in the main-repo root; scratch lives in the owning worktree's `run/`.

## 8. Model-quota policy (binding)

- Quota exhausted (agent deaths classified as the 5-hour cap): the patrol
  starts the unattended full test suite if not already running:
  `test/suite.sh --mode all --arch both --daemon --minutes 20 --hours 5`
  Agents are not restarted inside the window.
- Quota restored: FIRST read the window aggregate
  (`test/suite.sh --status`), THEN stop the test machines
  (`test/suite.sh --stop`), fix reported problems, and only then restart
  agents per section 3.

## 9. Canonical patrol prompt (deployed verbatim to the scheduler)

```
Rux 内核项目——多代理协调巡检规范（每 30 分钟执行一次）

主仓库：/home/william/workspace/Rux。当前模式：8 个开发代理并行工作，每个代理使用独立的 git 工作树（/home/william/rux-agents/<名称>/），由主协调会话统一管理。本巡检是协调工作的周期性心跳，规范文档见 test/patrol-runbook.md，巡检提示语必须与该文档第 9 节保持一致。

每轮动作：
1. 代理存活判定：对每个代理当前的任务标识执行 TaskOutput 仲裁。任务标识随批次轮换，以实际派发为准，提示语中不固化标识列表。代理角色：OpenHarmony launcher 启动代理、GNOME 桌面集成代理、LTP 一致性测试代理、长时压力测试代理、性能优化代理、可观测性工具代理、测试完备度代理、代码检视与一致性代理。
2. 死亡分类与处置：模型额度耗尽（5 小时用量上限）→ 静默记录，窗口内不重启代理，重置时间过后重启；服务提供方超时 → 携完整上下文立即重启。正常结束的代理通过 SendMessage 恢复或以自包含任务书重新拉起。
3. 孤儿虚拟机清理：仅杀死属主已死亡且自身无终止超时的虚拟机；按具体 PID 加命令行核对后终止，严禁 pkill。仍在产出有效数据且带超时上限的虚拟机令其自然结束并回收数据。
4. 里程碑监测：OpenHarmony launcher（bootevent.launcher.ready 置位、OnForeground、首批界面帧；探针色阶渐变不计入）；GNOME（交互桌面已达成，跟踪 README 证据资产与不少于两次引导的交互复现，以及 gdm 登录链推进）；LTP 通过数（当前裁定基线 r19=1018/1869）；性能门禁分层判定、基线刷新与冷启动差距跟踪。
5. 合并队列与门禁：协调者仅在各分支通过双架构构建（riscv64 与 PLATFORM=x86_64，构建后还原 config.rs）、内核单元测试全绿（共享镜像被占用时以 ROOTFS_IMG 私有副本执行）、性能门禁相应分层（静默窗口测量可记为显式债务并布防监视器）后合并入 main；推送 origin main 单独执行，必须先见到绿色判定行。
6. 主机资源纪律：每轮报告 QEMU 进程数量与可用内存（以 MemAvailable 计，下限 10 GB）；共享镜像一律 -snapshot 加 file.locking=off 启动；测试默认 -smp 2 -m 1G；主仓库根目录保持整洁，临时脚本只允许放在所属工作树的 run/ 目录。
7. 模型额度策略（强制）：额度耗尽且测试守护进程未运行时，立即在主仓库执行 test/suite.sh --mode all --arch both --daemon --minutes 20 --hours 5 启动无人值守全量测试；额度恢复后第一优先级为：test/suite.sh --status 读取全窗口跨轮聚合 → test/suite.sh --stop 关闭测试虚拟机 → 修复摘要报告的问题 → 按第 2 步重启代理。

汇报格式：每代理一行 + 里程碑一行 + 主机一行 + 测试执行状态一行（未运行 / 运行中（轮次与摘要路径）/ 已停止；无变化时并入主机行）。无变化的条目合并为一行。
```

## 10. Change history

- 2026-10-28: initial formal runbook; supersedes the ad-hoc six-lane
  patrol text; roster grown to 8 agents; quota policy and root-cleanliness
  rules incorporated.
