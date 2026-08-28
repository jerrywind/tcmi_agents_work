# 临时文件与日志清理规范（Cleanup Rules）

> 历史上本规范位于 `testing.md` 内并误引为 `cleanup-rules.md`（文件不存在）。
> 现独立为本文件，避免重复与失效引用。

## 1. 目标
- 减小磁盘压力、避免误提交大文件、隔离多会话、加速 CI。
- 统一命名，便于定位与批量清理。

## 2. 分类与位置

| 类别 | 位置 | 命名规则 | 生命周期 |
|---|---|---|---|
| 运行日志 | `logs/`（仓库根，已 gitignore） | `*.log`、`tcm-YYYYMMDD-*.log` | 保留 7 天 |
| 测试产物 | `.pytest_cache/`、`frontend/coverage/`、`htmlcov/` | 框架默认 | CI 后清除 |
| 临时上传/导出 | `_useless/backend/uploads/`（归档，**harness 不落盘图片**） | `consultation_id/...`、`tmp_*.ext` | 归档数据，随 backend 保留 |
| 构建产物 | `frontend/dist/`、**`server/target/`**、`__pycache__/`、`.ruff_cache/` | 框架默认 | gitignore，可随时删 |
| 诊断截图/转写 | `debug_images/`、`e2e_tests/images/` | `case_*.json`、`debug_*.png`、`sample.jpg` | 永久（样例数据） |

## 3. 清理命令
```bash
# 仓库级（PowerShell）
Remove-Item -Recurse -Force .pytest_cache, frontend\coverage, htmlcov, __pycache__
# Rust 构建产物（体积最大，可安全删除，下次 cargo build 会重建）
Remove-Item -Recurse -Force server\target
# 运行日志（保留最近 7 天外删除）
Get-ChildItem logs\*.log | Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-7) } | Remove-Item
```

> 也可用 `scripts/cleanup.ps1` 一键清理。注意：`server/target/` 已在 `.gitignore` 中
> （`**/target/`），不会误入版本库。

## 4. 规则
1. 临时文件**必须**进入上述受 gitignore 目录，禁止落入源码树。
2. 文件名含 `tmp_` / `debug_` 前缀便于识别与批量清理。
3. 测试不得依赖未被清理的临时状态；每个用例自创建、自清理。
4. CI 结束后统一执行第 3 节清理命令。
