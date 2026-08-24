# 开发文档（Development）

面向二次开发：新增 Sub-Agent、切换实现/模型、理解诊断 Loop 与协议层。

## 1. 技术栈

- **后端**：Python 3 + FastAPI + Pydantic；编排用自研状态机（诊断 Loop）；LLM 走 OpenAI 兼容协议（无 Key 自动降级 mock）。
- **前端**：Taro 4 + React + TypeScript + NutUI，一套代码编译到 H5 / 微信小程序 /（React Native）。
- **存储**：会话态默认内存版 `MemoryStore`（`app/store.py`），接口稳定，生产可替换为 PostgreSQL + Redis。

## 2. 目录结构

```
backend/
  app/
    models/schemas.py          # 全部 Pydantic 数据模型（Evidence/Hypothesis/TreatmentPlan/Report/...）
    protocol/                  # ★ 可替换协议层
      base.py                  #   AgentRequest / AgentResponse 信封 + Capability + SubAgent 基类
      registry.py              #   注册表 @register + resolve(capability, impl)
      llm.py                   #   LLMProvider 抽象（openai 兼容 / mock 降级）+ parse_json
    knowledge/                 # 中医知识库（证候权重表、十问题库、红旗词、诊疗方案库）
    agents/                    # 各 Sub-Agent（望/闻/问/切/辨证/安全/诊疗方案）
    mcp/                       # ★ MCP 集成（详见 docs/mcp.md）
      server.py                #   MCP Server：stdio + Streamable HTTP(/mcp)
      client.py                #   MCP Client：MCPToolHub，接入外部 MCP Server
      remote_agent.py          #   impl="mcp" 远程 Sub-Agent 桥（能力可远程化）
      tools/session.py         #   会话级工具（完整问诊流程，带 cid）
      tools/agents.py          #   Agent 级工具（7 项中医能力，无状态）
    core/orchestrator.py       # 主诊 Agent：望闻问切 Loop + 诊疗方案阶段
    routing.yaml               # ★ 切换实现/模型只改这里
    config.py                  # 配置（LLM base_url、模型映射、端口等，读环境变量）
    main.py                    # FastAPI 路由
    store.py                   # 会话存储（内存版）
  requirements.txt
  smoke_test.py                # 端到端自测
frontend/
  src/pages/index              # 档案创建
  src/pages/consult            # 问诊对话（含方案个性化追问）
  src/pages/report             # 诊断 + 诊疗方案报告
  src/pages/skills             # SKILL 技能管理（列出 / 装载 / 卸载）
  src/services/api.ts          # 后端接口封装
  config/{dev,prod}.ts         # 开发/生产代理与 API 地址
```

## 3. 本地开发环境

### 后端
```bash
cd backend
python -m venv .venv && source .venv/bin/activate   # Windows: .venv\Scripts\activate
pip install -r requirements.txt
uvicorn app.main:app --reload --port 8000
# 自测
python smoke_test.py
```
API 文档：启动后访问 `http://localhost:8000/docs`（Swagger）与 `/redoc`。

### 前端
```bash
cd frontend
npm install
npm run dev:h5          # H5 本地预览（默认代理到 http://localhost:8000）
# 微信小程序（需先在 project.config.json 配置 appid）
npm run dev:weapp
```

## 4. 核心架构

```
前端三页面 --> REST API(/api/*)
                    |
              Orchestrator（状态机）
   ┌─────────────────┼───────────────────────────┐
   ▼   望闻问切 Loop（最多 max_rounds 轮）          ▼ 诊断完成后
  望/闻/问/切/辨证/安全 Sub-Agent            诊疗方案 Sub-Agent(treatment.plan)
   （统一 AgentRequest/Response 信封）          （可个性化追问 1~2 条后出方案）
```

- **诊断 Loop**：每轮由辨证 Sub-Agent 给出候选证候与置信度，问诊 Sub-Agent 计算"信息增益最大"的下一问；置信度达标（Top1≥0.55 且领先≥0.15）或兼证接近即收敛，否则追问。
- **诊疗方案阶段**：`_finish` 收尾后自动进入 `_treatment_step`，结合证候 + 用户个体情况（煎药便利性、是否接受外治、是否愿做西医检查、孕期备孕）产出多模态方案。

## 5. 协议层与 Sub-Agent 可替换设计

所有 Sub-Agent 遵守统一协议：无状态、只消费 `AgentRequest`、只产出 `AgentResponse`（结构化 `evidences / hypotheses / question / alerts / plans`）。编排器**不 import 任何具体实现**，只通过 `routing.yaml` 解析器拿到实例。

`AgentResponse` 信封：
```python
AgentResponse {
  request_id, capability,
  status: ok | error | skip,
  evidences / hypotheses / question / alerts / plans,
  notes, error,
  meta: { impl, model, latency_ms }
}
```

### 5.1 新增一个 Sub-Agent（以 `treatment.plan` 为例）

