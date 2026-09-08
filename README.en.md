# WindBlue Tech TCM · Intelligent TCM Consultation Agent

> 🌐 [简体中文](./README.md) | **English**

> ⚠️ Disclaimer: this system is AI-generated and provides health references only.
> It **does not constitute medical diagnosis or prescription advice**. If you feel unwell
> or notice any red-flag symptom, seek in-person medical care promptly.

A protocol-driven Traditional Chinese Medicine (TCM) consultation system covering
**four examinations (inspection / auscultation / inquiry / palpation) → syndrome
differentiation → safety gate → treatment plan**:
the frontend (Taro, multi-platform) calls the Rust **harness**, which orchestrates
13 Sub-Agents. Inference is served by **LM Studio** on the host
(`llm_server` is an optional gateway), and home compute can be exposed to the cloud
through the `rrserver` reverse tunnel.

---

## 1. Architecture Overview

```
┌──────────────────┐     ┌───────────────────────┐
│  Frontend Taro   │────▶│  harness (Rust)       │  Inspection/Auscultation/Inquiry/
│  H5 / WeChat MP  │     │  13× Sub-Agent orchestr│  Palpation/Differentiation/Safety/Treatment
└──────────────────┘     └───────────┬───────────┘
                                      │ OpenAI-compatible (/v1/...)
                                      ▼
                          ┌───────────────────────┐   ┌──────────────────────┐
                          │  llm_server (optional) │──▶│  LM Studio :11223    │
                          │  prompt opt/tool/MCP/  │   │  google/gemma-4-12b  │
                          │  agent loop            │   │  (text + vision)     │
                          └───────────┬───────────┘   └──────────────────────┘
                                      │ expose home compute to the cloud (optional)
                                      ▼
                          ┌───────────────────────┐
                          │  rrserver rev. tunnel  │  cloud server :43302
                          │                        │  + home client :9000
                          └───────────────────────┘
```

| Component | Path | Role | Default port |
|---|---|---|---|
| Frontend | `frontend/` | Taro multi-platform (H5 / WeChat Mini Program), dev `:10086` | 10086 |
| Backend | `server/` (single image `tcmi_server`) | Rust harness orchestrating 13 Sub-Agents + rrserver relay (same container) | 43301 / 43302 |
| LLM gateway | `llm_server/` | Pure LM Studio gateway + agent middle layer (**hosts no model**, optional) | 8000 |
| Inference | LM Studio on host | `http://localhost:11223/v1`, model `google/gemma-4-12b-qat` | 11223 |
| Reverse tunnel | `server/rrserver/` | Rust relay: cloud server `:43302` + home client `:9000` (**same image/container as harness**) | 43302 / 9000 |
| Single entrypoint | `deploy/` | Standalone nginx: static hosting + reverse proxy `/api`, `/rr` (TLS termination) | 80 / 443 / 8080 |

harness endpoints (11 routes): `/health`, `/agents` (GET/POST), `/chat`,
`/chat/stream` (SSE), `/skills` (GET/POST), `/mcp`, `/reload`, `/reports`,
`/reports/:id` (full contract: [`docs/usage.md`](./docs/usage.md), in Chinese).

### Key facts (referenced consistently across docs)

- 🔒 **The backend depends on Docker entirely**: harness and rrserver are built, run and
  verified inside Docker; host-side `cargo build` artifacts are **never** used as evidence.
  Both binaries come from one multi-stage image `tcmi_server` (`server/Dockerfile`, compiled
  inside the image) and run in the **same container** (harness `43301` for external traffic /
  rrserver `43302` as relay). No Rust toolchain is needed on the build machine.
- **The harness is stateless**: one `POST /chat` serially runs every step of the active
  profile in `routing.yaml` and returns. There is **no server-side multi-turn loop** —
  the caller accumulates `messages` and increments `payload.round`. Report persistence is
  off by default.
- **Feedback-driven differentiation**: when information is insufficient the run **stops right
  after differentiation** and returns `status: "awaiting_input"` plus `loop.pending_questions`
  with **no treatment advice**. At the round limit (default 3) it is force-released with `forced`.
