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
- [ ] 阶段二 A：netcheck 15 项检查逻辑（并行子 agent）
- [ ] 阶段二 B：repairs 风险分级与命令构造（并行子 agent）
- [ ] 阶段二 C：DNS/子网计算纯函数（并行子 agent）
- [ ] 阶段二 D：UI offscreen 冒烟（并行子 agent）
- [ ] 覆盖率验证 ≥70% + 补缺口 + 晨报

## 红线执行情况

- 红线 1（禁真实修复动作）：conftest 哨兵强制执行，test_smoke 有专门验证用例 ✅
- 红线 2（禁碰真实数据目录）：autouse 重定向，test_smoke 有专门验证用例 ✅
- 红线 3（不改业务代码/不打包/不 push）：目前零业务代码改动；后续如发现疑似 bug 只记录到下表

## 疑似 bug 备忘（只记录不修改）

（暂无）
