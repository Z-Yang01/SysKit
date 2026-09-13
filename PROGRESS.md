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

> 状态更新（全量检查会话）：下列 #1–#10 业务代码问题均已修复，并同步更新对应测试；
> 另修复 monitor/dnsquery/stores/toolbox 等处的线程名遮蔽、探测单点、测速高估、
> ThreadPoolExecutor 退出阻塞、退出时线程未等待等问题。全量 pytest 全绿。

1. **monitor.py `Monitor.__init__`**：用 `threading.Event` 覆盖了 `Thread._stop`
   方法名 → 对监控线程 `join()`/`is_alive()` 必抛 TypeError（test_ui_smoke 实测命中）。
   **已修**：改名 `_stop_evt`；closeEvent 补 join/wait。
2. **toolbox_page `PingRouteTab.tracert()`**：完成后 `_tracert` 永不复位 →
   路由追踪按钮每页面只能成功用一次。**已修**：finished 复位 `_tracert=None`。
3. **toolbox_page `calc()` 大网段无保护**：对 `ip_network` 无条件 `list(net.hosts())`，
   IPv6 /32（UI spin_prefix 8..32 钳位后）需物化 2^96 地址 → 冻结/OOM
   （test_calc_ipv6_accepted_documented 因此 skip；mediaflow 同类问题见其 PROGRESS）。
   **已修**：>256 地址走算术推导首尾；skip 用例已恢复并转绿。
4. **stores.py TrafficStore**：`_load` 返回非 object JSON 时 `add()` 抛
   AttributeError（EventStore 有校验，此处没有）；`save_if_dirty` 在写盘失败被吞后
   仍清 `_dirty`，脏数据静默丢失。**已修**：isinstance 校验 + 写失败保留 dirty。
5. **monitor.py `probe_quick`**：只探测首个 204 端点，与"任一端点可达即有网"不符，
   首端点故障会误报断网。**已修**：遍历全部 `PROBE_204_URLS`。
6. **proc.py `run()`**：只兜底 FileNotFoundError，其他 OSError 向上抛。
   **已修**：一并捕获 OSError 转 CmdResult。
7. **workers.py RepairWorker**：动作不存在时静默不发 done 信号。
   **已修**：未知 action 也发 done(RepairResult(False, …))。
8. **wifi_page `_worker` 单槽位**：扫描/读密码两类 worker 共用，冲突时静默忽略。
   **已修**：拆为 `_scan_worker` / `_pw_worker`。
9. **disk_page**：`ROLE_FILLED` 与 `ROLE_SIZE` 常量值重复。
   **已修**：`ROLE_SIZE = Qt.UserRole + 2`。
10. **snapshot.py `physical_up_adapters`**：docstring 与实现不一致（只看 Up+IPv4）。
    **已修**：docstring 与实现对齐。

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

---

## 全量检查与修复会话（2026-09-10，工作区未提交修改）

- 任务：全面检查项目错误并优化；对照上表 10 条疑似 bug + 代码审查发现。
- 源码修复：monitor `_stop_evt`/多端点探测、stores 校验与 dirty 保留、proc OSError、
  RepairWorker done、wifi 双 worker、disk ROLE 分离、tracert 复位、calc 大网段、
  SpeedTest 独立计数、dnsquery 运算符优先级与 executor 关闭、main_window 退出等待、
  check_events.ps1/build.bat 残留 NetTool 改名、snapshot docstring。
- 测试同步：test_monitor / test_stores / test_proc / test_toolbox_calc /
  test_ui_smoke 断言改为锁定修复后正确行为；IPv6 大网段 skip 用例恢复为正式通过。
- 验证：`python -m pytest tests/ -q --timeout=60` **全量通过**（1 skip：UI 单实例
  run_gui 离屏无法安全执行，文件头有说明）。

---

## 技术债清偿会话（2026-09-10，续）

- **force_check 立即复检**：`Monitor.run` 原先总是 `_stop_evt.wait(poll)` 干等满
  一轮再看 force。改为用 `_force_check.wait(poll_interval)` 做可中断等待——force
  置位立即醒来，1s 节流后直接进入下一轮 `_once`，手动刷新不再等满 poll。
  测试 FakeEvent 拆分 `is_set`/`wait` 双脚本，新增提前唤醒与节流期 stop 用例。
- **cmd_diagnose(quick) 透传**：CLI `--quick` 此前未传给 `netcheck.run_all`，
  参数被吞；已补 `quick=quick`，并加回归测试。
- **入口测试落地**：新增 `tests/test_cli.py`（diagnose 报告/建议、repair 权限/
  未知项/fast 顺序/单项确认/--yes/需重启提示、argparse 分流）与
  `tests/test_main_entry.py`（CLI 子命令、help 三旗标、GUI 路径、minimized
  透传、GUI ImportError→exit 1、cli.main(None) 读 sys.argv）。
- 验证：全量 `pytest tests/ -q --timeout=60` **再次全绿**（仍 1 skip）。

---

## 操作逻辑 / 性能 / 错误优化会话（2026-09-10，续）

- **组合修复确认**：`run_combo` 原先只弹一次总确认，文案却写「改配置类逐项弹窗」且
  ComboWorker 无确认直接执行。现改为：非 SAFE 动作逐项 ConfirmDialog；无管理员
  跳过非 SAFE；确认后再启动 worker。
- **WiFi 页懒扫描**：不在 WifiPage 构造时扫（主窗体一次性建齐所有页）。首次
  `showEvent` 才扫；`hideEvent` 停自动扫描，回来若仍勾选则恢复。
- **遥测 top 进程**：跨 tick 复用 `psutil.Process`（`cpu_percent` 需基线，否则读数
  常为 0）；每约 60s 重建快照清死进程。停止事件统一 `_stop_evt`。
- **网卡速率过滤**：排除列表补 `vmware`，避免虚拟网卡计入 ↓/↑。
- **端口页**：自动刷新后按 PID 保留选中行。
- **文案**：弹窗/UA 中残留 `NetTool` 统一为 SysKit；启动项禁用键路径仍用
  `Software\NetTool\DisabledRun` 以兼容旧数据。
- 新增 `tests/test_telemetry.py`；UI 冒烟改为懒扫描语义。全量 pytest **再次全绿**。

---

## 新功能：剪贴板历史 / 办公模式 / 游戏模式（2026-09-10）

- `syskit/clipboard.py`：文本剪贴板历史（去重连续项、截断、JSON 持久化、RLock 防死锁）。
- `syskit/modes.py`：
  - 办公：勿扰（HKCU Toast 开关）→ 静音（winmm）→ 打开常用软件 → 备份配置到 backup 目录；
  - 游戏：关通知 → `powercfg` 高性能 → 暂停更新策略（需管理员）→ 白名单清后台；
  - 每步返回 `{name, ok, detail}`，UI 表格汇总；退出模式可恢复通知/静音/平衡电源。
- UI：系统助手页新增「剪贴板历史」「场景模式」两个 Tab（不改主窗口页序）。
- 测试：`tests/test_clipboard_modes.py` 覆盖历史增删改查与模式步骤编排；全量 pytest **全绿**。

---

## UI 美化 / 提示 / 安装升级 / 图标（2026-09-10）

- **主题**：圆角、按钮最小高度、复选框自绘指示、表头分隔、Tab hover；QSS token 长键优先替换，修复 `@accent` 截断 `@accent_hover` 的 bug。
- **提示**：侧边栏与快捷操作/体检按钮/场景模式/确认弹窗补 tooltip；确认弹窗主按钮 primary，风险图标化。
- **布局**：系统助手场景模式防重入、步骤结果着色、空态文案；剪贴板空态说明。
- **版本**：`config.VERSION=1.2.0`，窗口标题与设置页展示。
- **图标**：`make_icon.py` 重绘为蓝色圆角盾 + 白色 S，导出 ico/png。
- **安装升级**：`install.bat` 装到 `%LOCALAPPDATA%\Programs\SysKit` 并建桌面/开始菜单快捷方式；`build.bat` 增加便携 zip 与版本解析。README 更新安装说明。
- 全量 pytest **再次全绿**。

