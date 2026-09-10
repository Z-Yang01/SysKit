# SysKit 系统工具箱 代码结构与实现总结

> 生成时间：2026-09-10 13:00（分析于 `test/pytest-setup` 分支，最新提交 bfafc93）

## 项目概览

**项目名称**：SysKit 系统工具箱（内部标识 `APP_NAME = "SysKit"`，前身 NetTool，数据目录 `%APPDATA%\SysKit` / `%LOCALAPPDATA%\SysKit`，README.md:7-8）。

**主要功能**（README.md:10-19）：
- 针对 Windows 11「WiFi 已连接但显示无网络 / 网络时好时坏」问题的诊断修复工具（诊断页 15 项逐条检查、修复页 17 个单项动作 + 快速/深度/重置组合）；
- 已扩展为综合桌面工具箱：网络监控（延迟/速率/WiFi 信号/流量/事件时间线）、WiFi 工具（AP 扫描、信道推荐、二维码分享、密码查看）、工具箱（测速、局域网发现、端口占用查看器、DNS/子网计算、ping/路由追踪）、系统助手（性能监控、启动项管理、电池健康、垃圾清理、磁盘空间分析）。

**解决的问题域**（README.md:21-38 + WiFi无网络问题分析.md）：Windows 的联网判定依赖 NlaSvc 服务的 NCSI 探测，工具针对 NlaSvc 启动配置被改、幽灵持久路由、静态 DNS 残留、网卡关节电、MAC 随机化与 AP 授权冲突、NCSI 注册表被篡改等病灶提供逐项诊断与风险分级修复。`WiFi无网络问题分析.md` 是 2026-09-06 的实测排查报告，结论为「5 个问题叠加」（3 个系统配置 + 1 个省电策略 + 1 个公寓网络侧授权），**推断**该项目即由此报告孵化——诊断/修复项与报告中的病灶一一对应。

**目标用户**：Windows 11 个人用户，尤其是公寓/酒店等「按设备授权」网络（README.md:36-38 以自如为例）环境下的普通用户；提供 GUI（托盘常驻）与 CLI（`SysKit.exe diagnose`）两种形态。

**当前状态**（git log --oneline -15，共 6 个提交）：`03cf69b Initial commit` → `1b9c9a0 feat: 磁盘空间 Tab` → 测试分支 `test/pytest-setup` 上连续 4 个 pytest 建设提交（阶段一基础设施 → netcheck 130 例 99% → repairs 101 例 98% → toolbox 42 例 + UI 冒烟 35 例 + 逻辑补测 139 例）。据根目录 `.coverage` 历史数据（只读读取）：**总行覆盖率 80%（4376 语句 / 884 未覆盖）**，全部逻辑模块 ≥70%，工作树无未提交改动，项目处于「功能稳定、测试体系刚建成」阶段。遗留问题见 PROGRESS.md:41-58 记录的 10 条疑似 bug（只记录未修复）。

## 技术栈

| 类别 | 选型 | 证据 |
|------|------|------|
| 语言 | Python 3.10（真机 3.10.11） | README.md:93；PROGRESS.md:5 |
| GUI 框架 | PySide6 >=6.6（真机 6.11.2），纯 Qt Widgets + QPainter 自绘图表，非 QML/Quick | requirements.txt:1；PROGRESS.md:5；SysKit.spec:6-18 排除 QtQuick/QtQml；图表为 charts.py 手绘 paintEvent |
| 后台/系统调用 | 标准库 subprocess、winreg、socket、urllib、ctypes、threading；loguru >=0.7 日志；psutil >=5.9 进程/性能 | requirements.txt:2-3；syskit/icmp.py 用 ctypes 调 IPHLPAPI、syskit/diskusage.py:47 用 ctypes SHFileOperationW 回收站删除 |
| 其他运行时依赖 | qrcode >=7.4、pillow >=10.0（WiFi 二维码分享/保存） | requirements.txt:4-5 |
| 测试框架 | pytest 9.1.1 + pytest-qt 4.5.0（offscreen UI 冒烟）+ pytest-cov 7.1.0 + pytest-timeout（thread 模式） | PROGRESS.md:5、PROGRESS.md:73；pytest.ini 仅 `testpaths=tests` + `addopts=-q`，offscreen 平台在 conftest.py:18 设置 |
| 打包 | PyInstaller >=6.0 onefile 单文件，`uac_admin=True` 管理员清单，`console=False`，排除 40+ 个 Qt 重型模块，图标 assets/app.ico | SysKit.spec:6-19、36-49；build.bat 三步（生成图标 → 装依赖 → pyinstaller） |

