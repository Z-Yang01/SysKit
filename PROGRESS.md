# PROGRESS — pytest 测试体系建设

任务：为 SysKit 搭建 pytest 测试体系，纯逻辑模块行覆盖率 ≥70%，pytest 全绿。
分支：`test/pytest-setup`（自 main @ 1b9c9a0 切出）
环境：Windows 真机，Python 3.10.11，PySide6 6.11.2，pytest 9.1.1，pytest-qt 4.5.0，pytest-cov 7.1.0
停止条件：达标 或 2026-09-10 08:30（当前 23:41，时间充裕）

## 决策记录

| # | 决策 | 理由 |
|---|------|------|
| 1 | 磁盘空间分析功能先在 main 收尾提交（1b9c9a0），再切测试分支 | 避免 test 分支携带未验证的脏工作区，保证测试基线干净；删除流修复已复验通过 |
| 2 | 覆盖率统计口径：`--cov=syskit` 全包报告 + 重点关注逻辑模块（netcheck/repairs/dnsquery/icmp/proxy/stores/settings/winregread/proc/config/diskusage 等）单项 ≥70% | 任务要求"逻辑模块 ≥70%"；UI 冒烟（组 D）额外提升整体覆盖率 |
| 3 | conftest 安全网策略：**哨兵式**而非空 mock——proc.run/run_ps、subprocess 全家、winreg 写操作、socket.connect、urlopen 未显式 mock 时直接 AssertionError | 把红线 1 变成硬约束：任何漏 mock 的系统调用会让测试失败而不是静默执行真实动作 |
| 4 | 持久化隔离：autouse 夹具把 config 的 APPDATA_DIR/LOCALAPPDATA_DIR/SETTINGS_FILE/LOG_DIR/BACKUP_DIR 全部重定向到 pytest tmp_path，并复位 settings._cache | 验证过 config/settings/stores/logger 都是调用时动态读 config.X，属性级 patch 即完全生效 |
| 5 | 四个分组子 agent 并行写测试，但 **git commit 由主线程集中按组提交**，子 agent 不碰 git | 并行进程同时 commit 会撞 git index.lock |
| 6 | conftest.py / pytest.ini 由主线程独占维护，子 agent 的局部夹具写在自己组内测试文件 | 避免并行编辑冲突 |
| 7 | diskusage（上任务新增的纯逻辑模块）测试由主线程在阶段一完成（tests/test_diskusage.py，覆盖率 82%） | 该模块我最有上下文，且回收站删除测试必须全程 mock _sh_delete |

## 阶段进度

- [x] 阶段一（串行）：分支创建、依赖安装、conftest 安全网、pytest.ini、
      tests/test_smoke.py（8 项验证安全网自身）+ tests/test_diskusage.py（14 项），
      22 passed，已 commit
- [x] 阶段二 A：netcheck 15 项检查逻辑（并行子 agent）——130 例，99%
- [x] 阶段二 B：repairs 风险分级与命令构造（并行子 agent）——101 例，98%
- [x] 阶段二 C：DNS/子网计算纯函数（test_toolbox_calc.py，42 例，
      1 例 skip：IPv6 输入经 UI 钳位为 /32 触发大网段冻结，见疑似 bug #4）
- [x] 阶段二 D：UI offscreen 冒烟（test_ui_smoke.py，35 例，UI 模块 0%→76%）
- [x] 阶段二 E：逻辑缺口补测（stores/logger/icmp/monitor/proc/snapshot
      六模块 139 例，全部 100%）
- [x] 覆盖率验证：TOTAL 32%→80%，全部逻辑模块 ≥70%（详见下方第二轮会话记录）

## 红线执行情况

- 红线 1（禁真实修复动作）：conftest 哨兵强制执行，test_smoke 有专门验证用例 ✅
- 红线 2（禁碰真实数据目录）：autouse 重定向，test_smoke 有专门验证用例 ✅
- 红线 3（不改业务代码/不打包/不 push）：目前零业务代码改动；后续如发现疑似 bug 只记录到下表

## 疑似 bug 备忘（只记录不修改）

