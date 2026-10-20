# SysKit · Rust + Tauri（app-rs/，唯一在维护的版本）

原 Python+PySide6 版（仓库根 `syskit/`）的绞杀者模式重写，迁移已完成：
Python 版已于 2026-10 退役删除，完整快照在 git tag `python-final`（规格
对照走 `git show python-final`）。两版共用 `%APPDATA%\SysKit` 数据目录、
文件格式逐字节对齐——老用户从 Python 版升级数据无缝续用。

## 里程碑
| M | 内容 | 状态 |
|---|------|------|
| M0 | 脚手架 + 主题 token 奇偶校验 + 门禁 | ✅ |
| M1 | 托盘 + 三热键 + 快捷粘贴闭环（GO/NO-GO 决策点） | ✅ 代码完成（含剪贴板监听器）/ ⏳ **需真机复核** |
| M2 | 遥测采样 + 概览/网络监控 + 多粒度流量/日报 | ✅ 数据层+界面完成 / ⏳ 待真机看曲线 |
| M3 | 诊断与修复（netcheck 15 项 + 修复动作库） | ✅ **15/15 项检查 + 19/19 动作执行器** |
| M4 | 工具箱（测速/端口/Hosts/DNS 计算/ping 路由/局域网/DNS 测速/应用联网） | ✅ **全部 8 项已交付** |
| M5 | 系统助手（性能/启动项/电池/清理/磁盘/剪贴板/场景） | ✅ **全部 8 个分页已交付** |
| M6 | 密码库 | ✅ 加密层 + 数据层（SQLite，双向跨语言互开）+ 条目 CRUD/TOTP 全接 |
| M7 | 收尾（设置/日志/命令面板/打包） | ✅ **全部交付**（设置页 + 日志 + 命令面板 + NSIS 安装包） |

门禁全绿（历史快照 719 Rust + 469 前端 @2026-10-04；**当前 875 Rust + 618 前端**
——本机验证 873 passed / 1 ignored；`fix_nlasvc_round_trips_on_real_machine`
需要真实服务控制权限，受限沙箱会因 `sc.exe`/`reg.exe` 被拦截而失败，属环境限制。
真实剪贴板单测在桌面有剪贴板活动时可能环境性失败，属已知）。M1/M2 的**真机清单尚未执行**——
按 `UNATTENDED.md` §3，托盘里可能跑着用户实例、且测试红线禁止真实 Win32 注入，
因此 GO/NO-GO 判定必须由人工在真机完成。清单见下文「M1 真机复核清单」与
「M2 真机复核清单」。

### M7 交付状态

**设置页 + 日志 + 命令面板 provider + 打包**已交付（`settings.rs` 元数据层 /
`palette.rs` + `SettingsView.tsx`）。

**命令面板（`palette.rs`）**：算式直算、模糊匹配、动作排序、自定义命令解析。
匹配与排序在 Rust 侧（与 Python 逐项同口径），前端只渲染——两版同一个查询排出
**同样的顺序**，用户不会觉得"搜索结果变了"（含"命中位置 0 加 2 分""词首加 1 分"
"按长度微减分"这些细节，以及同分时保持注册顺序的稳定性）。

**算式求值自己写解析器**：Python 用 `ast.parse` + 算子白名单（绝不用 `eval`），
Rust 没有等价的安全子集求值，所以写了一个**只认数字与四则/幂/取模/括号**的递归下降
解析器——词法上就不存在标识符、属性访问、函数调用，从根上不可能执行任意代码。

**两道"不设就冻结界面"的护栏**（Python 版真机实测得来，面板在 GUI 线程同步求值）：
1. 指数绝对值 > 1000 直接拒；
2. 仅挡字面量指数不够：`((10**1000)**1000)**1000` 的右操作数全是 1000，结果却是
   330 万 bit。所以还按**左操作数位宽 × 指数**预估规模（>200k bit 即拒），
   这一步必须在乘方**计算之前**做。

**一处诚实的能力边界**：整数用 `i128` 而非 `i64`（`2**100` 这类常见算式要能算），
但 `10**100` 这种超出 i128 的仍然会**明确报"超出整数范围"**而不是静默给出错误数字
——Python 的 int 是任意精度，这里做不到，宁可知情地失败。静默算错比报错危险得多。

**元数据是装饰层，键清单的唯一来源仍是 `defaults()`**：`describe_all()` 遍历
`defaults()`，缺元数据的键退回键名而**不会从界面消失**。有专门的看门测试
（`decoration_covers_all_keys_...`）在新增键却忘写标签时失败，而不是等用户看到
一串英文键名。

**校验放在后端**（`set_one`）：界面可被绕过，而 `poll_interval_sec=0` 会让后台线程
忙碌空转、`clipboard_max=0` 会让剪贴板功能直接失效。前端 `validateValue` 做即时
反馈，但**与后端同口径**（含"布尔只认 0/1、不认字符串 `true`"这条既有语义），
否则会出现"界面说可以、后端拒绝"的错位。

**settings.json 读取容忍 UTF-8 BOM**：PowerShell 5.1 的
`Set-Content -Encoding UTF8`、部分编辑器会写 BOM，而 serde_json 不接受 BOM。
不剥离的话，一份**完全合法**的设置会被判成"损坏"并**静默回退默认值**——用户
看到的是"我的配置全没了"，日志里只有一句警告。`jsonio::strip_bom` 在读取端统一
处理（`settings.rs` 与所有 `jsonio::load_json` 调用方共用）；写入端仍保持无 BOM，
与 Python `json.dump` 逐字节一致。

日志查看只读**文件尾部**（按行数上限）：日志可能很大，整读会卡住界面。文件名做了
目录穿越校验（只允许纯文件名）。

**顺带固化了一条前端命名约定**（`tests/naming.test.ts`）：Windows 大小写不敏感，
`SettingsPage.tsx` 与 `settingsPage.ts` 会被 TS 判为同一文件的两种拼写并报
TS1149/TS1261——这个坑本项目已踩三次（`Hosts`/`hosts`、`Dashboard`/`dashboard`、
`SettingsPage`/`settingsPage`）。与其每次踩了再改，不如写成可执行断言：
组件 `Xxx.tsx`、逻辑模块 `xxx.ts`，**二者不得只差大小写**。

**打包（`tauri build` → NSIS）**：项目此前**根本没有接 Tauri CLI**（`@tauri-apps/cli`
未安装、`cargo tauri` 也不存在），所以打包这一步从未真正跑通过。现已补上：
CLI 2.12.1 与 Rust 侧 `tauri` 2.12.1 **版本对齐**（Tauri 要求二者主次版本一致）。
产物与实测：

| 产物 | 路径 | 实测大小 |
|---|---|---|
| 可执行文件 | `src-tauri/target/release/SysKit.exe` | 4.94 MB |
| NSIS 安装包 | `src-tauri/target/release/bundle/nsis/SysKit_1.7.4_x64-setup.exe` | 2.68 MB |

**安装 = 升级，不再"卸载旧版"（2026-10-20）**：Tauri 原版 NSIS 在检测到旧版本时
会弹一个选择页，且把「安装前卸载旧版本」设为**默认选中**——用户点「下一步」就先跑
旧卸载器：删掉 exe 与快捷方式、重建快捷方式，程序正在运行时还可能因文件占用失败
退回上一页，观感上就是"装新版把软件卸载了"。

改为 `bundle.windows.nsis.template = "nsis/installer.nsi"`（自定义模板，从本机
`@tauri-apps/cli` 二进制里提取的同版本官方模板，978 行），只在 `PageReinstall`
里加一段：**升级场景（`$R0 = 1`）直接 `Abort` 跳过该页**，由安装段覆盖文件并重写
卸载信息；同版本重装（`$R0 = 0`）与降级（`$R0 = -1`）仍保留原选择页。

为什么不用"选第二项（不卸载）"：那要用户每次手动选，且静默安装（`/S`）下模板
会自动选第一项，等于还是会卸载。用户数据在 `%APPDATA%\<bundle id>`（设置/密码库/
剪贴板历史），与安装目录无关，覆盖安装不碰它。

> **维护提示**：自定义模板意味着 Tauri 升级后模板的官方改动不会自动生效。
> 升级 Tauri CLI 后若 NSIS 行为异常，按同样方法重新提取模板再打一次补丁
> （`grep -n "SysKit 定制" src-tauri/nsis/installer.nsi` 找到补丁位置）。