注意两点：① pytest 系插件**不在 requirements.txt 中**（开发依赖未固化，版本只存在于 PROGRESS.md 会话记录）；② 无 lock 文件，依赖仅 `>=` 下限约束。

## 高价值场景三：测试覆盖盲区分析

### 3.1 覆盖率现状盘点（证据：根目录 .coverage，只读提取）

`.coverage` 为上一轮 pytest 会话的遗留产物，用 `python -m coverage report` 只读导出的完整数字：

| 模块 | 覆盖率 | 未覆盖行 |
|------|--------|----------|
| netcheck.py | 99% | 122, 460 |
| repairs.py | 98% | 235-236, 426-427, 497, 519-520 |
| icmp / logger / monitor / proc / proxy / snapshot / stores | 100% | — |
| config.py 93% / dnsquery.py 92% / winregread.py 92% | — | config 12-13；dnsquery 53-56, 92-93；winregread 36-37, 72-76 |
| settings.py | 89% | 19-21 |
| diskusage.py | 83% | 49-53, 62, 91, 94-95, 130, 174-180, 183-185, 192-196, 210-213 |
| **ui/main_window.py** | **79%** | 见 3.2 B2/B3 |
| **ui/toolbox_page.py** | **77%** | 36-77, 115-123, 159-177, 207-214, 324-348, 398, 430-452, 505, 543-569 |
| ui/charts.py 71% / ui/wifi_page.py **69%** / ui/system_page.py **65%** / ui/disk_page.py 81% / ui/workers.py 94% | — | 见 3.2 |
| **telemetry.py 26% / ui/app.py 35% / admin.py 38% / __init__.py 38% / autostart.py 43%** | — | 见 3.2 B6/B7 |
| **cli.py 0% / main.py 0% / \_\_main\_\_.py 0%** | — | 全部 112 行 |
| **TOTAL** | **80%** | 884 / 4376 语句 |

与 git log 对照确认的高覆盖模块：e6ab41a（test(A) netcheck 130 例→99%）、ec44e4c（test(B) repairs 101 例→98%）、bfafc93（C 组 dnsquery/计算 42 例、D 组 UI 冒烟 35 例使 UI 0%→76%、E 组 stores/logger/icmp/monitor/proc/snapshot 139 例→100%）。tests/ 共 12 个文件约 5800 行，每个逻辑模块都有专属测试文件（tests/test_netcheck.py 1078 行、test_repairs.py 886 行、test_monitor.py 455 行、test_snapshot.py 298 行、test_stores.py 203 行、test_diskusage.py 200 行、test_icmp.py 190 行、test_proc.py 165 行、test_logger.py 149 行、test_toolbox_calc.py 423 行、test_ui_smoke.py 945 行、test_smoke.py 63 行安全网自验证）。

**关键结论：剩余 884 条未覆盖语句中，约 700 条集中在 UI 层与入口/引导层，且相当部分是「触发修复/删除/杀进程的高风险编排逻辑」，而非纯视觉代码。**

### 3.2 剩余盲区清单（按风险从高到低，均有行号级证据）

**B1【高风险】CLI 入口 0% 覆盖**——`syskit/cli.py`（85 行全缺，覆盖率 0%）：`cmd_diagnose`（cli.py:9-32，诊断报告输出）、`cmd_repair`（cli.py:35-66，combos 映射 cli.py:40-44、`-y` 跳过确认 cli.py:53-60、逐项执行并回报结果）与 argparse 装配 `main`（cli.py:69-85）；连带 `syskit/main.py:5-19`（CLI/GUI 分流）与 `__main__.py`（4 行）均 0%。README.md:64-73 明确把 CLI 作为正式用法宣传，且 `repair` 子命令**不经 UI 确认弹窗直接执行修复动作**（仅靠 input 确认），是当前完全无测试的修复触发面。

