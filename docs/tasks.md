# 任务看板（Task Board）

> 将 [`plan.md`](./plan.md) 的里程碑拆解为可追踪的 issue 清单。
> 状态约定：🔲 待办 / 🔧 进行中 / ✅ 已完成 / 🚫 阻塞。
> 最后更新：2026-08-05

## 阶段一：质量与一致性加固（M1）

| # | 任务 | 验收 | 状态 |
|---|---|---|---|
| T1.1 | 文档-代码一致性巡检（能力名 `diagnosis.*`、接口路径、返回结构、环境变量） | README/usage/development/deployment 与 `routing.yaml` `config.py` `main.py` 零偏差 | ✅ 已完成（2026-08-05） |
| T1.2 | `scripts/run_e2e.ps1` 接入 CI（目前 CI 已含 backend/e2e/e2e-docker/frontend/contract/rrserver-dryrun，脚本为本地等效一键） | 本地 `pwsh run_e2e.ps1` 与 CI 用例集合一致 | ✅ 已具备 |
| T1.3 | 前端 `pages/skills` 在 `rule` 与 `llm` 模式下联调 | 新增 `tests/test_skill_routing_modes.py`，4 用例覆盖两种路由下列表/装载/卸载/错误分支，全部通过 | ✅ 已完成（2026-08-05） |
| T1.4 | `/api/consultations/{id}/trace` 增加 Token 用量与降级原因标注 | trace 每条含 `tokens`/`degraded`/`degraded_reason`；新增 `tests/test_trace_observability.py`（rule 无降级+llm 无 Key 运行时降级两类场景），全部通过 | ✅ 已完成（2026-08-05） |
| T1.5 | 补充 `tcm-safety` 红旗分级与就诊科室映射判定单测 | 新增 `tests/test_safety_redflag.py`：urgent/warning 分级、科室映射、模糊匹配、未命中兜底、经注册表运行、安全 Agent 多红旗扫描，共 11 用例全部通过 | ✅ 已完成（2026-08-05） |

## 阶段二：诊疗能力深化（M2）

| # | 任务 | 验收 | 状态 |
|---|---|---|---|
| T2.1 | 扩展 `llm_server` RAG 中医典籍/方剂语料 | `tcm-rag` 检索召回质量评估通过样例集 | 🔲 待办 |
| T2.2 | `diagnosis.differentiation` 支持兼证与支持/矛盾证据链 | 报告含多证候并存与证据来源标注 | 🔲 待办 |
| T2.3 | 沉淀「如何写自己的技能」示例 + 校验脚本 | `skills/` 下 example 技能 + `validate_skill.py` | 🔲 待办 |
| T2.4 | 安全 Sub-Agent 规则兜底与 LLM 叠加去重 | 重复红旗只报一次，分级正确 | 🔲 待办 |

## 阶段三：生产化与多端上线（M3）

| # | 任务 | 验收 | 状态 |
|---|---|---|---|
| T3.1 | 对象存储抽象 `StorageBackend` + 接入上传接口 | 本地/OSS/S3 可切换，契约不变 | 🔲 待办（依赖代码重构） |
| T3.2 | 图片上传对象存储集成测试 | `tests/test_object_storage.py` 用 fake 后端覆盖 | 🔲 待办（依赖 T3.1） |
| T3.3 | `MemoryStore`/`RedisStore` → 增加 PostgreSQL 归档实现 | `store.py` 新增 `PostgresStore`，`_build_store` 分支 | 🔲 待办 |
| T3.4 | 前端 H5 上 CDN + 微信小程序发布流程 | 构建产物可部署，小程序过审 | 🔲 待办 |
| T3.5 | 合规审计：免责强制展示、红旗路径不可移除、日志脱敏 | 安全评审单通过 | 🔲 待办 |

## 阶段四：家庭算力云化（M4，可并行）

| # | 任务 | 验收 | 状态 |
|---|---|---|---|
| T4.1 | `rrserver` 生产化：真实 TLS、强 token/随机 name、外部告警 | 经公网隧道稳定可用 | 🔲 待办 |
| T4.2 | 多隧道/多模型：文本 + 视觉分隧道路由 | backend 按 capability 自动选路 | 🔲 待办 |

## 依赖关系

```
T3.1 ──> T3.2
T1.4 （可并行，无依赖）
T2.x  （不依赖阶段三，可提前）
T4.x  （独立，可随时启动）
```

## 如何更新本表

- 完成某项：把状态改为 ✅ 并注明完成日期（如上方 T1.1/T1.2）。
- 新增 issue：按阶段插入行，编号 `T<阶段>.<序号>`。
- 阻塞项：标 🚫 并在「验收」列简述阻塞原因。

## 测试工程改进（2026-08-05）

完成 T1.5 时发现并修复了全量回归中 4 个 flaky 用例（test_store / listening / palpation /
trace：单文件运行通过、全量顺序下失败）。**真正的根因不是 pytest 双加载，而是测试对全局
状态的破坏性重建**：

- **根因**：`tests/test_skill_routing_modes.py` 的 `_make_client` 通过
  `del sys.modules`（删除所有 `app.*` 模块）+ 设置 `TCM_ROUTING_FILE` env 后重新
  `import app.main` 来切换路由。这种「重建 app」操作会触发 `app.main` 顶层的
  `discover_skills()` 重复装载 skills，并让 `app.agents.listening` 等 agent 模块被
  **重加载成全新对象**。后续用例在收集阶段已 `from app.agents.listening import
  ListeningLLMAgent` 绑定到**旧模块**，而运行时 `sys.modules["app.agents.listening"]`
  已是新模块；`monkeypatch.setattr("app.agents.listening.get_provider", ...)`
  命中新模块，旧模块的 `handle` 仍用未被 patch 的 `get_provider`，注入失效 →
  测试走向真实/默认 provider，evidences 为空或降级缺失。单文件运行时无重加载，故通过。
- **修复**：`test_skill_routing_modes.py` 不再删除任何 `app.*` 模块，改为直接
  `monkeypatch.setattr(settings, "routing"/"llm", ...)` 切换路由字典，复用已导入的
  `app.main.app`（路由读 `settings.route_of` 为动态方法，切 settings 即生效）。彻底消除
  模块重加载与 `discover_skills` 重复装载带来的全局污染。
- **配套加固**：
  - `get_provider()` 移除模块级 `_provider` 全局缓存，每次依据当前 `settings` 创建
    provider，避免 API Key 跨测试泄漏。
  - `tests/conftest.py` 增加 autouse fixture 在每个测试后还原
    `config.settings.routing`/`llm` 与 API Key 相关环境变量；另设 autouse fixture 浅
    拷贝还原全局 `skill_registry` 的 `_skills`/`_tools`，防止 skills 残留。
  - `test_store.py` 的 `isinstance` 断言改为同模块类型名比较，规避类身份不一致。
- **验证**：`cd backend && python -m pytest` 全量约 184 用例已全部通过。

> 运行全量测试：`cd backend && python -m pytest`。`pytest.ini` 的 `addopts` 已含
> `--import-mode=importlib`（避免 `prepend` 模式下的解析歧义）。