- **A failing step does not abort the run**: completed steps are returned with `failures` and
  `partial`. Only when *all* steps fail does it return `{"error"}`.
- **Differentiation output is structured**: primary / concurrent syndromes with confidence plus
  supporting and conflicting evidence are returned in `structured`, produced deterministically
  by the rule layer (no LLM, therefore regression-testable). Out-of-library cases **degrade
  honestly** (`primary=null` + `near`) instead of forcing a syndrome.
- **Streaming**: `POST /chat/stream` (SSE) pushes `step_start` / `step_delta` / `step_retry` /
  `step_done` / `confidence` / `summary` / `done`. If the upstream does not return SSE,
  the harness falls back to whole-body parsing automatically (`HARNESS_LLM_STREAM=false`
  disables streaming).
- **Compliance floor**: the disclaimer is delivered with every result; the safety gate
  **cannot be removed from `routing.yaml`** (it is force-inserted with a warning if missing)
  and is pinned after the collection phase and before differentiation; persisted content is redacted.
- **Without an LLM**: `/chat` fails (the harness has no MockProvider) while read-only endpoints
  still work; `llm_server` `/healthz` returns `degraded` and `/v1/models` returns 503.
- **Flow and data are separated**: syndromes (16), formulas (36), inquiry questions (11),
  care plans (16), safety red flags (6), contradictory manifestations (14), transformations (3),
  prompts (14), retrieval scopes (13) all live in `server/harness/resources/*.yaml`
  (English slug keys + Chinese values + Chinese comments). Run `POST /reload` or restart after edits.
- **11 built-in skills** (registered at compile time) plus external `mcp__*` tools mounted via
  `mcp_clients`; all 13 Sub-Agents go through `chat_with_tools`, so tools really fire during inference.

---

## 2. Quick Start

**Prerequisites**: Docker (required for the backend), Node 18+ (frontend),
Python 3.11+ (optional gateway/RAG), LM Studio (real inference).

```powershell
# 1) Backend: build the image inside Docker (multi-stage, no local Rust needed).
#    The single image tcmi_server contains both harness and rrserver and starts
#    them in one container: harness 43301 (external) / rrserver 43302 (relay).
cd server
docker build -t tcmi_server:local .
docker run -d --name tcmi_server -p 43301:43301 -p 43302:43302 `
  -e HARNESS_LLM_BASE_URL=http://host.docker.internal:11223/v1 `
  -e HARNESS_LLM_API_KEY=<LM Studio token> `
  tcmi_server:local

# 2) Verify: http://127.0.0.1:43301/health returns {"status":"ok","rag":{...}}
#    From inside a container, reach the host LM Studio via host.docker.internal (not localhost).
#    ⚠️ Always use 127.0.0.1 when verifying: on Windows "localhost" resolves to IPv6 ::1
#    first and the IPv4 fallback costs ~21 seconds.

# 3) Frontend
cd frontend && npm install && npm run dev:h5     # http://localhost:10086
```

One-command image build: `pwsh scripts\build-release.ps1` (equivalent to the `docker build` above).
Deployment, ports and environment variables: [`docs/deployment.md`](./docs/deployment.md).

> Exposing home compute to the cloud (optional): see `docs/deployment.md` section 5.

---

## 3. Documentation Map

Docs live in `docs/`, split by responsibility, with every fact defined in exactly one place.
The full index and responsibility matrix is in [`docs/README.md`](./docs/README.md) (Chinese).
English quick map:

| I want to… | Read (Chinese) |
|---|---|
| Run it / integrate the API | [`usage.md`](./docs/usage.md) |
| Deploy to production | [`deployment.md`](./docs/deployment.md) |
| Develop locally / pitfalls | [`development.md`](./docs/development.md) |
| Change agents / skills / protocol | [`agent-protocol.md`](./docs/agent-protocol.md), [`sub_agents.md`](./docs/sub_agents.md), [`skills.md`](./docs/skills.md) |
| Integrate MCP / use RAG | [`mcp.md`](./docs/mcp.md), [`rag.md`](./docs/rag.md) |
| Keep quality | [`testing.md`](./docs/testing.md), [`e2e.md`](./docs/e2e.md), [`samples/`](./docs/samples/README.md) |
| Track progress / roadmap | [`plan.md`](./docs/plan.md), [`tasks.md`](./docs/tasks.md) |

