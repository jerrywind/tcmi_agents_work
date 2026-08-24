# 文档中心

本目录包含中医智能问诊 Agent 的全部文档：

| 文档 | 读者 | 内容 |
|---|---|---|
| [开发文档](./development.md) | 开发者 | 架构、目录、如何新增/切换 Sub-Agent、测试 |
| [部署文档](./deployment.md) | 运维/SRE | 前后端部署、环境变量、Docker、反向代理、生产注意 |
| [使用文档](./usage.md) | 使用者/接入方 | 产品流程、API 调用示例、Postman/OpenAPI、LLM 配置、免责声明 |
| [Sub-Agent 协议规范](./agent-protocol.md) | 架构/集成 | 统一信封、能力标识、注册路由、可替换协议 |
| [Sub-Agent 设计与技能](./sub_agents.md) | 架构/开发者 | 各 Sub-Agent 职责、system prompt 设计、专属技能映射 |
| [llm_server 部署](./llm_server.md) | 运维/开发者 | 本地大模型服务（文本 qwen3.6-9B + 视觉 Qwen3-VL）的部署与 API |
| [RAG 检索服务](./rag.md) | 架构/开发者 | llm_server 内置的 Python3 文本/图像/图文对应 RAG 设计与 API |
| [测试与质量门禁](./testing.md) | 开发/SRE | 前后端测试体系、端到端(E2E)测试、覆盖率目标与 CI |
| [SKILL 工具集](./skills.md) | 开发者/集成 | LLM 可调用工具集：技能清单、装载（启动发现+运行时热插拔）、工具调用循环与自定义技能开发 |
| [MCP 集成](./mcp.md) | 架构/集成 | 作为 MCP Server 暴露中医能力（会话级 + Agent 级两层工具）、作为 MCP Client 接入外部工具、能力远程化（`impl: mcp`） |
| [项目计划与路线图](./plan.md) | 全员 | 当前实现现状、阶段目标、里程碑与风险 |
| [任务看板](./tasks.md) | 全员 | 里程碑拆解的 issue 清单（状态/验收/依赖），可执行跟踪 |
| [Postman 集合](./tcm-agent.postman_collection.json) | 接入方 | 全端点示例请求，导入即可联调；另见服务 `/openapi.json` |

> 仓库根目录另含 `README.md`（项目总览）与 `backend/smoke_test.py`（端到端自测）。
