# 全链路 E2E（e2e_tests）

跨组件端到端测试，覆盖 **harness → rrserver → llm_server**，在「无真实 LM Studio /
无 GPU」环境下也能跑通隧道与网关。

> **完整说明见 [`docs/e2e.md`](../docs/e2e.md)**，本文件只保留速查。

## 快速运行

```powershell
cd e2e_tests
.\run_full_chain_e2e.ps1                 # harness(镜像) + pytest + 前端契约测试
.\run_full_chain_e2e.ps1 -WithRrserver   # 额外跑 rrserver 隧道（需 TCM_RRSERVER_BIN）
.\run_full_chain_e2e.ps1 -SkipFrontend   # 只跑 pytest
.\run_full_chain_e2e.ps1 -SkipBuild      # 复用已有镜像

# 人工验收（需真实 LLM，产出归档到 docs/samples/<case>/）
$env:HARNESS_LLM_API_KEY = '<LM Studio 令牌>'
.\run_manual_e2e.ps1 -Case damp-heat     # 或 wind-cold / red-flag
```

前置：Docker（后端**完全依赖 Docker**，不使用宿主机 cargo 产物）。
人工验收还需 LM Studio 已启动并加载 `google/gemma-4-12b-qat`。

## 运行期产物

`images/`（样例图片）、`_reports/`（容器归档的报告）均由脚本生成，已 gitignore。
