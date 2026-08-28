# 文档索引（Documentation Index）

> 本目录按**职责拆分**文档，单一事实只在一处权威描述，其余位置**引用而非复述**，
> 以减少重复与漂移。找内容先看本表。

## 职责矩阵

| 文档 | 面向角色 | 回答的核心问题 | 权威事实（不重复，引用它） |
|---|---|---|---|
| `README.md`（仓库根） | 所有人 | 项目是什么、怎么跑起来、文档在哪 | 架构图、端口、快速开始 |
| `usage.md` | 终端用户 / API 接入方 | 怎么用前端完成问诊？怎么用 REST API 集成？ | 前端流程、API 端点、切换真实 LLM |
| `deployment.md` | 运维 | 怎么部署四组件？端口/配置/网络？ | 部署清单、端口映射、环境变量 |
| `development.md` | 开发者 | 本地怎么开发调试？常见坑？ | 本地环境、调试、FAQ |
| `agent-protocol.md` | 架构 / 扩展者 | Sub-Agent 怎么做到可替换？信封/路由？ | capability、Envelope、注册路由 |
| `sub_agents.md` | Agent 开发者 | 7 个 Sub-Agent 各管什么、prompt、技能？ | 各 agent 职责、技能绑定 |
| `skills.md` | 技能开发者 | 怎么写/装载一个 SKILL 工具？ | SKILL 契约、内置技能、热装载 |
| `mcp.md` | 集成方 | 怎么双向接 MCP（暴露能力/接入外部）？ | MCP Server/Client 设计、端点 |
| `llm_server.md` | LLM 网关运维 | llm_server 网关是什么、怎么跑？ | 网关架构、API、配置、**模型与降级事实** |
| `rag.md` | RAG 运维 | 可选 RAG 服务怎么用？ | 文本/图像/图文检索 |
| `testing.md` | 测试 / CI | 单元/集成/E2E 怎么组织、覆盖率？ | 测试体系、CI job |
| `e2e.md` | 测试 | 全链路（前端→后端→rrserver→llm_server）怎么跑？ | 全链路 e2e 套件、编排脚本 |
| `cleanup.md` | 开发者 | 临时文件/日志怎么命名与清理？ | 命名约定、清理脚本 |
| `plan.md` | 管理者 | 现状、路线图、风险？ | 里程碑、风险 |
| `tasks.md` | 管理者 | 任务清单与进度？ | issue 看板 |

## 跨文档一致的“单一事实源”

以下内容**只在其权威文档定义一次**，其它文档引用，请勿在各处各自复述：

1. **模型与降级事实** → 权威在 [`llm_server.md`](./llm_server.md)：
   - `llm_server` 是纯 LM Studio 网关，**不托管模型**；
   - 模型统一 `google/gemma-4-12b-qat`（文本+视觉共用，原生多模态）；
   - LM Studio 默认 `http://localhost:11223/v1`；无上游时 llm_server `degraded`/`503`，
     **harness 无 MockProvider**：`/chat` 会报错，只读端点（`/health`、`/agents`、`/skills`）仍可用。
2. **端口** → 权威在 [`deployment.md`](./deployment.md) 的「端口与地址」表（harness 为 `8011`）。
3. **REST API 端点** → 权威在 [`usage.md`](./usage.md) 第 2 节（接入方）。
4. **能力名 `diagnosis.*` / `treatment.plan`** → 权威在 [`agent-protocol.md`](./agent-protocol.md)。
   harness 的 `Capability` 枚举为 `inspection/listening/inquiry/palpation/differentiation/safety/treatment`。
5. **7 个 Sub-Agent 实现类与路由** → 权威在 [`sub_agents.md`](./sub_agents.md)。
6. **SKILL 工具清单（tcm-*）** → 权威在 [`skills.md`](./skills.md)（harness 内置 9 个，编译期注册）。
7. **后端架构（Rust harness + YAML 资源分离）** → 权威在根 [`README.md`](../README.md)
   与 [`deployment.md`](./deployment.md) 第 3 节。

## 阅读路径建议

- **我想跑起来看看**：`README.md` 第 2 节 → `docs/deployment.md` → `docs/usage.md`
- **我要改 Agent / 加能力**：`agent-protocol.md` → `sub_agents.md` → `skills.md`
- **我要接外部系统**：`mcp.md`（双向）
- **我要保证质量**：`testing.md` + `e2e.md` + `cleanup.md`
- **我要做规划**：`plan.md` + `tasks.md`
