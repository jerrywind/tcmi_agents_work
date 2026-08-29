<#
.SYNOPSIS
  构建后端 Docker 镜像（harness / rrserver）。

.DESCRIPTION
  后端**完全依赖 Docker** 构建：编译在两个多阶段 Dockerfile 内完成，
  **不使用宿主机或 WSL2 的本地 cargo 产物**。

  本脚本此前负责「在 WSL2 里预编译 Linux 二进制再拷回 server/target/release/」，
  这是因为旧版 Dockerfile 直接 COPY 预编译二进制。该流程已废弃——
  镜像内编译让构建可复现、CI 可独立执行，也不再需要 WSL2 与本地 Rust 工具链。

  镜像内的层缓存策略（见各 Dockerfile）：
    1) 先只复制清单 + 占位源码预编译第三方依赖（依赖不变则命中缓存）
    2) 再复制真实源码，只重编本 workspace 的 crate

.PARAMETER Tag
  镜像标签，默认 local（产出 tcm-harness:local / tcm-rrserver:local）。

.PARAMETER NoCache
  忽略 Docker 层缓存，全量重建。

.PARAMETER SkipTests
  跳过「在 Docker 内跑 cargo test」。

.EXAMPLE
  powershell -NoProfile -File scripts\build-release.ps1

  然后部署：
    cd frontend && npm run build:h5
    docker compose -f deploy/docker-compose.yml up -d --build
#>
param(
  [string]$Tag = 'local',
  [switch]$NoCache,
  [switch]$SkipTests
)

$ErrorActionPreference = 'Stop'

$RepoRoot  = Split-Path $PSScriptRoot -Parent          # tcm_work
$ServerDir = Join-Path $RepoRoot 'server'

if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
  throw 'docker not found. 后端一律通过 Docker 构建，请先安装 Docker Desktop。'
}
docker version --format '{{.Server.Version}}' *> $null
if ($LASTEXITCODE -ne 0) { throw 'Docker 守护进程未运行，请启动 Docker Desktop。' }

Write-Host "[build] workspace : $ServerDir" -ForegroundColor Cyan

# ---------- 1. 测试（在 Docker 内，不依赖本地 Rust） ----------
if (-not $SkipTests) {
  Write-Host "[test ] cargo test -p harness (Docker) ..." -ForegroundColor Yellow
  docker run --rm -v "${ServerDir}:/build" -w /build rust:1.98-bookworm cargo test -p harness
  if ($LASTEXITCODE -ne 0) { throw "harness 测试失败（exit $LASTEXITCODE）" }
  Write-Host "[test ] OK harness" -ForegroundColor Green
}

# ---------- 2. 构建镜像 ----------
# 构建上下文必须是 workspace 根（server/）：两个 crate 共享 target/，
# 且 harness 依赖 rrserver 的 lib，两者源码都要进上下文。
Push-Location $ServerDir
try {
  foreach ($crate in @('harness', 'rrserver')) {
    $image = "tcm-${crate}:${Tag}"
    Write-Host "[build] docker build $image ..." -ForegroundColor Yellow

    $dockerArgs = @('build', '-f', "${crate}/Dockerfile", '-t', $image)
    if ($NoCache) { $dockerArgs += '--no-cache' }
    $dockerArgs += '.'

    & docker @dockerArgs
    if ($LASTEXITCODE -ne 0) {
      throw "docker build 失败：$crate（exit $LASTEXITCODE）"
    }
    Write-Host "[build] OK $image" -ForegroundColor Green
  }
} finally {
  Pop-Location
}

Write-Host ''
Write-Host '[build] done. 下一步：' -ForegroundColor Cyan
Write-Host '        cd frontend && npm run build:h5            # nginx 托管的静态产物'
Write-Host '        docker compose -f deploy/docker-compose.yml up -d --build'
