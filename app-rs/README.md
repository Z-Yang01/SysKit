# SysKit · Rust + Tauri 重写（app-rs/）

Python+PySide6 版（仓库根 `syskit/`）的绞杀者模式重写。迁移决策与里程碑见
原分支提交（214816f 后的批准计划）；**Python 版迁移期继续服役**，两版共用
`%APPDATA%\SysKit` 数据目录、文件格式逐字节对齐、不同时运行。

## 里程碑

| M | 内容 | 状态 |
|---|------|------|
| M0 | 脚手架 + 主题 token 奇偶校验 + 门禁 | ✅ |
| M1 | 托盘 + 三热键 + 快捷粘贴闭环（GO/NO-GO 决策点） | ✅ 代码完成 / ⏳ **需真机复核** |
| M2 | 遥测采样 + 概览/网络监控 + 多粒度流量/日报 | ✅ 数据层+界面完成 / ⏳ 待真机看曲线 |
| M3-M7 | 诊断修复/工具箱/系统助手/密码库/收尾 | 待做 |

门禁全绿（**145 例 Rust + 57 例前端**）。M1/M2 的**真机清单尚未执行**——按
`UNATTENDED.md` §3，托盘里可能跑着用户实例、且测试红线禁止真实 Win32 注入，
因此 GO/NO-GO 判定必须由人工在真机完成。清单见下文「M1 真机复核清单」与
「M2 真机复核清单」。

### M2 相对 Python 版的两处有意简化（不是漏做）

- **WiFi 信号/SSID 暂用「未实现」占位**：Python 版每 5 秒起一次 `netsh wlan`
  子进程。Rust 侧不引子进程（进程创建开销大且难测），改用 `wlanapi.dll`
  （`WlanOpenHandle`/`WlanQueryInterface`）是正确做法——留待下一轮，届时
  `rate_stats` 的 SSID 归因才真正启用（当前所有流量归「以太网/未知」桶，
  **与 Python 未连接时的行为一致**，数据不丢）。
- **Top 进程（系统页）未做**：属 M5 范围，且 `procs` 采样与 WiFi 一样是
  「昂贵分组」，留到系统助手页一起接。

## 门禁（每里程碑必过）

```powershell
powershell -ExecutionPolicy Bypass -File check.ps1
# = cargo fmt --check + clippy -D warnings + cargo test + tsc --noEmit + vitest run
```

> 环境备注：`cargo`（构建锁）与 `vitest`（esbuild 子进程管道）需要**完全权限**
> 才能运行；受限沙箱下会分别报 `os error 5` 与 `spawn EPERM`。这不是代码问题。


## 目录