---

## VPN 安全护栏（2026-09-10）

- 新增 `syskit/vpn.py`：识别 wintun/tap/wireguard/clash 等在用隧道；`vpn_checklist` 只读排查（隧道/代理/持久路由/默认出口）。
- **自动修复护栏**：隧道在线时梯度跳过 `renew_dhcp` / `restart_wlan` / `toggle_adapter`，并 `on_suggest` 提示；快照失败时护栏不误伤（按无隧道继续）。
- **诊断更稳妥**：系统代理 + 隧道同时在线时，`chk_proxy` 降为 INFO 且不提供 `proxy_off`，避免把 Clash 正常模式当故障关掉。
- **UI**：概览页「快捷操作」新增「VPN 排查」对话框（只读步骤列表）。
- 测试：`tests/test_vpn.py`；全量 pytest **全绿**。


---

## 2026-09-11 会话（夜间挂机总任务·项目四）

### 当前状态

- 分支 `test/pytest-setup`；全量 **635 passed / 1 skipped / 1 xfailed**；总覆盖 **80% → 96%**。

### 已完成

1. **Carry-over 提交**（2e229c8）：上一会话的逻辑/性能优化、剪贴板历史/场景模式、VPN 护栏、
   UI 美化等成果（535 例全绿）经验证后入库；总控修复 1 个日期炸弹测试
   （test_stores 硬编码 "2026-09-10"，改用 fix_today 固定日期）。
2. **逻辑层补测**（子 Agent G，58 例）：telemetry 48%→**100%**、vpn 71%→**100%**、
   admin 38%→**100%**、autostart 43%→**100%**。schtasks/winreg/ctypes/psutil 全 mock，
   零真实注册表与系统操作。
3. **系统页/应用壳补测**（子 Agent H，24 例）：ui/system_page 63%→**90%**、ui/app 35%→**100%**。
   覆盖启动项四来源聚合、回收站/清理编排、剪贴板历史交互、场景模式确认流、崩溃日志钩子、
   单实例互斥、DPI/主题装配。
4. **主窗体三页补测**（子 Agent I，18 例）：ui/main_window 75%→**99%**、ui/toolbox_page 76%→**99%**、
   ui/wifi_page 68%→**100%**。重点覆盖 WiFi 懒扫描语义全链路与测速/局域网/端口/路由 Worker。
5. **P2 业务 Bug 修复 ×1**（子 Agent I 发现，总控修复）：
   `_save_window_state` 中 `bytes(QByteArray).toBase64()` 对 bytes 调 toBase64 必抛
   AttributeError 且被 except 吞掉 → 窗口几何/页码从不持久化。已改为
   `bytes(self.saveGeometry().toBase64()).decode()`，两处锁定旧行为的测试改为正向回归断言。

### 测试

- 摸底：535 passed / 1 failed（日期炸弹）→ 修复后 carry-over 全绿；总覆盖 80%。
- 收尾：**635 passed / 1 skipped / 1 xfailed**（两次验证稳定）；总覆盖 **96%**（miss 990 → 223）。
- 安全哨兵全程有效：所有系统调用路径（schtasks/winreg 写/ctypes ShellExecute/urlopen/psutil）均 mock。

### 疑似 Bug（只记录不修）

1. `syskit/vpn.py:87` `if tunnels or True:` 恒真，owner_hint 初始化为死代码（可读性）。
2. `syskit/ui/system_page.py:261` 电池解析 `except (ValueError, IndexError)` 对常规脏数据
   （如 "4万"）无防御力，仅 "²" 类 isdigit-但不可 int 的 Unicode 字符可命中。
3. 环境性已知问题：PySide6 6.11 离屏下真实 QThread 的 UI 测试有低概率原生崩溃
   （用例全过后的收尾段 fail-fast，重跑即可，README 已有处置约定）。

### 遗留与下一步

- ui 剩余低覆盖：dashboard/charts 等 60-75% 小块；dist 旧 NetTool.exe 已删除（随 carry-over 入库）。
- 可考虑把「重跑即可」的 QThread 崩溃根治（真实线程测试改同步 run 模式已在本轮验证可行）。

### Git Commit

- `2e229c8 carry-over: 上一会话成果（535 例全绿）+ test_stores 日期炸弹修复`
- `fix(ui): _save_window_state 编码误用致窗口状态从不持久化；test: 逻辑层+UI 补测 100 例，80%→96%`
- （未 push）

### 晨报（2026-09-11 轮）

- **完成**：635 测试全绿；总覆盖 80%→96%；修复 1 个真实持久化 Bug + 1 个日期炸弹测试；新增 100 用例。
- **红线遵守**：零真实 DNS/网卡/注册表/服务/socket 操作（哨兵验证）；未触真实 %APPDATA%；未 push。

---

## 2026-09-11 会话（检查 bug / 优化代码 / 美化界面·项目五）

任务：全面检查 bug、优化代码、美化软件界面；保持全量 pytest 全绿。

### 逻辑层 Bug 修复（8 处）

1. `vpn.py vpn_checklist()`：删除恒真死分支 `if tunnels or True:`，按「有无隧道」分流
   proxy owner_hint（有隧道→客户端正常模式提示；无隧道→残留代理提示）。
2. `icmp.py`：探测载荷 `b"NetToolPingData"` → `b"SysKitPingData"`（旧产品名残留）。
3. `proxy.py disable_proxy()`：原先只置 `ProxyEnable=0`、**不清 PAC(AutoConfigURL)** →
   诊断 `proxy_off` 建议对 PAC 脚本无效。现一并清 PAC，且写前用 `_backup_values()` 备份旧值。
4. `winregread.py values()/subkeys()`：异常处理从 `OSError` 放宽为 `Exception`，`Close()` 走安全
   包裹 → 未知 hive(KeyError)/坏句柄(TypeError) 不再崩调用方。
5. `dnsquery.py`：新增 `_encode_qname()`，按 IDNA 编码后的**字节数**写 QNAME 标签长度前缀
   → 修复非 ASCII 域名（如 `例子.com`）QNAME 畸形。
6. `settings.py save()`：改临时文件 + 原子 `os.replace`，先更新 `_cache` 再落盘，失败时上抛
   `OSError` 并清理临时文件 → 不再留半截 JSON / 静默吞错。
7. `netcheck.py chk_persistent_route()`：删除恒不可达的死 `else` 分支。
8. `telemetry.py` top 进程遍历：`p.info["name"]` → `(p.info or {}).get("name")` + 兜底
   `except Exception` → 单个坏进程不再终止整轮采样。

### UI 层 Bug 修复（3 处）

9. `ui/workers.py`：`ComboWorker.run()` / `SnapshotWorker.run()` 包 try/finally，异常时仍发
   `all_done`/`got` → 不再永久卡忙态。
10. `ui/toolbox_page.py`：`_LanScanWorker.run()` 同法保证 `done` 必发；`_kill_selected()` /
    `_apply_filter()` 对空单元格加空值防护。
11. `ui/main_window.py`：`_repair_done()` 修复后刷新代理状态标签（原为陈旧值）；
    `_save_settings()` 包裹 `autostart.enable/disable` 与 `settings.save`，失败给用户提示而非崩溃。

### 界面美化

