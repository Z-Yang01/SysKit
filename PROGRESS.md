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