**B2【高风险】UI 手动修复执行编排未测**——`syskit/ui/main_window.py` 79%，但修复执行主路径连续约 70 行 0 覆盖：`run_repair`（main_window.py:725-742，含非 SAFE 动作的管理员校验 :729-731、ConfirmDialog 拦截 :732-735、`_busy` 防重入 :727/736-738、RepairWorker 启动 :740-742）、`_repair_done`（:744-758，写修复记录 + 自动 quick 复诊 :758）、`run_combo`（:760-779，组合确认弹窗 + ComboWorker）、`_combo_item`/`_combo_done`（:781-794）。这是 README 宣传的「双击未通过项直接修复」「一键组合修复」的咽喉路径，任何编排错误都意味着真实系统动作失控。

**B3【高风险】监控自动修复回调链与已知 bug 的行为盲区**——`syskit/ui/main_window.py` 中监控回调基本未测：`_on_monitor_action`（:814-821，missing 815-819）、`_on_monitor_suggest`（:831-837，missing 832-835）、`_play_alert`（:822 起，missing 828-829）、`closeEvent`（:838，missing 844-845）、`quit_app`（:847，missing 848-854）。更关键的是 `syskit/monitor.py` 行覆盖率 100% 但 PROGRESS.md:42-43 已确认存在行为 bug：#1 `Monitor.__init__` 用 `threading.Event` 覆写 `Thread._stop`（monitor.py:34）导致对监控线程 `join()`/`is_alive()` 必抛 TypeError；#5 `probe_quick` 只探测首个 204 端点（monitor.py:15 `config.PROBE_204_URLS[:1]`，而 config.py:20-23 定义了两个端点），首端点故障会误报断网。**这证明剩余风险在「断言质量」而非行数——100% 行覆盖不等于行为正确。**

**B4【高风险】WiFi 核心解析器 0 覆盖**——`syskit/ui/wifi_page.py` 69%，但两个 netsh 解析函数体完全未覆盖：`scan_networks`（wifi_page.py:27-74，中英文双编码 netsh wlan show networks 逐行正则解析 + 2.4G/5G 频段分类 + 按信号排序，missing 32-74）、`profile_password`（:77-89，key=clear 密码提取，missing 79-89）、`ScanWorker.run`（:96-100，missing 97-100）；连带 `syskit/telemetry.py`（26%）的 `_read_wifi_signal`（telemetry.py:111-117，托盘/监控页 WiFi 信号数据源）。WiFi 是本项目立项目的（WiFi无网络问题分析.md 全篇），正则解析对系统语言/编码最敏感，却恰好零测试。

**B5【高风险】破坏性 UI 动作路径未测**——三处直接「改系统/删文件/杀进程」的 UI 逻辑：
- `syskit/ui/system_page.py` 65%：启动项启停 `_toggle_selected`（:165）/`_disable_one`（:183）/`_enable_one`（:199）missing 166-214 全链（涉及注册表 HKCU/HKLM Run 与服务写入）；垃圾清理 `_CleanWorker.run`（:331-354，`os.remove` 真实删文件 + thumbcache 前缀过滤 :346-347，missing 332-354）、`_ScanWorker.run`（:293，missing 294-297）、`clean`（:409）/`_clean_done`（:423，missing 411-432）；
- `syskit/ui/toolbox_page.py` 77%：端口页 `_kill_selected`（:542-569，`psutil.Process(pid).kill()` 结束进程，missing 543-569）；测速 worker `SpeedTestWorker.run`（:39-77，urllib 下载测速，missing 40-77）；
- `syskit/ui/disk_page.py` 81%：删除编排 `delete_checked`（:320，missing 322/328-339）与 `_delete_done`（:341，missing 342-355，删除后重扫刷新）——底层 `DeleteWorker.run` 已测，但 UI 侧勾选收集→确认→完成后刷新的编排链没测。

**B6【中风险】入口/引导层低覆盖**——`syskit/ui/app.py` 35%：`run_gui`（:31-71）主体 missing 32-71，含单实例互斥（CreateMutexW + ERROR_ALREADY_EXISTS :32-37）、Qt 消息处理器接管（:48-56）、`--minimized` 启动分支（:67-70）；`syskit/admin.py` 38%：`elevate_and_restart`（:21-35，UAC 自提权重启）全缺；`syskit/autostart.py` 43%：`enable/disable/enabled`（:17-44，schtasks 计划任务创建/删除/查询、旧任务名 NetToolAutostart 清理）全缺；`syskit/__init__.py` 38%：`_fix_windows_stdio` 的 dup2 重定向分支（:17-31，windowed 打包防 0xC0000409 崩溃的关键防线）未测。