**release 二进制冒烟测试通过**：启动后进程存活、内存约 30 MB、正常初始化数据目录。
（冒烟测试**不污染用户数据**——实测 `%APPDATA%\SysKit` 下无任何文件被本次测试改动；
该目录里的 `*.migrated` 是 Python 版自己早期迁移留下的（09-17/09-19），与本次无关。）

首次 release 构建约 **6 分钟**：`[profile.release]` 开了 `lto = true` +
`codegen-units = 1` + `strip = true`，体积与启动速度优先。`panic = "abort"` 意味着
发布版里任何 panic 都直接终止进程——所以修错路径一律返回 `Result` 而不是 `unwrap`。

### M6 交付状态（2026-10-03 数据层补全，决定已更新）

**数据层（`vault_store.rs`，rusqlite bundled）已交付**：`syskit/vault.py` 逐语义移植
（SCHEMA 逐字、搜索转义、touch 语义、批量解密降级契约、global_search 分词 AND）、
TOTP RFC 6238、会话式命令（解锁后数据密钥驻留托管会话 + 自动重锁/信任窗口两设置接线）。
VaultPage 渲染真实条目（站点/账号 CRUD、复制密码/TOTP 走 `sk_vault_copy_secret`——
明文不过前端 JS，三重敏感防护后端完成）。早期"数据层不实现"的界面诚实边界随之失效，
由 capability 门与解锁三态提示接替（历史决策见 git）。

**加密层（`vault_crypto.rs`）先期交付并双向跨语言验证；vault.key 创建/解锁/锁定、
主密码设置/校验/移除均为真实能力。**

**账号级联系方式（v2，2026-10-18）**：一个账号可挂多个邮箱 / 多个电话
（换绑前的旧号、备用号都留得住），新表 `account_contacts`，命令
`sk_vault_contacts` / `sk_vault_accounts_contacts` / `sk_vault_add_contact` /
`sk_vault_update_contact` / `sk_vault_delete_contact` / `sk_vault_set_primary_contact`。
- **为什么用迁移而不是改 `SCHEMA` 常量**：`SCHEMA` 必须与 Python 版 `vault.py`
  逐字一致（同一个 vault.db 两版互开是硬约束）。新表走 `user_version` 迁移追加，
  Python 侧不认识这张表但只读写 sites/accounts，互不冲突；Rust 打开 Python 建的
  老库就地补上。迁移测试里额外断言了 `accounts` 表的列清单不变。
- **主项唯一 + 删除递补**：同类型第一条自动成为主项，设新主项时旧的落为普通项，
  删掉主项则由剩下的第一条递补——不留"这个账号没有主邮箱"的空档（列表展示与
  双击复制都默认取主项）。账号删除时联系方式随外键 CASCADE 清空。
- 校验只挡明显填错（邮箱要 `@`、电话 ≥5 位数字）：内部短号、分机、各种国家码
  写法都得能存——**保存不进去比格式不规整严重得多**。前后端同口径，
  `tests/vault.test.ts` 用源码守卫把阈值钉在一起（改一侧必须改另一侧）。
- **全局搜索下推到 SQL**：旧实现把所有行捞到 Rust 里拼字符串再 `contains`
  （每次按键 O(行数) 次堆分配）。现在每个分词直接编译成 7 条 `LIKE` 的 OR
  （站点名/域名/别名/标签 + 账号名/用户名 + `EXISTS` 子查询命中联系方式），
  分词之间 AND。**等价性前提**：分词按空白切分，不可能跨越字段边界，
  所以"拼接后包含"与"某字段包含"判定一致。

**`vault.key` 的字节级格式**（两个版本必须互相能读）：
```text
密码模式：{"mode":"password","kdf":{"salt":<b64 16B>,"iter":600000},
           "wrapped_key":<b64(nonce12 + gcm_ct||tag)>,   aad="vault-key"
           "verifier":  <b64(nonce12 + gcm_ct||tag)>}   aad="vault-verify"
免密模式：{"mode":"dpapi","wrapped_key":<b64 DPAPI blob>}
```
数据密钥 32 字节；PBKDF2-HMAC-SHA256（600k 迭代）；verifier 明文 `syskit-vault-ok`；
字段加密 aad=`"field"`；DPAPI 描述串 `SysKit vault key`。

**用 BCrypt 而不是新引依赖**：AES-256-GCM 与 PBKDF2 都在 `windows` crate 里
（`BCryptEncrypt`/`BCryptDeriveKeyPBKDF2`），DPAPI 本来就用 `CryptProtectData`。
不引新依赖就少一处供应链风险；BCrypt 与 Python 的 OpenSSL 在同标准下字节级兼容。

**双向验证**（这是 M6 最关键的一条；两个驱动脚本已随 Python 版退役删除，
方法存档见 `git show python-final:tools/gen_vault_fixture.py`）：
- 正向：Python 生成 `vault.key`（**故意用 2000 次迭代
  而不是常量 600000**）+ 一份 aad=`field` 的密文，Rust 单测读它并解出同样的明文；
- 反向：Rust 生成 `vault.key` + 密文，交给 Python 读并解密。
**两个方向都通过**——用户无论从哪一版切到另一版都不会看到"主密码错误"。

三个必须保留的语义：
1. **迭代次数从文件里读**（`kdf.iter`），不是每次都用当前常量。将来上调 `KDF_ITER`
   时，存量库若按新常量派生会解不开 verifier，用户会看到"主密码错误"并可能**误删整个库**；
2. **解锁失败返回原因而不是抛异常**：`vault.key` 损坏（掉电截断）时报 `Corrupt`，
   与"主密码错误"严格区分——否则用户会反复试密码，甚至以为库没了而删掉；
3. **`BCryptSetProperty` 设 GCM 链模式时值必须带结尾 NUL**：该属性是宽字符串常量，
   BCrypt 做 NUL 结尾比较，少一个 NUL 就返回 `STATUS_INVALID_PARAMETER`
   （本机实测就是这样失败的）。不设的话默认链模式是 CBC，完全不是 GCM。

### M5 交付状态（✅ 全部 8 个分页已交付）

**性能监控 + WiFi + Top 进程 + 启动项 + 磁盘空间 + 垃圾清理 + 电池健康 + 场景模式
+ 剪贴板历史**已交付（`wifi.rs` / `portscan.rs` / `startup.rs` / `disk.rs` /
`cleanup.rs` / `battery.rs` / `modes.rs` + `SystemPage.tsx`）。**M5 的 8 个分页全部完成。**

**剪贴板历史分页**（`ToolboxClipboard.tsx` + `cliphistory.ts`）与快捷粘贴面板
（M1 的 `ClipPanel`）**分工明确**：面板只做"输入即搜 + 回车粘贴"（无边框弹出窗、
键盘优先）；这里是完整管理界面（类型筛选、置顶、删除、清空、按天数清理、另存为）。
两者共用同一份历史数据与同一批后端命令（`sk_clip_*`）。

**系统助手 7 分页优化（2026-10-19）**：
1. **Top 进程不再起子进程**（本页最重的一处）：原来每次 `tasklist /fo csv`
   实测 **1430ms**，而性能页每 3 秒刷一次——等于常驻半个核，用户会直接看到
   "SysKit 自己很吃 CPU"。改用 `w32::snapshot_process_memory`
   （Toolhelp 快照 + `K32GetProcessMemoryInfo`）：本机 196 个进程 **16ms**，
   约 **89 倍**，且不产生子进程（不触发安全软件的子进程审计）。
   `tasklist` 解析保留为兜底路径；受保护进程打不开时**跳过而不是记 0**
   （记 0 会让 Top 榜混进一堆 0 字节条目，把真正吃内存的挤出去）。
   回归守卫：单测断言快照 < 500ms 且能看到自己、工作集非 0
   （同时验证 `dwSize`/`cb` 两个结构体大小字段没写错）。
2. **电池健康**：面板不可见（切到别的 tab/最小化）时停掉 10 秒轮询——ui Tabs
   是 keepMounted，切走后组件仍活着，没人看时刷新是纯浪费。
3. **启动项管理**：枚举要读四个注册表位置 + 两个启动文件夹（数百毫秒），
   原来点 🔄 没有任何反馈，用户会以为按钮坏了并重复点（并发枚举）。
   补 `loading` 态。
4. **磁盘空间·分析总览**：补盘符快捷按钮（点击填入根路径但不自动开扫——
   整盘全量扫描可能数十秒，误点就开扫等于替用户做了个重决定）。
5. **启动项管理·排序**：列表常有 50+ 项、每页 15 行，默认是注册表遍历顺序，
   想集中看 HKLM（需管理员）只能一页页翻。新增「默认/按位置/按名称」排序，
   位置分组后再按 `localeCompare(.., "zh-CN")` 排名称（码点序会把中文全排到
   最后，读起来像"根本没排"）。