```
app-rs/
├─ src/            # React 前端（AntD + ECharts；主题取 --sk-* CSS 变量）
│  ├─ main.tsx             # 三窗口共用入口：按 Tauri 窗口 label 分派组件
│  ├─ App.tsx              # 主界面（M0 骨架，M2 起替换为真实页面）
│  ├─ ClipPanel.tsx        # 快捷粘贴面板（label `paste`）
│  ├─ CommandPalette.tsx   # 命令面板（label `palette`）
│  └─ pastePanel.ts        # 面板状态机（纯逻辑，无 DOM，vitest 全覆盖）
├─ tests/          # vitest（含两处跨语言奇偶校验）
├─ src-tauri/      # Rust 后端（windows-rs 原语 + Tauri 命令/事件）
│  ├─ src/w32.rs             # 唯一 FFI 出口：SendInput/剪贴板/DPAPI/热键/焦点
│  ├─ src/hotkey.rs          # 热键解析(纯) + RegisterHotKey 消息泵线程
│  ├─ src/input.rs           # 焦点守卫决策(纯) + 注键
│  ├─ src/clipboard_history.rs  # 条目 schema/置顶/淘汰/搜索/敏感防护
│  ├─ src/clipboard_write.rs    # text/html/image/file 写回（CF_HTML/CF_HDROP/CF_DIB）
│  ├─ src/secure_store.rs    # DPAPI 容器（与 Python 逐字节对齐）
│  ├─ src/settings.rs        # DEFAULTS + 类型收敛（与 config.py 奇偶校验锁定）
│  ├─ src/stores.rs          # 原子 JSON 读写
│  ├─ src/paths.rs           # 目录拓扑 + 便携模式 + 测试目录覆盖
│  ├─ src/tray.rs            # 托盘（代码绘制状态色图标 + 菜单）
│  ├─ src/input_panel.rs     # 两个无边框面板窗口 + 粘贴链序号守卫
│  ├─ src/timefmt.rs         # 本地时间格式化（格式串集中，锁 JSON 键形态）
│  ├─ src/rate_store.rs      # TrafficStore + EventStore（traffic/events.json 对齐）
│  ├─ src/rate_stats.rs      # 多粒度流量桶 netstats_buckets.json（含整点归并）
│  ├─ src/netstats.rs        # 日报聚合 + 多粒度序列（纯逻辑）
│  ├─ src/telemetry.rs       # 1 秒采样（GetIfTable2/GetSystemTimes/内存/磁盘）
│  ├─ src/telemetry_hub.rs   # 采样线程 + 共享状态 + sk://tick 事件
│  ├─ src/commands.rs        # invoke 接口
│  └─ capabilities/default.json  # 窗口能力（前端 show/hide/listen 必需）
└─ check.ps1       # 门禁一条命令
```

## M2 真机复核清单（数据正确性）

M2 的自动化测试已锁住**格式与聚合口径**（与 Python 逐值比对），但「真机上数字
对不对」只能人工看。启动后（同样先退出 Python 版）：

| # | 靶 | 通过标准 |
|---|-----|---------|
| 1 | 速率合理性 | 概览页下行/上行随实际流量变化；**空闲时接近 0**（若恒为 0 或恒为某固定值，说明基线算错） |
| 2 | 虚拟网卡不计入 | 用 Clash/VPN/Hyper-V 时，速率不应把隧道或虚拟交换机的转发量重复计入 |
| 3 | CPU/内存 | 与任务管理器数值同量级（允许 1-2 秒采样相位差） |
| 4 | 托盘速率 | tooltip 每秒刷新速率；**空闲时不闪**（去重生效） |
| 5 | 多粒度切换 | 近 60 分钟/48 小时/30 天/12 月四个 Tab 都有柱子；切粒度不报错 |
| 6 | 跨版本数据 | 先跑 Python 版产生流量 → 退出 → 跑 Rust 版：**柱状图能看到 Python 版写下的桶**（这是共用文件的硬证明） |
| 7 | 日报 | 事件时间线在无事件时显示空态文案；有事件时按「时间 · 状态 · 说明」列出 |
| 8 | 落盘 | 运行 ≥60 秒后退出，`%APPDATA%\SysKit\netstats_buckets.json` 的 mtime 已更新且 JSON 合法 |

> 靶 6 是 M2 最有价值的一条：它同时验证 `traffic.json` / `events.json` /
> `netstats_buckets.json` 三份文件的跨版本兼容。


## M1 真机复核清单（GO/NO-GO 判定，必须人工执行）

前置：**先退出托盘里正在运行的 Python 版 SysKit**（两版共用 `%APPDATA%\SysKit`
且都会注册同名全局热键，同时运行必然互相抢键、并可能并发写同一份
`clipboard.json`）。然后 `npm run build` + `cargo run --manifest-path src-tauri/Cargo.toml`。