1. 在 `protocol/base.py` 的 `Capability` 增加能力标识（已有 `TREATMENT`）。
2. 新建 `app/agents/treatment.py`，继承 `SubAgent`，声明 `capability` / `impl_name`，实现 `async handle(self, req) -> AgentResponse`，并 `@register`：
   ```python
   from ..protocol.base import AgentRequest, AgentResponse, Capability, SubAgent
   from ..protocol.registry import register

   @register
   class TreatmentRuleAgent(SubAgent):
       capability = Capability.TREATMENT
       impl_name = "rule"
       description = "..."

       async def handle(self, req: AgentRequest) -> AgentResponse:
           # 1) 从 req 取输入（hypotheses / evidences / payload）
           # 2) 计算
           # 3) 返回结构化结果
           return AgentResponse(capability=self.capability, plans=[...])
   ```
   - 基类 `SubAgent.run()` 已自动包装计时、异常兜底（异常 → `status="error"`，不崩溃）。
   - 多个实现（如 `rule` / `llm`）用不同 `impl_name`，互不冲突，运行时按 `routing.yaml` 选择。
3. 在 `app/agents/__init__.py` 增加 `from . import treatment`，确保被 import 触发注册。
4. 在 `routing.yaml` 增加该能力的路由（见下）。

### 5.2 切换实现 / 模型

只改 `backend/app/routing.yaml`（或指向 `routing.llm.yaml`），无需动代码：
```yaml
routing:
  diagnosis.inspection:        # 望诊（舌象/面相/患处图像）
    impl: rule                 # rule | llm_vision | mcp —— 接多模态模型时改这里
    model: vision-default
  diagnosis.differentiation:  # 辨证
    impl: rule                 # rule | llm | mcp
    model: text-default
  treatment.plan:             # 诊疗方案
    impl: rule                 # rule | llm | mcp —— 切换为 llm 即由大模型生成综合方案
    model: text-default
    options:
      max_questions: 2         # 个性化追问最多条数
llm:
  base_url: ""                 # 由 TCM_LLM_BASE_URL 注入（如 http://llm_server:8000/v1）
  api_key_env: TCM_LLM_API_KEY
  models:                     # 逻辑模型名 -> 实际模型 id（二级映射）
    text-default: qwen3.6-9B          # 文本问诊（听/问/切/辨证/安全/施治）
    vision-default: Qwen3-VL-8B       # 望诊视觉模型（原生多模态）
```
> 顶层键为 `routing`（非 `routes`）。启用全部 LLM 实现最简方式：用环境变量
> `TCM_ROUTING_FILE` 指向 `routing.llm.yaml`（compose 的 `llm` profile 已设好）。
> 运行 `GET /api/system/agents` 可查看当前生效的路由。

除 `rule` / `llm` 外，任一能力还可设 `impl: mcp`，把该能力整体路由到**远程 MCP Server**
（进程外、可独立扩缩容），编排器代码无需改动：

```yaml
routing:
  diagnosis.inspection:
    impl: mcp
    options:
      server: vision_farm      # mcp.clients 中的连接名
      tool: agent_inspection   # 可选，默认按 capability 推断
```

远端不可用时自动降级为 `status=error` 信封，不中断问诊。详见 [`MCP 集成`](./mcp.md)。

### 5.3 LLM Provider 抽象

`protocol/llm.py` 的 `LLMProvider` 封装 OpenAI 兼容调用（`chat(messages, model, json_mode)`）。
未配置 `TCM_LLM_API_KEY` 时自动降级为 mock（返回受控结构化结果），保证**全链路无 Key 可跑通**。

### 5.4 为 Sub-Agent 绑定技能（SKILL）

LLM 实现的子智能体可通过 `run_tool_loop` 在推理时调用技能工具。给某个能力增加可用技能只需两步：

1. 在 `app/agents/skills_map.py` 的 `AGENT_SKILLS` 中，把技能名加入对应 `Capability` 的列表
   （如给闻诊加 `tcm-auscultation`）。运行时 `skill_registry.tools_for(capability)` 据此向 LLM 注入工具声明。
2. 在 `app/agents/prompts.py` 对应能力的 system prompt 中，用自然语言说明何时调用这些工具，
   让 LLM 知道工具的存在与适用场景（如"可用 `lookup_diet_therapy` 按证候返回食疗建议"）。

技能本身的开发（声明 `SKILL` 清单 + `HANDLERS`）见 [`SKILL 工具集`](./skills.md) 第 6 节；
新技能会被 `discover_skills` 自动装载，也可经 `POST /api/skills/load` 热装载，无需重启服务。
各能力当前绑定的技能与文档 [`Sub-Agent 设计与技能`](./sub_agents.md) 总览表保持一致。

## 6. 测试

- `backend/smoke_test.py`：新建档案 → 多轮模拟回答 → 触发诊疗方案 → 断言生成「脾胃湿热」诊断与多模态方案。
  ```bash
  cd backend && python smoke_test.py
  ```
- 可观测性：每次 Sub-Agent 调用写入 `consultation.trace`，`GET /api/consultations/{id}/trace` 查看每个能力用了哪个 impl/model、耗时多少。
- 前端类型校验：`cd frontend && npx tsc --noEmit`。

## 7. 代码规范要点

- 数据模型集中在 `models/schemas.py`，跨模块复用同一 Pydantic 定义。
- 新能力必须先定义 `Capability` 枚举与 `AgentResponse` 字段，再写实现。
- Sub-Agent 保持无状态；需要用户上下文时通过 `req.payload` 传入（如 `patient`、`qa`、`diagnoses`）。
- 任何用户输入都过 `safety` 红旗检查；涉及医疗安全以"建议线下就医"为兜底。
