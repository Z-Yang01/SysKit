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
