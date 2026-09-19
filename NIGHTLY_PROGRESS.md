# NIGHTLY_PROGRESS — SysKit 夜间自动化（2026-09-20 凌晨）

参数：最多 10 轮 × 45 分钟；08:10 后不开新轮；08:55 硬停；分支 test/pytest-setup（开工时干净，无 wip 分支需要）。

## 基线（第 1 轮实测，01:12）

- `CI=1 python -m pytest -q` → **1207 passed / 0 failed / 0 error / 1 skipped**（junitxml 复核；末尾汇总行被 offscreen 已知原生 teardown 吞掉，以 xml 为准）。
- `pyflakes syskit tools conftest.py` → 0。
- **任务提示中的「1205 过 / 1 失败」未复现**：该失败即 `test_http_get_bypasses_system_proxy`
  （环境依赖断言：CPython 3.10 无系统代理时 ProxyHandler 不装配进 handlers），
  已随主体会话 commit `12df081` 修复并入库。基线本来就绿，第一优先级达成（无需改动）。
- 工作树干净（12df081 = 优化第 5 步全部内容），未创建 wip 分支/未做 wip 提交（指令条件不成立）。

## 轮次记录

| 轮 | 开始 | 内容 | 提交 | 结果 |
|---|---|---|---|---|
| 1 | 01:12 | 基线实测 + 夜间文档建立 | （见 git log nightly round 1） | 1207 全绿确认 |

## 待选改进池（后续轮次从中取，做前重估）

1. `tools/run_full_check.py`：把「SEG1+SEG2+junitxml 计数+pyflakes」两段式全绿口径钉成一个工具（夜间任务自用 + 消灭手敲序列）。
2. `tools/diff_preview.py` 缺陷：ONLY-A/ONLY-B（一边为空）退出码仍为 0（补53 已记录的脚手架隐患）→ 改非零 + 契约冒烟测试。
3. 端口页筛选框每击键全表 O(N) 遍历 → 防抖 QTimer（需核对既有测试的即时性假设）。
4. PROGRESS.md 头部任务说明已停在 2026-09-10（pytest 体系建设期）→ 刷新为现状摘要。