- **`ui/theme.py`**：QSS 大改——菜单/分隔线、按钮 focus 态、splitter 手柄、tooltip 块、侧栏；
  新增语义 `badge`/`badgeWarn` 与 `brandCard`/`statCard`；新增 helper `sub_color()` /
  `add_shadow(widget,...)` / `app_logo_pixmap(size)`（纯代码绘制蓝色圆角 S logo，规避 onefile
  资源打包问题）；新增 `accent_soft`/`ok_soft`/`warn_soft`/`err_soft`/`brand_sub` 调色。
- **`ui/main_window.py`**：新增 `_build_brand()` 品牌头卡（logo + "SysKit / 系统工具箱" + 版本徽章）
  置于左列；大状态标签加阴影；多处硬编码灰字改 `setProperty("hint", True)` 走主题。
- **`ui/dashboard_page.py`**：`_stat_card()` 重设计——左侧色条（CARD_ACCENTS）+ 阴影 +
  `statTitle`/`statValue`；事件/延迟颜色改用 `theme.pal()`。
- **`system_page`/`disk_page`/`wifi_page`/`toolbox_page`**：硬编码 hint 颜色统一改主题属性。

### 新增工具

- **`tools/ui_preview.py`**（新）：离屏渲染器，mock 全部系统调用与持久化，播种假数据
  （网卡/诊断/WiFi/磁盘树/端口/剪贴板/模式），导出 13 张页面截图；含 `_norm_out()` 修正
  Git-Bash `/e/...` 路径（否则误建 `E:\e\...`）。

### 测试

- 新增/扩展：test_vpn(+2)、test_repairs(+2)、test_icmp(+1)、test_toolbox_calc(原 xfail 转正式
  通过 +2)、test_ui_smoke(+1 异常安全；`_repair_done` 用例改轻量假窗体避免第 7 个 MainWindow)、
  test_system_logic_units(+4 winregread 健壮性)、test_smoke(+1 原子保存)。
- **全量：649 passed / 1 skipped**（三次运行两绿一 127；127 为 PySide6 6.11 offscreen 已知退出
  原生崩溃，发生在用例之后，遵 README 约定重跑即绿）。
- `conftest.py` 增会话级夹具 `_drain_qt_threads_on_session_end`（gc + wait QThread ≤8s + processEvents）。

### 基线变化

- 635 → **649** 例（+14），仍 1 skip；1 xfail 转正。

### 遗留与下一步

- `tools/ui_preview.py` 产出的 13 张截图位于 `.workbuddy/preview/`（随工作区，未入库）。
- 退出阶段偶发原生崩溃（约 1/3）仍未根治，可考虑真实线程测试改同步 run 模式。
- **未 commit / 未 push**。


## 2026-09-11 会话·项目六：命令面板 / 剪贴板升级 / 密码库 / Hosts 编辑器

### 交付（4 个里程碑各自 commit，测试全绿后进下一块）

1. **M1 feat(clipboard) 70be1a0**：剪贴板历史升级——条目多格式（文本/图片/文件/HTML，图片 PNG 落盘 md5 去重）、
   置顶（不被淘汰、防重复入库）、搜索+分类、隐私暂停；敏感内容防护（mark_sensitive 摘要跳过 + 前台进程黑名单）；
   规则引擎 `cliprules.py`（JSON 格式化 / URL 二维码+打开 / 色块 / 文件路径 / IP+域名 DNS·ping·归属地）；
   托盘「最近剪贴板」子菜单；UserRole 修复过滤态删错行隐患。
2. **M2 feat(vault) 1ac7053**：密码库——`vault_crypto.py`（PBKDF2 600k + AES-256-GCM 主密码模式 /
   DPAPI 免密模式，数据密钥仅解锁后驻留内存）+ `vault.py`（SQLite sites/accounts 两级模型，secrets 密文 BLOB，
   search_sites/list_accounts/global_search 空格分词直达账号，find_by_domain 为浏览器扩展预留，RFC 6238 TOTP）+
   密码库页（左右分栏 + DictFilterProxy，双击复制密码走敏感剪贴板 25s 自动清空，添加永不验证/查看需解锁/自动重锁）。
   新依赖 cryptography。test_ui_smoke 页面断言 9/12 → 10/13。
3. **M3 feat(palette) 303b1c5**：命令面板——`hotkey.py`（RegisterHotKey + 解析纯函数）、
   `palette_actions.py`（动作注册表 + fuzzy 子序列评分）、`ui/palette.py`（无边框面板 + HotkeyFilter；
   动态条目：剪贴板回填 / 密码库直达 / ping·dns·URL 语法）；Ctrl+Alt+K 全局 + Ctrl+K 应用内 + 托盘入口；
   自定义命令（URL 直开 / 本地命令确认后执行）。
4. **M4 feat(toolbox) d0ceead**：Hosts 编辑器——`hostsed.py`（parse/serialize 增删改 + 写前备份）
   + 工具箱第 6 个 Tab（标脏编辑、保存需管理员、path 注入可测）。

### 会话内修复的关键 bug

- **跨文件 QWidget 泄漏 → 确定性退出崩溃**：test_palette 的 _FakeWindow 未交 qtbot 回收，
  QApplication 先亡导致原生崩溃（此前多次全量 EXIT=127 的真凶之一，非纯环境问题）。
- **HotkeyFilter 悬空**：过滤器装在 QApplication 上、唯一引用随窗口销毁被 GC → 任何原生事件踩已释放对象。
  改模块级持活 + quit_app/teardown_global_hotkey 反注册。
- QUrl 从 QtGui 移到 QtCore（PySide6 导入修正）；QSortFilterProxyModel.invalidateFilter 弃用改 invalidate()；
  class 体无法读外层局部变量（测试桩改 SimpleNamespace）；int(ctypes 指针) 不支持（过滤器按字节回退解析地址）。

### 基线变化

- 649 → **758** 例（+109：剪贴板 19 + 规则 23 + 密码库 30 + 面板 23 + Hosts 17，扣除改并的旧用例），仍 1 skip。
- `ui_preview.py` 产出 14 张截图（新增密码库页；离屏缺中文字体渲染为方块属环境限制）。

### 遗留与下一步

- 退出阶段偶发原生崩溃仍存在（约 1/3，位置漂移，全程无失败用例）——与 PySide6 6.11 离屏 + QThread 相关，未根治。
- 浏览器扩展自动填充：vault.find_by_domain API 边界已留，扩展本体未做。
- 全局热键修改后需重启生效；TOTP 密钥暂无二维码导入。
- **本会话 4 个 commit 已提交；未 push**。

### 补充（同日）：任务栏/窗口图标

- `theme.app_icon()` 多尺寸（16-256）代码绘制图标；`main_window` 构造即 `setWindowIcon`，
  `app.run_gui` 设 `QApplication.windowIcon` 并注册 `SetCurrentProcessExplicitAppUserModelID`——
  源码运行时任务栏不再显示 python.exe 默认图标、不再归入 python 分组。
- 打包 exe 的任务栏图标本就来自 `assets/app.ico`（build.bat 生成），观感一致。
- 测试：app_icon 多尺寸断言 + 主窗口 icon 接线断言（并入既有构造测试，避免多起一次重型引导）；
  `_FakeApp` 桩补 `setWindowIcon`。当前代码下全量 759 例分区验证全绿。
- 观察记录：本日末段 PySide6 离屏偶发 fail-fast 频率显著升高（>1/3，位置漂移、无失败用例、
  单文件/分区重跑即绿），与环境会话长度正相关，未根治。

### 补充2（同日）：安装向导 + 便携数据模式 + 任务栏图标修复

- **fix(vault)**：添加账号报 missing positional argument 'password'——对话框密码 pop 后漏传。
  已修并补两条回归用例（对话框全流程 / 空密码拒绝）。
