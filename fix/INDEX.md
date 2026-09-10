# fix/INDEX — 四项目批量找错修复（2026-09-10）

任务：依次处理 mediaflow / Z-agent / PixYang / SysKit，按 P0→P3 找错修复；
git 仓库建分支 `fix/auto-2026-09-10`，工作区脏则只报错不改；每错独立 FIX 编号
（改前可回滚、改后验证、证据 file:行）。

## 项目总表

| # | 项目 | 实际路径 | 状态 | 结果 |
|---|------|----------|------|------|
| 1 | mediaflow | WSL Ubuntu `/home/boyu/code/project/mediaflow` | ✅ 已修复 | 干净仓库 → 建分支 fix/auto-2026-09-10，**11 个 FIX 全部提交并验证**；全量回归 293 passed+13 failed → **306 passed / 0 failed** |
| 2 | Z-agent | WSL Ubuntu `/home/boyu/code/project/Z-agent` | ⏭️ 跳过 | 工作区脏（`M backend/.coverage`）→ 只诊断：3 条 P1/P2 实锤 + 3 例 redis 环境性失败复验。见 `2026-09-10/SKIP-Z-agent-工作区脏.md` |
| 3 | PixYang | `E:/Code/react/PixYang`（/mnt/e 回退） | ⏭️ 跳过 | 工作区脏（12 改+8 新增）→ 只诊断：vitest **183 passed / 0 failed**、P3 扫描干净，无可修项。见 `2026-09-10/SKIP-PixYang-工作区脏.md` |
| 4 | SysKit | `E:/Code/project/SysKit`（/mnt/e/projec 拼写回退） | ⏭️ 跳过 | 工作区脏（37 项）→ 沿用本日既存诊断记录并复核：pytest 零失败，结论成立。见 `2026-09-10/SKIP-工作区脏.md` |

> 路径备注：四个路径均以 Linux/WSL 形式给出。`/home/boyu/...` 两项在 Windows 侧不存在，
> 但实测存在于 WSL Ubuntu 发行版内（经 `wsl.exe -d Ubuntu` 访问），故按真实位置处理而非跳过；
> `/mnt/e/...` 按 WSL→Windows 映射回退到 `E:/...`（SysKit 原路径 `projec` 为拼写残缺，回退 `project` 命中）。

## mediaflow 修复明细（FIX-001..011，分支 fix/auto-2026-09-10，基线 901decb）

| 编号 | 严重度 | 类型 | 位置 | 一句话 | 提交 | 记录 |
|------|--------|------|------|--------|------|------|
| FIX-001 | P1 | app | blog/service.py:278-305 | TagService dict 缺必填 created_at → /tags、/tags/suggest 有数据即 500 | 599e7df | FIX-001.md |
| FIX-002 | P1 | app | rag/repository.py:157,162 | `:embedding::vector` 被 text() 吞冒号 → asyncpg 语法错误，检索/answer 全 500 | cc0eb69 | FIX-002.md |
| FIX-003 | P1 | app | rag/chunker.py:91-92 | 递归兜底返回 list[str] 违反 ChunkData 契约 → AttributeError | 9f69128 | FIX-003.md |
| FIX-004 | P2 | app | rag/chunker.py:137 | 切分游标不保证前进 → 死循环/OOM 7.4GB（疑似bug#3 实锤） | 9dd63bc | FIX-004.md |
| FIX-005 | P1 | app | comment/api.py:38,50,109 | 三个列表端点漏 await → 响应为 coroutine → 500 | d25a7f0 | FIX-005.md |
| FIX-006 | P1 | app | blog/api.py:559 | 用未导入的 Favorite 统计 → /collections NameError 500 | 541acff | FIX-006.md |
| FIX-007 | P1 | test | tests/test_engage_media.py:99,170 | 误用 ORM 类 `.c` 属性（应为 `__table__.c`） | 2a71c6f | FIX-007.md |
| FIX-008 | P1 | app | comment/service.py:149 | reject 成功返回 None 被判 404（删除副作用已发生） | d04e5e8 | FIX-008.md |
| FIX-009 | P2 | test | test_engage_like.py / test_engage_comment.py | 计数断言踩 identity map 旧对象 → 加 populate_existing | 4bdfdab | FIX-009.md |
| FIX-010 | P2 | app | document/api.py:105 | 非ASCII文件名回退名丢 basename（"截图.epub"→".epub"） | a15f799 | FIX-010.md |
| FIX-011 | P2 | test | test_mediaconv_document.py:105,121 | content-type 断言过严（Starlette 自动加 charset） | 8401a3d | FIX-011.md |

补丁：`2026-09-10/mediaflow/FIX-001.patch .. FIX-011.patch`（git format-patch，含提交信息）。
项目侧另按其 AGENTS.md 惯例在 `mediaflow/error/` 落了 8 篇说明文档（该目录 gitignore，仅本地）。

## 验证总账（mediaflow）

- P0：`compileall` exit 0；前端 `tsc --noEmit` exit 0（工作区保持零改动）
- P1：xfail 治理 2+9+1 例全部转正移除；失败 13 → **0**
- 全量：`pytest` → **306 passed in 170s**（基线：293 passed / 13 failed）
- 专项：test_ai_rag_unit.py 18 passed（修复前执行即 OOM）；chunker 死循环复现脚本由
  MemoryError → 57 chunks 正常返回

## P3 与依赖审计（mediaflow）
- except-pass 集中于 WebSocket 推送/协同容错与「忽略非法查询参数/启动期 best-effort 注册」，
  判定为可接受惯例，未改动（记录于各 FIX/SKIP 文档）
- 无裸 except、无硬编码密钥、无 shell=True
- 依赖审计：WSL 内 uv 版本不支持 `pip audit`、pip-audit/npm audit 未执行（不新增依赖原则）
  → 跳过并记录

## 环境操作备注（可复现）
- WSL Ubuntu 内 `docker` CLI 缺失（Docker Desktop 集成未开）→ 建了 `~/bin/docker` shim
  转发 `docker.exe`（仅供本次测试基础设施使用，conftest 起 pgvector 容器用）
- 测试复用已存在的 `mediaflow-pytest-pg-55433` 容器；WSL→容器端口 localhost 直达
- 未 push、未重构、未新增依赖、未改 API/schema/CI
