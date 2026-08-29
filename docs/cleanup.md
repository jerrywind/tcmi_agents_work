# 临时文件与日志清理规范（Cleanup Rules）

## 1. 目标

- 减小磁盘压力、避免误提交大文件、隔离多会话、加速 CI。
- 统一命名，便于定位与批量清理。

## 2. 分类与位置

| 类别 | 位置 | 命名规则 | 生命周期 |
|---|---|---|---|
| 运行日志 | 仓库根 `*.log` / `*.err`（已 gitignore） | `tcm-YYYYMMDD-*.log` | 保留 7 天 |
| 测试产物 | `.pytest_cache/`、`frontend/coverage/`、`htmlcov/`、`e2e_tests/images/` | 框架默认 | CI 后清除 |
| 构建产物 | `frontend/dist/`、`server/target/`、`__pycache__/` | 框架默认 | gitignore，可随时删 |
| 诊断截图/转写 | `debug_images/`、`debug_*.png` | `case_*.json`、`debug_*.png` | 用完即删 |

> harness **不落盘任何图片**：图片以 base64 / URL 随请求传入，无上传目录。

## 3. 清理命令

```powershell
# 一键脚本（推荐）
powershell -NoProfile -File scripts\cleanup.ps1

# 手动：仓库级
Remove-Item -Recurse -Force .pytest_cache, frontend\coverage, htmlcov, frontend\dist
Get-ChildItem -Recurse -Directory -Filter __pycache__ | Remove-Item -Recurse -Force

# Rust 构建产物（体积最大，通常 3 GB+，可安全删除，下次 cargo build 会重建）
Remove-Item -Recurse -Force server\target

# 运行日志（保留最近 7 天）
Get-ChildItem *.log, *.err |
  Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-7) } | Remove-Item -Force
```

## 4. 规则

1. 临时文件 **必须** 落在已被 `.gitignore` 覆盖的路径，禁止落入源码树。
2. 文件名带 `tmp_` / `debug_` 前缀，便于识别与批量清理。
3. 测试不得依赖未被清理的临时状态；每个用例自创建、自清理。
4. CI 结束后统一执行第 3 节的清理命令。
5. **禁止把日志/错误输出提交进仓库**（`.gitignore` 已覆盖 `*.log` / `*.err` / `*.out`）。
   若发现已被跟踪的历史文件，用 `git rm --cached <file>` 解除跟踪。