**B7【中风险】遥测与图表**——`syskit/telemetry.py` 26%：`TelemetryWorker.run`（:54-70，每秒采样循环 + 每 60 tick 落盘 TrafficStore）、`_sample`（:72-109，速率/CPU/内存/磁盘/延迟/今日流量聚合）、`_top_procs`（:119-131）全部 missing 55-131。遥测是监控页、托盘悬浮窗、流量统计的唯一数据源；charts.py 71% 缺 `BarChart.paintEvent`（:137-180，纯绘制，风险较低）。

**B8【低风险·易补】微盲区**——`syskit/settings.py:19-21`：`load()` 成功读盘并合并 stored 配置的分支从未命中（现有测试只走了「文件不存在→默认值回退」路径，没有任何测试做「写文件→复位 `_cache`→重读合并」闭环）；`syskit/diskusage.py:174-196`：`scan_tree` 的 stop 中断与 progress 回调路径（README.md:62 宣传的「扫描可中途停止」）；`syskit/dnsquery.py:53-56, 92-93`（响应解析异常分支）；`syskit/config.py:12-13`（非 win32 路径回退）；`syskit/winregread.py:36-37, 72-76`。

**B9【体系外代码，无任何测试设施覆盖】**——① 根目录 `check_events.ps1`（7 行，PowerShell 事件日志排查脚本）：**第 2 行仍匹配旧标识 `'NetTool'`**，而项目已改名 SysKit（README.md:7-8、config.py:5），疑似已失效且无测试可能；② `build.bat:10` 构建成功提示仍写 `dist\NetTool.exe`，与 SysKit.spec:42 `name="SysKit"` 不一致；③ 根目录 `test_ui_shots.py`（44 行，UI 截图验收脚本）在 pytest.ini `testpaths=tests` 之外，**不会被 pytest 收集**，处于测试体系盲区；④ `tools/make_icon.py`（32 行构建期图标生成）无测试，风险低。

**B10【流程盲区】开发依赖未固化**——requirements.txt 不含 pytest/pytest-qt/pytest-cov/pytest-timeout，无法从零复现测试环境；且无单独的 dev-requirements 或 lock 文件。

### 3.3 建议优先补充的 5 个测试用例

1. **（P0）CLI 修复编排测试**（目标 `syskit/cli.py` 0%→约 90%）：mock `repairs.get_action`/`act.execute` 与 `admin.is_admin`，验证——`main(["repair","fast"])` 按 cli.py:41 combos 顺序执行 4 个安全动作；未知修复项返回 2 并列出可用 id（cli.py:46-48）；不带 `-y` 时对非 y 输入逐项跳过（cli.py:53-60）；`diagnose` 调 `netcheck.run_all` 并按 `verdict` 输出报告与建议修复项（cli.py:24-31）。**理由**：这是唯一 0% 的用户可见入口模块，全部是纯编排逻辑，conftest 哨兵已确保不会触真实动作，是 884 条缺失语句里性价比最高的一块。
2. **（P0）UI 手动修复路径测试**（目标 main_window.py 79%→88%）：对 `run_repair` 验证——非管理员执行 RISK_CONFIG 动作弹权限警告且不启动 worker（main_window.py:729-731）；ConfirmDialog 拒绝后不执行（:732-735）；`_busy` 时拒绝重入（:727/736-738）；monkeypatch RepairWorker 后断言 `_repair_done` 写入修复记录并自动触发 quick 复诊（:749-758）。**理由**：是剩余未覆盖中最大连续块，直接守护「改配置类动作必须确认」这一产品红线。
3. **（P1）WiFi 解析器测试**（目标 wifi_page.py 69%→90%、telemetry.py 26%→40%）：用固定的中文/英文 netsh 输出样例喂 `scan_networks`，断言 SSID/BSSID/信号/信道/身份验证字段的解析、2.4G/5G 频段分类（wifi_page.py:72）与按信号降序排序（:73）；`profile_password` 断言认证类型与「关键内容」提取、缺失时返回空串（:82-89）；位置服务报错时 `ScanWorker` 发 failed 信号（:33-34, 96-100）。**理由**：项目起源场景的核心逻辑，纯解析零系统调用，且是对系统语言最敏感、回归概率最高的代码。
4. **（P1）监控行为回归测试（针对已知 bug，锁住修复后行为）**：断言 `Monitor` 实例 `join()`/`is_alive()` 不抛 TypeError（PROGRESS bug#1，monitor.py:34 覆写 `_stop`）；`probe_quick` 在首个 204 端点 urlopen 抛异常、第二个端点返回 204 时应返回 True（bug#5，monitor.py:15 vs config.py:20-23）。**理由**：monitor.py 行覆盖 100% 但藏着 2 个已确认行为缺陷，是「覆盖率盲区≠测试有效」的实证；修复前后都需要测试钉住行为。
5. **（P2）破坏性 UI 动作编排测试**（目标 system_page.py 65%→85%）：`_CleanWorker.run` 用 tmp_path 构造 thumbcache/非 thumbcache 文件，断言只删除前缀匹配文件且 OSError 记入 errors（system_page.py:344-353）；`StartupTab._disable_one/_enable_one` 用假 `proc.run` + winreg 读哨兵验证调用序列与禁用/启用位置选择（:183-214）。**理由**：这是测试体系内唯一会真实删文件/写注册表的未测路径，conftest 红线设施现成可用，风险最高；顺带可补 `PortTab._kill_selected` 的确认/权限分支（toolbox_page.py:542-569）。

