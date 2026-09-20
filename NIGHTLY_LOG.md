# NIGHTLY_LOG — SysKit 夜间自动化

## 2026-09-20

### Round 1（01:12–01:16）

- 启动检查：分支 test/pytest-setup，**工作树干净**（12df081），无未提交改动 → 按指令免建 wip 分支。
- 基线实测：`CI=1 python -m pytest -q` → 1207 passed / 0 failed / 0 error / 1 skipped
  （junitxml：`.workbuddy/nightly_r1_base.xml`；汇总行被 offscreen 已知原生 teardown
  吞掉属既有环境现象，README 有处置约定）。pyflakes 0。
- **第一优先级结论**：任务提示的「1205 过 / 1 失败」未复现。对应失败为
  `test_http_get_bypasses_system_proxy`（环境依赖断言，机器关代理后必红），
  已在主体会话 `12df081` 修复入库。基线全绿，无需修复动作。
- 建立 NIGHTLY_PROGRESS.md / NIGHTLY_LOG.md；登记后续改进池。
- 本轮零产品代码改动；提交仅含两个夜间文档。

### Round 2（01:45–01:52）

- **新增 `tools/run_full_check.py`**：把「SEG1（--ignore=tests/test_ui_smoke.py）+
  SEG2（ui_smoke）+ pyflakes 门」两段式全绿口径钉成一条命令——计数读 junitxml
  （不信会被 offscreen 原生 teardown 吞掉的 stdout 摘要），段中途崩（xml 缺失）
  按失败处理并打印末尾 50 行；退出码 0=全绿。文件头注明：脚手架不进单测
  （conftest 哨兵拦测试内 subprocess.*），验证方式就是每晚真实跑它。
- 实跑验证：`python tools/run_full_check.py` → SEG1 28.5s（1163，0F/0E）+
  SEG2 4.5s（44，0F/0E/1skip）+ pyflakes 0.4s → **FULL-CHECK PASS**，退出码 0。
  该结果即本轮全量回归证明。
- README「开发与测试」补一行命令。
- 过程自查：本轮编辑 README 时又用了一次 heredoc 脚本（单行插入，diff --stat
  核实仅 +1 行无损伤）——纪律再次重申：后续一律 Edit/Write。
- 改动 2 文件 +~100 行（工具）+1 行（README），符合 <200 行/3-5 文件约束。

### Round 3（02:30–02:38）

- **原计划「修复 ONLY-A/ONLY-B 退出码缺陷」经实证不成立**：读 tools/diff_preview.py
  全文 + 真目录实测（preview_final vs 空目录 → 打印 ONLY-A 并退出 **1**；自比对 →
  退出 0），当前实现行为正确。PROGRESS:1605 补53 的「退出码仍是 0」为误记
  （或记录后该未跟踪文件又得到修正）；按仓库惯例以实测为准，不改历史记录，
  用测试固化现状。
- **新增 tests/test_diff_preview_contract.py 6 例**：SAME→0（非 png 忽略）、
  DIFF→1、SIZE-DIFF→1、ONLY-A→1、ONLY-B→1、混合时逐张报告且退出码取最坏。
  importlib 直载 + monkeypatch sys.argv 调 main()（conftest 哨兵拦 subprocess）。
- 全量验证：`python tools/run_full_check.py` → SEG1 28.9s（**1169**，0F/0E）+
  SEG2 4.6s（44，0F/0E/1skip）+ pyflakes 0 → PASS（即本轮回归证明；总数
  1207→1213，+6 契约用例）。
- 改动 1 个新测试文件（~95 行），零产品代码改动。

### Round 4（03:15–03:24）

- **修复 R2 引入的 README CR 字节损坏**：heredoc 转义事故把
  `tools\run_full_check.py` 写成了 `tools`+CR+`un_full_check.py`（补23/24/44/50
  同款事故第 4 次现身，且这次是我无视自己刚写入 PROGRESS 的纪律）。修复方式：
  Write 一次性字节级脚本（内联 python -c 的 bash 转义同样炸了，立即停手），
  断言恰好 1 处损坏 → 替换 → 验证无游离 CR → 脚本删除。git diff 恰 1 行。
- PROGRESS.md 头部现状化：原标题停在 2026-09-10 的 pytest 建设期任务（「停止
  条件 2026-09-10 08:30」早已过期）——顶部加「头部现状」块（1213 例/99%/一键
  验证命令），原文保留为历史。
- README 测试计数 1207→1213（R3 新增 6 例后未同步）。
- 全量验证：run_full_check → SEG1 1169 + SEG2 44 (0F/0E/1skip) + pyflakes 0 →
  PASS（文档轮，改动不触代码，仍按轮次规则跑了全绿）。
- **教训加重入档**：heredoc/内联脚本改仓库文件在第 4 次事故后彻底禁止——哪怕
  修复事故本身也要走 Write 独立脚本 + 字节断言。

### 终止记录（2026-09-20 11:49）

- R4 完成后的「休眠至 04:00 槽位」后台任务被会话中断打断；恢复时本地时间
  **11:49:35**，远超硬停止线（08:55 任何情况必停 / ≥08:10 不开新轮）。
- 按任务规则：**写最终日志并停止，不再开新轮**。实际执行 4 轮（计划 10 轮）。
- 残留清理：中断遗留的 `sleep.exe`（PID 2092）已终止；无 python/pytest 残留。
- 最终状态：工作树干净；HEAD = fda4267（round 4）。

### 最终汇总（对应任务要求的停止输出）

| 项 | 结果 |
|---|---|
| 实际完成轮次 | 4 / 10（R5 起因超时未开） |
| 每轮提交 | R1 01:12 `6c72578` 基线验证+夜间文档 |
| — | R2 01:45 `69e47a9` run_full_check 工具化 |
| — | R3 02:30 `3d395af` diff_preview 契约测试 6 例 |
| — | R4 03:15 `fda4267` 文档一致性 + CR 字节损坏修复 |
| 最终 pytest | R4 轮 `run_full_check`：SEG1 1169 + SEG2 44 = **1213 passed / 0 failed / 0 error / 1 skipped**，pyflakes 0 |
| 基线失败是否修复 | **是（无需本轮动手）**：提示的「1205 过 / 1 失败」= `test_http_get_bypasses_system_proxy` 环境依赖断言，已随主体会话 `12df081` 修复入库；R1 实测即 1207 全绿 |