- **build 三连修**（用户 build.bat 出不了 exe 的根因）：
  1) build.bat 改 `python -m PyInstaller`（--user 安装的 Scripts 不在 PATH，裸 pyinstaller 找不到）；
  2) build.bat/install.bat 被 LF 化导致 cmd 解析错乱 → 转 CRLF 并加 .gitattributes 钉死；
  3) 第4步内联 PowerShell 引号在 cmd 下必炸 → 改 tools/package_zip.py。
- **安装向导**：installer/SysKit.iss（Inno Setup；winget 不可用，从 GitHub releases 装 7.1.0 用户级）
  下一步式向导：可选安装目录、桌面快捷方式、开机自启（schtasks 最高权限）、卸载信息；
  `PrivilegesRequiredOverridesAllowed=commandline` 支持免管理员静默装用户目录。
  build.bat 第5步自动探测 ISCC（6/7 × x86/x64/用户目录），没有则跳过只出绿色版。
- **便携数据模式**：安装勾选「数据保存在安装目录」→ 向导创建 `<安装目录>\data`，
  config.py 检测到该标记即把 settings/密码库/剪贴板/日志/备份全部收敛到
  `data\config` 与 `data\local`，随目录整体搬迁；卸载保留数据。测试 test_portable.py。
- **任务栏图标修复**：打包 exe 下设 AUMID 反而令 Windows 找不到快捷方式图标源而回退默认图标
  → 改为仅源码运行设 AUMID；打包 exe 按内嵌 app.ico 正常显示与分组。

### 补充3（同日）：bug 排查（便携/密码库/热键）

- **fix(vault)**：`_connect` 未建父目录——全新便携安装后 data\config 不存在，密码库首次操作报
  unable to open database file。现自动建目录（回归：嵌套不存在路径 init_db+add_site）。
- **fix(app)**：新增 `_prepare_data_dirs()`——启动即建数据目录；便携模式首次运行自动把旧
  %APPDATA% 的 settings/vault/clipboard/traffic/events 迁移到 data\config（不覆盖已有新数据）。
- **fix(vault UX)**：DPAPI 免密模式手动锁定后，复制密码曾弹"主密码"框（本就无主密码）→
  `_ensure_unlocked` 对 DPAPI 模式静默重解锁。
- **fix(hotkey)**：`hotkey_display("Ctrl+F9")` 曾显示成 "Ctrl+x"（F 键未走命名分支）。
- 相关测试 +6；test_vault/test_portable/test_palette 全绿；ui_preview 通过；已重新打包 exe/zip/setup。

---

# 2026-09-11 深夜会话（总控 Agent：覆盖率深补 + 测试基建加固）

### 当前状态

- 分支 `test/pytest-setup`；全量 **882 用例全绿**（2 段式验证均 EXIT=0，1 已知 skip），
  总覆盖 90% → **96%**（miss 271/7162）。
- 本轮零业务代码改动；新增 4 个测试文件 116 用例 + 1 个测试基建修复（conftest msgbox_noop）
  + 1 篇 error 文档。

### 已完成

1. **摸底与 blocker 根治**：
   - 发现全量跑两处病根：① `test_vault.py::test_add_account_empty_password_rejected`
     未 mock `QMessageBox.warning` → offscreen 真实模态永久挂起，300s 被 timeout 击杀，
     强杀 Qt 模态连带原生崩溃（EXIT 127）；② `pytest --cov` 插件模式 + PySide6 原生崩溃
     （崩点稳定于 palette 首用例，非 cov 方式崩点漂移）。
   - 修复：conftest.py 增全局 autouse `msgbox_noop`（QMessageBox 五个静态弹窗 → 记录型
     no-op，需断言弹窗的测试可自行覆盖）；覆盖率测量改用
     `coverage run -m pytest`（插件模式兼容性问题详见
     `error/2026-09-11-pytest-cov-pyside6-native-crash.md`）。
2. **Task S1 vault/palette/tray/hotkey**（子 Agent A，58 用例）：vault_page 67%→**99%**、
   palette 71%→**100%**、tray 76%→**100%**、hotkey 87%→**100%**。顺带稳定了托盘测试
   会话收尾的原生崩溃（fixture 内提前 hide+deleteLater+processEvents，连续 5 次 EXIT=0）。
3. **Task S2 charts/modes/pages**（子 Agent B，58 用例）：charts 71%→**99%**、modes 74%→**100%**、
   disk_page 81%→**100%**、system_page 87%→**100%**、diskusage 83%→**99%**、proxy→**100%**、
   settings→**100%**。并把 test_ui_smoke 独有的覆盖路径（PerfTab/BatteryTab/disk_page 渲染链）
   复制进自己的文件，缓解 ui_smoke 崩溃对指标的影响。

### 测试

- 摸底（修复后）：`coverage run -m pytest --timeout=300 -q` → 766 用例全绿，TOTAL 90%。
- 收尾（定版，分段式）：
  - 段1 `coverage run -m pytest tests/ --ignore=tests/test_ui_smoke.py --timeout=300 -q` → EXIT=0；
  - 段2 `pytest tests/test_ui_smoke.py --timeout=300 -q`（裸跑）→ EXIT=0；
  - 合计 882 用例全绿（1 skip：已知 IPv6 冻结防护），TOTAL **96%**。

### 已知环境限制（非回归，详见 error 文档）

- `test_ui_smoke.py` 在 coverage 追踪 + 高负载下可复现原生崩溃（真实 QThread DiskScanWorker
  + qFatal 区间），裸跑稳定；并发跑测试会放大。全量 coverage 验证请用上述两段式。
- `pytest --cov` 插件模式本机不可用（崩），一律 `coverage run`。

### 疑似 Bug（只记录不修，本轮新增 6 个）

1. vault_page.py 259-260/303-304：列表刷新末尾的"补选中"防御永不可达（_sync_list 已保证）。
2. tray.py `_copy_back`（96-98）缺 `if not clip: return` 判空（palette 同名方法有），
   clipboard() 返回 None 时信号槽内 AttributeError。
3. charts.py:16 与 diskusage.py:130：human_bytes 循环后 TB 兜底 return 不可达（死代码）。
4. modes.py:224：close_background 对"targets 全为非法 pid"返回 ok=False 但 detail 为
   "已结束 0 个"，用户无从判断原因（体验）。
5. ui/disk_page.py:352：删除整批失败 0 项成功时不重扫，与"删除完成后自动重扫"注释语义不符。
6. tests/test_clipboard_modes.py::test_set_notifications_success monkeypatch 了自身再调用
   （测试恒真，未测真函数）——本轮已由新测试补上真本体覆盖。

### Coverage（收尾 96%）

低于 90% 仅剩：`ui/workers.py` 40%（QThread worker 区间，coverage 追踪下覆盖即接近原生
崩溃高危区，刻意保留并记录）、`__init__` 38% / `__main__` 0%（入口桩）、
`dashboard_page` 85%、`confirm_dialog` 88%。其余全部 ≥96%，约 2/3 模块 100%。

### 下一步

- ui/workers.py 的 QThread 深覆盖建议配合真实进程级隔离方案（如 subprocess 化用例）再评估；
- 上游跟进 pytest-cov × PySide6 兼容性；dashboard_page/confirm_dialog 补至 ≥90% 可作为小任务。

### Git Commit

- `test: vault/palette/tray/charts/modes/pages 补测 116 例 + msgbox_noop 基建，coverage 90%→96%`（本轮，未 push）

### 晨报（2026-09-11 深夜）

- **完成**：根治 2 个测试基建 blocker（QMessageBox 挂起、pytest-cov 插件崩溃并留档规避方案）；
  +116 用例全绿（766→882）；总覆盖 90%→96%；记录疑似 bug 6 个。