6. **WiFi 已存密码·筛选**：已存配置常有几十上百条（公司/学校网络+热点），
   左侧列表补筛选框，标题计数变成「命中/总数」。
7. **磁盘表列宽收紧**：980 宽窗口下右栏约 740px，原来列宽合计 790 会横向
   滚动、最后一列被切一半；收到 676 后完整可见。
8. **Top 进程轮询 3s → 2s**：原生快照只要 16ms，跟着后端 2 秒 TTL 走，
   榜单更跟手而代价可忽略。
9. **两栏页滚动（2026-10-20 修，用户报"滚动 WiFi 时整页在滚"）**：
   `useFillRowHeight` 原本用 `querySelector(".ant-layout-content")` 取滚动容器，
   而 v1.4 已整体移除 AntD——选择器永远匹配不到，`measure()` 直接 return，
   行高一直是 `undefined`，于是条目一多整页（含标签栏）被撑开。改为**按计算
   样式向上找最近的 `overflow-y:auto|scroll` 祖先**，并按「行顶相对容器内容区
   + scrollTop」算高度；同时观察容器尺寸变化。WiFi 已存密码、密码库两个两栏
   页同时受益。这类故障不会报错、不会变红，只有"滚动位置不对"，因此加了
   源码守卫（禁止按 `.ant-*` 取容器）+ 纯函数用例。
10. **垃圾清理**：重新扫描后把已选 key 与当前目标求交集——清理后某项整个
    消失时，残留选中会让「已选 N 项」虚高并把不存在的 key 传给后端；
    长路径行补 `min-w-0 + truncate`（多根目录拼接会把行撑出横向滚动条）。
