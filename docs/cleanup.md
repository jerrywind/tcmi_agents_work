# 临时文件与日志清理规范（Cleanup Rules）

## 1. 目标

减小磁盘压力、避免误提交大文件、隔离多会话、加速 CI。统一命名，便于定位与批量清理。

## 2. 分类与位置

| 类别 | 位置 | 命名规则 | 生命周期 |
|---|---|---|---|
| 运行日志 | 仓库根 `*.log` / `*.err` / `*.out`（已 gitignore） | `tcm-YYYYMMDD-*.log` | 保留 7 天 |
| 测试产物 | `.pytest_cache/`、`htmlcov/`、`e2e_tests/images/` | 框架默认 | CI 后清除 |
| E2E 归档报告 | `e2e_tests/_reports/` | 报告 id 命名 | 用完即删（已 gitignore） |
| LLM 评测报告 | `server/target/tmp/llm_eval_report.json` | 固定名 | 随 `target/` 清理 |
| RAG 索引 | `rag_data/_index/*.sqlite3` | 固定名 | 语料变更后重建 |
| 构建产物 | `server/target/`、`frontend/dist/`、`__pycache__/` | 框架默认 | gitignore，可随时删 |

> harness **默认不落盘任何数据**：图片以 base64 / URL 随请求传入，无上传目录；
> 报告持久化需显式配置 `HARNESS_STORE_DIR` 才启用。

## 3. 清理命令

```powershell
# 一键脚本（推荐）
powershell -NoProfile -File scripts\cleanup.ps1

# 手动：仓库级
Remove-Item -Recurse -Force .pytest_cache, frontend\coverage, htmlcov, frontend\dist
Get-ChildItem -Recurse -Directory -Filter __pycache__ | Remove-Item -Recurse -Force

# Rust 构建产物（体积最大，通常 3 GB+，可安全删除，下次构建会重建）
Remove-Item -Recurse -Force server\target

# 运行日志（保留最近 7 天）
Get-ChildItem *.log, *.err |
  Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-7) } | Remove-Item -Force
```

## 4. 规则

1. 临时文件 **必须** 落在已被 `.gitignore` 覆盖的路径，禁止落入源码树。
2. 文件名带 `tmp_` / `debug_` 前缀，便于识别与批量清理。
3. 测试不得依赖未被清理的临时状态；每个用例自创建、自清理
   （Windows 上未关闭的 sqlite 连接会让临时目录删不掉，记得 `close()`）。
4. CI 结束后统一执行第 3 节的清理命令。
5. **禁止把日志/错误输出提交进仓库**（`.gitignore` 已覆盖 `*.log` / `*.err` / `*.out`）。
   若发现已被跟踪的历史文件，用 `git rm --cached <file>` 解除跟踪。
6. **禁止把密钥提交进仓库**：`.env` 已被忽略，但 `.env.example` **没有**——
   它只能放占位符。发现误提交立即轮换密钥。
