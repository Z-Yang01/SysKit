# SysKit · Rust + Tauri 重写（app-rs/）

Python+PySide6 版（仓库根 `syskit/`）的绞杀者模式重写。迁移决策与里程碑见
原分支提交（214816f 后的批准计划）；**Python 版迁移期继续服役**，两版共用
`%APPDATA%\SysKit` 数据目录、文件格式逐字节对齐、不同时运行。

## 里程碑

| M | 内容 | 状态 |
|---|------|------|
| M0 | 脚手架 + 主题 token 奇偶校验 + 门禁 | ✅ |
| M1 | 托盘 + 三热键 + 快捷粘贴闭环（GO/NO-GO 决策点） | 待做 |
| M2-M7 | 遥测/诊断修复/工具箱/系统助手/密码库/收尾 | 待做 |

## 门禁（每里程碑必过）

```powershell
powershell -ExecutionPolicy Bypass -File check.ps1
# = cargo fmt --check + clippy -D warnings + cargo test + tsc --noEmit + vitest run
```

## 主题单源纪律

32 个语义 token 的唯一事实源当前是 **Python 侧 `syskit/ui/theme.py`**；
`src/theme.ts` 逐值对齐，`tests/theme.parity.test.ts` 跨语言奇偶校验锁定。
改色必须两边同步改；Python 版退役后单源切到 TS。

## M1 起点必读：Python 版真机换来的教训（平移清单）

1. **x64 句柄**：HWND/HANDLE 一律按指针宽度处理（Python 侧 c_int 截断有过前科）；
   windows-rs 的类型体系天然安全，但 FFI 边界不得手写 int 转换。
2. **UIPI**：目标窗口提权时非管理员进程的 SendInput 被静默拦截——必须
   计数断言 + 降级文案（安装包带 requireAdministrator 清单可规避，开发态要测）。
3. **WM_ACTIVATE 时序**：还焦后**不能同步等待**——激活消息要经事件循环派发，
   注键前至少一拍（Tauri 侧用 tokio::time / tauri async runtime，勿 std::thread::sleep）。
4. **兜底分支是一等公民**：还焦失败/前台漂移/图片读取失败的重试与降级路径，
   每一条都要有测试与真机靶（离屏断言不算数）。
5. **NOREPEAT 语义**：只抑制按住自动重复，不抑制离散重按（三连按=三次触发）。
6. **SendInput 注入不带硬件自动重复**；dwExtraInfo 统一 magic `0x53594B54`。
7. **数据格式对齐是硬约束**：DPAPI 容器 `{"v":1,"enc":"dpapi","data":<b64>}`、
   clipboard 条目 schema、netstats 桶键格式——对齐测试先行（M1 起）。

## 目录

```
app-rs/
├─ src/            # React 前端（AntD + ECharts；主题取 --sk-* CSS 变量）
├─ tests/          # vitest（含跨语言奇偶校验）
├─ src-tauri/      # Rust 后端（windows-rs 原语 + Tauri 命令/事件）
└─ check.ps1       # 门禁一条命令
```