11. **磁盘下钻**：新增「↑ 上一级」（`parentPath` 纯函数，覆盖盘根 `C:\`、
    `C:`、尾斜杠、UNC 共享根——盘根若按 `\` 切会得到空串，传给后端等于
    扫描当前工作目录，属最坏那类静默错误）。
12. **性能监控**：曲线卡片标注窗口长度（`SERIES_LEN` 180 点 × 1 秒 = 近 3 分钟），
    之前只画线不说时间，用户无从判断"这条线是多长时间"。

**全面优化（v1.6.0，2026-10-20）**：

1. **速率平滑**（`dashboard.ts::ema/smoothTick`，α=0.35 ≈ 3 秒时间常数）：
   后端每秒给的是"上一秒的字节增量"，直接显示会在几 MB/s 与 0 之间狂跳，
   曲线全是刺。概览页与系统页的**上下行/磁盘读写**统一走平滑值；
   **cpu/mem 不平滑**——它们本来就是系统计数的区间平均，再平滑会迟滞，
   反而看不清瞬时峰值。断流后重置基准，避免把旧值当起点。
2. **数据新鲜度**（`freshness`）：卡片标题旁显示"刚刚更新 / N 秒前"，
   超过 10 秒（漏 3 拍）转警告色。数字停住而不提示，用户会把卡住的读数
   当成真实网速——这比显示错误更危险。
3. **启动项批量操作**：勾选列（不可操作项不给勾）+「全选 / 批量启用 /
   批量禁用」，逐条串行写注册表，失败项汇总报告。目标从**全量列表**取而非
   当前筛选结果，避免"已选 5 项只执行了 2 项"这种静默偏差。
4. **垃圾清理上次记录**：清理后显示「上次清理 HH:MM · 释放 X」，存在组件
   状态里（Tabs 保活，切页不丢）；**重启归零**——跨重启要往设置 schema 加键，
   为一个只读展示不值当，已在代码注释里写明这个取舍。
5. **后台轮询可见性守卫**（本轮）：keepMounted 面板切走后组件仍活着，四处
   轮询在没人看时白烧——
   - 端口页 auto 每 5 秒一个 `netstat` 子进程（最贵，实测 0.5~1 秒/次）；
   - WiFi 自动扫描每 8 秒一个 `netsh`（原来只查 `document.hidden`，
     切到本页其他 tab 时窗口可见，守卫形同虚设）；
   - 设置页后台监控状态每 5 秒读后端；网络质量卡 60s/300s 拉趋势。
   统一为 `document.hidden || 面板 offsetParent===null` 双条件。
   **教训**：给 QualityCard 加守卫时一段 Python 批量替换把文件写坏了
   （内容变成 SettingsView），靠 `git checkout --` 恢复——多文件批量
   替换后必须 `git diff` 逐文件核对再提交。
6. **Top 进程卡支持结束进程**（复用端口页的 `sk_kill_process`，后端已拒绝
   PID 0/4 与 SysKit 自身）：行级 busy（不全表转圈）、危险色确认框、结束后
   立即重拉榜单——刚被结束的进程还挂在榜上会误导，且 PID 可能被复用。
   `loadProcs` 抽成 useCallback 保持引用稳定，否则每秒重渲染换新函数，
   卡片的 memo 就白做了。

**网络修复优化（v1.7.1，2026-10-20）**：

1. **修复记录会话内持久化**（`repair.ts::loadRecords/saveRecords`）：记录此前
   只存组件 state，刷新/切窗即丢——而排查恰恰发生在"刚才那下坏了"之后。
   与诊断报告同一策略（sessionStorage，关窗即清）：记录可能带网络环境信息
   （SSID/网关），不进 localStorage 这类持久存储。恢复时逐条校验字段，
   脏数据剔除而不是整包丢弃。
2. **一键修复摘要点名关键信息**：原来只报"成功 N 项，失败 M 项（详见修复
   记录）"——`need_reboot` 的动作完全不提示（用户改完 DNS 以为立即生效），
   失败项也得去记录面板翻。现在摘要带"其中 M 项需重启后生效"与失败项标题
   （最多点名 2 个）。
3. **批量重查失败留痕**：`recheckAllFailed` 单项重查失败原来只进
   `console.warn`——用户永远看不到。现在写进修复记录（只进 console 的错误
   等于没记录）。

**剪贴板图片灯箱缩放（v1.7.2，2026-10-20）**：

灯箱此前只能"看整图"——大图被 max-h/max-w 压到一屏里，想看清二维码/验证码
局部只能干瞪眼。现在：

- **滚轮缩放**（1.15 步进，范围 1~8 倍）；wheel 用原生监听 + `passive:false`，
  React 合成 onWheel 挡不住默认滚动，缩放时页面会跟着滚
- **拖动平移**（scale>1 时 grab 光标，拖动中关掉 transition 避免跟手延迟）
- **双击 2×/适应**、工具条 −/＋/适应/关闭、百分比实时显示
- **Esc 关闭**（此前也没有键盘出口）；缩回 1 时清平移偏移，
  残留偏移会把图移出视野，看起来像"卡死"
- 数值逻辑抽成 `cliphistory.ts::clampZoom/zoomBy` 纯函数（3 例测试）

新增 `sk_clip_save_as`：图片走 PNG、其它走文本，用 Win32 `GetSaveFileNameW`
（带 `OFN_OVERWRITEPROMPT` 与 `OFN_NOCHANGEDIR`）。文本的默认文件名由内容生成并
做**路径分隔符消毒**——条目内容不可信，`..\` 之类的片段不该出现在默认路径里。

三个"用户预期"相关的文案/判定：**置顶条目确实免于淘汰**（`evictionCandidates`
只统计非置顶项）、**清空会连带删掉置顶条目**（确认文案必须写明）、
**隐私暂停恢复后不补录**（否则用户对隐私产生错误预期）。

**图片显示清晰度（2026-10-19 修）**：用户实测"剪贴板里的图片很模糊"，两处成因：
1. 缩略图用 `image::thumbnail()`（快速采样、无完整抗锯齿），截图的细文字缩到
   96px 就糊成一团 → 改 `resize(.., Lanczos3)` 等比缩放（只缩不放）；
2. 96px 不够：列表显示 56 CSS px，Windows 125%/150%/200% 缩放下物理像素需求是
   70/84/112，96px 在 200% 屏上仍被放大 → 提到 **192px**（单条仅几十 KB，落在
   256 条 LRU 里）；
3. **点击放大原本直接把缩略图拉满全屏**——96px 铺到上千像素必然糊 → 新增
   `sk_clip_image_full`：原尺寸直出（≤4MB 的 PNG 不解码重编码，零质量损失；
   超 1600 长边才 Lanczos3 缩放），灯箱先显示缩略图占位、原图到位后替换。
   原图缓存单独一个 8 条的小 LRU，>2MB 的不进缓存（内存有界）。

**行尾 ··· 菜单被裁切（2026-10-19 修）**：菜单原来 `absolute` 挂在触发行里，
账号表在 `overflow-x-auto` 容器 + 外层卡片 `overflow-hidden`，一打开就被容器
边缘裁掉（用户实测：「显示密码」整条看不见）。改为 **Portal + fixed 定位**
（不受任何 overflow 祖先裁剪），贴边收进视口、下方放不下向上翻、滚动即收起、
Esc 关闭。注意 Portal 的经典坑：点外部关闭的判断必须同时查菜单自身，否则
mousedown 先关菜单、click 到不了菜单项（全变哑按钮）。

**密码可见性与编辑入口（2026-10-19 补）**：密码列悬停浮现 👁（点一下显示
明文 30 秒、再点隐藏）——不做成常驻图标（用户实锤过"行内图标太多"），但也
不能全藏进 ··· 菜单（用户实测找不到"密码怎么看"）。双击账号行 = 编辑该账号
（用户名/密码/备注都在同一个表单；密码留空 = 不修改）。

**按天查看（2026-10-19 加）**：剪贴板页工具栏新增日期下拉，只列**当前筛选结果
里真的有内容**的日期（`全部日期（N）` / `今天（N）` / `昨天（N）` / `YYYY-MM-DD（N）`）。
选定某天后列表与分页都只在该天内；选「全部日期」时按天分节，节标题粘性置顶。
日期一律按**本地时区**取（`getFullYear/Month/Date`）——用 `toISOString()` 是 UTC，
东八区 00:00–08:00 复制的内容会被归到"昨天"，这是该功能最容易被发现的错误。
置顶条目可能来自更早的日期，所以分组用 Map 去重而不是"相邻两行比较"，
否则同一天会被切成好几段、出现重复日期头。

**删除入口（2026-10-19 补）**：账号删除本来在行尾 ··· 菜单最后一项（与其它
动作用分隔线隔开，防手滑），但旧版菜单被容器裁切，用户只看到「置顶/编辑」，
据此反馈"没有删除功能"。除了修裁切，左侧站点列表也补了每行悬停 ···
（编辑/删除站点）——此前必须先选中站点、再去右栏标题栏找菜单，等于把删除
藏在两级入口之后。两处删除都保留二次确认。

**场景模式**（`modes.rs`）三条原则（与 Python 相同）：**每一步返回
`(名称, 是否成功, 说明)`，单步失败不中断后续**；改系统配置尽量走注册表/`powercfg`；
**清后台只处理白名单**，绝不碰 explorer/系统关键进程。

两个诚实性细节：
- **`NoAutoUpdate` 是持久策略，不是"暂停 N 天"**。文案不得暗示到期自动恢复——
  否则用户以为 7 天后自动恢复，实际一直关着。有专门断言禁止文案出现"7 天/自动恢复"；
- **取消静音要恢复进模式前的音量**，而不是想当然置最大（旧行为会把用户 30% 变成
  100%）。存档值必须同时约束类型与范围：`0`（本来就是静音态）、`bool`（JSON 手改成
  `true`，在多数语言里等于 1）、越界——任一不满足都按"无记录"处理，**并如实说明
  "未找到原音量，已设为最大"**，而不是谎报"已恢复原音量"。另外存档失败要在**静音时**
  就告知用户（否则退出时才发现声音回不来）。

**清后台的 PID 复用防护**：从"列出候选"到"结束它"之间进程可能已退出、PID 被系统
分给另一个无关进程；击杀前**复验进程名**，不一致就跳过而不是误杀。

**测试不污染开发机**：电源方案测试断言用**方案 GUID**（`powercfg` 输出随系统语言变化，
按本地化名称断言会在别的语言机器上假失败），测完用 GUID 复原原方案；通知开关测试
先存原值、跑完复原，并单独验证过"原值为 1 时也留 1"。

**电池**（`battery.rs`）分两层：实时状态走 `GetSystemPowerStatus`（微秒级，可 10 秒轮询）；
**健康度只能查 WMI**（`BatteryStaticData`/`BatteryFullChargedCapacity`），那一步要起
PowerShell、最坏 30 秒，所以**只由用户点刷新触发，不轮询**。数据源与 Python 是同一段
CIM 脚本，两版读数天然一致。

几个"没有数据时不能编造"的点：
- **无电池不是错误**（台式机很常见），必须给出明确说明而不是空白或 0%；
- **`design`/`full` 为 0 时健康度返回 None 而不是 NaN**：WMI 在没有电池时会给 0，
  `0/0` 是 NaN，界面会显示 `NaN%`；
- 健康度 **>150% 一律视为不可信**（多半是 WMI 给了不属于同一块电池的数据），
  宁可显示"未读取到"也不给用户一个荒谬的数字；但 2% 这种"电池快废了"的合法低值
  要照实显示，不能因为看着离谱就吞掉；
- 解析 CIM 输出时**空字段按缺席处理而不是 0**；电量越界只作废电量字段，
  不连坐丢掉设计/满充容量（丢了就再也算不出健康度，而那正是用户最想看的）；
- 低电量提醒**接着交流电时不发**——那会变成"插着电还让我充电"的骚扰。

**磁盘空间**（`disk.rs`）：`GetLogicalDrives` + `GetDiskFreeSpaceExW`，纯换算与分档
可逐条断言。两个容易混的语义：
- **`free_to_caller` 与 `free_total` 不同**：前者受磁盘配额限制，后者是整卷空闲。
  "这个盘还剩多少"必须用 `free_total`——用配额值会让用户在配额场景下看到虚假的
  "空间紧张"；
- `used()` 用 `saturating_sub`：配额/统计异常时 `free` 可能大于 `total`，
  普通减法会下溢成天文数字并画出一条爆表的进度条。

**垃圾清理**（`cleanup.rs`）的安全模型三条：**白名单**（只清理写死的四类位置，
不接受调用方传路径）、**只删文件不删目录**、**正在使用的文件跳过并计数**。
最后一条是诚实性要求：只报"清理成功"会让用户以为清干净了，而正在使用的文件
一个都没动。另有 `is_within` 做防越界删除的最后一道闸——按小写 + 反斜杠比较，
且要求根目录以分隔符结尾，否则 `C:\Windows\TempBackup` 会被误判为在
`C:\Windows\Temp` 之内（有专门用例）。

缩略图缓存**只清理 `thumbcache*`**：`Explorer` 目录里还放着用户的其他资源管理器
配置，无差别清空会把它们一起删掉。回收站走 Shell COM（`Clear-RecycleBin`），
没有可遍历的目录路径——把它当普通目录遍历会删到别的东西。

**启动项的禁用机制**（Windows 的 `Run` 键**没有**启用/禁用标志位——`StartupApproved`
是另一套且位含义未文档化）：把值从 `Run` **搬**到 `HKCU\Software\NetTool\DisabledRun`，
存储格式 `<位置>|<命令>`；启动文件夹项改名加 `.disabled` 后缀。
因此有两处**跨版本共享标识**，改错就会让两版各藏一份自己的禁用列表：
- `DISABLED_KEY` 是 `Software\NetTool\DisabledRun`——**沿用旧产品名 `NetTool`**，
  不是 `SysKit`。改名会让用户已禁用的项在另一版里"凭空消失"（`Run` 里没有、
  禁用键里也找不到）。有专门测试锁住这一点；
- 后缀固定 `.disabled`，启用时**只剥一层**（旧版连锁改名可能留下
  `.disabled.disabled`，剥全部会丢一层语义）。

其余移植细节：
- **先记录再删除**：禁用时先写禁用键、再删 `Run` 值。反过来的话中途失败会两头都没有
  ——用户的启动项直接消失（既不自启也无法从界面恢复）；
- 删除失败时**回滚**禁用记录，否则会出现"禁用键有 + `Run` 也有"的重复状态；
- **启用时校验位置一致**：禁用记录属于 HKLM 却按 HKCU 启用必须拒绝，否则会悄悄把
  作用范围从"所有用户"改成"仅当前用户"；
- 已禁用项以**原名**+禁用态列出（后端剥掉 `.disabled`）：否则会显示成"已启用"且
  启用按钮永远点不到（它只在禁用态生效）；
- 去重键是 `(位置, 名称)` 而非名称：同名项可同时存在于 HKCU 与 HKLM 且各自生效。

**测试安全**：禁用/启用逻辑抽了 `StartupBackend` trait，用内存实现跑完整的
「禁用 → 枚举 → 启用」往返（含回滚、位置不一致、坏记录、写失败等分支）。
禁用启动项是系统级改动，测试里**绝不能**真去动 `HKCU\...\Run`——那会把开发机的
开机自启项搬走。

**Top 进程的数据源选择**（实测三条路）：
| 数据源 | 耗时 | 问题 |
|---|---|---|
| `Get-CimInstance Win32_PerfFormattedData_PerfProc_Process` | **3.3 s** | 太慢；`Name` 是 `chrome#20` 这种**性能计数器实例名**，不是真实进程名 |
| `Get-Process` | 0.3 s | 要多写一个 JSON 解析分支 |
| `tasklist /fo csv /nh` | 0.5 s | — |

