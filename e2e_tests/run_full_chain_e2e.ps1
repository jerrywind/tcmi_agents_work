<#
.SYNOPSIS
  风蓝科技 TCM 全链路端到端测试一键编排脚本。

.DESCRIPTION
  用 **Docker** 起 harness（后端完全依赖 Docker，不使用宿主机 cargo 产物），并运行：
    1) pytest 集成测试：llm_server 网关（默认）+ rrserver 隧道（-WithRrserver）
       （harness 内部的确定性回归请用 Docker 内 `cargo test --workspace`，
         其案例基准来自 cases.jsonl，不依赖 LLM）
    2) 前端契约测试（默认开启）：frontend/src/services/harness.contract.test.ts
       直连真实 harness 校验 /health、/agents、/skills、-WithFrontend 之外，
       后端不可达时该用例会自动 skip，因此无 Docker 环境也不会误报失败。

  注意：harness 无 MockProvider，问诊推进需要真实 LLM（LM Studio）；
  仅 /health、/agents、/skills 等只读端点可在无 LLM 下验证。

.PARAMETER SkipBuild
  跳过 harness 镜像构建，直接使用已存在的 -ImageName 镜像。

.PARAMETER WithRrserver
  包含 rrserver 隧道测试（需 TCM_RRSERVER_BIN 指向已编译二进制，默认跳过）。

.PARAMETER SkipFrontend
  跳过前端契约测试。

.PARAMETER ImageName
  harness 镜像名，默认 tcm-harness:e2e。

.EXAMPLE
  .\run_full_chain_e2e.ps1                 # harness(镜像) + pytest + 前端契约
  .\run_full_chain_e2e.ps1 -SkipFrontend   # 只跑 pytest
  .\run_full_chain_e2e.ps1 -WithRrserver   # 额外包含 rrserver 隧道测试
#>
param(
  [switch]$SkipBuild,
  [switch]$WithRrserver,
  [switch]$SkipFrontend,
  [switch]$WithFrontend,
  [string]$ImageName = 'tcm-harness:e2e'
)

$ErrorActionPreference = 'Continue'
$ROOT = Split-Path $PSScriptRoot -Parent          # tcm_work
$E2E  = $PSScriptRoot                              # tcm_work/e2e_tests
$SERVER = Join-Path $ROOT 'server'                 # Cargo workspace 根（构建上下文）
$FRONT = Join-Path $ROOT 'frontend'

$HARNESS_PORT = 8011
$CONTAINER = 'tcm-harness-e2e'

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

function Stop-Container {
  docker rm -f $CONTAINER 2>$null | Out-Null
}

# ---------- 0. 准备样例图片 ----------
python "$E2E/_make_sample_image.py"

# ---------- 1. 构建 harness 镜像（Docker 内多阶段编译） ----------
if (-not $SkipBuild) {
  Write-Host "`n[e2e] === 构建 harness 镜像 $ImageName ===" -ForegroundColor Yellow
  Push-Location $SERVER
  docker build -f harness/Dockerfile -t $ImageName .
  $buildExit = $LASTEXITCODE
  Pop-Location
  if ($buildExit -ne 0) {
    Write-Host "[e2e] 镜像构建失败" -ForegroundColor Red
    exit 1
  }
}

# ---------- 2. 启动 harness 容器 ----------
Write-Host "`n[e2e] === 启动 harness 容器（端口 $HARNESS_PORT）===" -ForegroundColor Yellow
Stop-Container
$runArgs = @('run','-d','--name',$CONTAINER,'-p',"$($HARNESS_PORT):8011")
foreach ($k in @('HARNESS_LLM_BASE_URL','HARNESS_LLM_API_KEY','HARNESS_MODEL','HARNESS_RAG_ENDPOINT')) {
  $v = [Environment]::GetEnvironmentVariable($k)
  if ($v) { $runArgs += @('-e', "$k=$v") }
}
$runArgs += $ImageName
& docker @runArgs | Out-Null
if ($LASTEXITCODE -ne 0) {
  Write-Host "[e2e] harness 容器启动失败" -ForegroundColor Red
  exit 1
}

if (-not (Wait-Healthy "http://127.0.0.1:$HARNESS_PORT/health" 90)) {
  Write-Host "[e2e] harness 未就绪，终止；容器日志：" -ForegroundColor Red
  docker logs --tail 30 $CONTAINER
  Stop-Container
  exit 1
}
Write-Host "[e2e] harness 已就绪（端口 $HARNESS_PORT）" -ForegroundColor Green

# ---------- 3. 运行 pytest 集成测试 ----------
Write-Host "`n[e2e] === pytest 集成测试 ===" -ForegroundColor Yellow
$pytestArgs = @('-q', '-k', 'not rrserver')
if ($WithRrserver) {
  $pytestArgs = @('-q')                          # 全部（含 rrserver）
}
$env:TCM_HARNESS_BASE = "http://127.0.0.1:$HARNESS_PORT"
Push-Location $E2E
$pytestExit = 0
try {
  & python -m pytest @pytestArgs
  $pytestExit = $LASTEXITCODE
} catch { $pytestExit = 1 }
Pop-Location

# ---------- 4. 前端契约测试（默认开启） ----------
# 跑真实 harness 的只读端点契约；后端不可达时用例自动 skip，不会误报失败。
$frontExit = 0
if (-not $SkipFrontend) {
  Write-Host "`n[e2e] === 前端契约测试（真实 harness）===" -ForegroundColor Yellow
  $env:VITE_API_BASE = "http://127.0.0.1:$HARNESS_PORT"
  Push-Location $FRONT
  try {
    & npx vitest run src/services/harness.contract.test.ts
    $frontExit = $LASTEXITCODE
  } catch { $frontExit = 1 }
  Pop-Location
} else {
  Write-Host "`n[e2e] 跳过前端契约测试（-SkipFrontend 已指定）" -ForegroundColor DarkYellow
}
# 兼容旧参数：-WithFrontend 曾是开启开关，现为默认行为
if ($WithFrontend -and $SkipFrontend) {
  Write-Host "[e2e] -WithFrontend 与 -SkipFrontend 同时传入，以 -SkipFrontend 为准" -ForegroundColor DarkYellow
}

# ---------- 收尾 ----------
Stop-Container
Write-Host "`n[e2e] 结果：pytest=$pytestExit  frontend=$frontExit" -ForegroundColor Cyan
if ($pytestExit -eq 0 -and $frontExit -eq 0) {
  Write-Host "[e2e] 全链路 e2e 通过" -ForegroundColor Green
  exit 0
} else {
  Write-Host "[e2e] 存在失败用例" -ForegroundColor Red
  exit 1
}