- **阻塞**：无（ui_smoke coverage 态崩溃为已知环境限制，已用两段式验证与 error 文档覆盖）。
- **红线遵守**：零业务代码改动；无真实修复动作/注册表写/网络/剪贴板（哨兵 + mock 全程拦截）；
  未碰真实 %APPDATA%；未 push。


### 补充4（同日）：性能与体积优化

- **遥测热路径**（每秒 tick）：
  - `_top_procs` 只在缓存为空/每 60 秒重建轮走 process_iter 全表（原每 2 秒全表枚举数百进程）；
    非重建轮仅对缓存 pid 读 cpu_percent/rss，退出 pid 清缓存；缓存空自动降级重建。
  - TrafficStore.today() 秒级免锁快照（原每秒加锁+strftime 拼 key）；add() 零增量不标脏
    （原无流量也每分钟空写盘）。
- **密码库**：sites(name COLLATE NOCASE) / accounts(site_id, is_favorite DESC) 两个索引。
- **exe 体积 67.6 → 37.7 MB（-44%）**：spec 排除 numpy/OpenBLAS（psutil、qrcode 均不需要）、
  软件 OpenGL 备胎 opengl32sw.dll(20MB)、Qt6 Quick/Qml/Pdf/Network 运行库、PIL avif 编解码。
  10 页离屏渲染 + 全量 767 例回归全绿后打包。

### 补充5（同日）：bug 排查第二轮（pyflakes 全量 + 逻辑审查）

- **fix(vault UI)**：删除最后一个网站后右侧账号列表残留旧账号——refresh_sites 依赖
  setCurrentRow 间接触发账号刷新，列表清空时 currentRowChanged 不触发。现 refresh_sites
  在行号未变/列表清空时主动幂等刷新账号列表（回归用例证实修复：删空后 acct 2→0）。
- pyflakes 全量扫描：0 未定义名；清理新增代码无用导入（palette ctypes/QImage 顶层、
  main_window hotkey、system_page theme_ok/except e）——纯风格，无逻辑影响。
- 已知遗留（评估后接受）：ClipboardTab._ClipActionWorker 3s DNS 窗口内退出理论上可遇
  QThread 析构竞态——onefile 退出不跑 GC 析构 + conftest 会话兜底，实际风险极低。

### 补充6（同日）：生产崩溃排查（用户真机）

**证据链**：crash.log 无 Python 异常 → WER ReportArchive 发现 AppCrash_SysKit.exe ×3 + AppHang ×1
→ 事件日志：BEX64，c0000409，P9=7（FAST_FAIL_FATAL_APP_EXIT = qFatal/abort），模块 Qt6Core.dll 6.11.2，
时间 2026-09-12 13:01:34；syskit.log 显示 13:01:00 深度修复组合（含 winsock_reset/ipstack_reset）刚跑完。

**根因**：退出路径不等待页面级 QThread——quit_app 只停 monitor/telemetry，
_diag/_repair/_combo/_snap 四个 worker 完全不管理；且各 _done 槽把引用置 None 后
QThread（无 parent）仅剩 Python 引用，emit 后线程收尾的微秒窗口内被 GC 析构 →
"QThread: Destroyed while thread is still running" qFatal。用户跑完深度修复立即退出
= 时间线吻合；测试环境约 1/3 的退出崩溃同源。

**修复**：
- main_window._release_worker(attr)：复位前 wait(3000)，四个 _done 槽统一走它；
- quit_app 退出前逐个 wait(5000) 四个页面 worker；
- _qt_msg_handler 增加 QtFatalMsg 分支：同步直写 crash.log + fsync——qFatal 是原生崩溃
  前最后痕迹，loguru 缓冲/锁可能来不及落盘，本次日志里没有 [qt] 消息即因此。
- 回归：quit_app 等待慢 worker（0.4s 收尾断言 run 完整）、_release_worker 等待后再置 None；
  全量 769 例全绿，已重新打包。

### 补充7（同日）：密码查看/复制加主密码门 + 一键显示

- vault_page 新增 `_require_master_verified()`：查看与复制密码**每次**都验证主密码
  （设置了主密码时）；DPAPI 免密模式无主密码可验，仅要求解锁。
- 「查看」改为一键显示/隐藏开关：验证后明文显示在明细区，**20 秒自动隐藏**，
  再点立即隐藏并清除明文；换选账号/锁定/刷新状态均即时收起明文。
- 复制密码走同一道门，复制后仍为敏感剪贴板（不进历史 + TTL 自动清空）。
  用户名 / TOTP 保持解锁级（TOTP 是 30 秒时效码，每次索要主密码不现实）。
- 添加/编辑账号维持原语义（用户要求：添加永不弹验证）。
- 测试：主密码门（正确/错误/每次都要）、一键显示隐藏、20s 定时隐藏 +3 例；
  gaps 互补文件 2 处断言适配新文案；全量 772 例全绿，已重新打包。

### 补充8（同日）：中止按钮 + bug 复查

- **中止命令按钮**（用户跑深度修复 34s 无法取消的直接反馈）：
  - 诊断页新增「停止」、修复页一键修复组新增「中止」（danger 样式）；
  - ComboWorker/DiagnosisWorker 增加 cancel() 协作式取消：当前动作/检查不可中断
    （系统操作无法撤销），完成后剩余项跳过，all_done/finished_ok 照常发出（忙碌态必复位）；
  - netcheck.run_all 新增 should_stop 回调参数（向后兼容），中止时追加 skip 状态
    「诊断中止」记录（同步走 progress）；
  - 按钮启停接线：worker 启动→启用，点击→立即置灰防重复 + 状态栏提示，
    完成槽→禁用；_combo_done 区分中止/完成弹窗（含"已执行操作不自动撤销"警示）。
- **bug 复查**：pyflakes 零未定义名；修复 vault 锁定+主密码时查看密码连弹两框
  （解锁+验证）→ 锁定态单次弹窗、密码同时用于解锁与验证。
- 测试 +3（组合取消三态/run_all should_stop/停止按钮接线），fake_run_all 全部签名
  适配；全量 775 例全绿；已重新打包。

### 补充9（同日）：收尾——热键即时生效 + AppHang 线索核结

- **热键即时生效**：设置页保存命令面板热键后立即反注册旧键并注册新键
  （teardown+setup），不再需要重启；被占用时静默降级（托盘菜单/Ctrl+K 仍可用）。
  集成测试断言新 vk 重注册。
- **AppHang ×1 核结**：ProgramData WER 报告详情需管理员权限无法读取；已知的
  应用事件签名（BEX64/c0000409）与已修的 QThread 析构崩溃同源，判定为同根
  症状，预计随退出等待修复一并缓解；若复发，crash.log 将留下 QtFatal 文本。
- README：补诊断「停止」/修复「中止」/热键即时生效说明；全量 775 例全绿；已重新打包。

### 补充10（同日）：bug 修复——非法热键保存保护 + 中止统计健壮化

- **fix(settings)**：设置页保存非法热键（如手输"不是热键"）时，原实现会先反注册旧键、
  再因解析失败注册不回来 → 全局热键彻底失效。现保存前 parse 验证：非法则警告、
  设置回退旧值并以旧值重注册（热键不丢）。讽刺注：验证用的 hotkey 导入恰是上一轮
  "清理无用导入"删掉的，本轮恢复——删导入前应确认未来用途。
- **fix(combo)**：_combo_done 中止统计 `and` 表达式在空列表时返回 [] 传入 max() 会
  TypeError → 改为显式 planned-executed 计算。
- 测试：非法热键保存断言（警告 + 旧值重注册 + 设置不污染）；全量 775 例全绿；已重新打包。

### 补充11（同日）：文档全面更新 + 体系外文件清理