选 `tasklist` 的决定性理由：**端口页已经在解析同一份 CSV**（`split_csv_quoted`），
复用同一份规则就没有第二处解析可能出错。内存列解析不写死乘 1024——格式是
`"919,488 K"`，单位随系统可能是 K/M/G，写死会在某些系统上差 1024 倍。
后端有 2 秒缓存（`tasklist` 一次 0.5 秒，不该每次刷新都跑）。

占比分母用**进程内存合计**而非物理内存：tasklist 报的是工作集，合计会大于物理内存，
用合计作分母自洽且不需要额外读数；文案里写明是"占进程合计"以免误读。

**WiFi 走 `wlanapi.dll`，不再起 `netsh` 子进程**：Python 版每 5 秒起一次 `netsh wlan
show interfaces` 并解析本地化文本（信号行有 `信号` / `Signal` 两种写法）。Rust 侧
直接调 `WlanOpenHandle`/`WlanQueryInterface`，一次约几十微秒。更重要的原因：
**SSID 决定流量归因桶**，解析失败就会静默归到「以太网/未知」——没有解析环节就没有
解析失败。M2 里"所有流量归未知桶"的简化**就此消除**，`rate_stats` 的 SSID 归因真正启用。

三个真机级细节：
- `dot11Ssid` 是**定长数组 + 长度字段**，必须按 `uSSIDLength` 截断。按 NUL 结尾读会
  把数组里的残留垃圾拼进 SSID → 用户看到乱码网络名，流量也归错桶；
- **是否连接看 `isState`，不看信号值是否为 0**：质量 0 是有效读数（贴着覆盖边缘），
  当成"未连接"会让用户看到 SSID 存在却报断开；
- 句柄与 API 分配的内存都用 RAII guard（`HandleGuard`/`MemGuard`）释放。漏一个句柄
  在 1 秒采样下会很快耗尽资源——Python 版用子进程不存在这个问题，移植时必须补。

`quality→dBm` 换算与 Windows 相同（`dBm = quality/2 - 100`），**对奇数 quality 会丢 1**
（整数除法），测试里明写这一点而不是声称"往返一致"。

**断流守卫**（`isStale`）：页面隐藏期间采样被跳过，切回时若断流超过 10 分钟清空旧序列
——否则会把 10 分钟前的点与现在的点画成一条"平稳曲线"，用户以为一直很稳而实际没有数据。

**隐藏网络显示**：已连接但 SSID 为空 = 隐藏网络，必须显示「隐藏网络」而不是空白，
否则用户看到"已连接 + 空名字"会以为程序坏了。

### M4 交付状态

**Hosts 编辑**（`hostsed.rs` + `ToolboxHosts.tsx`）：解析/序列化/增删改切换 + 备份
写入 + 保存前变更摘要。

**端口/进程 + 局域网设备**（`portscan.rs` + `ToolboxPorts.tsx`）：`netstat -ano` /
`tasklist /fo csv` / `arp -a` 解析；筛选、5 秒自动刷新、结束进程（受保护 PID 前后端
双重拒绝）；并发 ping 唤醒邻居后读 ARP 表。

**DNS / 子网 / ping·路由**（`subnet.rs` + `ToolboxDns.tsx`）：子网计算**自己实现**
（不引 `ipnet`/`cidr`——IPv4 前缀就是位运算，手写能把 /0、/31、/32 边界逐条断言，
也少一个依赖）；DNS 查询走自研 UDP 客户端（可选系统解析器或指定服务器）；
ping 复用 netcheck 的丢包率解析；tracert 逐跳解析。

- **域名编码支持 IDNA**：非 ASCII 标签（如 `例子.com`）按 RFC 3492 punycode 编码为
  `xn--` 形式，长度前缀按**编码后字节数**计（字节数 ≠ 字符数）；中文输入法常见的
  全角句号（`。`/`．`/`｡`）按标签分隔符处理，ASCII 标签统一小写化（与 Python
  `str.encode("idna")` 主路径一致；不做 NFC 归一化）。**同时校验全名线格式
  ≤255 字节**——只校验单标签 ≤63 是不够的，4 个 63 字节标签拼起来就是 257 字节。
  自己实现而非引 `idna` crate：本模块的定位就是「DNS 不可信时的对照工具」，
  依赖越少越好，算法边界可用固定向量逐条断言。
- **IPv6 DNS 服务器可用**：服务器地址先解析为 `IpAddr` 再配 53 端口。旧实现
  `format!("{server}:53")` 对 IPv6 会拼出 `2001:4860:4860::8888:53`——这是一个
  合法 IPv6 地址但**没有端口**，`SocketAddr` 解析必然失败。

- **`/0` 的前缀掩码左移 32 位是 UB**，必须单独处理，否则结果是随机的。
- **`/31` 按 RFC 3021 算 2 个可用**、`/32` 算 1 个——不能套用「减 2」的公式。
- tracert 全超时行的主机部分是本地化的「请求超时。」，**不能**当主机名。
- ping 结果里的「超时」按 warning 而非 error 显示：那是网络慢，不是用户输错了域名。

**DNS 测速**（`dnsspeed.rs` + `ToolboxSpeed.tsx`）：并发测 6 台公共 DNS 的解析延迟与
成功率，推荐最优。每台服务器**串行** N 次（避免自我并发干扰）、服务器之间并发
（单台超时不拖总时长）；**单次超时按失败计入、不重发**——重发会把丢包服务器测成
"慢但成功"，与 Python `attempts=1` 同口径。推荐项只从"至少成功一次"的服务器里选
（全失败的不能因为"没有延迟"被当最优）。

**下载测速已接入（`netdl.rs`，WinInet）。** 走系统 TLS + 系统根证书 + 跟随系统代理
——与 Python 版同一栈，不引入 rustls/native-tls；三个测速源与 Python 一致，每源
限时下载（默认 8 秒，下载即断不落盘），支持取消，多源隔离窗口互不拖累。
`sk_speedtest_status` 返回 `available: true`；界面按可用性三态渲染（后端不可用时
仍如实显示原因，不造假速率——**一个假的速率数字比没有更糟**）。

**应用联网**（`appnet.rs` + `ToolboxAppNet.tsx`）：用 Windows 防火墙出站规则按程序
阻断联网，台账与 Python 版共享 `%APPDATA%\SysKit\appnet.json`。

**跨版本身份契约（本模块最重要的约束）**：规则名 =
`SysKit-AppNet-<sha1(norm_path(program))[:10]>`，**两版必须逐字节一致**，否则同一个
exe 会被两个版本各建一条规则，用户在 `wf.msc` 里看到重复项，而"放行"只关掉其中一条
——程序仍连不上网，用户却已被告知"已放行"。因此有两个**不可混用**的路径形态：
- `norm_path`：**身份键**，小写 + 反斜杠，只用于比对与哈希（= Python `normcase(abspath)`）；
- `real_path`：**下发给 netsh 的值**，保留原始大小写（防火墙不敏感，但把用户程序名
  改成小写会让规则在 `wf.msc` 里显得像伪造的）。

已用真实 Python 输出校准为锚点测试：`C:\Program Files\A\a.exe` → `5bd6cecf63`、
`C:\Windows\notepad.exe` → `84a531d86f`、`C:\a.exe` → `c0bae2df43`。

**三态展示，`drift` 不可省**：`on`（启用且动作=阻止）/ `off`（规则停用）/ `drift`
（动作被外部改成"允许"）。用户把动作改成允许后 `Enabled` 仍是 `True`——只看 Enabled
或只看台账 `blocked`，都会把**已经放行的程序报成"已禁止联网"**，对安全控制属实质性
错报。前端 `isReallyBlocking` 一律以实际状态为准，并有专门测试锁住"台账 blocked=true
+ 状态 drift 时不得算在拦"。