> **Language**: this README has a [Chinese version](./README.md); `docs/` is Chinese-only for now.
> See section 7 "Documentation languages and i18n".

---

## 4. Testing

```powershell
# Backend: 233 cases (harness 86 + rrserver 147) + fmt + strict clippy, all inside Docker
docker run --rm -v "${PWD}/server:/build" -w /build rust:1.98-bookworm `
  bash -c "rustup component add rustfmt clippy && cargo fmt --all -- --check && `
           cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace"

# Frontend: 132 cases across 11 files (8 contract cases need a running local harness,
# otherwise they are skipped automatically)
cd frontend && npm run test

# llm_server (8 pytest cases) + its RAG subpackage (65 unittest cases)
cd llm_server && python -m pytest tests -q
cd llm_server/rag && python -m unittest test_corpus test_rag test_taxonomy test_api_scope
```

- `--test cases` uses `cases.jsonl` as a **resource-integrity guard** (expected syndromes exist,
  have formulas or care plans, keywords hit). It needs no LLM. Note it is a **synthetic baseline**:
  93 rows with only 5 distinct chief complaints, 3 syndrome combinations, and 37 rows whose
  complaint is the literal `x` — it guards "no data is missing", not "differentiation is correct".
  See [`testing.md`](./docs/testing.md).
- `golden_cases.jsonl` (21 rows) asserts **top-1 hit** and "no syndrome for out-of-library cases",
  covering what `cases.jsonl` cannot.
- Full-chain E2E: `e2e_tests/run_full_chain_e2e.ps1` (stubbed, no real LLM needed).
- **Manual acceptance** (requires a real LLM): `e2e_tests/run_manual_e2e.ps1 -Case damp-heat`;
  artifacts are archived under [`docs/samples/`](./docs/samples/README.md).

See [`docs/testing.md`](./docs/testing.md).

---

## 5. Repository Layout

| Path | Description |
|---|---|
| `server/harness/` | Orchestration backend (Rust): `src/` logic, editable `resources/*.yaml`, `cases.jsonl` synthetic guard, `golden_cases.jsonl` golden set |
| `server/rrserver/` | Reverse tunnel (Rust): cloud server + home client + model deployment wrapper |
| `frontend/` | Taro multi-platform (H5 / WeChat Mini Program): 6 pages + `services/{harness,session,stream,members}.ts` |
| `llm_server/` | LM Studio gateway (Python, optional); `rag/` is its retrieval subpackage |
| `deploy/` | Unified nginx entrypoint (static hosting + `/api`, `/rr` reverse proxy + TLS) and compose stack |
| `e2e_tests/` | Full-chain E2E and manual acceptance scripts |
| `scripts/` | `build-release.ps1` (build image), `cleanup.ps1` (cleanup) |
| `docs/samples/` | End-to-end samples produced against a real LLM, used as regression references |
| `rag_data/` | TCM classics corpus (694 txt files; index covers 696 books / 66.18 M chars), **not committed**, see `docs/rag.md` |

---

## 6. License and Compliance

AI health reference, not medical diagnosis. A compliance review is required before launch:
mandatory disclaimer display, non-removable red-flag interruption path, log redaction,
report retention policy. Checklist: `docs/deployment.md` section 7.

---

## 7. Documentation Languages and i18n

| File | Language | Notes |
|---|---|---|
| [`README.md`](./README.md) | Simplified Chinese | Chinese entrypoint |
| [`README.en.md`](./README.en.md) | English | This file — **same structure and section numbers** as the Chinese one, content in one-to-one correspondence |
| `docs/*.md` | Simplified Chinese | Detailed docs, not yet translated; section 3 above provides an English quick map |

Conventions:

- Both READMEs carry a language switch line at the top (`🌐 简体中文 | English`) linking to each other;
- **Edit both together**: section numbers and key figures (ports, test counts, resource entry counts)
  must stay identical;
- To add a language, copy `README.en.md` to `README.<lang>.md` and register it in this table
  and in the switch line of the other versions.