| # | 靶 | 通过标准 |
|---|-----|---------|
| 1 | 三热键注册 | Ctrl+Alt+K / Ctrl+Alt+V / Ctrl+Win+V 各呼出对应面板；长按不重复触发（NOREPEAT）；日志无「注册失败」 |
| 2 | 被占回退 | 先让别的程序占用 Ctrl+Alt+K，启动后该键应静默降级（无崩溃、托盘菜单仍可呼出面板） |
| 3 | 快捷粘贴闭环 | 在记事本复制文本 → 别处复制新内容 → 呼出面板选旧条目 → 回车，**内容应直接粘进记事本** |
| 4 | 焦点漂移 | 呼出面板后先点别的窗口再按回车 → 提示「目标窗口已切换，已取消自动粘贴」，且内容仍在剪贴板 |
| 5 | Word/Excel 富文本 | 复制带格式网页内容 → 粘贴到 Word/Excel，格式与纯文本兜底都正常（CF_HTML 偏移正确性） |
| 6 | 文件条目 | 在资源管理器复制文件 → 面板选中 → 回车，**粘出的是文件本体**而不是路径文字（CF_HDROP） |
| 7 | 图片条目 | 粘贴一张截图 → 面板显示 `[图片 W×H]` → 选中回车，目标窗口收到图片（CF_DIB 方向/位序） |
| 8 | UIPI 降级 | 对**以管理员运行**的目标（如管理员记事本）粘贴：应提示「自动粘贴未送达，请按 Ctrl+V」而**不是静默无反应** |

另需目视：托盘图标为状态色圆点、tooltip 正确；两个面板无边框、置顶、圆角；
失焦即收起；Esc 收起；数字键 1-9 直选与双击都能粘贴。

## 主题单源纪律

32 个语义 token 的唯一事实源当前是 **Python 侧 `syskit/ui/theme.py`**；
`src/theme.ts` 逐值对齐，`tests/theme.parity.test.ts` 跨语言奇偶校验锁定。
改色必须两边同步改；Python 版退役后单源切到 TS。

同类纪律**同样适用于设置表**：默认值/键名的唯一事实源是 `syskit/config.py::DEFAULTS`，
`src-tauri/src/settings.rs::defaults()` 逐键对齐，`tests/settings.parity.test.ts`
解析 Python 源码锁定（键集、类型、字面量三者都不许分叉）。

## M1 起点必读：Python 版真机换来的教训（平移清单）

1. **x64 句柄**：HWND/HANDLE 一律按指针宽度处理（Python 侧 c_int 截断有过前科）；
   windows-rs 的类型体系天然安全，但 FFI 边界不得手写 int 转换。
2. **UIPI**：目标窗口提权时非管理员进程的 SendInput 被静默拦截——必须
   计数断言 + 降级文案（安装包带 requireAdministrator 清单可规避，开发态要测）。
3. **WM_ACTIVATE 时序**：还焦后**不能同步等待**——激活消息要经事件循环派发，
   注键前至少一拍。Rust 侧实现见 `input_panel.rs::tokio_sleep`
   （`spawn_blocking` + async await，绝不阻塞 UI/消息泵线程）。
4. **兜底分支是一等公民**：还焦失败/前台漂移/图片读取失败的重试与降级路径，
   每一条都要有测试与真机靶（离屏断言不算数）。焦点守卫被提成纯函数
   `input::decide`，五条分支全部有单测。
5. **NOREPEAT 语义**：只抑制按住自动重复，不抑制离散重按（三连按=三次触发）。
6. **SendInput 注入不带硬件自动重复**；dwExtraInfo 统一 magic `0x53594B54`。
7. **数据格式对齐是硬约束**：DPAPI 容器 `{"v":1,"enc":"dpapi","data":<b64>}`、
   clipboard 条目 schema、netstats 桶键格式——对齐测试先行（M1 起）。

### M1 与 Python 版的两处有意偏离（已核实，非疏漏）

- **热键接收不用 Qt 原生事件过滤器**：Tauri 无等价钩子，改为**专用线程**
  `RegisterHotKey(NULL,…)` + 同线程 `GetMessageW` 消息泵。WM_HOTKEY 只投递给
  注册线程的队列，同线程泵是唯一正确接法。
- **剪贴板写回不走插件**：`tauri-plugin-clipboard-manager` 对
  CF_HTML / CF_HDROP / CF_DIB **只实现 Getter，没有 Setter**（已核对 crate 源码），
  因此 text/html/image/file 四类写回全部用 `w32.rs` 直接构造载荷。

