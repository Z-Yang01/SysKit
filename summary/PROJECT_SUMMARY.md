# SysKit 系统工具箱 代码结构与实现总结

> 生成时间：2026-09-12（分析于 `test/pytest-setup` 分支，最新提交 afbee4b；
> 前一版 2026-09-10 快照（bfafc93，覆盖率 80% 时期）已整体重写为当前状态）

## 项目概览

**项目名称**：SysKit 系统工具箱（内部标识 `APP_NAME = "SysKit"`，前身 NetTool）。

**数据目录**：默认 `%APPDATA%\SysKit`（设置/密码库/剪贴板历史）与 `%LOCALAPPDATA%\SysKit`
（日志/备份）；**便携模式**下（exe 旁存在 `data\` 目录，安装向导勾选「数据保存在安装目录」
时创建）全部数据收敛到 `<安装目录>\data\config` 与 `<安装目录>\data\local`，随目录整体
搬迁（config.py 动态检测）。

**主要功能**（README.md 功能总览）：Windows 11「已连接但显示无网络」诊断修复工具 +
综合桌面工具箱——网络监控、WiFi 工具、15 项诊断（可中止）、修复（17 个单项动作 + 3 组合 +
组合中止）、工具箱（测速/局域网/端口/DNS 子网/ping 路由/**Hosts 编辑器**）、系统助手
（性能/启动项/电池/清理/磁盘分析/**剪贴板历史**（多格式+置顶+搜索+规则引擎+敏感防护）/
**场景模式**）、**密码库**（sites/accounts 两级 + AES-256-GCM/DPAPI + TOTP + 主密码门）、
**命令面板**（全局热键 + 统一搜索入口）。

**当前状态**（2026-09-12）：分支 `test/pytest-setup`，基线 **775 passed / 1 skipped**，
全量回归全绿；本日连续交付 10 个 commit（剪贴板升级/密码库/命令面板/Hosts 编辑器/
安装向导/便携模式/退出崩溃修复/中止按钮/热键即时生效/非法热键保护）。遗留见
PROGRESS.md 各会话「遗留与下一步」（浏览器扩展自动填充 API 边界已留、TOTP 二维码导入、
PySide6 升级验证退出崩溃）。

## 技术栈

| 类别 | 选型 | 说明 |
|------|------|------|
| 语言 | Python 3.10 | 项目根 launcher.py 入口 |
| GUI | PySide6 >=6.6（真机 6.11.2） | 纯 Qt Widgets + QPainter 自绘图表，QML/Quick 全排除 |
| 后台/系统调用 | 标准库 subprocess/winreg/socket/urllib/ctypes/threading | proc.py 双编码自适应；icmp.py ctypes IPHLPAPI |
| 加密 | cryptography >=42.0 | 密码库 AES-256-GCM + PBKDF2（600k 迭代）；DPAPI 走 stdlib ctypes |
| 其他运行时 | loguru、psutil、qrcode、pillow | 日志/进程/二维码（剪贴板与 WiFi 复用） |
| 测试 | pytest + pytest-qt（offscreen）+ pytest-cov + pytest-timeout | 开发依赖未固化（不在 requirements.txt），29 个测试文件 775 例 |
| 打包 | PyInstaller >=6.0 onefile + `uac_admin` 清单 | SysKit.spec 排除 Qt 重型模块 + numpy/OpenBLAS/opengl32sw 等（exe 67.6→37.7MB） |
| 安装器 | Inno Setup 6.3+/7（installer/SysKit.iss） | 下一步式向导：可选目录/桌面快捷方式/开机自启/卸载信息/便携数据标记；build.bat 第 5 步自动探测 ISCC |

## 代码结构分析

**syskit/ 逻辑层（27 个模块，除 ui 外零 Qt 依赖）**：
- *诊断域*：`snapshot.py`（PowerShell 采集网卡/路由/DNS → Snapshot dataclass）；`netcheck.py`
  （15 个 `chk_*` + `ALL_CHECKS` 注册表 + `run_all(progress, quick, should_stop)`（中止回调）
  + `verdict` 四态判定）；`proxy.py`；`winregread.py`；`dnsquery.py`（自拼 DNS 报文）。
- *修复域*：`repairs.py`（RepairAction 基类 + 17 个动作子类 + `COMBOS` 组合表 + 备份设施
  `_backup_dir/_reg_export/_save_note`）。
- *监控域*：`monitor.py`（Monitor 守护线程 + 梯度修复 `_run_ladder`，只含 SAFE 动作）；
  `telemetry.py`（每秒采样；`_top_procs` 走进程缓存、TrafficStore 免锁快照/零增量不落盘）。
- *工具箱域*：`icmp.py`；`dnsquery.py`（复用）；`diskusage.py`（扫描树支持 stop 中断 +
  回收站删除）。
- *剪贴板域*：`clipboard.py`（多格式条目 kind/image 落盘 md5 去重/置顶 `_resort+_evict`/
  search 带 idx 映射/mark_sensitive 敏感摘要/should_record_now 前台进程黑名单）；
  `cliprules.py`（规则引擎：JSON/URL/色块/路径/IP 域名 → 动作列表，纯逻辑无 Qt 无网络）。
- *密码库域*：`vault_crypto.py`（VaultCrypto：PBKDF2+AES-GCM 主密码模式 / DPAPI 免密模式；
  verify_password/remove_master_password；数据密钥仅解锁后驻留内存）；`vault.py`
  （SQLite sites/accounts 两级模型，secrets 密文 BLOB；search_sites/list_accounts/
  global_search 空格分词/find_by_domain（浏览器扩展预留）/totp_code RFC 6238）。
- *面板/热键域*：`hotkey.py`（RegisterHotKey ctypes 封装 + parse_hotkey/hotkey_display
  纯函数）；`palette_actions.py`（动作注册表 + fuzzy 子序列评分 + load_custom_commands）。
- *其他域*：`hostsed.py`（hosts 解析/序列化/增删改/写前备份，首 token 须为合法 IP）；`modes.py`
  （办公/游戏场景模式编排）；`vpn.py`（Wintun 等隧道识别 + 自动修复护栏）。
- *基础域*：`proc.py`（subprocess 双编码）；`config.py`（路径含便携检测 `_portable_data_dir`/
  探测端点/DNS 预设/DEFAULTS）；`settings.py`；`stores.py`（TrafficStore 免锁快照/
  EventStore）；`logger.py`（loguru 双文件 5MB 轮转 7 天清扫）；`admin.py`；`autostart.py`
  （schtasks SysKitAutostart）；`cli.py`/`main.py`（CLI diagnose/repair）。

**syskit/ui/ UI 层（15 个模块，PySide6）**：`app.py`（单实例互斥 + 源码运行 AUMID +
崩溃捕获 + `_prepare_data_dirs` 便携迁移 + QtFatalMsg 直写 crash.log）；`main_window.py`
（侧边栏 10 页导航；诊断/修复/设置/日志内建页；`_release_worker` 复位前 wait；quit_app
等待全部线程与 worker；`_save_settings` 热键 parse 验证 + 即时重注册；`_stop_diag`/
`_stop_combo` 中止接线）；`dashboard_page.py`；`wifi_page.py`；`toolbox_page.py`（六组
Tab：测速/局域网/端口/DNS 子网/ping 路由/HostsTab）；`system_page.py`（性能/启动项/电池/
清理/磁盘/ClipboardTab（搜索分类置顶暂停/规则动作/图片缩略图/隐私暂停）/ModesTab）；
`disk_page.py`；`vault_page.py`（左右分栏 + DictListModel/DictFilterProxy；`_require_master_verified`
主密码门；一键显示/隐藏 + 20s 自动隐藏；敏感剪贴板）；`palette.py`（CommandPalette +
HotkeyFilter + setup/teardown_global_hotkey（模块级持活））；`workers.py`（Diagnosis/
Repair/Combo/Snapshot 四 QThread，前两者带协作式 cancel）；`charts.py`；`tray.py`（状态
图标 + 最近剪贴板子菜单 + 命令面板入口）；`theme.py`（双主题 + app_icon 多尺寸代码绘制）；
`confirm_dialog.py`。

**入口**：`launcher.py` → `syskit/main.run()`（CLI/GUI 分流）→ `cli.py` 或
`ui/app.py:run_gui`；`__main__.py` 支持 `python -m syskit`。

**tests/ 组织（29 个文件，775 例）**：逻辑层一一对应（netcheck/repairs/monitor/snapshot/
stores/diskusage/icmp/proc/logger/cli/main_entry/telemetry/system_logic_units/vpn/
vault/portable/hostsed/clip_rules/clipboard_modes/toolbox_calc/palette/charts_modes_gaps）
+ UI（ui_smoke/ui_pages_units/ui_system_units/ui_vault_page_gaps/disk_system_page_gaps/
palette_tray_gaps）。**conftest.py 三层设施**不变（isolated_appdata 路径重定向 /
block_system_calls 哨兵 / fake_cmd_result 工厂 + 会话级 QThread 排空兜底）。

**tools/ 与 installer/**：`tools/make_icon.py`（构建期图标）；`tools/ui_preview.py`
（离屏 14 张截图验收，种子数据覆盖全部页面）；`tools/package_zip.py`（便携 zip + 版本号
打印，替代 cmd 下易碎的内联 PowerShell）；`tools/make_shortcuts.ps1`（install.bat 快捷
方式）；`tools/check_events.ps1`（WER 事件日志过滤，排查崩溃用）；`installer/SysKit.iss`
（Inno Setup 向导脚本）。

## 架构与实现方式

**架构类型**：逻辑层/UI 层二分的轻量分层架构 + 单窗口多页导航（10 页）+ QThread Worker
异步模式。核心特征仍是**逻辑层完全不 import Qt**。

**三种入口共用同一核心**：GUI / CLI / `python -m syskit` 收敛到 `netcheck.run_all` +
`repairs.get_action(...).execute`。

**核心工作流**（采集→检查→判定→修复→后台守护）与 9/10 版一致，新增两条：
- **密码库数据流**：UI（_require_master_verified 主密码门）→ `vault.get_secret(id, crypto, field)`
  按需解密 → `_copy_sensitive`（mark_sensitive 摘要标记 + TTL singleShot 自动清空 +
  清前覆盖校验）→ 剪贴板监听端 `ClipboardHistory.add` 遇敏感摘要/黑名单前台进程直接跳过。
- **命令面板数据流**：热键（HotkeyFilter 拦 WM_HOTKEY）/Ctrl+K/托盘 → popup() →
  静态动作（build_actions：页面/Tab/快捷/自定义）+ 动态条目（剪贴板最近/密码库
  global_search/ping·dns·URL 语法）→ fuzzy 排序 → Enter 执行并隐藏。

**设计模式**：注册表模式 ×3（ALL_CHECKS/ACTIONS+COMBOS/palette 动作表）、风险分级策略
（SAFE/CONFIG/AGGRESSIVE 被 Monitor/UI/CLI 三方消费）、模板方法（RepairAction.execute）、
观察者信号槽、防御性工程（stdio dup2/Qt 消息接管/双编码/哨兵 conftest）——均延续；
**新增**：协作式取消（worker.cancel 标志 + 项间检查，QThread 不可安全强杀下的正确模型）、
敏感数据三层防线（密文落盘 → 按需解密 → 剪贴板防记录）。

**已知技术债**（详见 PROGRESS.md 各会话遗留）：浏览器扩展自动填充（vault.find_by_domain
API 边界已留）；TOTP 二维码导入；退出阶段偶发原生崩溃已定位 QThread 析构竞态并修复
（quit_app 等待 + _release_worker），PySide6 升级验证待做；测试环境 PySide6 离屏偶发
fail-fast（约 1/3、重跑即绿、位置漂移）仍属环境性问题；开发依赖未固化（B10，沿用）。

## 目录树

```text
./.gitignore / .gitattributes（*.bat/*.cmd 钉 CRLF）
./assets/app.ico（构建期生成）
./build.bat（5 步：图标→依赖→pyinstaller→zip→Inno Setup）
./install.bat（用户目录绿色安装，setup.exe 为推荐安装方式）
./installer/SysKit.iss（Inno Setup 向导脚本）
./conftest.py（测试安全网三层设施）
./launcher.py（PyInstaller 入口）
./PROGRESS.md / README.md / WiFi无网络问题分析.md
./pytest.ini / requirements.txt / SysKit.spec
./syskit/（27 模块，见代码结构分析）
  ├─ 剪贴板域：clipboard.py / cliprules.py
  ├─ 密码库域：vault.py / vault_crypto.py
  ├─ 面板/热键域：hotkey.py / palette_actions.py
  ├─ 工具箱域：hostsed.py
  └─ 其余见上
./syskit/ui/（15 模块：app/charts/confirm_dialog/dashboard_page/disk_page/main_window/
             palette/system_page/theme/toolbox_page/tray/vault_page/wifi_page/workers）
./tests/（29 文件 775 例，与模块对应 + smoke/gaps 系列）
./tools/（make_icon.py / ui_preview.py / package_zip.py / make_shortcuts.ps1 / check_events.ps1）
./dist/（SysKit.exe / SysKit-<版本>-portable.zip / SysKit-<版本>-setup.exe，gitignore）
```