其余移植细节：`netsh add rule` **没有** `group`/`displayName` 参数（那是 PowerShell
`New-NetFirewallRule` 的概念，带上必报"不是这个指令的有效参数"）；PS 批量查询脚本
末尾必须 `exit 0`（`-Name` 里有名字不存在时 `powershell.exe` 即使输出正常也返回 1）；
没有 `Get-NetFirewallRule` 哨兵时**退回 netsh 逐条查**，不能把"查不到"当成"规则不存在"。
文件选择器用 Win32 `GetOpenFileNameW`（带 `OFN_NOCHANGEDIR`，避免对话框改掉进程 cwd
而影响 `real_path` 的相对路径解析），不为一次选择引入 `tauri-plugin-dialog`。

两条**用户数据保护护栏**原样移植（Python 注释里是损坏事故换来的）：
- **BOM**：按 `utf-8-sig` 剥离——BOM 会污染首个 token，导致 IP 判定失败、
  整行被当成普通注释，用户的映射行就此"消失"；
- **写回前拒绝可疑编码**：文件含 NUL（UTF-16/二进制）或 U+FFFD（ANSI/GBK 中文
  注释被按 UTF-8 解出替换字符）时**拒绝写入并保留原文件**。保存会把注释永久
  写成问号，这是不可逆的数据损失。

序列化契约（与 Python 相同，已在测试里写明）：**纯注释行与空行逐字保留**；
**映射行正文重建为规范单空格**（原文件的制表符/对齐缩进不保留——语义不变，
换掉逐字段 diff 的复杂度）。

### M3 交付状态

**修复动作库**：19 项（元数据与 Python `ACTION_IDS` 逐项对齐）+ 3 个组合 +
管理员门禁 + 前端分级界面，**执行器全部接入**（批次 0 安全类 6 项、批次 A 无需
管理员 5 项、批次 B 需管理员 8 项）。`DEFERRED_ACTION_IDS` 已清空。

批次 B 的要点：
- **管理员门禁前置**：非提升进程直接给出「请以管理员身份重启 SysKit」而不是
  底层 API 的「拒绝访问」——后者用户不知道该怎么办。
- `restore_point`：`Checkpoint-Computer -ErrorAction Stop` 必须带——PowerShell
  非终止错误的 `-Command` 退出码恒 0，不转终止的话「系统保护未开启」会被当成
  已创建，而还原点是组合首位安全网。
- `power_off`：PnPCapabilities 必须写 **REG_DWORD 0x18**；写 REG_SZ "24" 不被
  NDIS 电源策略识别、修复静默无效，而读回 `str()=="24"` 恰好自洽形成假成功。
- `mac_random_*`：改 WiFi 配置 XML 的 `EnableRandomization`（纯函数 + 单测），
  改完调 `restart_wlan` 重连生效；无该块时插到 `</WLANProfile>` 前。
- `dns_preset` 修正为 Python 的三步：`set` 主 DNS → `add` 备用（index=2）→ 清 v6
  静态。原先写成 `set ... static 主 备` 是错的（netsh 只接受一个地址）。

**网络诊断 15 项：全部实现**（`ALL_CHECKS` 顺序、名称、quick 跳过集合与
Python 逐项对齐）。为支持依赖网卡/路由的 5 项，新增 `snapshot.rs` 采集层：
`Get-NetAdapter` / `Get-NetIPAddress` / `Get-NetRoute` / `Get-NetIPInterface` /
`Get-DnsClientServerAddress` / `Get-NetIPConfiguration` 一次采回 JSON，
**解析全部是 Rust 原生纯函数**（含 `ifMetrics` 驼峰键、按 ifIndex 归并、
CIDR 换算）；持久路由走注册表 `Tcpip\Parameters\PersistentRoutes`（权威来源，
不必解析本地化 `route print`）。全部 15 项**一次快照复用**，不重复起 PowerShell。

**两处相对 Python 的降级（代码与界面都标注）**：
- **HTTPS 只验证 TCP 可达**（Rust 标准库无 TLS）。`real_conn` 详情明确写
  「TCP可达(未校验证书)」，不把「未验证」写成「已验证」。
- **诊断热路径已换原生 ICMP**（`icmp.rs` IcmpSendEcho，无本地化/无子进程）；工具箱的持续 ping 与局域网扫描仍走 `ping.exe`（流式输出语义不同，属工具行为本身），后续可按需换原生。

**测试里逮到的两个自己的错（都是静默失败型）**：
1. `RawRoot` 漏了 `#[serde(rename_all = "camelCase")]` —— serde 把不认识的键
   **静默**丢进 default，接口跃点表恒为空且不报错。
2. `if_metrics` 的填充顺序排在 routes 之后 → `if_metric` 恒为 0。
   两处都是「结果看起来正常但数值全错」，靠断言具体数值才暴露。

### M2 相对 Python 版的两处有意简化（不是漏做）

- **WiFi 信号/SSID 暂用「未实现」占位**：Python 版每 5 秒起一次 `netsh wlan`
  子进程。Rust 侧不引子进程（进程创建开销大且难测），改用 `wlanapi.dll`
  （`WlanOpenHandle`/`WlanQueryInterface`）是正确做法——留待下一轮，届时
  `rate_stats` 的 SSID 归因才真正启用（当前所有流量归「以太网/未知」桶，
  **与 Python 未连接时的行为一致**，数据不丢）。
- **Top 进程（系统页）未做**：属 M5 范围，留到系统助手页一起接。
  ✅ **SSID 归因已补齐**（M5 起走 `wlanapi.dll`，见上「M5 交付状态」）。

## 打包（M7）

前置：`npm install`（含 `@tauri-apps/cli`，devDependency）。**CLI 版本必须与 Rust 侧
`tauri` 同版本**——两处都锁在 2.12.x，主次版本不一致时 Tauri 会直接拒绝构建。

```powershell
cd app-rs
npm run app        # 开发模式（tauri dev）
npm run package    # 出安装包（tauri build → NSIS）
```

产物位置：

| 产物 | 路径 |
|---|---|
| 可执行文件 | `src-tauri/target/release/SysKit.exe` |
| NSIS 安装包 | `src-tauri/target/release/bundle/nsis/SysKit_1.7.4_x64-setup.exe` |

配置要点（`src-tauri/tauri.conf.json`）：
- `bundle.targets = ["nsis"]`，图标 `icons/icon.ico`（内含 7 个尺寸）；
- `beforeBuildCommand = "npm run build"`（已含 `tsc --noEmit`，前端类型不过不会出包）；
- `identifier = com.syskit.desktop`——**安装包的升级识别靠它**，改动会让新包被
  当成另一个应用（用户装出两份）。

两个注意点：
- `[profile.release]` 开了 `lto = true` + `codegen-units = 1` + `strip = true`，
  **首次 release 构建很慢**（分钟级），这是有意的：体积与启动速度优先；
