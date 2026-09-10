# Z-agent 诊断记录 — 工作区脏，跳过修复（只报错不改）

## 状态
跳过修复（按任务规则二：工作区脏 → 只报错不修改，未做任何源码改动）

## 环境
- 路径：WSL Ubuntu `/home/boyu/code/project/Z-agent`（Windows 侧不可见，经 wsl.exe 访问）
- git：分支 `test/coverage-push`，HEAD `e16ddb1`
- 工作区：**脏** —— `M backend/.coverage`（1 个已跟踪的覆盖率二进制产物）
  → 按规则不建 fix 分支、不改代码。若后续 `git checkout -- backend/.coverage`
  （或将其移出版本库）后可重新执行修复流程。

## 诊断表（P0→P3）

| 编号 | 严重度 | 位置 | 症状 | 证据 |
|------|--------|------|------|------|
| Z-1 | P1 | backend/app/modules/agents/api.py:130-138 vs service.py:564-572 | `POST /agents/{name}/run` 恒 500：端点以位置参数传 `data.task_mode`（第 7 参），而 `AgentService.run` 签名仅 6 参（无 task_mode）→ TypeError | PROGRESS.md「优先修复建议#1」+ 源码比对实锤；流式端点不受影响（`stream` 签名 service.py:233-243 **含** task_mode） |
| Z-2 | P2 | backend/app/modules/agents/schema.py:19 + api.py:150-157 | `confirm_required_tools` 请求字段存在、`service.stream` 也支持（service.py:242），但 agent_run/agent_stream 两端点都**未转发** → 工具确认白名单不生效 | PROGRESS.md「优先修复建议#2」+ grep 全库仅 service/schema 有该名，api.py 调用处无 |
| Z-3 | P2 | backend/app/modules/rag/api/answers.py:49 | 同步 answer 端点 `raise HTTPException(500, detail=str(e))`：AppError（404/400 语义）落成裸 500 且 `str(e)` 泄露内部信息（流式端点已统一文案） | PROGRESS.md「优先修复建议#3」+ 源码行实锤；search.py:54 同模式（400） |
| Z-4 | P1（环境） | backend/tests/test_rag_documents_api.py | 3 例失败：test_delete_ok / test_reindex_ok / test_batch_duplicate_skipped（:185 `assert False is True`），PROGRESS 判定为 WSL 内 localhost:6379 redis「监听但 reset」环境问题 | 本次复验：`pytest tests/test_rag_documents_api.py --no-cov` → **3 failed, 14 passed**（与记录一致） |
| Z-5 | P3 | backend/app/modules/chat/api.py:64-65、settings/service.py:59-60、agents/config_service.py:52-53 | 3 处 `except ...: pass` 吞异常（含两处宽泛 `except Exception`） | grep 扫描命中；量少，未逐一深查语义 |
| Z-6 | P3（环境） | docker 容器 zagent-searxng | 曾处 Restarting(127) 循环；诊断时已自愈（granian 正常监听 8080），日志有 `missing config file: limiter.toml` 警告与 X-Forwarded-For 告警 | docker.exe logs 实测 |

## 其他检查（通过）
- P0：`.venv/bin/python -m compileall -q app` → exit 0（无语法错误）
- P3：无裸 `except:`、无硬编码密钥候选、无 `shell=True`
- 测试全量：PROGRESS 记录 723 passed / 3 failed（本轮仅复验失败文件，见 Z-4；跑全量会再写
  `.coverage` 加剧脏区，故未跑）
- 依赖审计：uv 版本不支持 `pip audit`、pip-audit 未安装 → 按规则不新增依赖，跳过

## 根因与建议（不修改）
Z-1：run() 签名补 task_mode 形参，或端点改关键字传参；Z-2：两端点补转发
`confirm_required_tools=data.confirm_required_tools`；Z-3：注册 AppError 全局 exception
handler 并统一文案；三者均为小改动，待工作区干净后处理。

## 修改
无（工作区脏，规则禁止）。

## 风险与回滚
无（零改动）。