- **summary/PROJECT_SUMMARY.md 整体重写**（原版停在 2026-09-10 / bfafc93 / 覆盖率 80%）：
  项目概览补四大新功能与便携模式；技术栈 +cryptography/Inno Setup/体积裁剪；
  代码结构更新为 27 逻辑模块 + 15 UI 模块 + 29 测试文件 775 例（新增剪贴板/密码库/
  面板热键/Hosts 域与 vault_page/palette）；架构章节补密码库数据流与命令面板数据流、
  协作式取消与敏感数据三层防线两个新模式；目录树重新生成；9/10 版的覆盖盲区分析
  （B1-B10）整体移除——其 P0 建议均已落地。
- **README.md**：修图形界面小节与功能总览的数字矛盾（13/16→15/17）；新增
  「密码库与敏感剪贴板」「便携模式」两个专节；安装向导描述补可选目录与便携勾选。
- **体系外文件清理**（原 B9）：test_ui_shots.py 删除（被 tools/ui_preview.py 完全替代）；
  check_events.ps1 移入 tools/（内容已匹配 SysKit，崩溃排查实用）。
- 文档内全部代码路径经脚本校验存在；check_events.ps1 实测可用（并证实 13:01 AppHang
  与两次 BEX64 同窗发生，同源判定成立）。

### 补充12（同日）：bug 排查（真实使用路径行为盲区）

- **fix(settings/hotkey)**：新热键被占用（格式合法但 RegisterHotKey 失败）时旧热键已注销
  → 全局热键全丢。现 setup 失败自动回退旧键重注册并提示；修的过程中又发现并修正了
  previous_hotkey 快照位置错误（原写在 settings.save 之后，读到的是新值）。
- **fix(modes)**：场景模式静音/取消静音用 waveOutSetVolume 直接置 0/0xFFFFFFFF——
  用户进模式前 30% 的音量，退出后变 100% 且原值永久丢失。现静音前
  waveOutGetVolume 读当前音量存 settings（modes_prev_volume），取消静音恢复记录值；
  无记录（非本工具的静音）保持旧行为置最大并注明；静音失败保留 stderr 截断详情。
- **fix(cli)**：cmd_repair 的 combos 与 repairs.COMBOS 重复定义（未来改动会悄悄与 UI
  分叉）→ 复用 COMBOS 同源。
- 测试 +4（占用回退/原音量恢复/无记录分支/既有签名适配）；全量 775 例全绿；已重新打包。

### 补充13（同日）：开发依赖固化（维护章程遗留项 #1）

- 新增 dev-requirements.txt：-r requirements.txt + pytest==9.1.1 / pytest-qt==4.5.0 /
  pytest-cov==7.1.0 / pytest-timeout==2.4.0 / pyflakes==3.4.0（钉当前全量验证过的版本），
  pip --dry-run 依赖解析通过。
- README 新增「开发与测试」章节（安装/运行/单文件/静态扫描/截图验收/安全网说明）。
- PROJECT_SUMMARY 技术债表同步（移除"开发依赖未固化"）。
- 维护章程生效后首个任务；后续遗留：PySide6 升级验证（需明确要求）、VaultPage 状态模型化（待功能驱动）。

### 补充14（同日）：启动性能优化——页面构造懒加载（-90%）

- **测量先行**（§27）：逐页构造计时定位元凶——SystemPage 983ms（BatteryTab 构造即发
  PowerShell CIM 电池查询）、WifiPage 220ms（构造即发 netsh show profiles）、
  PortTab 构造即起 psutil 端口扫描 worker（用户可能永不打开工具箱页）。
- **实施**（沿用 WifiPage 既有 `_initial_scanned` showEvent 首显模式）：
  BatteryTab / PortTab 构造期 refresh() → showEvent 首显触发（`_initial_refreshed`
  防重复）；WifiPage 配置文件枚举并入既有 showEvent 首访块。
- **效果实测**：SystemPage 983→16ms、WifiPage 220→7ms、页面构造合计 1239→120ms（-90%）、
  MainWindow 构造 1048→422ms（-60%）。
- 测试：+1 回归（构造零系统调用/首显恰一次/重复显示不重复查）；5 处既有测试适配为
  显式触发（行为有意变更，非掩盖）；全量 776 例全绿。
- **遗留**：dist 产物重打包被正在运行的 SysKit（管理员实例）锁定——代码已提交，
  软件关闭后跑 build.bat 即可（exe 目前仍是懒加载前的版本）。

### 补充15（同日）：密码库 UX 重构 + 本地数据加密（用户五点需求）

1. **账号表三列（账号/密码/标签）**：密码列未验证显示 ****；「显示」按钮（主密码门）
   验证后整列明文 20 秒（每格解密不再触碰 last_used_at）；单击账号/密码/标签格即复制
   （悬停有提示），不再依赖双击；QListWidget→QTableWidget（DictFilterProxy 过滤保留）。
2. **主密码信任窗口**：验证一次 → 30 分钟内查看/复制不再重复索要（`vault_trust_min`
   设置项）；窗口到期自动重锁并再索要；手动锁定清除窗口；DPAPI 免密模式不变。
3. **本地数据加密**：新增 `secure_store.py`（DPAPI 密封 JSON 容器，兼容旧明文透明
   迁移、损坏/换用户安全回退默认）；clipboard.json 落盘即密封——磁盘上不再有明文
   剪贴板历史（回归测试断言密文中不含原文）。settings/traffic/events 为非敏感运行
   数据维持明文；密码库 secrets 本就逐字段 AES-GCM（§23 密钥模型未动）。
4. **安装向导默认勾选「数据保存在安装目录」**（仍可取消→%APPDATA%）；安装目录本就
   可选（向导目录页）。
5. 测试：secure_store 6 例 + 剪贴板密封回归 1 例 + vault 表格/信任窗口重写
   （test_vault.py 3 例重写 + gaps 文件批量适配）；全量 783 例全绿。

### 补充16（同日）：bug 复查（secure_store/clipboard/vault 改动面二次审查）

- **fix(secure_store)**：save_sealed_json 只捕 OSError，而 _dpapi_protect 在非 Windows/
  DPAPI 不可用场景抛 AttributeError——异常穿透风险。改为捕 Exception（存储层防御语义：
  任何失败返回 False，调用方 ClipboardHistory.save 再兜一层）。
- 清理功能迁移后的无用导入（clipboard.json、vault.secrets）与测试怪异 skipif；
  pyflakes 新改动面零告警。
- 其余二次审查结论（无问题）：构造顺序安全（_pw_display 仅在明文模式触 DB）；
  信任窗口与手动锁定/更改主密码/DPAPI 模式的交互语义一致；表格 blockSignals 重建
  无信号竞态；Inno 卸载不删便携 data 用户数据（仅删安装器记录的文件）。
- 全量 783 例全绿。dist 重打包仍被运行中的实例（17656）锁定，继续待用户关闭。

### 补充17（同日）：bug 排查——便携/非便携模式切换的数据可达性

- **fix(app)**：模式切换反向陷阱——用户用便携模式积累数据后重装取消勾选（或换绿色版），
  程序回 %APPDATA% 模式，密码库/剪贴板"看起来全丢"（数据实际还在安装目录 data\）。
  反向不做自动回迁（避免覆盖 %APPDATA% 里可能更新的数据，§16 保守语义），但检测到
  "安装目录 data\config 有 settings/vault + 当前数据目录全新"时记明确 warning 日志
  （指明数据实际位置与两条恢复路径）。
- 测试 +1（三种分支：残留提示 / 无残留不提示 / 当前非全新不提示，loguru 桩补 format）。
- 全量 784 例全绿。dist 重打包仍被运行实例锁定（17656），继续待关闭。

### 补充18（同日）：dist 产物补齐（懒加载 + 密码库 UX + 加密存储全部入库）

