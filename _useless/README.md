# _useless · 归档目录

本目录存放项目清理时识别出的**废弃、未完成或临时残留**的文件/目录，已从原路径移出，
不再参与构建、测试或部署。保留仅供历史回溯与审计。

> 如需彻底删除，确认无回溯需求后可直接移除整个 `_useless/` 目录。`legacy_rust_rewrite/server/target/`
> 为 Rust 编译产物，已被该目录下的 `.gitignore` 忽略（可随时 `cargo clean` 重建，无保留价值）。

## 归档清单

### `legacy_rust_rewrite/`
- `server/` — 未完成的「Rust 全量重写 backend + rrserver」实验骨架（阶段 A）。仅含
  `diagnose::api_router()` 空壳与 `/health`、`/api/health` 占位，诊断/API 业务均未实现。
  实际运行系统仍为 `backend/`（FastAPI）+ `rrserver/`（Rust），故整套重写实验归档。
- `server_rewrite_design.md` — 该重写实验的配套设计文档（含分阶段计划与契约测试方案），
  随实现一并归档。

### `temp_artifacts/`
- `backend_run.err` — `backend/` 运行时 uvicorn 启动残留的错误日志碎片（已被 `.gitignore` 忽略）。
- `_vitest_err.txt` — 前端 `vitest` 运行时弃用警告（Vite CJS Node API deprecated）的 stderr 残片。
- `backend_smoke_test.py` — `backend/` 下的遗留冒烟测试脚本，未被任何 CI/流水线调用，
  全链路 E2E 已由 `e2e_tests/` 与 `scripts/run_e2e.ps1` 覆盖。