- `panic = "abort"`：发布版里 panic 直接终止进程而不展开。这是桌面工具的合理选择
  （出问题时干净退出好过留下半死状态），但意味着**任何 panic 都不可恢复**，
  所以修错路径必须返回 `Result` 而不是 `unwrap`。

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
│  ├─ src/telemetry/         # 遥测域（core 采样纯逻辑 + hub 运行时线程）
│  ├─ src/net/               # 网络工具域（client/speed/history/dl 四件）
│  ├─ src/netcheck/          # 15 项网络诊断 + verdict 综合判定（mod/probe/checks）
│  ├─ src/snapshot.rs        # 网卡/路由/服务/WLAN 快照（PowerShell JSON + 原生解析）
│  ├─ src/repairs/           # 修复域（类型+注册表+三批执行器+backup/restore）
│  ├─ src/w32/               # 唯一 FFI 出口（input/clipboard/crypto/process/sysui）
│  ├─ src/vault/             # 密码库域（crypto/store/pwgen）
│  ├─ src/appnet/            # 应用网络域（firewall 防火墙 + traffic ETW 流量）
│  ├─ src/disk/              # 磁盘域（overview 空间 + usage 占用分析）
│  ├─ src/paste_guard.rs     # 粘贴守卫链（焦点守卫 + SendInput 编排）
│  ├─ src/jsonio.rs          # 原子 JSON 读写（tmp+replace 纪律）
│  ├─ src/commands/          # invoke 接口（mod + clipboard_cmds/diag/network/palette/toolbox/vault/settings_cmds/system 八域）
│  └─ capabilities/default.json  # 窗口能力（前端 show/hide/listen 必需）
└─ check.ps1       # 门禁一条命令
```

## 进程执行层的一条硬教训（M3 踩到）

`CreateProcessW` 有两个坑，都表现为「进程正常退出但零输出」，极难查：

1. **给了 `lpApplicationName` 就不再搜 PATH** —— 裸名 `cmd`/`ipconfig`/`netsh`
   全部报「系统找不到指定的文件」。必须自己做 PATH + PATHEXT 解析
   （`w32::resolve_executable`，对应 Python subprocess 的既有行为）。
2. **句柄继承**：`STARTF_USESTDHANDLES` 要求句柄可继承，手工
   `CreateFileW` + `SetHandleInformation` 也必须配合正确的
   `bInheritHandles`/`STARTUPINFO` 组合，否则子进程拿到无效句柄、
   写不进文件。

最终改用 `std::process::Command`（std 已正确处理这两件事），只用它的
`creation_flags(CREATE_NO_WINDOW)` 压黑窗；超时用 `try_wait` 轮询 + `kill`，
不引额外依赖。**结论：能交给 std 就不要手写 CreateProcess。**


## M2 真机复核清单（数据正确性）

M2 的自动化测试已锁住**格式与聚合口径**（与 Python 逐值比对），但「真机上数字
对不对」只能人工看。启动后（先退出正在运行的 SysKit 实例）：

| # | 靶 | 通过标准 |
|---|-----|---------|
| 1 | 速率合理性 | 概览页下行/上行随实际流量变化；**空闲时接近 0**（若恒为 0 或恒为某固定值，说明基线算错） |
| 2 | 虚拟网卡不计入 | 用 Clash/VPN/Hyper-V 时，速率不应把隧道或虚拟交换机的转发量重复计入 |
| 3 | CPU/内存 | 与任务管理器数值同量级（允许 1-2 秒采样相位差） |
| 4 | 托盘速率 | tooltip 每秒刷新速率；**空闲时不闪**（去重生效） |
| 5 | 多粒度切换 | 近 60 分钟/48 小时/30 天/12 月四个 Tab 都有柱子；切粒度不报错（2026-10-11 起速率时序图已改为连续平滑曲线） |
| 6 | 跨版本数据 | 先跑 Python 版产生流量 → 退出 → 跑 Rust 版：**柱状图能看到 Python 版写下的桶**（这是共用文件的硬证明） |
| 7 | 日报 | 事件时间线在无事件时显示空态文案；有事件时按「时间 · 状态 · 说明」列出 |
| 8 | 落盘 | 运行 ≥60 秒后退出，`%APPDATA%\SysKit\netstats_buckets.json` 的 mtime 已更新且 JSON 合法 |

> 靶 6 是 M2 最有价值的一条：它同时验证 `traffic.json` / `events.json` /
> `netstats_buckets.json` 三份文件的跨版本兼容。


### 已知缺口（诚实清单，尚未实现）

以下是**逐键审计后确认存在、目前没做**的能力。列出来是因为"没做"必须让使用者知道，
否则会被当成"应该能用却坏了"。**设置页里未生效的项已加「暂未生效」徽标并在页顶汇总**
（`settings.rs::UNWIRED` + `tests` 锁住清单），所以用户改了没效果时能立刻看到原因。

**⚠ 已闭环：「自动诊断/修复」整组功能已移植**（`monitor.rs`，对应 Python `monitor.py`）。
六项设置（`auto_repair_enabled` / `poll_interval_sec` / `auto_repair_max_level` /
`fail_threshold` / `cooldown_sec` / `level3_cooldown_sec`）**已接线并被消费**，
设置页不再标「暂未生效」。

保守梯度：L1 `flush_dns` → L2 +`start_nlasvc`/`renew_dhcp` → L3
+`restart_wlan`/`toggle_adapter`。后三项是"改配置/断网"类，**这里是唯一自动执行
它们的路径**，靠三道闸约束：L3 限频 + 通用冷却 + **VPN 护栏**；其余改配置类动作
只给建议、绝不自动执行。设置页新增「后台监控」分页，显示状态、失败连击、已执行
次数，并**预览当前梯度**（含被 VPN 跳过项）——让用户在动网卡之前就看到会发生什么。

三处实测细节（照搬 Python）：**先探连通后抓快照**（快照是全量 PowerShell 采集
4~6 秒，只在探测失败时才有意义）；**无活动物理网卡时失败计数归零**（纯虚拟网卡
场景"探测失败"≠断网）；**L3 时间戳用单调时钟**（写入 epoch 秒会让差值为巨负、
L3 冷却永久命中，自动修复被静默降级到 L2）。

顺带修正一处**安全相关的不一致**：Rust 的 `is_tunnel` 关键字比 Python 的
`TUNNEL_PAT` 少了 11 个产品（少了 uu加速/hamachi/zerotier/tailscale/oray/pgyer/
easyconnect/sangfor/nekoray/hysteria/utun 等）。VPN 护栏依赖它——**漏检一个产品，
用户挂着 VPN 就会被自动修复踢下线**。已补齐到 23 个关键字并加断言（含"普通网卡
不得误判"的反向用例）。

其余单项（仍未做）：
1. ~~二维码渲染未内置~~ ✅ 已闭环：`qrcode` crate（纯 Rust）本地渲染 SVG（纠错级别 M，
   对齐 Python `qrcode.make` 默认），前端展示 + 导出 PNG（`clip_qr.png`，canvas 白底
   转换）。**不调用第三方二维码服务**——复制的链接不出本机；外圈静区取 4 模块
   （Python 版 border=2 偏紧，4 是扫码可靠性推荐值，只影响留白不影响内容）。
2. **IP 归属地查询不代查**：同上，避免静默把你的 IP 发给第三方服务。
3. ~~`minimize_to_tray`~~ ✅ 已接线且**真机复核通过**（2026-10-04 02:37：起真实实例→主窗发 WM_CLOSE→进程存活+日志出现「窗口关闭：按设置最小化到托盘」；真退出仍走托盘菜单-退出，`app.exit` 不触发关闭请求，两路径互不干扰）。
4. ~~`sound_alert_enabled`~~ ✅ 已接线：状态切换发声（winmm PlaySound 别名异步，ok=SystemAsterisk/其余=SystemExclamation，首态与同态不响；测试运行静默）。**需真机复核**。
5. ~~`vault_autolock_min` / `vault_trust_min`~~ ✅ 已随 M6 数据层接线（会话超窗自动收锁 / 验证后续期窗口）。

**✅ 已闭环：敏感内容剪贴板 + WiFi 密码查看**（`sensitive.rs` / `wifi.rs`）。
新增系统助手页「WiFi 密码」分页：枚举已保存配置、读取明文密钥（需管理员）、
复制。密码类内容**只能**经 `sk_clip_copy_sensitive` 进剪贴板。

**复制密码要堵三处泄露面，少堵一处等于没堵**：
1. `mark_sensitive` —— 本应用历史在 TTL 内不记录该文本；
2. `exclude_os_history` —— 不进 Win+V 历史、不上传云剪贴板（KeePass 同款三个
   注册格式）。**只做第 1 条的话密码照样进系统历史并可能同步到别的设备**；
3. 到期清空剪贴板 —— 否则下一个读剪贴板的程序就拿到了。

**自动清空的关键语义**：只在剪贴板内容**仍是当初那段敏感文本**时才清。用户复制
密码后往往马上复制别的东西，无条件清空会把用户刚要用的新内容也抹掉——那是**数据
丢失**，比"密码多留一会儿"严重得多。有专门用例锁住这条（`real_clipboard_keeps_
newer_content_past_deadline`）。

`clip_sensitive_ttl` 因此接线：**0 表示"不清空"（用户显式关闭），不是"立即清空"**；
配置可被手工编辑，TTL 再夹一次到 3600。

WiFi 密钥解析**中英文都认**（netsh 输出随系统语言变化，只认一种会在另一种语言下
静默返回空密码——用户看到"密码是空的"却不知道是解析失败）；按**第一个**冒号切分
（密码本身可能含冒号，按最后一个切会截断）。

**真机验证**：写入敏感串 → 装一个已到期槽位 → `tick_clear()` 报告已清空 →
读回剪贴板确认内容**真的没了**。（单测里判对了但清空失败，用户看到的仍是"密码
一直留在剪贴板"，所以这一步必须真机确认。）

**✅ 已闭环：内容识别式敏感检测**（`sensrules.rs`）。复制的内容本身长得像凭据
（信用卡/身份证/API 密钥）时不进历史——与 `sensitive.rs`（到期清空）、
进程黑名单（来源侧）三套机制互不替代。误报控制是硬要求：信用卡必须过 **Luhn**、
身份证必须过 **GB 11643 校验位**、令牌类须带已知前缀或 ≥20 字符且**观测字符集
香农熵 ≥3.5 bits/char**（32 位英文句子熵约 2.2，不会命中）。

**热路径按字节扫描（2026-10 优化）**：检测器在**每次剪贴板变化**时跑一遍，
原实现里 `has_credit_card` 与 `has_id_number` 各对整串做一次
`chars().collect::<Vec<char>>()`——复制 1MB 文本就是两次百万级分配。三类命中
形态全是 ASCII，而非 ASCII 的 UTF-8 字节（≥0x80）天然起分隔符作用，因此改成
**字节窗口/字节取词**后与按 char 切分**等价**、且零分配。实测（O2，约 100 万字节
语料，判定结果逐例一致）：英文长文 11.10 → 3.83 ms（2.9x），数字噪声
42.74 → 29.78 ms（1.4x），中文长文本 10.56 → 9.31 ms（1.1x）。等价性由
`byte_scan_treats_non_ascii_as_separator` 锁住（非 ASCII 若被当成延续，两段 8 位
数字会拼成 16 位卡号而误命中）。

**✅ 已闭环：剪贴板图片捕获**（`dib.rs`）。此前只登记占位条目不搬运像素，现在走完整
路径：`CF_DIB`/`CF_DIBV5` → 解析 → RGBA → PNG → 落盘 → 入库，`clip_capture_images`
因此不再标「部分生效」。

`CF_DIB` 的每个变量都能让图像**静默错位**（不是崩溃，是看起来正常的错图），所以逐条断言：
- `biHeight` 负数表示自上而下、正数自下而上，搞反图就倒过来；
- 行按 **4 字节对齐**（`stride = ((bpp*width+31)/32)*4`），按 `width*bpp/8` 取行会逐行错位；
- 32 位 `BI_RGB` 是 **BGRA**，且第 4 字节常是"未使用"的 0——当 alpha 用会让**整张图全透明**；
- `BI_BITFIELDS`/`BI_ALPHABITFIELDS` 在头后紧跟 3~4 个 DWORD 掩码，起点要相应后移；
- 索引色（1/4/8 位）查调色板（BGRX 四字节），1 位图**高位在前**；
- 头部声明的尺寸可能极大 → **分配前**拒绝（防 OOM），像素数据不足则报错而非补零
  （补零会得到"有一半是黑"的错图）。

**真机验证**：用脚本把一个已知的 4×2 位图（底行红、顶行蓝）放进系统剪贴板，
Rust 侧读出 156 字节（系统做了填充）、解析出 4×2、顶行 `[0,0,255]`、底行 `[255,0,0]`
——与放进去的完全一致，行序与 BGRA→RGBA 转换都正确。

**✅ 已闭环：剪贴板内容规则动作**（`cliprules.rs`，对应 Python `cliprules.py`）。
识别 JSON / URL / 颜色 / Windows 路径 / IP / 域名并给出动作；系统助手页的剪贴板历史
每条都有「动作」按钮，点开显示可用操作（打开链接、打开文件/定位、格式化 JSON 回写、
ping、DNS 解析、色块）。两处真机细节照搬 Python：
- **URL 必须排除 CJK 与全角标点**：中文语境里链接后常紧跟句号或全角括号，
  不排除就会把标点抓进 URL，二维码/打开链接拿到错误地址；
- **`exists()` 之前先做盘型预检**：失效网络盘上的 `exists()` 会阻塞数秒到数十秒，
  而 `detect()` 在 UI 线程被调用（选中历史即触发）——一次卡顿就是界面冻结。
  只对本地盘型（可移动/固定/光驱/RAM）探测。

已接线（供对照）：`tray_rate_enabled`、`dark_mode`、`dns_preset`、`clipboard_max`、
`clipboard_watch`、`clipboard_retain_days`、`clip_process_blacklist`、
`clip_capture_images`（部分）、三个热键、`palette_commands`、**六项 auto_repair_***
（本轮接入 monitor.rs）。

> 标注纪律：`settings.rs` 里的 `UNWIRED` 常量是**手工审计的清单**，同时是一份待办。
> 接上消费方后从清单删掉该项；删掉时 `unwired_keys_are_marked_and_explained` 会失败
> ——这正是提醒我们摘掉过期标签的机制。

### M1 真机复核清单（GO/NO-GO 判定，必须人工执行）
前置：**先退出托盘里正在运行的 SysKit**（Python 版已退役；数据目录共用 `%APPDATA%\SysKit`
且都会注册同名全局热键，同时运行必然互相抢键、并可能并发写同一份
`clipboard.json`）。然后 `npm run build` + `cargo run --manifest-path src-tauri/Cargo.toml`。

| # | 靶 | 通过标准 |
|---|-----|---------|
| 1 | 三热键注册 | Ctrl+Alt+K / Ctrl+Alt+V / Ctrl+Win+V 各呼出对应面板；长按不重复触发（NOREPEAT）；日志无「注册失败」 |
| 2 | 被占回退 | 先让别的程序占用 Ctrl+Alt+K，启动后该键应静默降级（无崩溃、托盘菜单仍可呼出面板） |
| 3 | 快捷粘贴闭环 | 在记事本复制文本 → 别处复制新内容 → 呼出面板选旧条目 → 回车，**内容应直接粘进记事本**（前置：设置里开启「剪贴板监听」，或已在系统助手页勾选） |
| 4 | 焦点漂移 | 呼出面板后先点别的窗口再按回车 → 提示「目标窗口已切换，已取消自动粘贴」，且内容仍在剪贴板 |
| 5 | Word/Excel 富文本 | 复制带格式网页内容 → 粘贴到 Word/Excel，格式与纯文本兜底都正常（CF_HTML 偏移正确性） |
| 6 | 文件条目 | 在资源管理器复制文件 → 面板选中 → 回车，**粘出的是文件本体**而不是路径文字（CF_HDROP） |
| 7 | 图片条目 | 粘贴一张截图 → 面板显示 `[图片 W×H]` → 选中回车，目标窗口收到图片（CF_DIB 方向/位序） |
| 8 | UIPI 降级 | 对**以管理员运行**的目标（如管理员记事本）粘贴：应提示「自动粘贴未送达，请按 Ctrl+V」而**不是静默无反应** |

另需目视：托盘图标为状态色圆点、tooltip 正确；两个面板无边框、置顶、圆角；
失焦即收起；Esc 收起；数字键 1-9 直选与双击都能粘贴。

## 主题单源纪律

32 个语义 token 的唯一事实源是 **`src/theme.ts`**（`LIGHT`/`DARK`）；
`tests/theme.parity.test.ts` 与**冻结快照**逐值比对锁定——快照值 =
Python 版退役（tag `python-final`）时与 `syskit/ui/theme.py` 逐值核对过的终态。
改色必须有意识地更新快照并在提交说明里写明理由，否则测试红。

同类纪律**同样适用于设置表**：默认值/键名的唯一事实源是
`src-tauri/src/settings.rs::defaults()`，`tests/settings.parity.test.ts`
解析该函数源码、与冻结快照比对（键集、类型、字面量三者都不许分叉；
快照值同为 python-final 对齐终态）。

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


## Step 5 切换 runbook（已完成）

前置：真机冒烟 12 项全绿（`tools/test_smoke_real_rs.py --run-real`）。

1. **产物**：`npx tauri build` → `src-tauri/target/release/SysKit.exe` +
   `bundle/nsis/SysKit_1.7.4_x64-setup.exe`（安装向导，含卸载信息）。
2. **数据零迁移**：Rust 版与 Python 版读写同一 `%APPDATA%\SysKit`（便携模式同
   `<安装目录>\data`）——settings/clipboard.json/traffic/events/netstats_buckets/
   vault.db+key 全部格式逐字节对齐（各有奇偶校验或双向 fixture 锁定）。
3. **安装**：退出一切 SysKit 实例 → 跑 NSIS 向导（或绿色版直接放置 exe）。
   一键换装：`tools\deploy_swap_rs.bat`（守卫双实例/备份链/字节校验）；
   加 `with-data` 参数把 `%APPDATA%\SysKit` 的数据携入便携目录（vault db+key
   成对拷、双方先备份）。
4. **退役（已完成，2026-10）**：Python 版源码已从工作区删除，
   完整快照在 **git tag `python-final`**——规格对照走
   `git show python-final`。UNATTENDED.md 已改写为 Rust 侧口径。
