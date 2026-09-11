# SysKit 系统工具箱

一个针对 Windows 11「已连接但显示无网络 / 网络时好时坏」问题的诊断修复工具，
并扩展为覆盖网络监控、网络工具与系统助手的桌面工具箱。
Python 3.10 + PySide6 + loguru + psutil + cryptography 实现，托盘常驻、自动监控、逐级修复。

> 前身为 NetTool。显示名与内部标识均已改为 SysKit（数据目录
> `%APPDATA%\SysKit` / `%LOCALAPPDATA%\SysKit`），从旧版本升级时历史数据会自动迁移。

## 功能总览（侧边栏导航）

- **🏠 概览**：网卡一览 + 大字状态徽章 + 快捷操作（刷 DNS / 重连 WiFi / 系统代理检测与一键关闭）+ 一键体检
- **📈 网络监控**：网关/公网延迟曲线、实时上下行速率、WiFi 信号历史、每日流量累计、断网/恢复事件时间线；托盘悬浮实时速率；断网/恢复声音提醒
- **📶 WiFi 工具**：周边 AP 扫描、信道占用分析与推荐信道、WiFi 二维码分享、已存密码查看
- **🔍 诊断**：15 项逐条检查（NlaSvc / 幽灵路由 / 静态 DNS / **系统代理残留** / NCSI / 授权拦截等），双击未通过项直接修复
- **🔧 修复**：一键快速/深度/重置组合 + 17 个单项动作（按风险分级）+ WiFi 专区（MAC 随机化开关/重连）+ 修复记录
- **⚡ 工具箱**：速度测试、局域网设备发现、**端口占用查看器（找进程/结束进程）**、DNS/子网计算、持续 ping 与路由追踪、**Hosts 编辑器**（增删改/一键启用禁用，保存前自动备份）
- **🧰 系统助手**：性能监控（CPU/内存/磁盘曲线 + Top 进程）、启动项管理、电池健康、白名单垃圾清理、**磁盘空间分析**（目录占用树/文件类型分布/大文件排行，删除仅移入回收站）、**剪贴板历史**（文本/图片/文件/HTML，搜索+分类+置顶，敏感内容与密码管理器前台进程防记录，规则引擎：JSON 格式化 / URL 二维码 / 色块 / 打开目录 / DNS+ping+归属地）、**场景模式**（办公：勿扰+静音+常用软件+备份配置；游戏：关通知+高性能+暂停更新+白名单清后台）
- **🔐 密码库**：网站/账号两级密码管理（SQLite + AES-256-GCM 密文落盘），主密码可选（未设时 Windows DPAPI 免密保护）；搜索先定位网站再过滤账号，全局搜索 `github alice` 直达账号；TOTP 动态码；复制的密码不进剪贴板历史且 25 秒自动清空；自动重锁
- **⌘ 命令面板**（全局热键 `Ctrl+Alt+K` / 应用内 `Ctrl+K`）：输入即搜统一入口——页面与工具跳转、快捷诊断修复、剪贴板最近条目回填、密码库全局直达、`ping 主机` / `dns 域名` / 粘贴网址语法、自定义命令（URL / 本地命令）
- **⚙ 设置 / 📋 日志**：自动修复策略、声音提醒、深色模式、开机自启、剪贴板/密码库/命令面板选项；loguru 双日志（syskit.log / error.log）7 天自动清理

## 这个工具解决什么问题

Windows 判断「有没有网」依赖 **NCSI 探测**（访问 `www.msftconnecttest.com/connecttest.txt`），
而这个探测由 **NlaSvc（网络位置感知）服务**执行。以下任一环节出问题，都会出现
「WiFi/热点明明连上，系统却显示无网络」：

| 病灶 | 典型来源 | SysKit 的对策 |
|------|---------|---------------|
| NlaSvc 被改为「手动启动+无触发器」，从未运行 | 系统优化软件/管家类工具乱改服务 | 诊断项 #1；`fix_nlasvc` 一键改回自动并启动 |
| 手工添加的**持久默认路由**指向失效网关 | 酒店/公寓/公司网络遗留 | 诊断项 #2/#13（幽灵路由）；`del_persistent_route` 删除并存档还原命令 |
| 网卡残留**静态 DNS**（含国内不可达的 8.8.8.8） | DNS 优选工具/加速器 | 诊断项 #3；`dns_auto` / `dns_preset` |
| 网卡被允许「关机省电」导致掉线 | Windows 默认 + 电源管理 WMI 损坏的机器 | 诊断项 #10；`power_off`（注册表 PnPCapabilities 方案） |
| MAC 随机化与公寓/企业 AP 的授权机制冲突 | Win11 默认行为 | 修复页「WiFi 专区」提供开关按钮 |
| NCSI 探测配置被改 | 手工调整/软件篡改 | 诊断项 #9；`ncsi_restore` 恢复微软默认（改前 reg 备份） |

> **注意**：公寓/酒店等「按设备授权」的网络（如自如），若 AP 侧从未授权本机
> （连上但 DHCP 拿不到地址、网关 ping 全丢），需要在 **自如 App / 管家 / 网络认证页**
> 重新授权设备。SysKit 的诊断会明确告诉你这种情况（网关 100% 丢包 = AP 二层拦截）。

## 使用

### 安装（推荐：安装向导）

```bat
build.bat        :: 产出 dist\SysKit.exe + 便携 zip + SysKit-<版本>-setup.exe
```

双击 `dist\SysKit-<版本>-setup.exe`，按向导「下一步」完成安装：可选桌面快捷方式、
开机自启，自动写入卸载信息（控制面板"卸载程序"可见，卸载不删除 %APPDATA% 下的用户数据）。
需要 Inno Setup 6.3+/7（一次性安装：https://jrsoftware.org/isdl.php ）；未安装时
build.bat 自动跳过第 5 步，只产出绿色版 exe/zip，此时可用 install.bat 装到用户目录。

