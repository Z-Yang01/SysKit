# 错误记录:pytest-cov 插件模式下 PySide6 测试进程原生崩溃（EXIT 127）

- 日期：2026-09-11 深夜
- 环境：Windows 11 真机，Python 3.10.11，PySide6 6.11.2，pytest 9.1.1，pytest-cov 7.1.0，
  pytest-timeout 2.4.0（thread 方法）

## 现象

- `python -m pytest --cov=syskit -q` 全量跑：进程在**精确相同位置**（第 320 个用例 =
  `tests/test_palette.py` 首个用例，累计 37%）无声崩溃，exit 127，无 traceback、无摘要行。
- 同命令去掉 `--cov`：崩溃点漂移到别处（约 75%，ui_pages 区间），且逐文件单跑时
  `test_ui_pages_units.py` 也偶发 127——崩溃点随测量开销漂移，具备强 flaky 性。
- `python -m coverage run --source=syskit -m pytest --timeout=300 -q`：**稳定全绿**
  （766 用例，1 已知 skip），coverage 报告正常生成。

## 根因分析

- pytest-cov 插件在采集开始/结束时对 trace 函数做集中接管，与 PySide6 offscreen 下的
  Qt 事件循环/原生代码交互存在已知类别的兼容性问题；崩溃发生在 Qt 侧原生代码，
  Python 层无栈可打（与 MediaFlow 仓库 2026-09-11 记录的 `pytest --cov` + asyncpg
  EXIT 139 段错误同类：都是「coverage 插件模式 × 原生扩展」组合病）。
- 旁证：`coverage run -m pytest`（不加载 pytest-cov 插件）同一套件稳定通过，
  说明问题仅在插件集成层，不在 coverage 测量本身。
- 另发现的独立问题：`test_vault.py::test_add_account_empty_password_rejected` 未 mock
  `QMessageBox.warning`，offscreen 下真实模态永久阻塞，300s 被 pytest-timeout 击杀；
  对 Qt 模态的线程强杀同样可引发原生崩溃。已在 conftest.py 增加全局 autouse
  `msgbox_noop` 夹具根治该类挂起（记录型 no-op，需要断言弹窗的测试可自行覆盖）。

## 修复 / 规避

1. 覆盖率测量一律用：
   ```bash
   python -m coverage run --source=syskit -m pytest --timeout=300 -q
   python -m coverage report
   ```
   不要用 `pytest --cov` 插件模式。
2. QMessageBox 挂起已由 conftest autouse `msgbox_noop` 根治。
3. 快速诊断套路（留档）：全量崩溃但单文件通过 → 用 `--collect-only -q` 的每文件计数
   累加定位崩点用例；EXIT 127 + 无 traceback → 优先怀疑原生崩溃而非测试逻辑。

## 遗留

- pytest-cov 与 PySide6 的兼容性属上游问题，未深挖；若未来需要插件模式
  （如 pytest-xdist 集成），可尝试 pytest-cov 的 `cov-core` 关闭并行追加或升级版本再评估。

## 验证

- `coverage run` 方式全量：766 用例全绿（1 skip 为已知 IPv6 冻结防护），TOTAL 90%。