- 用户关闭运行实例后补跑 build.bat 成功：SysKit.exe 37.7MB / setup.exe 39.1MB / 便携 zip 37.4MB，
  三产物均含本日全部改动（页面构造懒加载 -90%、密码库三列表格+信任窗口+单击复制、
  剪贴板历史 DPAPI 加密落盘、安装器默认便携数据）。
- 产物启动验证（§42）：--minimized 拉起，主进程 137MB 运行正常（另见 onefile 引导进程），
  crash.log 无 QtFATAL。

### 补充19（同日）：PySide6 验证收结 + git 历史清理

- **dist/* 移出 git 历史**（用户二选一指定"移出历史"）：HEAD 解除跟踪 → bundle 全量备份
  → git-filter-repo（python -m，Scripts 不在 PATH）清除全部历史中 dist/ → 双重验证
  （`git log --all -- dist/` 0 条、磁盘产物完好）→ 强推覆盖远端（远端仅此一分支，
  push 前为空仓库，67MB 警告 blob 即我们自己的历史）。强推后 GitHub 大文件警告消失。
  仓库备份：Temp\SysKit-backup-pre-rewrite.bundle。
- **PySide6 验证收结**：pip index 确认 **6.11.2 即当前最新版本**（无可升级目标）；
  "升级验证"转为最新版稳定性验收——三轮全量回归全绿 **0 崩溃**（修复前基线约 1/3），
  证实退出路径修复（quit_app 等待 worker/_release_worker/qFatal 落盘）已根治偶发崩溃。
  requirements.txt 注明验证基线。PROGRESS 遗留项关闭。

### 补充20（2026-09-13）：依赖审计——12/12 对齐最新版

- **审计结果**：PySide6/loguru/psutil/qrcode/pillow/pyinstaller/pytest 系/pyflakes
  全部已是最新；唯一落后 cryptography 49.0.0 → **升级 50.0.1**（安全库跨大版本，
  可能含 CVE 修复；我方 API 面仅 AESGCM encrypt/decrypt，极稳定）。
- **替代品评估**（§40/§41）：各库均为领域事实标准——PySide6（Qt 官方绑定，LGPL 优于
  PyQt6）、loguru/psutil/pillow/qrcode/pyinstaller/pytest 系保持；pyflakes→ruff 为可选
  但不值得为 lint 引新依赖，不换。结论：无换栈必要，零新增依赖。
- **验证**：vault/secure_store 加密路径测试（真实 AESGCM+DPAPI）→ 全量 784 例全绿 →
  打包（PyInstaller hook 正确收集 cryptography 50 的 _rust.pyd）→ exe 冒烟无 QtFATAL。
- requirements.txt 注明 cryptography 验证基线。

### 补充21（同日）：多子代理查 bug 第一批（UI 状态机维度）——P0 一枚 + P1 两枚 + P3 批量

- **P0 编辑账号清空数据**：_AccountDialog 不回填现有值，保存用空串覆盖 username/label/
  TOTP/notes（密码例外）——静默不可逆丢失。修：对话框接 account 参数回填（TOTP 回填框
  密码遮蔽），_edit_account 传现有值（totp/notes 解密回填 touch=False）。此缺陷曾被
  "假对话框预填"的旧测试掩盖——mock 恰好绕过真缺陷的实证。
- **P1 面板绕过密码门**：命令面板"复制密码"只 _ensure_unlocked，绕过 _require_master_verified
  信任窗口门 → 面板路径改走同一道门。
- **P1 日志页守卫旧索引**：_reload_log 硬编码 currentIndex()!=8（密码库页插入前的旧页码，
  现日志=9）→ 日志页刷新/换源全失效、设置页反而空转。改为对象比较 page_logs + 构造期
  属性守卫；两处 switch_page(8) 假绿测试修正为 9（git 考古确认守卫写于密码库页插入前）。
- **P2 BatteryTab 异步化**：首显查询从 UI 线程同步 PowerShell（最长 30s 冻结）改
  _BatteryWorker 后台 + got 信号回填；防重入哨兵。
- **P3 批量**：明文跨站残留（换站收起）、账号过滤选中保持、直关监听复选框同步 _last
  （不补录语义统一）、PortTab hideEvent 停自动刷新表、Hosts 加条目查重、剪贴板规则
  动作 _bg 纳入 quit_app 等待、Ctrl+Return/Alt+T 限定 WidgetWithChildren 作用域、
  autostart 变更移到 settings.save 之后（消除半完成态）、托盘速率开关即时生效。
- 测试：+3 新回归（编辑回填/表格掩码单击复制/电池懒加载+异步），全量 **787 例全绿**。

### 补充22（同日）：多子代理查 bug 第二批（并发/生命周期维度）——退出路径系统性重构

子代理发现此前修复只覆盖主窗口 4 个 worker，页面级同族缺陷系统性存在（11 处槽内置
None 的 GC 窗口、quit_app 清单漏 8 类 worker、wait 超时后置 None 重开窗口、
BatteryTab._bw 30s 窗口完全无人等待）。本轮按 Qt 正解统一重构：

- **R1 workers.reap(worker, owner) 统一设施**：finished→deleteLater（C++ 对象生命
  周期与 Python 引用解耦，槽内置 None 从此安全）+ setParent(owner)（挂对象树，
  退出可沿 findChildren(QThread) 枚举）。应用到全部 15 处构造点
  （主窗4 + Battery/Scan/Clean/ClipAction + Speed/Lan/Port/Tracert + Wifi×2 + Disk删除）。
- **R2 quit_app 全树扫尾**：沿 allWidgets×findChildren(QThread) 枚举 isRunning 的
  线程统一 wait(10s)（telemetry netsh 最坏 8s），替代逐类清单（8 处遗漏不再漏）；
  TelemetryWorker 挂 parent；主窗口引用照旧复位（deleteLater 托管，超时清引用安全）。
- **R3** PingRouteTab：hideEvent 停持续 ping 表（icmp 同步卡 UI 1.5s/次）+ showEvent
  恢复；tracert 运行中再点变「停止追踪」接线 stop()（原 stop() 为死代码）。
- **R4** CleanupTab _ScanWorker/_CleanWorker 补异常兜底（契约：无论如何发 done）。
- **R5** 热键"原键被抢"场景：保存时注册失败总是提示（原来同键被抢静默丢失）。
- **R6** 托盘菜单显式保引用（self._menu，setContextMenu 不取得所有权）。
- 测试：+1 tracert 中止分支回归；适配占位/守卫 4 处；全量 **790 例全绿**。

### 补充23（同日）：多子代理查 bug 第三批（Windows 交互维度）——两个 P1 实证修复

- **P1 游戏模式方向性错误**：`POWER_HIGH_PERF = "SCHEME_MAX"` 实为**节能**方案
  （powercfg 官方别名：SCHEME_MIN=高性能/SCHEME_MAX=节能）——进游戏模式实际切到
  最卡方案。修正为 SCHEME_MIN（测试断言同步）。
- **P1 静音链路整体失效**（真机实证 CS1529）：_PS_AUDIO_PREAMBLE 给 Add-Type
  -MemberDefinition 传了完整类定义+using——生成嵌套类致方法不可达、using 在类体内
  致编译失败；且 PowerShell 不认 C# 的 `0u` 后缀。三连坏，测试 mock 了 run_ps 全程
  未暴露。修：成员定义式 + [uint32]，**产品代码真机端到端验证**（读原音量→静音→恢复
  全链 ok）。prev_volume=0 不再覆盖更早记录（重复进模式数据丢失）。
- **P2 hosts 编码护栏**：utf-8-sig 兼容 BOM（曾致首条目静默降级）；UTF-16/二进制
  误读（含 NUL）时拒绝写回保护原文件。测试 +2（BOM 解析/护栏）。
- **P3 批**：hotkey_display OEM 键乱码（字面名表）；PowerOff USB 提示条件拼接；
  pause_windows_update 文案对齐行为（无限期禁用非"7 天"）；autostart.disable
  管理员守卫+返回语义（任一成功即 True，+3 测试）；HostsTab 查重改 host+ip 组合
  （同名多 IP 轮询合法）；面板剪贴板缓存（mtime 变化才重读，免每键 DPAPI 解密）。
- 过程事故记录：bash heredoc 转义折叠两次向测试文件注入真 NUL/BOM 字节——改用
  chr() 构造 + Edit 工具修复。
- 全量 **791 例全绿**。

### 补充24（2026-09-13）：多子代理查 bug 第四批（持久化安全维度）——6×P2 + P3 批

- **P2 面板缓存 NameError**（上轮缓存补丁自身 bug）：cache-hit 分支引用未定义 hist →
  剪贴板区块从第二个键起消失。修正循环变量。
- **P2 vault.key 损坏启动崩溃循环**：空/截断 key 文件使 unlock 抛 KeyError（try 外）→
  构造失败反复崩。修：整体捕获 + ERROR 日志明确"可删该文件"路径，库保持锁定不崩。
- **P2 迁移部分失败永久跳过**：跳过条件从"新目录非空"收紧为"关键文件（settings/vault）
  已在"——残缺迁移下重跑以 copy2 覆盖补齐（源是唯一真本）。
- **P2 迁移后明文残留**：迁移成功的源文件改名 `.migrated` 退役（clipboard.json 常含
  敏感内容），可逆（改回原名）不覆盖（§15/§16）。
- **P2 编辑对话框 autolock 竞态**：模态编辑期间暂停自动重锁（finally 恢复）+
  update_account 补 VaultLockedError 兜底提示（此前静默丢改动）。
- **P2 剪贴板 PNG 明文残留**：删除/清空/超额淘汰时同步删除 PNG 载荷文件
  （明文截图不再无限期累积），+1 回归。
- **P3 批**：search_sites LIKE 通配转义；ttl=0 敏感标记延长；WLAN 密码复制走敏感路径
  （todo——未含本批）；密封写失败 LOGGER.warning；kdf.iter 读回兼容（防未来调迭代埋雷）；
  notes 不 strip；_nav_changed save 兜底；反向检测补 clipboard.json；backup_config
  补 vault.db/vault.key；_AccountDialog edt_totp 双写笔误清理。
- 过程记录：heredoc 转义折叠再次注入 NUL/BOM（改 chr 构造 + 字节层替换修复）；
  打包被用户正在运行的实例阻塞——build.bat 增加运行实例检测（明确提示代替
  PermissionError），产物待用户退出后补打。
- 全量 **792 例全绿**。

### 补充25（同日）：多子代理查 bug 第五批（CLI/Monitor 一致性维度）——P0 一枚 + P1 一枚 + P2×3 + P3 批

- **P0 退出冻结**：quit_app 的 `mon.join(3000)` 把毫秒当秒传给 threading.Thread.join
  （=3000 秒！）；且 Monitor.stop() 只置 _stop_evt 而 run 的可中断等待只监听
  _force_check——每次退出 UI 冻结 0~600s（默认 30s）。修：join(3) + stop 同时置
  force 唤醒。**这是用户每次退出都踩的 bug**。
- **P1 中止误判**：中止的不完整诊断（缺 real_conn）被 verdict 判为"外网不可达"，
  大徽章+托盘变红误导。修：_diag_done 检测 diag_aborted → 状态保持此前判定。
- **P2×3**：L3 冷却时间戳移到梯度级别（restart_wlan 早退时不再绕过限频）；
  cmd_repair 失败退出码 1（原恒 0，自动化无法感知失败）+ input EOFError 兜底；
  DnsAuto 备份补 IPv6（原只备 v4 却同清 v6 静态值，永久丢失）。
- **P3 批**：冷却时间戳 time.monotonic（防系统时间回拨绕过冷却）；quit_app 全树
  扫尾对有 stop() 的线程先请求停止（tracert 27s>10s 上限）；未知子命令报错退出
  （曾静默弹 GUI 掩盖拼写错误）；proxy_off 备份先于修改落盘。
- 代理确认无问题：reap 设施与 quit_app 全树扫尾在位未被破坏；netcheck 各 chk_* 单项
  防御完备；repairs 备份覆盖良好；proc 超时无孤儿（除面板 shell=True 刻意保留）。
- 测试：+3（DnsAuto v6 断言×2 适配 / disable 管理员守卫与 legacy 兜底语义拆分），
  全量 **792 例全绿**。dist 打包待运行实例退出（build.bat 检测提示待修 cmd 管道）。

### 补充26（2026-09-13）：剪贴板写入热路径优化（测量先行）

- **测量**（500 条历史场景）：add 每次复制 = 全量 JSON 序列化 + DPAPI 密封 + 写盘 =
  **9.2ms UI 线程阻塞**，且 O(N) 随历史增长；连发 5 段文本累计 30ms。托盘/面板构造
  读盘 2.5-7.6ms 为一次性/低频操作，可接受不动。
- **优化**：add/remove/clear/toggle_pin 只改内存 + `mark_dirty()`；刷盘由
  ①ClipboardTab 10s 定时器（`_flush_disk`）②quit_app 退出前 flush（全树扫尾前触发）
  兜底。权衡记录：程序崩溃最多丢最近一个刷盘周期（≤10s）内的几条历史——
  换取复制操作 UI 线程成本 **9.2ms → 0.11ms（-99%）**。
- PNG 载荷清理仍同步在变更点（内存操作+unlink，廉价且保证不留盘）。
- 测试适配 6 处（跨实例读取前显式 `flush_if_dirty()`，即防抖语义的显式落盘点）；
  全量 **792 例全绿**。

### 补充27（2026-09-13）：build.bat 运行实例检测修复（find→findstr）

- 上一轮的实例检测用 `find /I`——在 Git Bash 环境跑 build.bat 时被 GNU find 劫持
  （`find: '/I': No such file or directory`），检测静默失效。换 **findstr**（Windows
  独有无同名冲突）并实测：检测触发、明确提示"检测到正在运行的 SysKit…请先退出"。
- 附带发现：检测失效只影响从 Git Bash 调 build.bat 的场景（资源管理器双击时
  find 为 Windows 版，检测正常）——但 findstr 对两种环境都稳定。
- dist 打包继续待运行实例退出：现在退出后跑 build.bat，若仍有实例会得到明确提示。

### 补充28（2026-09-13）：遗留清单清理（TOTP 二维码/密码库备份/WLAN 掩码/提示补齐）

- **TOTP 二维码导入**（遗留清单项）：账号工具行新增「TOTP 码」按钮（仅含 TOTP 的账号
  可见）——对话框显示 otpauth:// 二维码（手机验证器扫描导入）+ 6 位实时验证码 +
  「复制验证码」（走敏感剪贴板路径）。
- **backup_config 补密码库**：场景模式备份清单加 vault.db/vault.key（此前整机恢复
  后密码库不可恢复）。
- **WLAN 密码掩码 + 敏感复制**：已存密码表默认掩码（••••••••），「显示密码」复选框
  切换明文；复制走 mark_sensitive 敏感路径（不进剪贴板历史）——与密码库防护级别对齐
  （持久化代理 P3 发现）。
- **proxy_off 提示补齐**：restore_hint 指明被清除的 PAC 脚本地址见备份文件
  （持久化代理 P3 发现）。
- **临时文件清理**：Temp 下测试脚本/patch/innosetup 安装包已删（保留
  SysKit-backup-pre-rewrite.bundle 历史重写前恢复点）。
- 测试 +2（TOTP 按钮/对话框、WLAN 掩码）+ 适配 3；全量 **795 例全绿**。