### 图形界面（推荐）

双击 `dist\SysKit.exe`（UAC 点「是」），最小化到托盘后台守护：

- **状态页**：网卡一览 + 大字判定（正常 / NCSI 误报 / DNS 异常 / 断网）
- **诊断页**：13 项逐条检查，双击未通过项可直接执行其建议修复
- **修复页**：一键「快速修复 / 深度修复 / 重置网络栈」+ 16 个单项动作 +
  **WiFi 专区（MAC 随机化开关 / 重连）**
- **设置页**：自动修复开关、探测间隔、修复上限、开机自启（计划任务免 UAC）
- **日志页**：全部动作留痕

托盘图标颜色 = 网络状态（绿正常 / 黄连上无网 / 灰离线）。后台每 30 秒探测一次，
连续 2 次失败自动按梯度修复（刷缓存 → 拉起服务/续租 → 重启网卡），改配置类动作
**永不自动执行**、只弹建议。

### 磁盘空间分析

「系统助手 → 磁盘空间」：选择磁盘后点「扫描」，得到目录占用树（按大小排序、懒加载展开）、
文件类型占用 Top 8、大文件 Top 200（双击定位所在文件夹）。勾选目录或在表格选中行后
「删除选中项」——**仅移入回收站，可在系统回收站还原**，删除完成后自动重新扫描刷新视图。
扫描可中途「停止」，已扫描部分照常展示。

### 命令行

```bat
SysKit.exe diagnose            :: 全量诊断（配合重定向可存文件）
SysKit.exe diagnose --quick    :: 快速诊断
SysKit.exe repair fast         :: 快速修复（逐项确认）
SysKit.exe repair fast -y      :: 快速修复（跳过确认）
SysKit.exe repair ncsi_restore :: 单项修复
python -m syskit diagnose        :: 源码运行（免打包调试）
```

## 修复动作与风险级别

| 级别 | 动作 | 说明 |
|------|------|------|
| 安全 | flush_dns / renew_dhcp / start_nlasvc / restart_wlan / toggle_adapter / restart_services | 无配置变更，自动修复梯度只使用这些 |
| 改配置 | fix_nlasvc / dns_auto / dns_preset / ncsi_restore / ncsi_disable / power_off / del_persistent_route / mac_random_off / mac_random_on | 执行前弹窗确认 + 自动备份原值，界面/日志均有还原方法 |
| 强力 | winsock_reset / ipstack_reset | 需重启电脑，仅手动触发 |

所有备份写入 `%LOCALAPPDATA%\SysKit\backup\<时间戳>\`（reg 导出、路由表存档、
DNS 记录、WiFi 配置文件 XML），日志在 `%LOCALAPPDATA%\SysKit\logs\`。

## 构建与安装升级

```bat
build.bat      :: 生成图标 + 安装依赖 + PyInstaller 打包 + 便携 zip
install.bat    :: 安装/升级到 %LOCALAPPDATA%\Programs\SysKit，创建桌面与开始菜单快捷方式
```

- **首次安装**：`build.bat` → `install.bat`（UAC 由 exe 清单触发，目录在用户区，无需拷贝 Program Files）
- **升级**：退出正在运行的 SysKit → 重新 `build.bat` → 再跑一次 `install.bat`（覆盖同路径 exe 即完成）；设置与日志保留在 `%APPDATA%\SysKit` / `%LOCALAPPDATA%\SysKit`
- **便携使用**：直接运行 `dist\SysKit.exe`，或解压 `dist\SysKit-<版本>-portable.zip`
- **卸载**：删除安装目录与快捷方式即可（可选清空上述数据目录）
- 版本号见 `syskit/config.py` 的 `VERSION`（窗口标题与设置页会显示）
- 技术栈：Python 3.10、PySide6、loguru、标准库（subprocess/winreg/socket/urllib）
- 打包：PyInstaller onefile + `uac_admin` 清单 + 裁剪 Qt 重型模块
- 日志：loguru 双文件分离——`syskit.log`（正常）与 `error.log`（错误+堆栈），
  5MB 轮转，**超过 7 天自动删除**（启动清扫 + 常驻运行每日清扫双保险）
- 已知坑（源码注释里有详细说明）：
  - windowed 打包后无 stderr，Qt 默认消息处理器会 fail-fast → 已接管 Qt 消息到日志
  - 电源管理 WMI 在部分机器损坏（Error 31）→ 走注册表 PnPCapabilities
  - `sc config` 的 `start= auto` 参数不能带引号 → subprocess 参数拆分
  - netsh 输出编码随系统设置在 UTF-8/GBK 间漂移 → 双编码自适应解码

## 常见问题

**Q：为什么显示「无Internet、安全」但网页能打开？**
NCSI 误报：探测源访问失败但真实网络正常。工具会明确判定这种情况，`ncsi_restore`/DNS 修复可解决。

**Q：连公寓 WiFi 拿到 169.254.x.x 是什么意思？**
DHCP 被拒绝（AP 没给地址）。先试「WiFi 专区」的 MAC 随机化开关换身份，再不行就是网络侧没授权本机，需要找网络管理方（自如管家等）。

**Q：AlibabaProtect 是什么？**
阿里系捆绑的「PC 保护服务」，会静默改 DNS（如写入 2400:3200::1）、动服务配置。本机诊断中 NlaSvc 触发器丢失、静态 DNS 残留都与其行为模式吻合。可在「设置-应用」中卸载（名字 AlibabaProtect），卸载后建议再跑一次全量诊断。
