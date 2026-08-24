# 项目计划与路线图（Plan & Roadmap）

> 本文档基于当前代码库实际实现状态整理，用于对齐后续工作优先级。
> 最后更新：2026-08-05

## 0. 现状概览（已实现）

系统为「中医智能问诊 Agent」，采用**望闻问切四诊 + 辨证 + 安全 + 诊疗方案**共 7 个 Sub-Agent
的协议化架构，后端 FastAPI、前端 Taro 多端、可选本地 `llm_server`（llama.cpp，qwen3.6-9B + Qwen3-VL）、
以及用于家庭算力上云的 `rrserver` 反向隧道。

| 模块 | 状态 | 关键产物 |
|---|---|---|
| `backend/app/agents/` | ✅ 完成 | 7 个 Sub-Agent，rule + llm 双实现 |
| `backend/app/skills/` | ✅ 完成 | 8 个内置 SKILL + registry/loader/toolcall + 热装载 |
| `backend/app/core` / `orchestrator` | ✅ 完成 | 望闻问切 Loop + 诊疗方案阶段 |
| `backend/app/routing*.yaml` | ✅ 完成 | rule 默认 + llm 覆盖，一行切实现/模型 |
| `frontend/src/pages/` | ✅ 完成 | index / consult / report / **skills** 四页 |
| `llm_server/` | ✅ 完成 | 文本 + 视觉 + 内置 RAG（rag.md） |
| `rrserver/` | ✅ 完成 | 云端中继 + 家庭端 client + 模型部署包装（Rust，100 测试） |
| 测试 | ✅ 较完整 | 17 个测试文件（unit + e2e），`scripts/run_e2e.ps1` |

结论：**核心功能链路已成形且可端到端跑通**；后续重心从「搭框架」转向「提质、补能力、上线加固」。

---

## 1. 阶段性目标

### 阶段一：质量与一致性加固（短期，1–2 周）
目标：消除实现与文档/契约的不一致，提升测试覆盖与可观测性。

- [ ] **补齐集成 e2e 流水**：把 `scripts/run_e2e.ps1` 串起 backend 启动 → `smoke_test.py`
      → `tests/e2e/*`，接入 CI（GitHub Actions / 本地定时）。
- [ ] **文档-代码一致性巡检**：以 `routing.yaml`、`config.py`、`main.py` 路由表为单一事实源，
      核对 `README / usage / development / deployment` 中的能力名（`diagnosis.*`）、
      接口路径、返回结构与环境变量表，确保零偏差。
- [ ] **前端 skills 页联调**：验证 `pages/skills` 的装载/卸载接口在 `llm` 与纯 `rule` 模式下均可用。
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
- [ ] **多隧道/多模型**：支持多家庭节点、文本 + 视觉分隧道路由，backend 按 capability 自动选路。

---

## 2. 里程碑（建议）

| 里程碑 | 交付物 | 验收 |
|---|---|---|
| M1 一致性收口 | 文档与代码零偏差 + 一键 e2e 通过 | `run_e2e.ps1` 绿灯，文档 diff 清零 |
| M2 诊疗提质 | RAG 扩充 + 兼证证据链 + skills 示例 | 样例问诊报告质量评审通过 |
| M3 可上线 | PG/Redis + 对象存储 + 小程序发布 | 生产 compose 起得来、CORS/HTTPS 合规 |
| M4 云化降本 | rrserver 生产部署 + 多隧道 | 外部经隧道调用 LLM 延迟达标 |

---

## 3. 风险与待确认

1. **模型可得性**：本地 `qwen3.6-9B` / `Qwen3-VL-8B` 权重需自行准备，`llm_server` 默认离线走 rule。
2. **医疗合规**：当前为 AI 健康参考，非医疗诊断；上线前需法务/合规复核免责与红旗路径。
3. **测试覆盖缺口**：`RedisStore` 已实现且 `tests/test_store.py` 用内存桩覆盖了往返与 `_build_store`
   分支，**该项已完成**；待补的是「对象存储上传（OSS/S3）」测试——但该能力本身尚未实现，需先完成阶段三的对象存储抽象。
4. **对象存储尚未实现**：图片上传当前落到本地 `backend/uploads/`（`main.py` + `config.UPLOAD_DIR`），
   OSS/S3 后端需新增 `StorageBackend` 抽象并接入上传接口。
4. **知识库权威性**：`tcm-kb` 等技能的知识来源需标注出处，避免臆造方剂/剂量。

---

## 4. 如何参与

- 新增 Sub-Agent：遵循 [`agent-protocol.md`](./agent-protocol.md)，改 `routing.yaml` 即生效。
- 新增技能：见 [`skills.md`](./skills.md) 第 6 节，声明 `SKILL` + `HANDLERS` 即可。
- 本地联调：`backend/ smoke_test.py` + `frontend/ npm run dev:h5` + 可选 `llm_server` 与 `rrserver`。
- **任务跟踪**：里程碑已拆解为可追踪的 issue 清单，见 [`tasks.md`](./tasks.md)。