## 代码结构分析

**syskit/ 逻辑层（17 个模块，零 Qt 依赖）**：
- *诊断域*：`snapshot.py`（174 行——`take()` 经 proc.run_ps 调 PowerShell 采集网卡/路由/DNS → `Snapshot`/`Adapter` dataclass；`current_ssid`）；`netcheck.py`（464 行——15 个 `chk_*` 检查函数 + `ALL_CHECKS` 注册表 :416 + `run_all` :423 + `verdict` 综合判定 :442）；`proxy.py`（系统代理状态/关闭）；`winregread.py`（只读注册表封装）；`dnsquery.py`（自拼 DNS 报文的原生查询 + `query_safe`/`resolve_system`）。
- *修复域*：`repairs.py`（585 行——`RepairAction` 基类 :44 + 17 个动作子类（FlushDns/StartNlaSvc/FixNlaSvc/DnsAuto/NcsiRestore/PowerOff/DelPersistentRoute/ProxyOff/MacRandomOff/WinsockReset/IpStackReset 等）+ `COMBOS` 组合表 :566 + `get_action`/`ACTION_IDS` :581-584 + 修复前备份 `_backup_dir`/`_reg_export`/`_save_note` :23-43）。
- *监控域*：`monitor.py`（136 行——`probe_quick` 快探 + `Monitor` 守护线程，含设置钳位 `settings()` :65-74 与梯度修复 `_run_ladder` :112-133）；`telemetry.py`（131 行——`TelemetryWorker(QThread)` 每秒采样速率/CPU/内存/磁盘/延迟/WiFi 信号/Top 进程，每 60 tick 落盘）。
- *工具箱域*：`icmp.py`（ctypes 调 IPHLPAPI 的 ping + `default_gateway`）；`dnsquery.py`（复用）；`diskusage.py`（223 行——驱动器列举、`scan_tree` 目录占用树（支持 stop 中断/progress 回调）、`human_bytes`、ctypes SHFileOperationW 的 `delete_to_recycle_bin` 回收站删除）。
- *基础域*：`proc.py`（subprocess 封装 + UTF-8/GBK 双编码自适应 `_decode` :42）；`config.py`（路径/204 探测端点/NCSI 注册表/DNS 预设/`DEFAULTS` 设置默认值）；`settings.py`（JSON + 线程锁 + `_cache`）；`stores.py`（`TrafficStore`/`EventStore` JSON 持久化）；`logger.py`（loguru 双文件 nettool.log/error.log，5MB 轮转 + 7 天清扫 :40）；`admin.py`（UAC 检测/自提权）；`autostart.py`（schtasks 计划任务开机自启，含旧任务名清理）；`__init__.py`（windowed 模式 stdio 句柄 dup2 修复）。

**syskit/ui/ UI 层（12 个模块，PySide6）**：`app.py`（单实例互斥 + Fusion 风格 + 崩溃日志 crash.log + Qt 消息接管）；`main_window.py`（858 行——侧边栏 9 页导航、诊断/修复/设置/日志四合一主窗体，修复编排与监控回调）；`dashboard_page.py`（概览页）、`wifi_page.py`（AP 扫描/信道分析/二维码/密码）、`toolbox_page.py`（583 行——测速/局域网/计算/Ping 路由/端口五组 Tab）、`system_page.py`（性能/启动项/电池/清理）、`disk_page.py`（磁盘空间分析/回收站删除）；`workers.py`（Diagnosis/Repair/Combo/Snapshot 四个 QThread worker + `action_summary`）；`charts.py`（QPainter 手绘 LineChart/BarChart，不依赖 QtCharts）；`tray.py`（托盘图标随网络状态变色）；`theme.py`（浅/深色）；`confirm_dialog.py`（修复前确认弹窗，展示风险/说明/还原方法）。

