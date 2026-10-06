# UNATTENDED — 无人值守会话作业规范

> 本文件是夜间无人值守迭代的硬约束，多数条目是真实事故换来的（历史详见 git log）。
> 违反任何一条都可能让仓库回到不可信状态。

## 0. 定位与轮次

- 每轮做一件小事：测试补强 / 冗余审计 / 已确认 bug 修复 / 小步功能或布局迭代。
- 提交信息中文、约定式前缀（`feat/fix/test/perf/docs/refactor`），
  首句结论，正文列改动与验证计数。

## 1. 门禁与环境

- 环境：Windows 真机，Rust stable + Node LTS（`app-rs/` 内开发）。
- 唯一权威验证：`cd app-rs && ./check.ps1` **exit 0**
  （fmt + clippy -D warnings + cargo test + tsc + vitest 五段，任一失败即非零）。
- 跑全量门禁期间**绝不编辑被测代码**——cargo test 单线程跑约 30s，
  中途改动会造成假绿/假红混杂的无效结论。
- 基线（2026-10-07，Python 版退役后）：cargo **770** 例 + vitest **572** 例全绿。
  动了总例数就在提交信息里写明增删原因。

## 2. 测试安全网

- Rust/前端单测不得触真实系统：不 kill 用户托盘实例、不写真实用户目录、
  不做真实网络动作（单测内的网络探测全部走注入的假数据）。
- 真机冒烟（Win32 注键/真实剪贴板）只能由 `tools/test_smoke_real_rs.py` 承担，
  且**必须人工在场**：`python -m pytest tools/test_smoke_real_rs.py --run-real -v`。
  无人值守会话严禁运行（会向真实桌面注入按键）。
- `tools/conftest.py` 的系统调用哨兵（subprocess/os.startfile/winreg 写/socket）
  不得为了让用例通过而拆除；豁免条件 = `@pytest.mark.real` + `--run-real` 双满足。

## 3. 活体程序红线

- 托盘里运行的 SysKit.exe 是用户实例，持有单实例互斥量：
  **绝不 kill 进程**，也不启动第二个实例（会抢热键、污染系统剪贴板）。
- 需要真实桌面冒烟（Win32 输入/激活链路）而受上述限制时：停手，把问题记为
  「需人工决策」写进提交信息，留给白天会话。

## 4. 提交纪律

- 工作区与白天会话共享：提交前必须 `git status`，且**只用 pathspec 提交**
  本轮自己动过的干净文件：`git commit -m "……" -- 文件1 文件2`；禁止 `git add -A`。
- **不要创建任何会话日志 .md**：每轮结论写进提交信息——首句结论，正文列改动与验证计数。
- 禁止 push、merge、tag、`--no-verify`——远端推送由用户白天统一操作。
- 遇到白天会话遗留的未提交改动：不接管、不还原、不顺手提交，绕开它们工作。
