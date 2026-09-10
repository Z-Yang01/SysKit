# SysKit 诊断记录 — 工作区脏，跳过修复

## 状态
跳过（工作区脏，按任务规则只报错不修改，未做任何源码改动）

## 环境
- 路径：E:/Code/project/SysKit
- git：分支 `test/pytest-setup`，HEAD `bfafc930ddbebe001a38ed243fc4ff745a9e4ee5`
- 工作区：脏（30+ 已修改跟踪文件 + 多个未跟踪文件），故不建 fix 分支、不改代码

## 诊断表
| 编号 | 严重度 | 文件:行 | 症状 | 证据 |
|------|--------|---------|------|------|
| S-1 | P2 | syskit/clipboard.py:39-41（`_save`） | 剪贴板历史落盘失败被 `except OSError: pass` 静默吞掉，历史数据无提示丢失 | `grep -rn -A2 "except[^:]*:$" syskit/` 命中；上下文为原子写(tmp→replace)后直接 pass |
| S-2 | P2 | syskit/ui/toolbox_page.py:68-70（下载线程） | 工具箱下载循环整体 `except Exception: pass`，网络中断时进度静默停止、无失败反馈 | 同上 grep 命中；read 循环外层兜底吞掉全部异常 |
| S-3 | P3 | syskit/settings.py:22-23 | 配置文件损坏时静默回退默认值，无日志可排查 | grep 命中，`load_settings` 外层 except-pass |
| S-4 | P3 | syskit/ui/main_window.py:167-169 | 窗口几何/最后页面保存失败静默 | grep 命中，`settings.save(...)` 外层 except-pass |
| S-5 | P3 | syskit/ui/app.py:52-54 | Qt 消息 handler 内 LOGGER.debug 失败被吞 | grep 命中 |
| S-6 | P3 | syskit/__init__.py:17-18, 30-31 | stdio 重定向引导失败静默（best-effort，可接受） | grep 命中 |
| S-7 | P3 | syskit/ui/toolbox_page.py:356-358 | traceroute 单行解析失败静默（逐行容错，可接受） | grep 命中 |
| S-8 | P3 | syskit/ui/main_window.py:877-879 | winsound 播放失败静默（非 Windows 兜底，可接受） | grep 命中 |

未发现 P0/P1：无裸 `except:`、无 shell=True、无 SQL 字符串拼接、无硬编码密钥、无可变默认参数。

## 证据（关键命令与结果）
- `python -m pytest` → `527 passed, 1 skipped, 1 xfailed in 41.28s`，退出码 0（无测试失败）
- `python -m compileall -q syskit tools tests launcher.py` → 退出码 0（无语法错误）
- `grep -rn "shell=True" syskit/ tools/` → 无命中
- `grep -rn -E "execute\((f['\"]|...%)" syskit/` → 无命中
- `grep -rni -E "(password|secret|api_key|token)\s*=\s*['\"]..." syskit/ tools/` → 无命中
- 工具链：Python 3.10.11；ruff/pip-audit 未安装（不新增依赖，跳过）

## 根因
不适用（未修复）。S-1/S-2 属吞异常导致失败不可见，建议后续在对应 except 分支补 LOGGER.warning 并向 UI 发失败信号。

## 修改
无（工作区脏，规则禁止修改）。

## 验证
见上「证据」；无修复故无回归验证。

> **2026-09-10 复核**（四项目批量修复任务）：工作区仍脏（37 项），分支不变；
> 重跑 `python -m pytest` → **零失败**（仅预期 xfail×1 / skip×1），上述诊断结论维持成立。

## 风险与回滚
- 风险等级：无（零改动）
- 回滚命令：不适用
