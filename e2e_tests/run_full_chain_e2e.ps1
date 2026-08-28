<#
.SYNOPSIS
  风蓝科技 TCM 全链路端到端测试一键编排脚本。

.DESCRIPTION
  依次启动：llm_server（可选）、harness（Rust 重写后的后端），
  并运行：
    1) pytest 集成测试：rrserver 隧道 / llm_server 网关
       （backend 的 Python 契约用例 test_backend_llm_integration_e2e.py 已随
        backend 归档到 _useless/，默认排除；harness 的回归测试请用
        `cargo test -p harness`，其案例基准来自 cases.jsonl）
    2) 前端 vitest 函数级 e2e（默认跳过：前端 api.ts 仍按旧 backend 契约，
       需完成契约对齐后用 -WithFrontend 开启）

  注意：harness 无 MockProvider，问诊推进需要真实 LLM（LM Studio）；
  仅 /health、/agents、/skills 等只读端点可在无 LLM 下验证。

.PARAMETER SkipRrserver
  跳过 rrserver 隧道测试（需要本地 Rust 编译产物，默认跳过除非传入 -WithRrserver）。

.PARAMETER WithRrserver
  包含 rrserver 隧道测试（需先 cargo build rrserver）。

.PARAMETER SkipFrontend
  跳过前端 vitest e2e。

.EXAMPLE
  .\run_full_chain_e2e.ps1                 # 跑 llm + backend + 前端（不含 rrserver）
  .\run_full_chain_e2e.ps1 -WithRrserver   # 额外包含 rrserver 隧道测试
#>
param(
  [switch]$SkipRrserver,
  [switch]$WithRrserver,
  [switch]$SkipFrontend,
  [switch]$WithFrontend
)

$ErrorActionPreference = 'Continue'
$ROOT = Split-Path $PSScriptRoot -Parent          # tcm_work
$E2E  = $PSScriptRoot                              # tcm_work/e2e_tests
# backend 已归档至 _useless/backend，其后继为 Rust 实现 server/harness
$HARNESS = Join-Path $ROOT 'server/harness'
$LLM     = Join-Path $ROOT 'llm_server'
$FRONT   = Join-Path $ROOT 'frontend'
$RRS     = Join-Path $ROOT 'server/rrserver'

$HARNESS_PORT = 8011
$LLM_PORT     = 8002
$SHUTDOWN = [System.Collections.ArrayList]::new()

function Start-Background {
  param($Name, $Exe, $Args, $Cwd, $Env)
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $Exe
  $psi.Arguments = $Args
  $psi.UseShellExecute = $false
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  if ($Cwd) { $psi.WorkingDirectory = $Cwd }
  foreach ($k in $Env.Keys) { $psi.Environment[$k] = $Env[$k] }
  $p = [System.Diagnostics.Process]::Start($psi)
  [void]$SHUTDOWN.Add($p)
  Write-Host "[e2e] 启动 $Name (pid=$($p.Id))" -ForegroundColor Cyan
  return $p
}

function Wait-Healthy {
  param($Url, $Timeout = 60)
  $deadline = (Get-Date).AddSeconds($Timeout)
  while ((Get-Date) -lt $deadline) {
    try {
      $r = Invoke-WebRequest -Uri $Url -UseBasicParsing -TimeoutSec 3 -ErrorAction SilentlyContinue
      if ($r.StatusCode -lt 500) { return $true }
    } catch {}
    Start-Sleep -Seconds 1
  }
  return $false
}

function Stop-All {
  foreach ($p in $SHUTDOWN) {
    try { if (-not $p.HasExited) { $p.Kill() } } catch {}
  }
}

# ---------- 0. 准备样例图片 ----------
python "$E2E/_make_sample_image.py"

# ---------- 1. 启动 harness（Rust 后端；只读端点无需 LLM） ----------
Write-Host "`n[e2e] === 启动 harness（server/harness）===" -ForegroundColor Yellow
$harnessExe = Join-Path $HARNESS 'target/debug/harness.exe'
if (-not (Test-Path $harnessExe)) { $harnessExe = Join-Path $HARNESS 'target/release/harness.exe' }
if (-not (Test-Path $harnessExe)) {
  Write-Host "[e2e] 未找到 harness 二进制，请先：`cargo build -p harness`（在 server/ 目录）" -ForegroundColor Red
  Stop-All; exit 1
}
$harnessEnv = @{
  # harness 读 HARNESS_* 前缀；未配置 LLM 时仅只读端点（/health、/agents、/skills）可用
  HARNESS_LLM_BASE_URL = "${env:HARNESS_LLM_BASE_URL}"
  HARNESS_LLM_API_KEY  = "${env:HARNESS_LLM_API_KEY}"
}
Start-Background -Name 'harness' -Exe $harnessExe `
  -Args "--listen 127.0.0.1:$HARNESS_PORT" `
  -Cwd $HARNESS -Env $harnessEnv

# harness 的只读端点为 /health（前端经 nginx 以 /api 前缀代理到此）
if (-not (Wait-Healthy "http://127.0.0.1:$HARNESS_PORT/health" 90)) {
  Write-Host "[e2e] harness 未就绪，终止" -ForegroundColor Red
  Stop-All; exit 1
}
Write-Host "[e2e] harness 已就绪（端口 $HARNESS_PORT）" -ForegroundColor Green

# ---------- 2. 运行 pytest 集成测试 ----------
Write-Host "`n[e2e] === pytest 集成测试 ===" -ForegroundColor Yellow
# 默认排除 rrserver（需 Rust 编译产物）与 backend（Python 契约用例已随 backend 归档）
$pytestArgs = @(
  '-q'
  "-k", "not rrserver and not backend"
)
if ($WithRrserver -and -not $SkipRrserver) {
  $pytestArgs = @('-q', '-k', 'not backend')   # 包含 rrserver
}
$env:TCM_HARNESS_BASE = "http://127.0.0.1:$HARNESS_PORT"
$env:TCM_BACKEND_LLM_BASE = 'http://127.0.0.1:9/none'
Push-Location $E2E
$pytestExit = 0
try {
  & python -m pytest @pytestArgs
  $pytestExit = $LASTEXITCODE
} catch { $pytestExit = 1 }
Pop-Location

# ---------- 3. 前端 vitest e2e ----------
# 前端 src/services/api.ts 仍按旧 backend 契约（/api/consultations 等），
# 与 harness 端点尚未对齐，故默认跳过；完成契约对齐后用 -WithFrontend 开启。
$frontExit = 0
if ($WithFrontend -and -not $SkipFrontend) {
  Write-Host "`n[e2e] === 前端 vitest e2e ===" -ForegroundColor Yellow
  $env:TCM_API_BASE = "http://127.0.0.1:$HARNESS_PORT"
  Push-Location $FRONT
  try {
    & npx vitest run src/services/api.e2e.test.ts
    $frontExit = $LASTEXITCODE
  } catch { $frontExit = 1 }
  Pop-Location
} else {
  Write-Host "`n[e2e] 跳过前端 vitest e2e（契约未对齐；加 -WithFrontend 强制开启）" -ForegroundColor DarkYellow
}

# ---------- 收尾 ----------
Stop-All
Write-Host "`n[e2e] 结果：pytest=$pytestExit  frontend=$frontExit" -ForegroundColor Cyan
if ($pytestExit -eq 0 -and $frontExit -eq 0) {
  Write-Host "[e2e] 全链路 e2e 通过 ✅" -ForegroundColor Green
  exit 0
} else {
  Write-Host "[e2e] 存在失败用例 ❌" -ForegroundColor Red
  exit 1
}
