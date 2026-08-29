# 全链路 E2E（e2e_tests）

跨组件端到端测试，覆盖 **harness → rrserver → llm_server**，确保在「无真实 LM Studio /
无 GPU」环境下也能跑通隧道与网关。

> **完整说明见 [`docs/e2e.md`](../docs/e2e.md)**，本文件只保留目录内速查信息。

## 快速运行

```powershell
cd e2e_tests
.\run_full_chain_e2e.ps1                 # 默认：harness(镜像) + pytest + 前端契约测试
.\run_full_chain_e2e.ps1 -WithRrserver   # 额外跑 rrserver 隧道（需 TCM_RRSERVER_BIN）
.\run_full_chain_e2e.ps1 -SkipFrontend   # 只跑 pytest
.\run_full_chain_e2e.ps1 -SkipBuild      # 复用已有镜像，跳过构建
```

脚本流程：生成样例图片 → `docker build` harness 镜像（镜像内编译）→ `docker run` 起容器
（`:8011`）→ `/health` 探活 → 跑 pytest → 前端契约测试 → 删除容器。

前置：Docker（后端**完全依赖 Docker**，不使用宿主机 cargo 产物）。

## 目录文件

| 文件 | 作用 |
|---|---|
| `conftest.py` | 各组件 base_url、健康等待、httpx fixtures |
| `e2e_helpers.py` | 共享辅助 |
| `test_rrserver_e2e.py` | 隧道：server+client 启动、token 鉴权、`/t/<name>` 转发 |
| `test_llm_server_e2e.py` | 网关：`/healthz`(degraded/ok)、`/v1/models`、chat 透传 |
| `_make_sample_image.py` | 生成 1×1 样例 JPEG |
| `run_full_chain_e2e.ps1` | 一键编排 |
| `images/` | 运行期生成物（已 gitignore） |

## 分层与依赖

| 层 | 依赖真实 LLM? | 缺失时行为 |
|---|---|---|
| rrserver | 否（stub 充当本地 llm） | 无编译产物 → 自动 skip |
| llm_server | 否（stub 充当 LM Studio） | 缺 fastapi 依赖 → 自动 skip |
| harness | 否（仅探活 `/health`） | 未就绪 → 脚本终止 |
| 前端契约 | 否 | 后端不可达 → 用例自动 skip |

harness 的问诊链路（`/chat`）需真实 LLM，**不在本套件内**（无 mock 兜底）；
其确定性逻辑由 `cargo test -p harness --test cases` 覆盖。
