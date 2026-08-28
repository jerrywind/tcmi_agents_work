# 项目计划与路线图（Plan & Roadmap）

> 本文档基于当前代码库实际实现状态整理，用于对齐后续工作优先级。
> 最后更新：2026-08-25

## 0. 现状概览（已实现）

系统为「中医智能问诊 Agent」，采用**望闻问切四诊 + 辨证 + 安全 + 诊疗方案**共 7 个 Sub-Agent
的协议化架构，后端 **harness（Rust）**、前端 Taro 多端、可选本地 `llm_server`（LM Studio 网关 +
Agent 中间层，模型由 LM Studio 提供）、以及用于家庭算力上云的 `rrserver` 反向隧道。

> **后端重构已完成**：原 Python 实现 `backend/` 已归档为 `_useless/backend/`，
> 由 Rust 实现 `server/harness` 取代（流程与数据分离：程序逻辑在 `src/`，
> 可改数据在 `resources/*.yaml`）。rrserver 一并迁入 `server/` Cargo workspace。

| 模块 | 状态 | 关键产物 |
|---|---|---|
| `server/harness/src/agents/` | ✅ 完成 | 7 个 Sub-Agent（Rust），统一走 LLM |
| `server/harness/src/skills/` | ✅ 完成 | 9 个内置 SKILL（内置注册）+ toolcall 工具调用 |
| `server/harness/src/orchestrator.rs` | ✅ 完成 | 望闻问切 Loop + 辨证 + 安全门 + 治疗 |
| `server/harness/resources/*.yaml` | ✅ 完成 | 证候/问诊/方剂/调护/安全/关键词/路由/提示词，**流程与数据分离** |
| `frontend/src/pages/` | ✅ 完成 | index / consult / report / **skills** 四页（契约待对齐 harness） |
| `llm_server/` | ✅ 完成 | LM Studio 网关 + Agent 中间层（prompt 优化 / tool calling / MCP / agent） |
| `server/rrserver/` | ✅ 完成 | 云端中继 + 家庭端 client + 模型部署包装（Rust，100 测试） |
| 隧道集成 | ✅ 完成 | harness 可经 rrserver 隧道暴露（`--tunnel-*`，已端到端验证） |
| 测试 | ✅ 较完整 | `cargo test`（harness 案例回归 + rrserver 100 测试）、frontend vitest；**全链路 E2E**（`tcm_work/e2e_tests/`，一键 `run_full_chain_e2e.ps1`） |

结论：**核心功能链路已成形且可端到端跑通**；后续重心从「搭框架」转向「提质、补能力、上线加固」。

---

## 1. 阶段性目标

### 阶段一：质量与一致性加固（短期，1–2 周）
目标：消除实现与文档/契约的不一致，提升测试覆盖与可观测性。

- [x] **全链路 E2E 交付**：新增 `tcm_work/e2e_tests/`（pytest + 前端 vitest），覆盖
      rrserver→llm_server→harness 全链路；一键 `run_full_chain_e2e.ps1`
      （-WithRrserver / -WithFrontend）。llm_server/rrserver 用 stub 上游即可跑通；
      缺失产物自动 skip。详见 [`e2e.md`](./e2e.md)。
- [x] **后端 Rust 化 + 案例回归**：harness 以 `cases.jsonl`（93 条真实病例）做确定性回归，
      并以此驱动 YAML 资源扩充。原 `scripts/run_e2e.ps1`（backend pytest 编排）随 backend 归档。
- [x] **文档-代码一致性巡检**：以 `routing.yaml`、`config.py`、`main.py` 路由表为单一事实源，
      核对 `README / usage / development / deployment` 中的能力名（`diagnosis.*`）、
      接口路径、返回结构与环境变量表，确保零偏差。
- [x] **前端 skills 页联调**：验证 `pages/skills` 的装载/卸载接口在 `llm` 与纯 `rule` 模式下均可用。
- [x] **可观测性增强**：`/api/consultations/{id}/trace` 每条记录已含 `tokens`（LLM 累计
      token 用量）、`degraded`（bool）与 `degraded_reason`（resolve 级 / 运行时级降级原因），
      便于成本与质量归因。对应测试见 `tests/test_trace_observability.py`。

### 阶段二：诊疗能力深化（中期，3–6 周）
目标：提升诊断质量与方案可信度。

- [ ] **RAG 语料扩充**：扩展 `llm_server` RAG 的中医典籍/方剂库覆盖，校验 `tcm-rag` 检索召回质量。
- [ ] **辨证兼证与证据链**：完善 `diagnosis.differentiation` 对多证候并存、支持/矛盾证据的呈现，
      报告端可视化证据来源。
