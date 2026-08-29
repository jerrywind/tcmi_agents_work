# 文档索引（Documentation Index）

> 本目录按**职责拆分**文档，单一事实只在一处权威描述，其余位置**引用而非复述**。
> 找内容先看本表。

## 职责矩阵

| 文档 | 面向角色 | 回答的核心问题 | 权威内容（不重复，引用它） |
|---|---|---|---|
| `README.md`（仓库根） | 所有人 | 项目是什么、怎么跑起来、文档在哪 | 架构图、端口表、快速开始 |
| `usage.md` | 使用者 / API 接入方 | 前端怎么用？REST API 怎么集成？ | 端点契约、payload 字段、切真实 LLM |
| `deployment.md` | 运维 | 怎么部署四组件？端口/配置/网络？ | 端口映射、环境变量、compose 编排 |
| `development.md` | 开发者 | 本地怎么开发调试？常见坑？ | 目录结构、本地启动、FAQ |
| `agent-protocol.md` | 架构 / 扩展者 | Sub-Agent 的接口与注册方式？ | `Capability`、SubAgent trait、注册表 |
| `sub_agents.md` | Agent 开发者 | 7 个 Agent 各管什么、怎么改？ | 各 Agent 的规则层 + LLM 层实现 |
| `skills.md` | 技能开发者 | 9 个技能的入参与扩展方式？ | 技能清单、`Skill` 结构、工具调用流程 |
| `mcp.md` | 集成方 | MCP 怎么接进来、怎么暴露出去？ | client 挂载（`mcp_clients`）、Server 端 `/mcp` 工具表与错误约定 |
| `llm_server.md` | 网关运维 | llm_server 是什么、怎么跑？ | 网关架构、API、配置项 |
| `rag.md` | RAG 运维 | 可选 RAG 服务怎么用？典籍怎么检索？ | 三类向量检索、环境变量、HTTP API、**典籍全文检索与跑分**（T4.3） |
| `testing.md` | 测试 / CI | 各层测试怎么组织、怎么跑？ | 测试层级、命令、CI job |
| `e2e.md` | 测试 | 跨组件全链路怎么跑？ | e2e 套件结构、编排脚本、环境变量 |
| `cleanup.md` | 开发者 | 临时文件/日志怎么清理？ | 命名约定、清理命令、规则 |
| `plan.md` | 管理者 | 现状、下一步路线图、风险？ | 阶段目标、里程碑 |
| [`samples/`](./samples/README.md) | 验收 / 回归对照 | 真实 LLM 跑出来的是什么样？ | 连真实模型的端到端样例（输入/输出/耗时/工具调用） |
| `tasks.md` | 管理者 | 具体任务清单与进度？ | issue 看板 |

## 跨文档一致的「单一事实源」

以下内容**只在其权威文档定义一次**，其它文档引用，请勿各处复述：

1. **模型与降级事实** → [`llm_server.md`](./llm_server.md)：llm_server 是纯网关（不托管模型）；
   模型 `google/gemma-4-12b-qat`（文本+视觉共用）；LM Studio 默认 `:11223`；
   harness **无 MockProvider**。
2. **端口** → [`deployment.md`](./deployment.md)「端口与地址」表（harness 为 `8011`）。
3. **REST API 端点契约** → [`usage.md`](./usage.md) 第 2 节。
4. **Capability 标识（7 个无前缀 slug）** → [`agent-protocol.md`](./agent-protocol.md) 第 1 节。
5. **7 个 Sub-Agent 的实现细节** → [`sub_agents.md`](./sub_agents.md)。
6. **9 个技能的入参与归属** → [`skills.md`](./skills.md) 第 2 节。
7. **MCP（client 挂载 + server `/mcp` 工具表）** → [`mcp.md`](./mcp.md)。
8. **后端架构与 YAML 资源分离** → 根 [`README.md`](../README.md) 与
   [`development.md`](./development.md) 第 2 节。

## 阅读路径

- **我想跑起来看看**：根 `README.md` 第 2 节 → `deployment.md` → `usage.md`
- **我要改中医数据（不改代码）**：根 `README.md`「流程与数据分离」→ `sub_agents.md`「维护入口」
- **我要改 Agent / 加能力**：`agent-protocol.md` → `sub_agents.md` → `skills.md`
- **我要接外部系统**：`mcp.md`（先看「现状一句话」：client 与 server 两个方向都已通）
- **我要保证质量**：`testing.md` + `e2e.md` + `cleanup.md`
- **我要做规划**：`plan.md` + `tasks.md`

## 维护约定

- 新增文档请在本表登记，并声明其「权威内容」，避免与既有文档重复。
- 文档不得引用已删除的路径；若代码结构变化，请同步更新本表与对应权威文档。