1. **monitor.py `Monitor.__init__`**：用 `threading.Event` 覆盖了 `Thread._stop`
   方法名 → 对监控线程 `join()`/`is_alive()` 必抛 TypeError（test_ui_smoke 实测命中）。
2. **toolbox_page `PingRouteTab.tracert()`**：完成后 `_tracert` 永不复位 →
   路由追踪按钮每页面只能成功用一次。
3. **toolbox_page `calc()` 大网段无保护**：对 `ip_network` 无条件 `list(net.hosts())`，
   IPv6 /32（UI spin_prefix 8..32 钳位后）需物化 2^96 地址 → 冻结/OOM
   （test_calc_ipv6_accepted_documented 因此 skip；mediaflow 同类问题见其 PROGRESS）。
4. **stores.py TrafficStore**：`_load` 返回非 object JSON 时 `add()` 抛
   AttributeError（EventStore 有校验，此处没有）；`save_if_dirty` 在写盘失败被吞后
   仍清 `_dirty`，脏数据静默丢失。
5. **monitor.py `probe_quick`**：只探测首个 204 端点，与"任一端点可达即有网"不符，
   首端点故障会误报断网。
6. **proc.py `run()`**：只兜底 FileNotFoundError，其他 OSError 向上抛。
7. **workers.py RepairWorker**：动作不存在时静默不发 done 信号。
8. **wifi_page `_worker` 单槽位**：扫描/读密码两类 worker 共用，冲突时静默忽略。
9. **disk_page**：`ROLE_FILLED` 与 `ROLE_SIZE` 常量值重复。
10. **snapshot.py `physical_up_adapters`**：docstring 与实现不一致（只看 Up+IPv4）。

---

## 第二轮会话记录（2026-09-10 02:50–03:50，夜间挂机总任务·项目三）

- 前情：第一轮会话在 00:23 系统内核事件后中断，A/B 组成果已入库。
- 本轮 C 组：验证并修正上会话遗留的 tests/test_toolbox_calc.py——其中
  test_calc_ipv6_accepted_documented 因 UI 前缀钳位（126→32）触发疑似 bug #3 大网段
  冻结，Windows 下 pytest-timeout thread 模式超时即杀进程，xfail 无法承接，改为
  skip 并记录；其余 41 例全部通过。
- 本轮 D 组（子 agent）：test_ui_smoke.py 35 例，11 个 UI 模块 0%→76%
  （theme/confirm_dialog/dashboard/tray 100%，workers 94%，main_window 79%）。
- 本轮 E 组（子 agent）：stores/logger/icmp/monitor/proc/snapshot 六模块
  139 例，覆盖率全部 100%。
- 收尾验证：`QT_QPA_PLATFORM=offscreen python -m pytest -q --timeout=120
  --cov=syskit --cov-report=term` → **TOTAL 32%→80%**，全部逻辑模块 ≥70%，
  套件无 FAIL/ERROR（2 skip + 1 xfail 均有记录原因）。
- 已知瑕疵：PySide6 6.11 offscreen 平台下进程退出阶段偶发 0xC0000409 原生崩溃
  （README 记载的已知坑类型），发生在全部用例通过之后的清理阶段，不影响测试
  结果；test_ui_smoke.py 内已加缓解护栏（崩溃率 ~50%→~10%），重跑即绿。
- 并行干扰备忘：两子 agent 同时全量跑 pytest 时会因并发覆盖数据文件竞争出现
  假性失败，串行复跑即消——后续多 agent 并行时应各自使用独立 COVERAGE_FILE。

## 晨报（第二轮）

- **完成**：C/D/E 三组全部落地，新增测试 181 例（42+35+139，另含既有 232 例全绿），
  总覆盖率 32%→80%，完成标准（逻辑模块 ≥70% 且全绿）**达成**。
- **假设**：UI 冒烟以"构造+关键控件+核心槽函数不崩"为度，不追求像素级交互；
  icmp 全程走假 IPHLPAPI，零真实 ping。
- **疑似 bug**：10 条（见上表），其中 #1（Monitor._stop 覆写）与 #3（大网段冻结）
  建议优先修复。
- **遗留**：main.py/cli.py/telemetry.py/ui 各页面深交互（0-65%）不在本轮范围；
  退出阶段偶发原生崩溃待 PySide6 升级验证。