- [ ] **SKILL 自定义生态**：沉淀「如何写自己的技能」示例仓库 + 校验脚本，降低扩展门槛。
- [ ] **安全 Sub-Agent 强化**：`tcm-safety` 补更多红旗信号分级与就诊科室映射，规则兜底与 LLM 叠加去重更稳。

### 阶段三：生产化与多端上线（中长期，6–10 周）
目标：达到可对外提供服务的稳定性与合规要求。

- [ ] **持久化存储**：`MemoryStore` → PostgreSQL（会话/报告归档）+ Redis（多 worker 共享），
      完成 `TCM_STORE=redis|postgres` 路径。
- [ ] **对象存储**：图片上传从本地磁盘切到 OSS/S3（带签名 URL，`/api/.../images` 接口契约不变）。
- [ ] **前端多端发布**：H5 上 CDN；微信小程序走微信审核流程；评估 RN 产物可用性。
- [ ] **合规与安全审计**：免责声明强制展示、红旗中断路径不可移除、日志脱敏（不落健康明文）。

### 阶段四：家庭算力云化（按需，可并行）
目标：降低大模型推理成本。

- [ ] **rrserver 生产化**：真实 TLS 证书、强 token/随机 name、外部可观测与告警；
      评估 client/server 端到端应用层加密。
- [ ] **多隧道/多模型**：支持多家庭节点、文本 + 视觉分隧道路由，harness 按 capability 自动选路。
- [ ] **前端契约对齐 harness**：`frontend/src/services/api.ts` 仍按旧 backend 的
      `/api/consultations` 会话式契约，需改为 harness 的无状态端点（`/chat`、`/agents`、`/skills`），
      对齐后启用 `-WithFrontend` 的前端 e2e。
- [ ] **会话状态外置**：harness 无状态，多轮问诊由调用方维护 `messages`；
      若需服务端会话（续诊、报告留存），需新增存储层。

---

## 2. 里程碑（建议）

| 里程碑 | 交付物 | 验收 |
|---|---|---|
| M1 一致性收口 | 文档与代码零偏差 + 一键 e2e 通过 | `cargo test` + `run_full_chain_e2e.ps1` 绿灯，文档 diff 清零 |
| M1.5 前端契约对齐 | `api.ts` 对接 harness 无状态端点 | `-WithFrontend` e2e 绿灯 |
| M2 诊疗提质 | RAG 扩充 + 兼证证据链 + YAML 资源扩充 | 案例回归覆盖提升 + 样例问诊报告质量评审通过 |
| M3 可上线 | 会话存储（可选）+ 对象存储 + 小程序发布 | 生产 compose 起得来、CORS/HTTPS 合规 |
| M4 云化降本 | rrserver 生产部署 + 多隧道 | 外部经隧道调用 LLM 延迟达标 |

---

## 3. 风险与待确认

1. **模型可得性**：模型由宿主机 LM Studio 提供（默认 `google/gemma-4-12b-qat`）；LM Studio
   未启动时 `llm_server` 返回 503，**harness 的 `/chat` 会直接报错**（无 rule/mock 降级），
   只读端点仍可用。演示与验收需确保 LM Studio 已加载模型。
2. **医疗合规**：当前为 AI 健康参考，非医疗诊断；上线前需法务/合规复核免责与红旗路径。
3. **测试覆盖缺口**：harness 的确定性逻辑（证候推断、配伍禁忌、方剂检索、资源完整性）
   已由 `cargo test -p harness` 覆盖；**需真实 LLM 的 `/chat` 链路尚未自动化**，
   当前依赖手工验证。
4. **对象存储尚未实现**：harness 当前不保存图片（以 base64 / URL 随请求传入），
   也无 `/uploads` 静态目录；如需会话留存与图片归档，需新增存储层与对象存储抽象。
4. **知识库权威性**：`tcm-kb` 等技能的知识来源需标注出处，避免臆造方剂/剂量。

---

## 4. 如何参与

- 新增 Sub-Agent：遵循 [`agent-protocol.md`](./agent-protocol.md)，改 `routing.yaml` 即生效。
- 新增技能：见 [`skills.md`](./skills.md) 第 6 节，声明 `SKILL` + `HANDLERS` 即可。
- 本地联调：后端 `python -m uvicorn app.main:app` + 前端 `npm run dev:h5` + 可选 `llm_server` 与 `rrserver`；一键端到端见 `scripts/run_e2e.ps1` 与 `tcm_work/e2e_tests/run_full_chain_e2e.ps1`。
- **任务跟踪**：里程碑已拆解为可追踪的 issue 清单，见 [`tasks.md`](./tasks.md)。