**入口**：`launcher.py`（PyInstaller 入口）→ `syskit.main.run()`（main.py:5-19，按首个 argv 分流 CLI/GUI）→ `cli.py` 或 `ui/app.py:run_gui`；`__main__.py` 支持 `python -m syskit`。

**tests/ 组织**：与被测模块一一对应（netcheck/repairs/monitor/snapshot/stores/diskusage/icmp/proc/logger/toolbox_calc）+ 两类横切文件：`test_smoke.py`（8 例，验证 conftest 安全网自身：路径隔离、哨兵拦截 proc/subprocess/winreg/socket、offscreen qtbot）与 `test_ui_smoke.py`（35 例，覆盖 11 个 UI 模块的构造+关键控件+核心槽函数）。**conftest.py 三层设施**（conftest.py:29-108）：① autouse `isolated_appdata` 把 5 个持久化路径重定向到 tmp_path 并复位 settings._cache；② autouse `block_system_calls` 哨兵——未显式 mock 的 proc.run/run_ps、subprocess 全家、os.system、winreg 写操作、socket.connect、urlopen 一律抛 AssertionError（「漏 mock 即失败」而非静默执行）；③ `fake_cmd_result` 工厂构造 `proc.CmdResult`。

**tools/ 与 assets/**：`tools/make_icon.py`（32 行，Pillow 绘制并生成 assets/app.ico，构建期使用）；`assets/app.ico` 供 SysKit.spec 打包引用。`check_events.ps1` 为手工排查用的崩溃事件过滤脚本（体系外，见 B9）。

## 架构与实现方式

**架构类型**：逻辑层/UI 层二分的轻量分层架构 + 单窗口多 Tab 导航 + QThread Worker 异步模式。核心特征是**逻辑层完全不 import Qt**（netcheck/repairs/monitor/telemetry 等仅用标准库/loguru/psutil），UI 层只做编排与展示——这正是本轮测试建设能对逻辑模块做到 92-100% 覆盖的结构基础（推断：分层本身也是为可测试性服务的，PROGRESS.md 决策 #2 的覆盖率口径按逻辑模块划定）。

**三种入口共用同一核心**：GUI（run_gui）、CLI（cli.main）、`python -m syskit`（__main__）都收敛到 `netcheck.run_all` + `repairs.get_action(...).execute`，无重复实现。

**核心工作流（诊断→修复→报告数据流）**：
1. **采集**：`snapshot.take()`（snapshot.py:103）经 proc.run_ps 拉取网卡/路由/DNS/服务状态，容错解析为 `Snapshot` dataclass（`_parse_json_loose` 处理 PowerShell 的宽松 JSON）；
2. **检查**：`netcheck.run_all` 遍历 `ALL_CHECKS` 注册表，每个 `chk_*(snap)` 返回 `CheckResult`（状态 PASS/WARN/FAIL + `fix_ids` 建议修复项），单项异常被兜成 WARN 不中断（netcheck.py:431-436）；`quick=True` 跳过 5 个慢速项（:426-427）；
3. **判定**：`verdict`（:442-464）归并为 normal / ncsi_false_alarm（NCSI 误报）/ dns_broken / no_internet 四态——这是本工具与普通 ping 工具的本质差异（区分「误报」与「真断网」）；
4. **修复**：`CheckResult.fix_ids` → `repairs.get_action(aid)` → UI 侧 `run_repair` 闸门（管理员校验 + ConfirmDialog + `_busy` 防重入）→ `RepairWorker` 线程执行 → `RepairResult(success, message, need_reboot)` → `_repair_done` 写修复记录 + 自动 quick 复诊形成闭环（main_window.py:744-758）；
5. **后台守护**：`Monitor.run` 按 poll_interval 轮询——`probe_quick` 失败累计 `fail_threshold` 次后按冷却时间触发 `_run_ladder` 梯度修复（L1 刷 DNS → L2 拉服务/续租 → L3 重启网卡/WLAN，monitor.py:112-117，只含 RISK_SAFE 动作），改配置类只经 `on_suggest` 回调上报告警/建议；`TelemetryWorker` 每秒采样经 `tick` 信号驱动仪表/托盘/流量落盘。

**设计模式（代码证据）**：
- **注册表模式 ×3**：检查项注册表 `ALL_CHECKS`（netcheck.py:416）、修复动作注册表 `ACTIONS`/`ACTION_IDS`/`get_action`（repairs.py:581-584）、组合注册表 `COMBOS`（repairs.py:566，CLI 与 UI 共用）——新增检查/修复只需追加条目，UI 的 `_fill_actions` 自动渲染；
- **风险分级策略**：`RISK_SAFE / RISK_CONFIG / RISK_AGGRESSIVE` 三级（repairs.py:11-12），自动修复梯度只允许 SAFE（monitor.py:113-117）、CONFIG 需确认 + 先备份（`_reg_export`/`_save_note` 存还原信息，repairs.py:23-43，README.md:83 备份目录）、AGGRESSIVE 仅手动——该分级同时被 Monitor（自动）、UI（弹窗）、CLI（确认提示）三方消费；
- **模板方法**：`RepairAction.execute` 统一捕获异常返回 `RepairResult`（repairs.py:56-61），17 个子类只实现 `_run`；
- **观察者/信号槽**：Monitor 三回调（on_status/on_action/on_suggest，monitor.py:31-33）与 TelemetryWorker.tick → MainWindow._on_* 系列；UI 阻塞操作一律下沉到 QThread worker（workers.py 四件套 + 各页面自有 worker）避免冻结；
- **防御性工程**（README.md:97-101「已知坑」）：`__init__` stdio dup2 修复、app.py Qt 消息接管与 crash.log、proc 双编码解码、netcheck 单项异常兜底。

**已知技术债**：PROGRESS.md:41-58 记录 10 条疑似 bug（#1 Monitor._stop 覆写与 #3 toolbox 大网段冻结建议优先修）；`check_events.ps1:2` 与 `build.bat:10` 残留 NetTool 旧名；conftest 哨兵/隔离设施由主线程独占维护（PROGRESS.md 决策 #6）——补充测试时须遵守「局部夹具写在自己测试文件」的约定。

## 目录树

```text
./.coverage
./.gitignore
./.pytest_cache/.gitignore
./.pytest_cache/CACHEDIR.TAG
./.pytest_cache/README.md
./.zcode/plans/plan-sess_5aaf62a2-1325-4efa-88e0-c25f7e8441e2.md
./.zcode/plans/plan-sess_7282d368-0224-4f99-af23-3be22a3343d9.md
./assets/app.ico
./build.bat
./check_events.ps1
./conftest.py
./launcher.py
./PROGRESS.md
./pytest.ini
./README.md
./requirements.txt
./syskit/admin.py
./syskit/autostart.py
./syskit/cli.py
./syskit/config.py
./syskit/diskusage.py
./syskit/dnsquery.py
./syskit/icmp.py
./syskit/logger.py
./syskit/main.py
./syskit/monitor.py
./syskit/netcheck.py
./syskit/proc.py
./syskit/proxy.py
./syskit/repairs.py
./syskit/settings.py
./syskit/snapshot.py
./syskit/stores.py
./syskit/telemetry.py
./syskit/ui/app.py
./syskit/ui/charts.py
./syskit/ui/confirm_dialog.py
./syskit/ui/dashboard_page.py
./syskit/ui/disk_page.py
./syskit/ui/main_window.py
./syskit/ui/system_page.py
./syskit/ui/theme.py
./syskit/ui/toolbox_page.py
./syskit/ui/tray.py
./syskit/ui/wifi_page.py
./syskit/ui/workers.py
./syskit/ui/__init__.py
./syskit/winregread.py
./syskit/__init__.py
./syskit/__main__.py
./SysKit.spec
./tests/test_diskusage.py
./tests/test_icmp.py
./tests/test_logger.py
./tests/test_monitor.py
./tests/test_netcheck.py
./tests/test_proc.py
./tests/test_repairs.py
./tests/test_smoke.py
./tests/test_snapshot.py
./tests/test_stores.py
./tests/test_toolbox_calc.py
./tests/test_ui_smoke.py
./test_ui_shots.py
./tools/make_icon.py
./WiFi无网络问题分析.md
```
